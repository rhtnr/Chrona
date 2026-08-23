//! Add-watch modal (M4a Task 6): the toolbar watch combo's "+ Add new
//! watch…" row opens this. The pure outcome-decision function
//! (`add_watch_outcome`) is TDD'd first, below; the egui-facing render
//! function comes after and is exercised only by `cargo build` + the
//! workspace test suite (no window in CI), same split as
//! `controls.rs`/`session_panel.rs`.

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

fn with_alpha(c: egui::Color32, a: u8) -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), a)
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
    ctx.layer_painter(scrim_layer)
        .rect_filled(screen, 0.0, with_alpha(palette.overlay, 140));

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
}
