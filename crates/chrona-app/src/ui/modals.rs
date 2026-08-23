//! Add-watch modal (M4a Task 6): the toolbar watch combo's "+ Add new
//! watch…" row opens this. The pure outcome-decision function
//! (`add_watch_outcome`) is TDD'd first, below; the egui-facing render
//! function comes after and is exercised only by `cargo build` + the
//! workspace test suite (no window in CI), same split as
//! `controls.rs`/`toolbar.rs`.
//!
//! Metric help modal (M4a Task 7): each metrics card's "?" button opens
//! this with one of the 4 `HELP_TOPICS`. Copy is transcribed VERBATIM,
//! character-for-character, from the committed mockup asset
//! (`docs/superpowers/specs/assets/2026-08-23-chrona-redesign.dc.html`,
//! search `helpTopics = {`) — every dash/space/punctuation mark is
//! significant (this task's report-integrity requirement: the final review
//! diffs these strings against the mockup).

use eframe::egui;

use chrona_session::ConfigStore;

use crate::theme::Palette;

/// What saving the add-watch modal's text field should do, given the
/// trimmed input and the current watch list (behavior contract): blank/
/// whitespace-only input is rejected outright; an exact case-sensitive
/// match against an existing watch selects it rather than adding a
/// duplicate; anything else is a genuinely new watch (trimmed).
#[derive(Debug, Clone, PartialEq)]
pub enum AddWatchOutcome {
    Added(String),
    Selected(usize),
    Rejected,
}

/// Decides [`AddWatchOutcome`] for `input` against `watches`. PURE.
pub fn add_watch_outcome(input: &str, watches: &[String]) -> AddWatchOutcome {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return AddWatchOutcome::Rejected;
    }
    match watches.iter().position(|w| w == trimmed) {
        Some(idx) => AddWatchOutcome::Selected(idx),
        None => AddWatchOutcome::Added(trimmed.to_string()),
    }
}

// ---------------------------------------------------------------------
// Rendering: the add-watch modal. Exercised by `cargo build` + the
// workspace test suite; no window in CI, so nothing here is unit-tested
// directly — see the module doc comment.
// ---------------------------------------------------------------------

/// State for the "Add a watch" modal, owned by `ToolbarState`
/// (`ui::toolbar`).
#[derive(Debug, Clone, Default)]
pub struct AddWatchModalState {
    open: bool,
    input: String,
    /// True for exactly the first `render_add_watch_modal` call after
    /// `open_modal` — triggers the text field's one-shot `request_focus()`
    /// and suppresses that same call's click-outside-closes check (the
    /// click that opened the modal, e.g. the watch combo's "+ Add new
    /// watch…" row, would otherwise immediately close it again, since it
    /// necessarily landed outside the not-yet-shown card).
    just_opened: bool,
}

impl AddWatchModalState {
    /// Opens the modal with a blank input buffer. Called from the watch
    /// combo's "+ Add new watch…" row.
    pub fn open_modal(&mut self) {
        self.open = true;
        self.input.clear();
        self.just_opened = true;
    }
}

fn persist_config_now(config: &ConfigStore) {
    if let Err(e) = config.save() {
        eprintln!("chrona: failed to save config: {e}");
    }
}

