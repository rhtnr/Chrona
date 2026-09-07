//! Chrona's capture layer: cpal input streams behind a lock-free ring
//! (spec §4). Deliberately DSP-ignorant — no dependency on `chrona-dsp`.

pub mod capture;
pub mod devices;
pub mod monoize;

pub use capture::{CaptureError, CaptureHealth, CaptureStream, StreamInfo};
pub use devices::{DeviceInfo, list_input_devices};

/// Installs [`assert_no_alloc::AllocDisabler`] as *this process's* global
/// allocator (M4 Task 5, spec §2.6's audio-callback no-alloc guard).
///
/// `#[global_allocator]` is whole-binary linkage: exactly one crate in the
/// entire dependency graph may declare it, and whichever one does controls
/// every allocation for every crate linked into that binary — including
/// ones that have no idea it's there. Installing it unconditionally here
/// (even gated on plain `cfg(debug_assertions)`) would therefore force
/// `AllocDisabler` onto every downstream debug binary that merely links
/// chrona-audio, `chrona-app` included, whether it wanted the enforcement
/// or not. That's far more invasive than a capture-layer library has any
/// business being, so this only fires for chrona-audio's OWN test binary:
/// `cargo test -p chrona-audio` is what actually proves
/// `capture::process_callback` doesn't allocate (see its tests).
///
/// Everywhere else — including a debug build of the real app —
/// `capture::process_callback`'s `assert_no_alloc::assert_no_alloc(..)`
/// wrapping still runs (it's plain `cfg(debug_assertions)`, not
/// `cfg(test)`), but with no custom global allocator installed to enforce
/// it, so it is a correctly-shaped no-op there: present, but inert, until
/// (if ever) a consuming binary opts in itself — see `AllocDisabler`'s own
/// re-export below for that recipe.
#[cfg(all(test, debug_assertions))]
#[global_allocator]
static ALLOC_GUARD: assert_no_alloc::AllocDisabler = assert_no_alloc::AllocDisabler;

/// Re-exported so a consuming BINARY (e.g. `chrona-app`) can opt into the
/// same audio-callback no-alloc enforcement chrona-audio's own tests get,
/// without adding `assert_no_alloc` as a direct dependency of its own just
/// to name this type.
///
/// This is a type, not a function: installing a global allocator isn't a
/// runtime action a library can perform on a caller's behalf (see
/// `ALLOC_GUARD` above) — it has to be a `static` item, declared by name,
/// in the crate that produces the final binary. No opt-in `pub fn` can
/// actually do that job, so this is the honest shape of "opt in later if
/// wanted": a re-exported type plus the recipe below, not a callable
/// switch. Not adopted anywhere in this workspace yet (M4 Task 5 scope is
/// chrona-audio's own test coverage) — left for a future task if the app
/// wants the same guarantee end-to-end. To opt in, add this to
/// `chrona-app/src/main.rs`:
///
/// ```ignore
/// #[cfg(debug_assertions)]
/// #[global_allocator]
/// static ALLOC_GUARD: chrona_audio::AllocDisabler = chrona_audio::AllocDisabler;
/// ```
#[cfg(debug_assertions)]
pub use assert_no_alloc::AllocDisabler;
