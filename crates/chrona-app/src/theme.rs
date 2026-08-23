//! Theme foundation (spec §1): the color palette, embedded fonts, and the
//! `egui::Visuals` built from that palette. `Palette` is the single source
//! of `Color32` literals for the app — every other module draws colors from
//! a `&'static Palette` rather than calling `Color32::from_rgb` itself.

use eframe::egui;
use egui::{Color32, CornerRadius, FontDefinitions, FontFamily, Stroke};

/// Which persisted theme is active (spec §1: dark is the default; the
/// toolbar toggle switches instantly and the choice is saved to
/// `config.toml`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Theme {
    Dark,
    Light,
}

/// Named egui font family for Archivo SemiBold (headings/emphasis, spec §1).
pub const FAMILY_SEMIBOLD: &str = "archivo-semibold";
/// Named egui font family for Archivo Bold (headings/emphasis, spec §1).
pub const FAMILY_BOLD: &str = "archivo-bold";

/// One token per named color in the spec §1 palette table. Both themes
/// define every field, so a token is always available regardless of which
/// theme is active.
#[derive(Debug, Clone, Copy)]
pub struct Palette {
    pub bg: Color32,
    pub panel: Color32,
    pub panel2: Color32,
    pub border: Color32,
    pub border2: Color32,
    pub text: Color32,
    pub muted: Color32,
    pub faint: Color32,
    pub grid: Color32,
    pub grid0: Color32,
    pub chartbg: Color32,
    pub accent: Color32,
    pub accent_ink: Color32,
    pub tick: Color32,
    pub good: Color32,
    pub warnbg: Color32,
    pub warnfg: Color32,
    pub rec: Color32,
    pub overlay: Color32,
    /// Fixed white, both themes (M4a Task 6 pre-review fix): text painted
    /// on top of a `rec`-filled surface (the Record button's label while
    /// recording) needs to stay legible regardless of theme, unlike
    /// `accent_ink`, which flips dark/light with `accent`'s own contrast.
    pub rec_ink: Color32,
}

// Spec §1 table, dark column. Hex values are binding — do not "improve" them.
static DARK_PALETTE: Palette = Palette {
    bg: Color32::from_rgb(0x09, 0x0e, 0x11),
    panel: Color32::from_rgb(0x12, 0x17, 0x1a),
    panel2: Color32::from_rgb(0x1b, 0x20, 0x24),
    border: Color32::from_rgb(0x25, 0x2a, 0x2d),
    border2: Color32::from_rgb(0x33, 0x39, 0x3d),
    text: Color32::from_rgb(0xe5, 0xe8, 0xeb),
    muted: Color32::from_rgb(0x81, 0x87, 0x8c),
    faint: Color32::from_rgb(0x64, 0x6a, 0x6e),
    grid: Color32::from_rgb(0x1a, 0x1d, 0x20),
    grid0: Color32::from_rgb(0x36, 0x3b, 0x3f),
    chartbg: Color32::from_rgb(0x07, 0x0a, 0x0c),
    accent: Color32::from_rgb(0x4a, 0xb8, 0xe8),
    accent_ink: Color32::from_rgb(0x08, 0x0c, 0x0f),
    tick: Color32::from_rgb(0xe7, 0xb6, 0x43),
    good: Color32::from_rgb(0x53, 0xbe, 0x70),
    warnbg: Color32::from_rgb(0x3f, 0x29, 0x03),
    warnfg: Color32::from_rgb(0xf2, 0xc8, 0x6c),
    rec: Color32::from_rgb(0xc9, 0x2f, 0x33),
    overlay: Color32::from_rgb(0x02, 0x04, 0x05),
    rec_ink: Color32::from_rgb(0xff, 0xff, 0xff),
};

// Spec §1 table, light column.
static LIGHT_PALETTE: Palette = Palette {
    bg: Color32::from_rgb(0xf2, 0xf4, 0xf5),
    panel: Color32::from_rgb(0xff, 0xff, 0xff),
    panel2: Color32::from_rgb(0xec, 0xef, 0xf1),
    border: Color32::from_rgb(0xd5, 0xd8, 0xda),
    border2: Color32::from_rgb(0xba, 0xbe, 0xc1),
    text: Color32::from_rgb(0x1b, 0x20, 0x24),
    muted: Color32::from_rgb(0x5e, 0x64, 0x68),
    faint: Color32::from_rgb(0x75, 0x7b, 0x80),
    grid: Color32::from_rgb(0xe2, 0xe5, 0xe7),
    grid0: Color32::from_rgb(0xba, 0xbe, 0xc1),
    chartbg: Color32::from_rgb(0xfb, 0xfc, 0xfd),
    accent: Color32::from_rgb(0x00, 0x7c, 0xb8),
    accent_ink: Color32::from_rgb(0xfc, 0xfc, 0xfc),
    tick: Color32::from_rgb(0xce, 0x87, 0x1b),
    good: Color32::from_rgb(0x25, 0x98, 0x4d),
    warnbg: Color32::from_rgb(0xf7, 0xe6, 0xc3),
    warnfg: Color32::from_rgb(0x8a, 0x56, 0x00),
    rec: Color32::from_rgb(0xc9, 0x2f, 0x33),
    overlay: Color32::from_rgb(0x02, 0x04, 0x05),
    rec_ink: Color32::from_rgb(0xff, 0xff, 0xff),
};

