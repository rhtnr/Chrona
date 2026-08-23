//! Session-history index (design spec §7): every recorded session, sourced
//! entirely from the JSON sidecars a recordings directory already holds —
//! no database, no separate index file. Also the per-watch
//! position-comparison math (spec §8) that reads from it. Pure logic only:
//! no `egui`/`Color32` here (Task 6/9 render this from the outside).

use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use chrona_session::SessionSummary;

/// One recorded session, as reflected by its `.json` sidecar at the moment
/// it was read. `wav_path` is always the WAV path (never the sidecar's own
/// `.json` path) — see [`HistoryIndex::scan`]'s doc comment for why the WAV
/// itself need not exist on disk for an entry to appear here.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionEntry {
    pub wav_path: PathBuf,
    pub watch: Option<String>,
    pub position: Option<String>,
    pub started_unix_s: u64,
    pub summary: Option<SessionSummary>,
}

/// The six canonical timegrapher positions (spec §6/§8), code + display
/// label, in the fixed order every position-keyed array in this module
/// (e.g. [`PositionRates::by_code`]) uses.
pub const POSITIONS: [(&str, &str); 6] = [
    ("DU", "Dial up"),
    ("DD", "Dial down"),
    ("CU", "Crown up"),
    ("CD", "Crown down"),
    ("CL", "Crown left"),
    ("CR", "Crown right"),
];

/// In-memory session-history index (spec §7): every session sidecar found
/// in a recordings directory, kept sorted newest-first. Built once by
/// [`HistoryIndex::scan`] and kept in sync afterwards by
/// [`HistoryIndex::upsert`] — there is no watcher and no persisted index;
/// the sidecars on disk are the only source of truth.
#[derive(Debug, Clone, Default)]
pub struct HistoryIndex {
    entries: Vec<SessionEntry>,
}

impl HistoryIndex {
    /// Scans `dir` for session sidecars: every `*.json` file's WAV path
    /// (same stem, `.wav` extension) is passed to
    /// [`chrona_session::read_meta`], which re-derives the sidecar path
    /// from it internally — so each JSON file is read exactly once, via
    /// the WAV-keyed path, matching how every other part of this codebase
    /// addresses a session. **The WAV file itself need not exist** for its
    /// entry to appear: `read_meta` only ever touches the `.json` sidecar,
    /// never the WAV, so a sidecar whose audio was since deleted still
    /// indexes fine — the sidecar is the record, not the audio (spec §7:
    /// "index source of truth = the recordings directory... small JSON
    /// reads"). A `.json` file that fails to parse is skipped silently
    /// (mirrors `read_meta`'s own best-effort contract), and a `dir` that
    /// doesn't exist yet (no recording has ever been made) yields an empty
    /// index rather than an error.
    ///
    /// Timestamp resolution per entry: `meta.started_unix_s` when nonzero;
    /// when it's absent/zero (a synthetic or otherwise timestamp-less
    /// sidecar), falls back to the **sidecar file's own mtime** — not the
    /// WAV's, precisely because the WAV need not exist (see above), while
    /// the JSON we just parsed always does. Any error resolving that mtime
    /// (permissions, exotic filesystem, pre-epoch clock) falls back to `0`
    /// rather than panicking or fabricating "now".
    pub fn scan(dir: &Path) -> HistoryIndex {
        let mut entries = Vec::new();
        if let Ok(read_dir) = std::fs::read_dir(dir) {
            for dir_entry in read_dir.flatten() {
                let json_path = dir_entry.path();
                if json_path.extension().and_then(|e| e.to_str()) != Some("json") {
                    continue;
                }
                let wav_path = json_path.with_extension("wav");
                if let Some(session) = load_entry(&wav_path) {
                    entries.push(session);
                }
            }
        }
        let mut idx = HistoryIndex { entries };
        idx.sort();
        idx
    }

