//! Self-contained HTML export report (design spec §9): one file, inline
//! CSS only, no JS, no external requests, printable. Built with plain
//! string pushes rather than a templating engine — the whole document is
//! small and the point is that it's trivially auditable. Pure logic only:
//! no `egui`/`Color32` here. The inline CSS below hardcodes its own hex
//! colors (spec §1's palette) — that's fine for a standalone document; the
//! "no `Color32::from_rgb` outside the palette module" rule is about egui
//! widget code, not CSS strings in a generated file.

use crate::history::{POSITIONS, PositionRates, SessionEntry};

/// Everything `render_report` needs, already resolved by the caller (Task
/// 9's UI) — this module never reads config, the engine, or the
/// filesystem beyond what's handed to it here.
pub struct ReportInput<'a> {
    pub watch: &'a str,
    pub app_version: &'a str,
    pub lift_deg: f64,
    pub bph_mode: &'a str,
    pub averaging_s: f64,
    /// Calibration offset, ppm. Exactly `0.0` (the app's own default
    /// sentinel — never a rounding artifact) means "no calibration
    /// applied", which the report calls out with an explicit warning
    /// rather than silently presenting the numbers as trustworthy.
    pub ppm: f64,
    pub rates: &'a PositionRates,
    pub sessions: &'a [&'a SessionEntry],
    pub exported_unix_s: u64,
}

/// Renders one self-contained HTML report for `input.watch`: current
/// config (with an explicit uncalibrated warning when `ppm == 0.0`), the
/// position-comparison table (with `Δ spread` when there's one to show),
/// and the full session-history table for the watch. No JS, no external
/// requests — every value is inlined at render time, so the file is
/// exactly what it says it is, forever (no dead links).
pub fn render_report(input: &ReportInput) -> String {
    let mut html = String::new();
    html.push_str("<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n<meta charset=\"UTF-8\">\n");
    html.push_str(&format!(
        "<title>Chrona report — {}</title>\n",
        esc(input.watch)
    ));
    html.push_str(STYLE);
    html.push_str("</head>\n<body>\n");

    html.push_str("<header>\n");
    html.push_str(&format!("<h1>{}</h1>\n", esc(input.watch)));
    html.push_str(&format!(
        "<p class=\"meta\">chrona {} &middot; exported {}</p>\n",
        esc(input.app_version),
        fmt_date(input.exported_unix_s)
    ));
    html.push_str("</header>\n");

    html.push_str("<section>\n<h2>Configuration</h2>\n<table class=\"kv\">\n");
    html.push_str(&format!(
        "<tr><th>Lift angle</th><td>{:.1}&deg;</td></tr>\n",
        input.lift_deg
    ));
    html.push_str(&format!(
        "<tr><th>BPH mode</th><td>{}</td></tr>\n",
        esc(input.bph_mode)
    ));
    html.push_str(&format!(
        "<tr><th>Averaging</th><td>{:.0} s</td></tr>\n",
        input.averaging_s
    ));
    html.push_str(&format!(
        "<tr><th>Calibration</th><td>{:+.1} ppm",
        input.ppm
    ));
    if input.ppm == 0.0 {
        html.push_str(
            " <span class=\"warn\">&#9888; uncalibrated — measured against the system \
             clock, no calibration offset applied</span>",
        );
    }
    html.push_str("</td></tr>\n</table>\n</section>\n");

    html.push_str("<section>\n<h2>Position comparison</h2>\n<table>\n");
    html.push_str("<tr><th>Position</th><th>Rate</th></tr>\n");
    for (i, (code, label)) in POSITIONS.iter().enumerate() {
        html.push_str(&format!(
            "<tr><td>{code} &mdash; {label}</td><td>{}</td></tr>\n",
            fmt_rate(input.rates.by_code[i])
        ));
    }
    html.push_str("</table>\n");
    if let Some(spread) = input.rates.spread {
        html.push_str(&format!(
            "<p class=\"spread\">Δ spread: {spread:.1} s/d</p>\n"
        ));
    }
    html.push_str("</section>\n");

    html.push_str("<section>\n<h2>Session history</h2>\n<table>\n");
    html.push_str(
        "<tr><th>Date</th><th>Position</th><th>Rate</th><th>Beat error</th>\
         <th>Amplitude</th><th>Tier</th></tr>\n",
    );
    for entry in input.sessions {
        let position = esc(entry.position.as_deref().unwrap_or("—"));
        let (rate, beat_error, amplitude, tier) = match &entry.summary {
            Some(s) => (
                fmt_rate(s.rate_s_per_day),
                fmt_beat_error(s.beat_error_ms),
                fmt_amplitude(s.amplitude_deg),
                esc(&s.tier),
            ),
            None => (
                "—".to_string(),
                "—".to_string(),
                "—".to_string(),
                "—".to_string(),
            ),
        };
        html.push_str(&format!(
            "<tr><td>{}</td><td>{position}</td><td>{rate}</td><td>{beat_error}</td>\
             <td>{amplitude}</td><td>{tier}</td></tr>\n",
            fmt_date(entry.started_unix_s),
        ));
    }
    html.push_str("</table>\n</section>\n");

    html.push_str(
        "<footer>\n<p>Tiers: T1 = rate only, T2 = + beat error, T3 = full \
         (+ amplitude). Metrics were recorded once, at each session's end — \
         not a live reading.</p>\n</footer>\n",
    );

    html.push_str("</body>\n</html>\n");
    html
}

