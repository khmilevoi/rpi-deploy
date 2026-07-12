use std::sync::OnceLock;

use console::Style;

/// How a semantic role is painted. `Rgb` is reduced to the nearest xterm-256
/// index so `console` (which has no truecolor support) and `comfy-table`
/// render the identical colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Paint {
    #[allow(dead_code)] // no theme constructs this yet; reserved for a future default/no-op role
    Default,
    Cyan,
    Green,
    Yellow,
    Red,
    Rgb(u8, u8, u8),
}

impl Paint {
    /// Base `console` style (no modifiers) for this paint.
    pub fn console(self) -> Style {
        let s = Style::new();
        match self {
            Paint::Default => s,
            Paint::Cyan => s.cyan(),
            Paint::Green => s.green(),
            Paint::Yellow => s.yellow(),
            Paint::Red => s.red(),
            Paint::Rgb(r, g, b) => s.color256(rgb_to_ansi256(r, g, b)),
        }
    }

    /// Foreground colour for `comfy-table` cells; `None` = uncoloured.
    /// `Rgb` is exact under truecolor, else reduced to the nearest xterm-256.
    pub fn table_color(self, truecolor: bool) -> Option<comfy_table::Color> {
        use comfy_table::Color;
        match self {
            Paint::Default => None,
            Paint::Cyan => Some(Color::Cyan),
            Paint::Green => Some(Color::Green),
            Paint::Yellow => Some(Color::Yellow),
            Paint::Red => Some(Color::Red),
            Paint::Rgb(r, g, b) => Some(if truecolor {
                Color::Rgb { r, g, b }
            } else {
                Color::AnsiValue(rgb_to_ansi256(r, g, b))
            }),
        }
    }

    /// Foreground colour for a `ratatui` widget (chart line, sparkline);
    /// `None` = unstyled. `Rgb` is exact under truecolor, else reduced to the
    /// nearest xterm-256 — the same downgrade as `table_color`, so every
    /// backend paints the identical hue.
    pub fn ratatui_color(self, truecolor: bool) -> Option<ratatui::style::Color> {
        use ratatui::style::Color;
        match self {
            Paint::Default => None,
            Paint::Cyan => Some(Color::Cyan),
            Paint::Green => Some(Color::Green),
            Paint::Yellow => Some(Color::Yellow),
            Paint::Red => Some(Color::Red),
            Paint::Rgb(r, g, b) => Some(if truecolor {
                Color::Rgb(r, g, b)
            } else {
                Color::Indexed(rgb_to_ansi256(r, g, b))
            }),
        }
    }

    /// Colour token for an `indicatif` template (`{spinner:.<token>}`).
    /// `indicatif` parses it with `console::Style::from_dotted_str`, which
    /// accepts ANSI names and numeric `0-255` tokens. `None` = no colour.
    pub fn template_token(self) -> Option<String> {
        match self {
            Paint::Default => None,
            Paint::Cyan => Some("cyan".into()),
            Paint::Green => Some("green".into()),
            Paint::Yellow => Some("yellow".into()),
            Paint::Red => Some("red".into()),
            Paint::Rgb(r, g, b) => Some(rgb_to_ansi256(r, g, b).to_string()),
        }
    }
}

/// Nearest xterm-256 index for an RGB colour: the best of the 6x6x6 cube
/// (16..232) and the grey ramp (232..256) by squared RGB distance.
pub fn rgb_to_ansi256(r: u8, g: u8, b: u8) -> u8 {
    const LEVELS: [i32; 6] = [0, 95, 135, 175, 215, 255];
    fn nearest_level(v: u8) -> usize {
        let mut best = 0;
        for (i, l) in LEVELS.iter().enumerate() {
            if (v as i32 - l).abs() < (v as i32 - LEVELS[best]).abs() {
                best = i;
            }
        }
        best
    }
    fn dist(a: (i32, i32, i32), b: (i32, i32, i32)) -> i32 {
        (a.0 - b.0).pow(2) + (a.1 - b.1).pow(2) + (a.2 - b.2).pow(2)
    }
    let want = (r as i32, g as i32, b as i32);
    let (ri, gi, bi) = (nearest_level(r), nearest_level(g), nearest_level(b));
    let cube = (LEVELS[ri], LEVELS[gi], LEVELS[bi]);
    let cube_idx = (16 + 36 * ri + 6 * gi + bi) as u8;
    // Grey ramp entry i (0..24) has value 8 + 10*i.
    let grey_i = ((want.0 + want.1 + want.2) / 3 - 8).clamp(0, 230) / 10;
    let grey_v = 8 + 10 * grey_i;
    if dist(want, (grey_v, grey_v, grey_v)) < dist(want, cube) {
        (232 + grey_i) as u8
    } else {
        cube_idx
    }
}

/// A theme controls palette and glyphs only — structural output decisions
/// (which lines get a marker, streams, prefixes) are theme-independent.
pub struct Theme {
    pub accent: Paint,
    pub success: Paint,
    pub warn: Paint,
    pub error: Paint,
    /// Marker glyph: (unicode, ascii fallback), rendered via `console::Emoji`.
    pub marker: (&'static str, &'static str),
    /// true: the marker is always painted accent (the brand mark);
    /// false: the marker follows the line's own semantic colour.
    pub marker_accent: bool,
}