    /// Re-reads one sidecar (given its WAV path) and replaces its entry —
    /// matched on `wav_path` — or inserts it fresh if it wasn't indexed
    /// yet, then re-sorts. The hook for "a recording just finalized": the
    /// engine has already rewritten the JSON sidecar with its stop-time
    /// summary by the time a caller can observe that (see
    /// `ui::session_panel`'s recording→`None` transition), so re-reading
    /// here picks up the fresh summary without a full directory re-scan.
    /// If the sidecar no longer parses (e.g. deleted since), the stale
    /// entry is simply dropped rather than kept — never left inconsistent
    /// with what's actually on disk.
    pub fn upsert(&mut self, wav_path: &Path) {
        self.entries.retain(|e| e.wav_path != wav_path);
        if let Some(session) = load_entry(wav_path) {
            self.entries.push(session);
        }
        self.sort();
    }

    /// All indexed sessions, newest first.
    pub fn entries(&self) -> &[SessionEntry] {
        &self.entries
    }

    /// Indexed sessions for one watch (exact match), newest first —
    /// preserves `entries()`'s order since it's a plain filter over an
    /// already-sorted vec.
    pub fn for_watch<'a>(&'a self, watch: &str) -> impl Iterator<Item = &'a SessionEntry> {
        self.entries
            .iter()
            .filter(move |e| e.watch.as_deref() == Some(watch))
    }

    /// Stable DESC by resolved `started_unix_s`; ties break by `wav_path`
    /// so ordering is fully deterministic (needed for reproducible tests
    /// and a UI that doesn't jitter row order between identical scans).
    fn sort(&mut self) {
        self.entries.sort_by(|a, b| {
            b.started_unix_s
                .cmp(&a.started_unix_s)
                .then_with(|| a.wav_path.cmp(&b.wav_path))
        });
    }
}

/// Builds one `SessionEntry` from `wav_path`'s sidecar, or `None` if it
/// doesn't parse. See [`HistoryIndex::scan`]'s doc comment for the
/// timestamp-fallback chain this implements.
fn load_entry(wav_path: &Path) -> Option<SessionEntry> {
    let meta = chrona_session::read_meta(wav_path)?;
    let started_unix_s = if meta.started_unix_s != 0 {
        meta.started_unix_s
    } else {
        sidecar_mtime_unix_s(wav_path)
    };
    Some(SessionEntry {
        wav_path: wav_path.to_path_buf(),
        watch: meta.watch,
        position: meta.position,
        started_unix_s,
        summary: meta.summary,
    })
}