/// Renders the modal when `state.open` (no-op otherwise): a full-screen
/// `palette.overlay` scrim (mockup: ~55% alpha) plus a centered card
/// (mockup: title, helper line, autofocused text field, Cancel/Save).
/// Enter (while the field has focus) and the Save button both go through
/// [`add_watch_outcome`]; Esc, the Cancel button, and a click on the scrim
/// outside the card all close without touching `config` (mockup's
/// `closeAddWatch`). A successful save (`Added`/`Selected`) selects the
/// resulting watch into `*selected_watch`, mirrors it into
/// `config.last_watch`, saves `config` immediately (not the debounced save
/// the continuous-drag controls use), and closes.
pub fn render_add_watch_modal(
    ctx: &egui::Context,
    palette: &Palette,
    state: &mut AddWatchModalState,
    config: &mut ConfigStore,
    selected_watch: &mut Option<String>,
) {
    if !state.open {
        return;
    }
    let just_opened = state.just_opened;
    state.just_opened = false;

    let screen = ctx.input(|i| i.content_rect());
    let scrim_layer = egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new("chrona_add_watch_scrim"),
    );
    ctx.layer_painter(scrim_layer).rect_filled(
        screen,
        0.0,
        crate::theme::with_alpha(palette.overlay, 140),
    );

    let mut close = false;
    let mut save = false;

    let card_response = egui::Area::new(egui::Id::new("chrona_add_watch_card"))
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(palette.panel)
                .stroke(egui::Stroke::new(1.0, palette.border2))
                .corner_radius(14.0)
                .inner_margin(22.0)
                .show(ui, |ui| {
                    ui.set_width(356.0_f32.min(screen.width() - 48.0 - 44.0));

                    ui.label(
                        egui::RichText::new("Add a watch")
                            .font(egui::FontId::new(
                                16.0,
                                egui::FontFamily::Name(crate::theme::FAMILY_SEMIBOLD.into()),
                            ))
                            .color(palette.text),
                    );
                    ui.add_space(6.0);
                    ui.label(
                        egui::RichText::new(
                            "Sessions are saved under the watch, so you can track a movement over time.",
                        )
                        .size(13.0)
                        .color(palette.muted),
                    );
                    ui.add_space(14.0);

                    let field_width = ui.available_width();
                    let field = ui.add(
                        egui::TextEdit::singleline(&mut state.input)
                            .hint_text("e.g. Seiko SKX007 · 7S26")
                            .desired_width(field_width),
                    );
                    if just_opened {
                        field.request_focus();
                    }
                    if field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        save = true;
                    }

                    ui.add_space(14.0);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let save_resp = ui.add(
                            egui::Button::new(
                                egui::RichText::new("Save watch")
                                    .color(palette.accent_ink)
                                    .strong(),
                            )
                            .fill(palette.accent),
                        );
                        if save_resp.clicked() {
                            save = true;
                        }
                        let cancel_resp = ui.add(
                            egui::Button::new(egui::RichText::new("Cancel").color(palette.muted))
                                .fill(egui::Color32::TRANSPARENT)
                                .stroke(egui::Stroke::new(1.0, palette.border2)),
                        );
                        if cancel_resp.clicked() {
                            close = true;
                        }
                    });
                });
        })
        .response;

    if !just_opened
        && ctx.input(|i| i.pointer.any_click())
        && ctx
            .input(|i| i.pointer.interact_pos())
            .is_some_and(|pos| !card_response.rect.contains(pos))
    {
        close = true;
    }
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        close = true;
    }

    if save {
        match add_watch_outcome(&state.input, &config.watches) {
            AddWatchOutcome::Rejected => {} // stays open — behavior contract
            AddWatchOutcome::Selected(idx) => {
                let watch = config.watches[idx].clone();
                *selected_watch = Some(watch.clone());
                config.last_watch = Some(watch);
                persist_config_now(config);
                state.open = false;
            }
            AddWatchOutcome::Added(name) => {
                config.watches.push(name.clone());
                *selected_watch = Some(name.clone());
                config.last_watch = Some(name);
                persist_config_now(config);
                state.open = false;
            }
        }
    } else if close {
        state.open = false;
    }
}

// ---------------------------------------------------------------------
// Metric help modal (M4a Task 7).
// ---------------------------------------------------------------------

/// Which help topic is open — mirrors the mockup's `helpTopics` object
/// keys (`rate`/`beatError`/`amplitude`/`beatRate`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HelpTopicId {
    Rate,
    BeatError,
    Amplitude,
    BeatRate,
}

/// One help topic's copy (mockup's `helpTopics` object: title/what/why/
/// good-range strings) — see [`HELP_TOPICS`]'s doc comment.
pub struct HelpTopic {
    pub id: HelpTopicId,
    pub title: &'static str,
    pub what: &'static str,
    pub why: &'static str,
    pub good: &'static str,
}