impl Palette {
    /// The fixed palette for `theme` (spec §1 table).
    pub fn of(theme: Theme) -> &'static Palette {
        match theme {
            Theme::Dark => &DARK_PALETTE,
            Theme::Light => &LIGHT_PALETTE,
        }
    }
}

/// Derives a low-alpha variant of a palette color (Task 6 precedent: e.g.
/// `replay_mode_line`'s soft accent fill, the add-watch/help modal scrims,
/// the record button's pulsing dot). The single canonical spot for this —
/// callers reach for `theme::with_alpha` instead of writing
/// `Color32::from_rgba_unmultiplied` themselves, so every alpha-derived
/// color still traces back to this module, same spirit as the "no ad-hoc
/// `Color32` construction outside the palette module" rule (that rule is
/// about opaque literals; this is the one sanctioned exception, for
/// deriving alpha from a color the palette already produced — `c` is
/// always expected to be a `Palette` field, never a fresh literal).
pub(crate) fn with_alpha(c: Color32, a: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), a)
}

/// Builds the font set embedded into the binary (spec §1): `Proportional`
/// gets Archivo Regular ahead of egui's built-in fallbacks (so glyphs
/// outside Archivo's coverage, e.g. emoji, still render); `Monospace` gets
/// Fragment Mono Regular the same way; Archivo SemiBold/Bold are registered
/// as named families for headings/emphasis. Pure and side-effect free so
/// the family wiring is unit-testable without a live `egui::Context`.
fn font_definitions() -> FontDefinitions {
    let mut fonts = FontDefinitions::default();

    fonts.font_data.insert(
        "archivo-regular".to_owned(),
        std::sync::Arc::new(egui::FontData::from_static(include_bytes!(
            "../assets/fonts/Archivo-Regular.ttf"
        ))),
    );
    fonts.font_data.insert(
        FAMILY_SEMIBOLD.to_owned(),
        std::sync::Arc::new(egui::FontData::from_static(include_bytes!(
            "../assets/fonts/Archivo-SemiBold.ttf"
        ))),
    );
    fonts.font_data.insert(
        FAMILY_BOLD.to_owned(),
        std::sync::Arc::new(egui::FontData::from_static(include_bytes!(
            "../assets/fonts/Archivo-Bold.ttf"
        ))),
    );
    fonts.font_data.insert(
        "fragment-mono-regular".to_owned(),
        std::sync::Arc::new(egui::FontData::from_static(include_bytes!(
            "../assets/fonts/FragmentMono-Regular.ttf"
        ))),
    );

    fonts
        .families
        .entry(FontFamily::Proportional)
        .or_default()
        .insert(0, "archivo-regular".to_owned());
    fonts
        .families
        .entry(FontFamily::Monospace)
        .or_default()
        .insert(0, "fragment-mono-regular".to_owned());
    fonts.families.insert(
        FontFamily::Name(FAMILY_SEMIBOLD.into()),
        vec![FAMILY_SEMIBOLD.to_owned()],
    );
    fonts.families.insert(
        FontFamily::Name(FAMILY_BOLD.into()),
        vec![FAMILY_BOLD.to_owned()],
    );

    fonts
}

/// Registers the embedded fonts with `ctx`. Call once at startup — font
/// data never changes after (only the palette/visuals do, on theme switch).
pub fn install_fonts(ctx: &egui::Context) {
    ctx.set_fonts(font_definitions());
}

/// Builds the `egui::Visuals` for `theme` from its `Palette`. Pure so the
/// palette→visuals mapping is unit-testable without a live `egui::Context`.
fn visuals_for(theme: Theme) -> egui::Visuals {
    let p = Palette::of(theme);
    let mut visuals = match theme {
        Theme::Dark => egui::Visuals::dark(),
        Theme::Light => egui::Visuals::light(),
    };

    visuals.panel_fill = p.bg;
    visuals.window_fill = p.bg;
    visuals.hyperlink_color = p.accent;
    visuals.selection.bg_fill = p.accent;
    visuals.window_corner_radius = CornerRadius::same(12);

    for w in [
        &mut visuals.widgets.noninteractive,
        &mut visuals.widgets.inactive,
        &mut visuals.widgets.hovered,
        &mut visuals.widgets.active,
        &mut visuals.widgets.open,
    ] {
        w.bg_fill = p.panel2;
        w.weak_bg_fill = p.panel2;
        w.bg_stroke = Stroke::new(1.0, p.border2);
        w.fg_stroke = Stroke::new(1.0, p.text);
        w.corner_radius = CornerRadius::same(8);
    }

    visuals
}