/// The `.json` sidecar's mtime, in unix seconds, or `0` on any error —
/// see [`HistoryIndex::scan`] for why the sidecar (not the WAV) is used.
fn sidecar_mtime_unix_s(wav_path: &Path) -> u64 {
    let json_path = wav_path.with_extension("json");
    std::fs::metadata(&json_path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Latest summary rate per canonical position, for one watch (spec §8),
/// index-aligned with [`POSITIONS`].
#[derive(Debug, Clone, PartialEq)]
pub struct PositionRates {
    pub by_code: [Option<f64>; 6],
    /// `max - min` over positions that currently have a rate, when at
    /// least two do; `None` below that (never guessed from a single
    /// point, or from zero).
    pub spread: Option<f64>,
}

/// For `watch`: the most recent (highest resolved `started_unix_s`)
/// summary rate at each canonical position. A session contributes to a
/// position only when it has that `position`, *and* a `summary`, *and*
/// that summary's `rate_s_per_day` is `Some` — sessions missing any of
/// those are skipped for comparison purposes, never guessed or averaged
/// in (spec §3.1/§8 honesty). Because `idx.for_watch` iterates in the
/// index's newest-first order, the first rated session seen for a given
/// position *is* its newest — older rated sessions at the same position
/// never overwrite it.
pub fn position_rates(idx: &HistoryIndex, watch: &str) -> PositionRates {
    let mut by_code: [Option<f64>; 6] = [None; 6];
    for entry in idx.for_watch(watch) {
        let Some(position) = entry.position.as_deref() else {
            continue;
        };
        let Some(rate) = entry.summary.as_ref().and_then(|s| s.rate_s_per_day) else {
            continue;
        };
        let Some(slot) = POSITIONS.iter().position(|(code, _)| *code == position) else {
            continue;
        };
        // `for_watch` yields entries newest-first, so the first rated
        // match for a slot is its newest — never overwrite once set.
        if by_code[slot].is_none() {
            by_code[slot] = Some(rate);
        }
    }

    let rated: Vec<f64> = by_code.iter().filter_map(|r| *r).collect();
    let spread = if rated.len() >= 2 {
        let min = rated.iter().copied().fold(f64::INFINITY, f64::min);
        let max = rated.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        Some(max - min)
    } else {
        None
    };
    PositionRates { by_code, spread }
}

/// Bar geometry for the position-comparison chart (spec §8, mockup): the
/// bar's fixed scale runs ±15 s/d end to end, so a rate at that clamp
/// reaches exactly half the bar's width. Returns that half-width as a
/// fraction of the bar (`0.0..=0.5`), clamped so a rate beyond the scale
/// still fits rather than overflowing it.
pub fn bar_frac(rate: f64) -> f64 {
    const FULL_SCALE_S_PER_DAY: f64 = 15.0;
    const MAX_HALF_WIDTH: f64 = 0.5;
    (rate.abs() / FULL_SCALE_S_PER_DAY * MAX_HALF_WIDTH).min(MAX_HALF_WIDTH)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Hand-written v2 sidecar JSON, mirroring the literal-JSON fixtures
    /// `chrona_session::sidecar`'s own tests use (see its
    /// `v1_sidecar_loads_with_none_new_fields`) — avoids pulling
    /// `serde_json` into `chrona-app` just to build test fixtures.
    /// `summary` is `Some((tier, rate))` for a present summary object
    /// (`rate: None` renders `rate_s_per_day: null` — a summary with no
    /// rate, e.g. Tier 1 only), or `None` for `"summary": null` entirely.
    fn v2_json(
        watch: Option<&str>,
        position: Option<&str>,
        started_unix_s: u64,
        summary: Option<(&str, Option<f64>)>,
    ) -> String {
        let watch = watch.map_or("null".to_string(), |w| format!("\"{w}\""));
        let position = position.map_or("null".to_string(), |p| format!("\"{p}\""));
        let summary = summary.map_or("null".to_string(), |(tier, rate)| {
            let rate = rate.map_or("null".to_string(), |r| r.to_string());
            format!(
                r#"{{"tier":"{tier}","rate_s_per_day":{rate},"beat_error_ms":0.5,"amplitude_deg":260.0,"bph_detected":28800.0,"duration_s":45.0}}"#
            )
        });
        format!(
            r#"{{"schema_version":2,"device_name":"TestMic","sample_rate_hz":48000.0,"ppm_correction":0.0,"lift_angle_deg":52.0,"bph_mode":"auto","position":{position},"started_unix_s":{started_unix_s},"app_version":"test","watch":{watch},"summary":{summary}}}"#
        )
    }

    /// A v1-style sidecar: no `watch`/`summary` keys at all (they didn't
    /// exist yet), relying on `#[serde(default)]` — same shape as
    /// `chrona_session::sidecar`'s own `v1_sidecar_loads_with_none_new_fields`
    /// fixture.
    fn v1_json(position: Option<&str>, started_unix_s: u64) -> String {
        let position = position.map_or("null".to_string(), |p| format!("\"{p}\""));
        format!(
            r#"{{"schema_version":1,"device_name":"OldMic","sample_rate_hz":48000.0,"ppm_correction":0.0,"lift_angle_deg":52.0,"bph_mode":"auto","position":{position},"started_unix_s":{started_unix_s},"app_version":"0.1.0"}}"#
        )
    }

    fn write_json(dir: &Path, stem: &str, json: &str) {
        std::fs::write(dir.join(format!("{stem}.json")), json).unwrap();
    }

    fn now_unix_s() -> u64 {
        std::time::SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }

    #[test]
    fn scan_orders_desc_by_started_with_v1_and_v2_mix() {
        let dir = tempfile::tempdir().unwrap();
        write_json(
            dir.path(),
            "chrona-a",
            &v2_json(Some("Seiko 5"), Some("DU"), 3000, Some(("T3", Some(12.0)))),
        );
        write_json(dir.path(), "chrona-b", &v1_json(None, 1000));
        write_json(
            dir.path(),
            "chrona-c",
            &v2_json(Some("Omega"), Some("CU"), 2000, Some(("T1", None))),
        );

        let idx = HistoryIndex::scan(dir.path());
        let entries = idx.entries();
        assert_eq!(entries.len(), 3, "entries: {entries:?}");
        assert_eq!(entries[0].started_unix_s, 3000);
        assert_eq!(entries[0].watch.as_deref(), Some("Seiko 5"));
        assert_eq!(
            entries[0].summary.as_ref().and_then(|s| s.rate_s_per_day),
            Some(12.0)
        );
        assert_eq!(entries[1].started_unix_s, 2000);
        assert_eq!(entries[1].watch.as_deref(), Some("Omega"));
        assert_eq!(
            entries[1].summary.as_ref().and_then(|s| s.rate_s_per_day),
            None,
            "T1-only summary has no rate"
        );
        assert_eq!(entries[2].started_unix_s, 1000);
        assert_eq!(entries[2].watch, None, "v1 sidecar: watch defaults None");
        assert_eq!(
            entries[2].summary, None,
            "v1 sidecar: summary defaults None"
        );
    }

    #[test]
    fn scan_falls_back_to_sidecar_mtime_when_started_unix_s_is_zero() {
        let dir = tempfile::tempdir().unwrap();
        let before = now_unix_s();
        write_json(dir.path(), "chrona-zero", &v2_json(None, None, 0, None));
        let idx = HistoryIndex::scan(dir.path());
        let after = now_unix_s();

        let entries = idx.entries();
        assert_eq!(entries.len(), 1);
        assert!(
            entries[0].started_unix_s >= before && entries[0].started_unix_s <= after,
            "expected mtime fallback in [{before}, {after}], got {}",
            entries[0].started_unix_s
        );
    }

    #[test]
    fn scan_on_missing_dir_is_empty_not_error() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("never-created");
        let idx = HistoryIndex::scan(&missing);
        assert_eq!(idx.entries(), &[]);
    }

    #[test]
    fn upsert_inserts_new_then_replaces_existing_and_resorts() {
        let dir = tempfile::tempdir().unwrap();
        write_json(
            dir.path(),
            "chrona-a",
            &v2_json(Some("Seiko 5"), Some("DU"), 1000, Some(("T3", Some(1.0)))),
        );
        write_json(
            dir.path(),
            "chrona-b",
            &v2_json(Some("Seiko 5"), Some("DD"), 2000, Some(("T3", Some(2.0)))),
        );
        let mut idx = HistoryIndex::scan(dir.path());
        assert_eq!(idx.entries().len(), 2);

        // Insert a brand-new 4th (well, 3rd here) sidecar not yet indexed.
        write_json(
            dir.path(),
            "chrona-c",
            &v2_json(Some("Seiko 5"), Some("CU"), 3000, Some(("T3", Some(3.0)))),
        );
        idx.upsert(&dir.path().join("chrona-c.wav"));
        let entries = idx.entries();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].started_unix_s, 3000, "newest first: {entries:?}");
        assert_eq!(entries[1].started_unix_s, 2000);
        assert_eq!(entries[2].started_unix_s, 1000);

        // Rewrite "a"'s sidecar (as if the recording just finalized with a
        // later timestamp and a new rate) and upsert it again — must
        // replace, not duplicate.
        write_json(
            dir.path(),
            "chrona-a",
            &v2_json(Some("Seiko 5"), Some("DU"), 4000, Some(("T3", Some(9.0)))),
        );
        idx.upsert(&dir.path().join("chrona-a.wav"));
        let entries = idx.entries();
        assert_eq!(entries.len(), 3, "replace must not duplicate: {entries:?}");
        assert_eq!(entries[0].started_unix_s, 4000);
        assert_eq!(
            entries[0].summary.as_ref().and_then(|s| s.rate_s_per_day),
            Some(9.0)
        );
        assert_eq!(entries[1].started_unix_s, 3000);
        assert_eq!(entries[2].started_unix_s, 2000);
    }

    #[test]
    fn for_watch_filters_and_preserves_desc_order() {
        let dir = tempfile::tempdir().unwrap();
        write_json(
            dir.path(),
            "chrona-a",
            &v2_json(Some("Seiko 5"), Some("DU"), 1000, None),
        );
        write_json(
            dir.path(),
            "chrona-b",
            &v2_json(Some("Omega"), Some("DD"), 2000, None),
        );
        write_json(
            dir.path(),
            "chrona-c",
            &v2_json(Some("Seiko 5"), Some("CU"), 3000, None),
        );

        let idx = HistoryIndex::scan(dir.path());
        let seiko: Vec<&SessionEntry> = idx.for_watch("Seiko 5").collect();
        assert_eq!(seiko.len(), 2);
        assert_eq!(seiko[0].started_unix_s, 3000, "newest first");
        assert_eq!(seiko[1].started_unix_s, 1000);
        assert!(seiko.iter().all(|e| e.watch.as_deref() == Some("Seiko 5")));
    }

    #[test]
    fn position_rates_picks_newest_rated_per_position_skips_unrated_and_computes_spread() {
        let dir = tempfile::tempdir().unwrap();
        // DU: two sessions, newest (3000) must win over the older (2000).
        write_json(
            dir.path(),
            "chrona-du-new",
            &v2_json(Some("Seiko 5"), Some("DU"), 3000, Some(("T3", Some(10.0)))),
        );
        write_json(
            dir.path(),
            "chrona-du-old",
            &v2_json(Some("Seiko 5"), Some("DU"), 2000, Some(("T3", Some(999.0)))),
        );
        // DD: both sessions unrated (no summary; summary with no rate) —
        // position must end up None, not fabricated from either.
        write_json(
            dir.path(),
            "chrona-dd-nosummary",
            &v2_json(Some("Seiko 5"), Some("DD"), 2500, None),
        );
        write_json(
            dir.path(),
            "chrona-dd-norate",
            &v2_json(Some("Seiko 5"), Some("DD"), 1000, Some(("T1", None))),
        );
        // CD: exactly one rated session.
        write_json(
            dir.path(),
            "chrona-cd",
            &v2_json(Some("Seiko 5"), Some("CD"), 1500, Some(("T3", Some(-5.0)))),
        );
        // A different watch at DU with a huge rate — must not leak in.
        write_json(
            dir.path(),
            "chrona-omega-du",
            &v2_json(Some("Omega"), Some("DU"), 5000, Some(("T3", Some(-999.0)))),
        );

        let idx = HistoryIndex::scan(dir.path());
        let rates = position_rates(&idx, "Seiko 5");
        assert_eq!(rates.by_code[0], Some(10.0), "DU: newest rated wins");
        assert_eq!(rates.by_code[1], None, "DD: both sessions unrated");
        assert_eq!(rates.by_code[2], None, "CU: no sessions");
        assert_eq!(rates.by_code[3], Some(-5.0), "CD: single rated session");
        assert_eq!(rates.by_code[4], None, "CL: no sessions");
        assert_eq!(rates.by_code[5], None, "CR: no sessions");
        assert_eq!(
            rates.spread,
            Some(15.0),
            "spread over the 2 rated positions DU=10.0, CD=-5.0"
        );
    }

    #[test]
    fn position_rates_spread_none_below_two_rated_positions() {
        let dir = tempfile::tempdir().unwrap();
        write_json(
            dir.path(),
            "chrona-a",
            &v2_json(Some("Solo"), Some("DU"), 1000, Some(("T3", Some(7.0)))),
        );
        let idx = HistoryIndex::scan(dir.path());

        let one_rated = position_rates(&idx, "Solo");
        assert_eq!(one_rated.by_code[0], Some(7.0));
        assert_eq!(one_rated.spread, None, "only 1 rated position");

        let zero_rated = position_rates(&idx, "NoSuchWatch");
        assert_eq!(zero_rated.by_code, [None; 6]);
        assert_eq!(zero_rated.spread, None, "0 indexed sessions for this watch");
    }

    #[test]
    fn bar_frac_table() {
        assert_eq!(bar_frac(7.5), 0.25);
        assert_eq!(bar_frac(30.0), 0.5);
        assert_eq!(bar_frac(-7.5), 0.25, "symmetric in |rate|");
        assert_eq!(bar_frac(0.0), 0.0);
    }
}