/// The 4 metric help topics, transcribed VERBATIM character-for-character
/// from the committed mockup asset — see the module doc comment.
pub const HELP_TOPICS: [HelpTopic; 4] = [
    HelpTopic {
        id: HelpTopicId::Rate,
        title: "Rate (s/d)",
        what: "How many seconds per day the watch gains (+) or loses (−) compared to a perfect clock. +12.0 s/d means the watch runs about 12 seconds fast every day.",
        why: "This is the headline accuracy number. A regulator adjusts it by moving the regulator arm or timing screws. Large differences between positions point to poise or hairspring issues.",
        good: "±10 s/d for everyday mechanicals · −4/+6 s/d for COSC chronometers",
    },
    HelpTopic {
        id: HelpTopicId::BeatError,
        title: "Beat error (ms)",
        what: "The difference in time between the tick and the tock — the two halves of the balance swing. Ideally both take exactly the same time.",
        why: "A high beat error wastes power, can make the watch harder to start, and often signals the hairspring collet is off-center. It is fixed by adjusting the stud or collet, not the regulator.",
        good: "under 0.5 ms is excellent · under 1.0 ms is acceptable",
    },
    HelpTopic {
        id: HelpTopicId::Amplitude,
        title: "Amplitude (°)",
        what: "How far the balance wheel swings on each beat, in degrees of rotation. Calculated from the sound of the escapement using the lift angle you set.",
        why: "Amplitude is a health check for the movement: low amplitude suggests thickened oil, worn parts, or a weak mainspring, and usually means worse timekeeping. Durability issues show up here first.",
        good: "250–300° dial up on a full wind · above ~200° after 24h",
    },
    HelpTopic {
        id: HelpTopicId::BeatRate,
        title: "Beat rate (bph)",
        what: "How many beats (ticks) the movement makes per hour — a fixed design property of the movement. 28,800 bph = 8 beats per second (4 Hz).",
        why: "Chrona must know the beat rate to measure anything else; Auto detects it from the audio. If the detected rate looks wrong (e.g. 18,000 for a modern watch), the signal is probably noisy.",
        good: "common rates: 18,000 · 21,600 · 25,200 · 28,800 · 36,000 bph",
    },
];

