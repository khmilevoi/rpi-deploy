use std::time::Duration;

use super::theme;

/// Top row colour (pink `#F06CA0`) of the logo gradient.
const PINK: (u8, u8, u8) = (240, 108, 160);
/// Bottom row colour (raspberry `#C51A4A`) of the logo gradient.
const RASPBERRY: (u8, u8, u8) = (197, 26, 74);

/// The five right-pointing triangle rows, top→bottom, each filled with its
/// density-ramp glyph. Widths 2,4,6,4,2 make the point; the ramp
/// `░ ▒ ▓ ▓ █` darkens downward.
fn triangle_row(i: usize) -> String {
    const RAMP: [char; 5] = ['░', '▒', '▓', '▓', '█'];
    const WIDTH: [usize; 5] = [2, 4, 6, 4, 2];
    RAMP[i].to_string().repeat(WIDTH[i])
}

fn lerp(a: u8, b: u8, t: f32) -> u8 {
    (a as f32 + (b as f32 - a as f32) * t).round() as u8
}

/// Per-row gradient colour, pink (row 0) → raspberry (row 4).
fn row_rgb(i: usize) -> (u8, u8, u8) {
    let t = i as f32 / 4.0;
    (
        lerp(PINK.0, RASPBERRY.0, t),
        lerp(PINK.1, RASPBERRY.1, t),
        lerp(PINK.2, RASPBERRY.2, t),
    )
}

/// Raw 24-bit SGR wrap. Used only when truecolor is active.
fn sgr_truecolor(text: &str, (r, g, b): (u8, u8, u8)) -> String {
    format!("\u{1b}[38;2;{r};{g};{b}m{text}\u{1b}[39m")
}

/// Colour a triangle row for the current terminal: truecolor escape when
/// available, else the nearest xterm-256 via `console`; plain when colours off.
fn paint_row(text: &str, rgb: (u8, u8, u8)) -> String {
    if !console::colors_enabled() {
        return text.to_string();
    }
    if theme::truecolor_enabled() {
        sgr_truecolor(text, rgb)
    } else {
        let idx = theme::rgb_to_ansi256(rgb.0, rgb.1, rgb.2);
        console::Style::new()
            .color256(idx)
            .apply_to(text)
            .to_string()
    }
}

/// Wordmark styling per row: bold on the name row, dim on the URL row.
fn style_word(row: usize, word: &str) -> String {
    match row {
        1 => console::Style::new().bold().apply_to(word).to_string(),
        3 => console::Style::new().dim().apply_to(word).to_string(),
        _ => word.to_string(),
    }
}

/// Does this terminal render the block/emoji glyphs? Mirrors how the `▸`
/// marker degrades — non-unicode (and non-TTY) terminals get the plain form.
fn wants_unicode() -> bool {
    console::Term::stderr().features().wants_emoji()
}

pub fn stderr_is_tty() -> bool {
    console::Term::stderr().is_term()
}