/// Applies `theme`'s palette to `ctx`. Idempotent and cheap to call again on
/// every theme switch, not just at startup — it always pins `ctx`'s theme
/// preference to `theme` explicitly (rather than leaving it on the default
/// "follow the OS"), so the persisted user choice sticks regardless of the
/// system theme.
pub fn apply_style(ctx: &egui::Context, theme: Theme) {
    let egui_theme = match theme {
        Theme::Dark => egui::Theme::Dark,
        Theme::Light => egui::Theme::Light,
    };
    ctx.set_theme(egui_theme);
    ctx.set_visuals(visuals_for(theme));
}

/// Parses the persisted `theme` config value (spec §1: absent or invalid →
/// dark).
pub fn theme_from_config(s: Option<&str>) -> Theme {
    match s {
        Some("light") => Theme::Light,
        _ => Theme::Dark,
    }
}

/// The config string for `t` (round-trips through `theme_from_config`).
pub fn theme_config_value(t: Theme) -> &'static str {
    match t {
        Theme::Dark => "dark",
        Theme::Light => "light",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palette_tokens_exact() {
        let d = Palette::of(Theme::Dark);
        assert_eq!(d.bg, egui::Color32::from_rgb(0x09, 0x0e, 0x11));
        assert_eq!(d.accent, egui::Color32::from_rgb(0x4a, 0xb8, 0xe8));
        assert_eq!(d.tick, egui::Color32::from_rgb(0xe7, 0xb6, 0x43));
        let l = Palette::of(Theme::Light);
        assert_eq!(l.bg, egui::Color32::from_rgb(0xf2, 0xf4, 0xf5));
        assert_eq!(l.accent, egui::Color32::from_rgb(0x00, 0x7c, 0xb8));
        assert_ne!(d.text, l.text);
    }

    /// Independently re-derives every token's expected `Color32` from the
    /// spec §1 hex table (rather than importing values from the
    /// implementation above), so a transcription slip in `DARK_PALETTE` /
    /// `LIGHT_PALETTE` — e.g. two tokens swapped — actually gets caught.
    #[test]
    fn palette_completeness_every_token_both_themes() {
        fn hex(h: u32) -> Color32 {
            Color32::from_rgb((h >> 16) as u8, (h >> 8) as u8, h as u8)
        }

        // (token name, accessor, dark hex, light hex)
        type Row = (&'static str, fn(&Palette) -> Color32, u32, u32);
        let table: &[Row] = &[
            ("bg", |p| p.bg, 0x090e11, 0xf2f4f5),
            ("panel", |p| p.panel, 0x12171a, 0xffffff),
            ("panel2", |p| p.panel2, 0x1b2024, 0xeceff1),
            ("border", |p| p.border, 0x252a2d, 0xd5d8da),
            ("border2", |p| p.border2, 0x33393d, 0xbabec1),
            ("text", |p| p.text, 0xe5e8eb, 0x1b2024),
            ("muted", |p| p.muted, 0x81878c, 0x5e6468),
            ("faint", |p| p.faint, 0x646a6e, 0x757b80),
            ("grid", |p| p.grid, 0x1a1d20, 0xe2e5e7),
            ("grid0", |p| p.grid0, 0x363b3f, 0xbabec1),
            ("chartbg", |p| p.chartbg, 0x070a0c, 0xfbfcfd),
            ("accent", |p| p.accent, 0x4ab8e8, 0x007cb8),
            ("accent_ink", |p| p.accent_ink, 0x080c0f, 0xfcfcfc),
            ("tick", |p| p.tick, 0xe7b643, 0xce871b),
            ("good", |p| p.good, 0x53be70, 0x25984d),
            ("warnbg", |p| p.warnbg, 0x3f2903, 0xf7e6c3),
            ("warnfg", |p| p.warnfg, 0xf2c86c, 0x8a5600),
            ("rec", |p| p.rec, 0xc92f33, 0xc92f33),
            ("overlay", |p| p.overlay, 0x020405, 0x020405),
            ("rec_ink", |p| p.rec_ink, 0xffffff, 0xffffff),
        ];

        let dark = Palette::of(Theme::Dark);
        let light = Palette::of(Theme::Light);
        for (name, get, dark_hex, light_hex) in table.iter().copied() {
            assert_eq!(get(dark), hex(dark_hex), "dark.{name}");
            assert_eq!(get(light), hex(light_hex), "light.{name}");
        }
    }

    #[test]
    fn with_alpha_is_a_faithful_from_rgba_unmultiplied_passthrough() {
        let c = Color32::from_rgb(0x4a, 0xb8, 0xe8); // dark accent
        // `Color32` stores premultiplied alpha (`from_rgba_unmultiplied`'s
        // own doc comment / impl), so a translucent result's r/g/b are NOT
        // expected to equal the input's — only that `with_alpha` computes
        // exactly what calling `from_rgba_unmultiplied` directly would.
        assert_eq!(
            with_alpha(c, 40),
            Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), 40)
        );
        // At full alpha, `from_rgba_unmultiplied` takes its `255 =>
        // from_rgb` fast path, so r/g/b DO round-trip exactly here.
        let opaque = with_alpha(c, 255);
        assert_eq!(
            (opaque.r(), opaque.g(), opaque.b(), opaque.a()),
            (0x4a, 0xb8, 0xe8, 255)
        );
    }

    #[test]
    fn theme_config_roundtrip() {
        assert_eq!(theme_from_config(Some("light")), Theme::Light);
        assert_eq!(theme_from_config(Some("dark")), Theme::Dark);
        assert_eq!(theme_from_config(Some("mauve")), Theme::Dark); // invalid → dark
        assert_eq!(theme_from_config(None), Theme::Dark);
        assert_eq!(theme_config_value(Theme::Light), "light");
    }

    #[test]
    fn theme_config_value_roundtrips_through_parse() {
        for t in [Theme::Dark, Theme::Light] {
            assert_eq!(theme_from_config(Some(theme_config_value(t))), t);
        }
    }

    #[test]
    fn font_definitions_register_expected_families() {
        let defs = font_definitions();

        assert_eq!(
            defs.families[&FontFamily::Proportional][0],
            "archivo-regular"
        );
        assert_eq!(
            defs.families[&FontFamily::Monospace][0],
            "fragment-mono-regular"
        );
        assert_eq!(
            defs.families[&FontFamily::Name(FAMILY_SEMIBOLD.into())],
            vec![FAMILY_SEMIBOLD.to_owned()]
        );
        assert_eq!(
            defs.families[&FontFamily::Name(FAMILY_BOLD.into())],
            vec![FAMILY_BOLD.to_owned()]
        );

        for name in [
            "archivo-regular",
            FAMILY_SEMIBOLD,
            FAMILY_BOLD,
            "fragment-mono-regular",
        ] {
            assert!(
                defs.font_data.contains_key(name),
                "missing font_data entry for {name}"
            );
        }
    }

    #[test]
    fn visuals_for_maps_palette_onto_style() {
        for theme in [Theme::Dark, Theme::Light] {
            let p = Palette::of(theme);
            let v = visuals_for(theme);

            assert_eq!(v.dark_mode, theme == Theme::Dark);
            assert_eq!(v.panel_fill, p.bg);
            assert_eq!(v.window_fill, p.bg);
            assert_eq!(v.hyperlink_color, p.accent);
            assert_eq!(v.selection.bg_fill, p.accent);
            assert_eq!(v.window_corner_radius, CornerRadius::same(12));

            for w in [
                v.widgets.noninteractive,
                v.widgets.inactive,
                v.widgets.hovered,
                v.widgets.active,
                v.widgets.open,
            ] {
                assert_eq!(w.bg_fill, p.panel2);
                assert_eq!(w.weak_bg_fill, p.panel2);
                assert_eq!(w.bg_stroke.color, p.border2);
                assert_eq!(w.fg_stroke.color, p.text);
                assert_eq!(w.corner_radius, CornerRadius::same(8));
            }
        }
    }

    #[test]
    fn apply_style_targets_the_matching_theme_slot_and_is_idempotent() {
        let ctx = egui::Context::default();

        apply_style(&ctx, Theme::Light);
        assert_eq!(ctx.theme(), egui::Theme::Light);
        assert_eq!(
            ctx.style_of(egui::Theme::Light).visuals.panel_fill,
            Palette::of(Theme::Light).bg
        );

        // Re-applying (as the toolbar toggle does) must land on the newly
        // chosen theme's slot, not get stuck on the first one.
        apply_style(&ctx, Theme::Dark);
        assert_eq!(ctx.theme(), egui::Theme::Dark);
        assert_eq!(
            ctx.style_of(egui::Theme::Dark).visuals.panel_fill,
            Palette::of(Theme::Dark).bg
        );
    }
}