/// The report's inline stylesheet (design spec §1 palette, light theme —
/// chosen for a document meant to be printed). No `@import`, no external
/// fonts: generic system-font stacks only, so the file stays fully
/// self-contained.
const STYLE: &str = r#"<style>
  :root {
    --bg: #f2f4f5; --panel: #ffffff; --border: #d5d8da; --text: #1b2024;
    --muted: #5e6468; --faint: #757b80; --accent: #007cb8;
    --warnbg: #f7e6c3; --warnfg: #8a5600;
  }
  * { box-sizing: border-box; }
  body {
    margin: 0; padding: 32px; background: var(--bg); color: var(--text);
    font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Arial, sans-serif;
    line-height: 1.45;
  }
  header, section, footer {
    max-width: 780px; margin: 0 auto 20px; padding: 18px 24px;
  }
  section, footer {
    background: var(--panel); border: 1px solid var(--border); border-radius: 12px;
  }
  h1 { margin: 0 0 4px; font-size: 24px; }
  h2 {
    margin: 0 0 12px; font-size: 13px; text-transform: uppercase;
    letter-spacing: 0.06em; color: var(--muted);
  }
  p.meta {
    margin: 0; color: var(--muted);
    font-family: ui-monospace, "SF Mono", Menlo, Consolas, monospace; font-size: 13px;
  }
  table {
    width: 100%; border-collapse: collapse;
    font-family: ui-monospace, "SF Mono", Menlo, Consolas, monospace; font-size: 13px;
  }
  th, td { text-align: left; padding: 6px 10px; border-bottom: 1px solid var(--border); }
  th { color: var(--muted); font-weight: 600; }
  table.kv th { width: 160px; color: var(--text); }
  .warn {
    display: inline-block; margin-left: 8px; padding: 2px 8px; border-radius: 99px;
    background: var(--warnbg); color: var(--warnfg); font-size: 12px;
  }
  p.spread { margin: 12px 0 0; font-weight: 600; }
  footer p { margin: 0; color: var(--faint); font-size: 12px; }
  @media print {
    body { background: #fff; padding: 0; }
    section, footer { border: none; }
  }
</style>
"#;

/// `"+12.3 s/d"`, or `"—"` when absent. Same numeral shape as the app's
/// live rate card, without a per-render calibration badge: a report row
/// has no per-session calibration state to attribute (`SessionSummary`
/// doesn't carry one) — the report's one calibration caveat is stated
/// once, up front, against the *current* config (`ReportInput::ppm`).
fn fmt_rate(rate: Option<f64>) -> String {
    match rate {
        Some(r) => format!("{r:+.1} s/d"),
        None => "—".to_string(),
    }
}

/// `"0.8 ms"`, or `"—"` when absent (below Tier 2 at that session).
fn fmt_beat_error(be: Option<f64>) -> String {
    match be {
        Some(v) => format!("{v:.1} ms"),
        None => "—".to_string(),
    }
}

/// `"270&deg;"`, or `"—"` when absent (below Tier 3 at that session).
fn fmt_amplitude(a: Option<f64>) -> String {
    match a {
        Some(v) => format!("{v:.0}&deg;"),
        None => "—".to_string(),
    }
}

/// `"YYYY-MM-DD"`, UTC.
fn fmt_date(unix_s: u64) -> String {
    let (y, m, d) = chrona_session::civil::ymd(unix_s);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Filesystem/URL-safe slug: lowercased, ASCII-alphanumeric runs kept
/// verbatim, every run of anything else (spaces, punctuation, non-ASCII)
/// collapsed to a single `-`, and the result trimmed of leading/trailing
/// `-` — so `"Seiko SKX007 · 7S26"` becomes `"seiko-skx007-7s26"`, never
/// `"-seiko-skx007-7s26-"` or a run of doubled dashes.
pub fn watch_slug(watch: &str) -> String {
    let mut out = String::with_capacity(watch.len());
    let mut last_was_dash = false;
    for c in watch.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            last_was_dash = false;
        } else if !last_was_dash {
            out.push('-');
            last_was_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

/// Default save-dialog filename (spec §9): `chrona-report-<watch
/// slug>-<YYYY-MM-DD>.html`, dated by `unix_s` (the export time) in UTC.
pub fn default_report_name(watch: &str, unix_s: u64) -> String {
    let (y, m, d) = chrona_session::civil::ymd(unix_s);
    format!(
        "chrona-report-{}-{y:04}-{m:02}-{d:02}.html",
        watch_slug(watch)
    )
}

/// Minimal HTML-escaping for user-controlled strings embedded in the
/// report (watch names, freeform positions): `&` first (so it doesn't
/// double-escape the entities the other two replacements introduce), then
/// `<` and `>`. Not a full HTML-entity escaper (no quotes/apostrophes) —
/// everything this module embeds goes into element text content, never an
/// attribute value, so `&`/`<`/`>` are the only characters that can change
/// markup structure.
fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrona_session::SessionSummary;
    use std::path::PathBuf;

    fn rated_entry(position: &str, rate: f64, started_unix_s: u64) -> SessionEntry {
        SessionEntry {
            wav_path: PathBuf::from(format!("/rec/chrona-{started_unix_s}.wav")),
            watch: Some("Seiko SKX007".to_string()),
            position: Some(position.to_string()),
            started_unix_s,
            summary: Some(SessionSummary {
                tier: "T3".to_string(),
                rate_s_per_day: Some(rate),
                beat_error_ms: Some(0.6),
                amplitude_deg: Some(261.0),
                bph_detected: Some(28_800.0),
                duration_s: 45.0,
            }),
        }
    }

    fn unrated_entry(position: &str, started_unix_s: u64) -> SessionEntry {
        SessionEntry {
            wav_path: PathBuf::from(format!("/rec/chrona-{started_unix_s}.wav")),
            watch: Some("Seiko SKX007".to_string()),
            position: Some(position.to_string()),
            started_unix_s,
            summary: None,
        }
    }

    #[test]
    fn esc_escapes_ampersand_and_angle_brackets() {
        assert_eq!(esc("<b>Tag & Co</b>"), "&lt;b&gt;Tag &amp; Co&lt;/b&gt;");
        assert_eq!(esc("plain text"), "plain text");
    }

    #[test]
    fn watch_slug_examples() {
        assert_eq!(watch_slug("Seiko SKX007 · 7S26"), "seiko-skx007-7s26");
        assert_eq!(watch_slug("  Omega  "), "omega");
        assert_eq!(watch_slug("A/B//C"), "a-b-c");
    }

    #[test]
    fn default_report_name_formats_date() {
        // Same pinned instant chrona_session::civil's own tests verify:
        // 1_766_995_200 == 2025-12-29 UTC.
        assert_eq!(
            default_report_name("Seiko SKX007 · 7S26", 1_766_995_200),
            "chrona-report-seiko-skx007-7s26-2025-12-29.html"
        );
    }

    #[test]
    fn render_report_calibrated_case_with_spread_and_escaping() {
        let rated = rated_entry("DU", 10.3, 1_766_990_000);
        let unrated = unrated_entry("DD", 1_766_980_000);
        let sessions: Vec<&SessionEntry> = vec![&rated, &unrated];
        let rates = PositionRates {
            by_code: [Some(10.3), None, None, Some(-4.7), None, None],
            spread: Some(15.0),
        };
        let input = ReportInput {
            watch: "Seiko SKX007 <b>Test</b>",
            app_version: "0.9.0-test",
            lift_deg: 52.0,
            bph_mode: "Auto",
            averaging_s: 30.0,
            ppm: 12.5,
            rates: &rates,
            sessions: &sessions,
            exported_unix_s: 1_766_995_200,
        };

        let html = render_report(&input);

        assert!(
            html.starts_with("<!DOCTYPE html>"),
            "must start with the doctype"
        );
        assert!(html.contains("<style>"), "must have inline CSS");
        assert!(!html.contains("http"), "must be fully self-contained");
        assert!(html.contains("chrona"), "must credit the app");
        assert!(html.contains("0.9.0-test"), "must include the app version");
        assert!(
            html.contains("&lt;b&gt;Tag &amp; Co&lt;/b&gt;")
                || html.contains("&lt;b&gt;Test&lt;/b&gt;"),
            "watch name must be escaped somewhere in the doc"
        );
        assert!(
            !html.contains("<b>Test</b>"),
            "watch name must NOT appear unescaped"
        );
        assert!(html.contains("DU"), "must show the rated position code");
        assert!(html.contains("DD"), "must show the unrated position code");
        assert!(
            html.contains("+10.3 s/d"),
            "must show the rated session's formatted rate"
        );
        assert!(
            html.contains('—'),
            "unrated session row must render an em-dash"
        );
        assert!(
            html.contains("Δ spread"),
            "spread is Some, must show the Δ spread line"
        );
        assert!(
            !html.contains("uncalibrated"),
            "ppm != 0.0, must NOT show the uncalibrated warning"
        );
    }

    #[test]
    fn render_report_uncalibrated_case_without_spread() {
        let unrated = unrated_entry("CL", 1_766_980_000);
        let sessions: Vec<&SessionEntry> = vec![&unrated];
        let rates = PositionRates {
            by_code: [None; 6],
            spread: None,
        };
        let input = ReportInput {
            watch: "Omega Speedmaster",
            app_version: "0.9.0-test",
            lift_deg: 51.5,
            bph_mode: "Free",
            averaging_s: 10.0,
            ppm: 0.0,
            rates: &rates,
            sessions: &sessions,
            exported_unix_s: 1_766_995_200,
        };

        let html = render_report(&input);

        assert!(html.starts_with("<!DOCTYPE html>"));
        assert!(html.contains("<style>"));
        assert!(!html.contains("http"));
        assert!(html.contains("Omega Speedmaster"));
        assert!(
            html.contains("uncalibrated"),
            "ppm == 0.0 must show the uncalibrated warning"
        );
        assert!(
            !html.contains("Δ spread"),
            "spread is None, must NOT show the Δ spread line"
        );
        assert!(html.contains("CL"), "must show the sole session's position");
    }

    #[test]
    fn render_report_lists_every_canonical_position_label() {
        let sessions: Vec<&SessionEntry> = Vec::new();
        let rates = PositionRates {
            by_code: [None; 6],
            spread: None,
        };
        let input = ReportInput {
            watch: "No Sessions Yet",
            app_version: "0.9.0-test",
            lift_deg: 52.0,
            bph_mode: "Auto",
            averaging_s: 30.0,
            ppm: 3.0,
            rates: &rates,
            sessions: &sessions,
            exported_unix_s: 1_766_995_200,
        };
        let html = render_report(&input);
        for (code, _label) in POSITIONS {
            assert!(
                html.contains(code),
                "position-comparison table must list {code} even with no data"
            );
        }
    }
}