impl Theme {
    /// Brand theme from rpi-deploy-site: raspberry accent + triangle logo
    /// marker, site green/amber for success/warn.
    pub fn raspberry() -> Self {
        Theme {
            accent: Paint::Rgb(197, 26, 74),   // #C51A4A -> 161
            success: Paint::Rgb(117, 169, 40), // #75A928 -> 106
            warn: Paint::Rgb(212, 160, 23),    // #d4a017 -> 178
            error: Paint::Red,
            marker: ("▸", ">"),
            marker_accent: true,
        }
    }

    /// The pre-brand look: cyan accent, named ANSI colours, dot marker.
    pub fn classic() -> Self {
        Theme {
            accent: Paint::Cyan,
            success: Paint::Green,
            warn: Paint::Yellow,
            error: Paint::Red,
            marker: ("●", "*"),
            marker_accent: false,
        }
    }

    /// `PI_THEME` value -> theme. Unknown values silently fall back to the
    /// default so scripts never break on a typo.
    pub fn from_env_value(value: Option<&str>) -> Self {
        match value {
            Some("classic") => Self::classic(),
            _ => Self::raspberry(),
        }
    }
}

static ACTIVE: OnceLock<Theme> = OnceLock::new();

/// The process-wide theme, chosen once from `PI_THEME` on first use.
pub fn theme() -> &'static Theme {
    ACTIVE.get_or_init(|| Theme::from_env_value(std::env::var("PI_THEME").ok().as_deref()))
}

/// True when 24-bit colour should be emitted: colours are enabled and the
/// terminal advertises truecolor via `COLORTERM`. Pure core in `truecolor_from`.
pub fn truecolor_enabled() -> bool {
    truecolor_from(
        std::env::var("COLORTERM").ok().as_deref(),
        console::colors_enabled(),
    )
}

fn truecolor_from(colorterm: Option<&str>, colors_on: bool) -> bool {
    colors_on && matches!(colorterm, Some("truecolor") | Some("24bit"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brand_colours_map_to_expected_ansi256() {
        assert_eq!(rgb_to_ansi256(197, 26, 74), 161); // raspberry #C51A4A
        assert_eq!(rgb_to_ansi256(117, 169, 40), 106); // green #75A928
        assert_eq!(rgb_to_ansi256(212, 160, 23), 178); // amber #d4a017
    }

    #[test]
    fn greys_use_the_grey_ramp_and_extremes_use_the_cube() {
        assert_eq!(rgb_to_ansi256(128, 128, 128), 244); // mid grey -> ramp
        assert_eq!(rgb_to_ansi256(0, 0, 0), 16); // exact cube black
        assert_eq!(rgb_to_ansi256(255, 255, 255), 231); // exact cube white
    }

    #[test]
    fn env_value_selects_theme_and_unknown_falls_back() {
        assert!(
            Theme::from_env_value(None).marker_accent,
            "default = raspberry"
        );
        assert_eq!(Theme::from_env_value(Some("classic")).accent, Paint::Cyan);
        assert_eq!(Theme::from_env_value(Some("classic")).marker, ("●", "*"));
        assert!(
            Theme::from_env_value(Some("purple")).marker_accent,
            "unknown value falls back to raspberry"
        );
        assert_eq!(Theme::from_env_value(None).marker, ("▸", ">"));
    }

    #[test]
    fn paint_converts_to_each_backend() {
        // A style with no attributes never emits ANSI, colours on or off.
        assert_eq!(Paint::Default.console().apply_to("x").to_string(), "x");
        assert_eq!(Paint::Default.template_token(), None);
        assert_eq!(Paint::Cyan.template_token().as_deref(), Some("cyan"));
        assert_eq!(
            Paint::Rgb(197, 26, 74).template_token().as_deref(),
            Some("161")
        );
        assert!(Paint::Default.table_color(false).is_none());
        assert!(matches!(
            Paint::Rgb(197, 26, 74).table_color(false),
            Some(comfy_table::Color::AnsiValue(161))
        ));
        assert!(matches!(
            Paint::Rgb(197, 26, 74).table_color(true),
            Some(comfy_table::Color::Rgb {
                r: 197,
                g: 26,
                b: 74
            })
        ));
        assert!(matches!(
            Paint::Green.table_color(false),
            Some(comfy_table::Color::Green)
        ));
    }

    #[test]
    fn paint_converts_to_ratatui_colour() {
        use ratatui::style::Color;
        assert_eq!(Paint::Default.ratatui_color(true), None);
        assert_eq!(Paint::Cyan.ratatui_color(false), Some(Color::Cyan));
        // Raspberry accent: exact under truecolor, nearest xterm-256 otherwise.
        assert_eq!(
            Paint::Rgb(197, 26, 74).ratatui_color(true),
            Some(Color::Rgb(197, 26, 74))
        );
        assert_eq!(
            Paint::Rgb(197, 26, 74).ratatui_color(false),
            Some(Color::Indexed(161))
        );
    }

    #[test]
    fn truecolor_needs_colours_on_and_a_truecolor_colorterm() {
        assert!(truecolor_from(Some("truecolor"), true));
        assert!(truecolor_from(Some("24bit"), true));
        assert!(!truecolor_from(Some("truecolor"), false)); // colours off
        assert!(!truecolor_from(Some("256color"), true));
        assert!(!truecolor_from(None, true));
    }
}