/// Assemble the banner. `unicode=false` degrades to a one-line wordmark.
/// `line1` sits beside triangle row 1, `line2` row 2, `line3` (optional) row 3.
fn render_banner_inner(unicode: bool, line1: &str, line2: &str, line3: Option<&str>) -> String {
    if !unicode {
        return match line3 {
            Some(l3) => format!("rpi — {line2}  ({l3})"),
            None => format!("rpi — {line2}"),
        };
    }
    let words = ["", line1, line2, line3.unwrap_or(""), ""];
    (0..5)
        .map(|i| {
            let tri = paint_row(&format!("{:<6}", triangle_row(i)), row_rgb(i));
            if words[i].is_empty() {
                tri
            } else {
                format!("{tri}  {}", style_word(i, words[i]))
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Deploy header: triangle + `r p i` / `deploy · <project>`.
pub fn deploy_banner(project: &str) -> String {
    render_banner_inner(
        wants_unicode(),
        "r p i",
        &format!("deploy · {project}"),
        None,
    )
}

/// Brand banner for bare `rpi` / `--version`: triangle + version, tagline, URL.
pub fn brand_banner(version: &str) -> String {
    render_banner_inner(
        wants_unicode(),
        &format!("r p i v{version}"),
        "deploy anything to your Pi",
        Some("rpi.iiskelo.com"),
    )
}

pub enum StampOutcome {
    Success,
    Superseded,
    Failed,
}

/// Inner stamp builder with the unicode decision injected for testing. Returns
/// the summary text only (no marker/colour); callers pass it to the pane's
/// `finish_*` which add the `▸` marker and semantic colour.
fn deploy_stamp_inner(
    unicode: bool,
    outcome: StampOutcome,
    project: &str,
    url: Option<&str>,
    elapsed: Duration,
) -> String {
    let elapsed = crate::duration::format_elapsed(elapsed);
    match outcome {
        StampOutcome::Success => {
            let check = if unicode { "✓" } else { "ok" };
            let arrow = if unicode { "→" } else { "->" };
            let dest = url.map(|u| format!("  {arrow}  {u}")).unwrap_or_default();
            format!("deployed {check} {project}{dest} ({elapsed})")
        }
        StampOutcome::Superseded => {
            format!("deploy superseded — {project} (a newer deploy replaced this one) ({elapsed})")
        }
        StampOutcome::Failed => {
            let cross = if unicode { "✗" } else { "x" };
            format!("deploy failed {cross} {project} — see log above ({elapsed})")
        }
    }
}

pub fn deploy_stamp(
    outcome: StampOutcome,
    project: &str,
    url: Option<&str>,
    elapsed: Duration,
) -> String {
    deploy_stamp_inner(wants_unicode(), outcome, project, url, elapsed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn triangle_rows_have_the_expected_gradient_glyphs_and_widths() {
        assert_eq!(triangle_row(0), "░░");
        assert_eq!(triangle_row(1), "▒▒▒▒");
        assert_eq!(triangle_row(2), "▓▓▓▓▓▓");
        assert_eq!(triangle_row(3), "▓▓▓▓");
        assert_eq!(triangle_row(4), "██");
    }

    #[test]
    fn row_rgb_sweeps_pink_top_to_raspberry_bottom() {
        assert_eq!(row_rgb(0), (240, 108, 160)); // pink
        assert_eq!(row_rgb(4), (197, 26, 74)); // raspberry
                                               // middle row is strictly between the endpoints on every channel
        let (r, g, b) = row_rgb(2);
        assert!((197..=240).contains(&r) && (26..=108).contains(&g) && (74..=160).contains(&b));
    }

    #[test]
    fn sgr_truecolor_wraps_text_in_a_24bit_escape() {
        assert_eq!(
            sgr_truecolor("X", (197, 26, 74)),
            "\u{1b}[38;2;197;26;74mX\u{1b}[39m"
        );
    }

    #[test]
    fn unicode_deploy_banner_has_triangle_and_wordmark_no_ansi_when_colours_off() {
        // Colours are off under captured test output, so paint_row is a no-op.
        let s = render_banner_inner(true, "r p i", "deploy · myboard", None);
        assert!(s.contains("▒▒▒▒"), "{s:?}");
        assert!(s.contains("▓▓▓▓▓▓"), "{s:?}");
        assert!(s.contains("r p i"), "{s:?}");
        assert!(s.contains("deploy · myboard"), "{s:?}");
        assert!(!s.contains('\u{1b}'), "{s:?}");
    }

    #[test]
    fn ascii_banner_falls_back_to_a_plain_wordmark() {
        let s = render_banner_inner(false, "r p i v1.2.3", "tagline", Some("rpi.iiskelo.com"));
        assert!(s.contains("rpi"), "{s:?}");
        assert!(s.contains("tagline"), "{s:?}");
        assert!(!s.contains('░') && !s.contains('▓'), "{s:?}");
        assert!(!s.contains('\u{1b}'), "{s:?}");
    }

    #[test]
    fn stamp_success_carries_glyph_project_url_and_elapsed() {
        let uni = deploy_stamp_inner(
            true,
            StampOutcome::Success,
            "myboard",
            Some("rpi.iiskelo.com"),
            Duration::from_millis(12_400),
        );
        assert!(uni.contains("✓"), "{uni:?}");
        assert!(uni.contains("myboard"), "{uni:?}");
        assert!(uni.contains("rpi.iiskelo.com"), "{uni:?}");
        assert!(uni.contains("12.4s"), "{uni:?}");

        let ascii = deploy_stamp_inner(
            false,
            StampOutcome::Success,
            "myboard",
            None,
            Duration::from_secs(1),
        );
        assert!(ascii.contains("ok"), "{ascii:?}");
        assert!(!ascii.contains('✓'), "{ascii:?}");
        assert!(
            !ascii.contains('→'),
            "no arrow when url is absent: {ascii:?}"
        );
    }

    #[test]
    fn stamp_failed_uses_the_cross_glyph_and_superseded_is_neutral_text() {
        let failed = deploy_stamp_inner(
            true,
            StampOutcome::Failed,
            "api",
            None,
            Duration::from_secs(2),
        );
        assert!(
            failed.contains("✗") && failed.contains("failed"),
            "{failed:?}"
        );
        let sup = deploy_stamp_inner(
            true,
            StampOutcome::Superseded,
            "api",
            None,
            Duration::from_secs(2),
        );
        assert!(sup.to_lowercase().contains("superseded"), "{sup:?}");
    }
}