/// Renders the metric help modal when `open_help.is_some()` (mockup: "Help
/// popup"): overlay (`palette.overlay` at 55% alpha — mirrors
/// `render_add_watch_modal`'s scrim), centered panel (≤480px): title
/// (semibold 16), the "what" paragraph, a `panel2`-filled "WHY IT MATTERS"
/// boxed section (10-rounded), and a "Good range: " line (good-colored,
/// bold lead — mockup's `<span style="color:var(--good); font-weight:600;">
/// Good range: </span>`). ✕, a click on the scrim outside the card, and Esc
/// all close (mockup's `closeHelp`) — same three-way close contract as
/// `render_add_watch_modal`.
///
/// `just_opened_this_frame`: `true` on the exact frame `*open_help`
/// transitions to `Some` (a "?" button's click). `open_help` here is a
/// bare `Option<HelpTopicId>` (per this task's brief) rather than an
/// `AddWatchModalState`-style struct with its own `just_opened` flag, so
/// the caller (`ChronaApp::ui`) computes this itself at the point a card
/// reports a click — without it, the SAME click that opened the modal
/// would register as an "outside" click against the not-yet-shown card and
/// close it again in the same frame (identical bug/fix shape to
/// `render_add_watch_modal`'s own `just_opened`).
pub fn render_help_modal(
    ctx: &egui::Context,
    palette: &Palette,
    open_help: &mut Option<HelpTopicId>,
    just_opened_this_frame: bool,
) {
    let Some(id) = *open_help else {
        return;
    };
    let topic = HELP_TOPICS
        .iter()
        .find(|t| t.id == id)
        .expect("HELP_TOPICS covers every HelpTopicId");

    let screen = ctx.input(|i| i.content_rect());
    let scrim_layer =
        egui::LayerId::new(egui::Order::Foreground, egui::Id::new("chrona_help_scrim"));
    ctx.layer_painter(scrim_layer).rect_filled(
        screen,
        0.0,
        crate::theme::with_alpha(palette.overlay, 140),
    );

    let mut close = false;

    let card_response = egui::Area::new(egui::Id::new("chrona_help_card"))
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(palette.panel)
                .stroke(egui::Stroke::new(1.0, palette.border2))
                .corner_radius(14.0)
                .inner_margin(22.0)
                .show(ui, |ui| {
                    ui.set_width(480.0_f32.min(screen.width() - 48.0 - 44.0));

                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(topic.title)
                                .font(egui::FontId::new(
                                    16.0,
                                    egui::FontFamily::Name(crate::theme::FAMILY_SEMIBOLD.into()),
                                ))
                                .color(palette.text),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let close_resp = ui.add(
                                egui::Button::new(
                                    egui::RichText::new("✕").size(14.0).color(palette.muted),
                                )
                                .fill(palette.panel2)
                                .stroke(egui::Stroke::new(1.0, palette.border2)),
                            );
                            if close_resp.clicked() {
                                close = true;
                            }
                        });
                    });
                    ui.add_space(10.0);

                    ui.label(
                        egui::RichText::new(topic.what)
                            .size(14.0)
                            .color(palette.text),
                    );
                    ui.add_space(12.0);

                    egui::Frame::new()
                        .fill(palette.panel2)
                        .corner_radius(10.0)
                        .inner_margin(egui::Margin::symmetric(14, 12))
                        .show(ui, |ui| {
                            ui.label(
                                egui::RichText::new("WHY IT MATTERS")
                                    .size(11.0)
                                    .color(palette.muted),
                            );
                            ui.add_space(4.0);
                            ui.label(
                                egui::RichText::new(topic.why)
                                    .size(13.0)
                                    .color(palette.text),
                            );
                        });
                    ui.add_space(12.0);

                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            egui::RichText::new("Good range: ")
                                .size(13.0)
                                .strong()
                                .color(palette.good),
                        );
                        ui.label(
                            egui::RichText::new(topic.good)
                                .size(13.0)
                                .color(palette.muted),
                        );
                    });
                });
        })
        .response;

    if !just_opened_this_frame
        && ctx.input(|i| i.pointer.any_click())
        && ctx
            .input(|i| i.pointer.interact_pos())
            .is_some_and(|pos| !card_response.rect.contains(pos))
    {
        close = true;
    }
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        close = true;
    }

    if close {
        *open_help = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_watch_outcome_rejects_blank_and_whitespace() {
        assert_eq!(add_watch_outcome("", &[]), AddWatchOutcome::Rejected);
        assert_eq!(
            add_watch_outcome("   ", &["Seiko 5".to_string()]),
            AddWatchOutcome::Rejected
        );
    }

    #[test]
    fn add_watch_outcome_selects_existing_case_sensitive_exact() {
        let watches = vec!["Seiko 5".to_string(), "Omega".to_string()];
        assert_eq!(
            add_watch_outcome("Omega", &watches),
            AddWatchOutcome::Selected(1)
        );
        assert_eq!(
            add_watch_outcome("  Seiko 5  ", &watches),
            AddWatchOutcome::Selected(0),
            "surrounding whitespace trimmed before matching"
        );
        assert_eq!(
            add_watch_outcome("omega", &watches),
            AddWatchOutcome::Added("omega".to_string()),
            "case-sensitive: lowercase omega does not match Omega"
        );
    }

    #[test]
    fn add_watch_outcome_adds_new_trimmed() {
        let watches = vec!["Seiko 5".to_string()];
        assert_eq!(
            add_watch_outcome("  Omega Speedmaster  ", &watches),
            AddWatchOutcome::Added("Omega Speedmaster".to_string())
        );
    }

    #[test]
    fn help_topics_complete() {
        assert_eq!(HELP_TOPICS.len(), 4);
        for t in &HELP_TOPICS {
            assert!(!t.title.is_empty(), "{:?} title empty", t.id);
            assert!(!t.what.is_empty(), "{:?} what empty", t.id);
            assert!(!t.why.is_empty(), "{:?} why empty", t.id);
            assert!(!t.good.is_empty(), "{:?} good empty", t.id);
        }
        let rate = HELP_TOPICS
            .iter()
            .find(|t| t.id == HelpTopicId::Rate)
            .expect("rate topic present");
        assert!(
            rate.why.contains("regulator"),
            "rate.why must mention 'regulator': {:?}",
            rate.why
        );
        let beat_rate = HELP_TOPICS
            .iter()
            .find(|t| t.id == HelpTopicId::BeatRate)
            .expect("beatRate topic present");
        assert!(
            beat_rate.what.contains("28,800"),
            "beatRate.what must contain '28,800': {:?}",
            beat_rate.what
        );
    }
}
