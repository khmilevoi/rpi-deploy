use comfy_table::{Attribute, Cell, Color};

use crate::output::Sem;

pub fn table() -> comfy_table::Table {
    let mut t = comfy_table::Table::new();
    t.load_preset(comfy_table::presets::UTF8_FULL)
        .apply_modifier(comfy_table::modifiers::UTF8_ROUND_CORNERS)
        .set_content_arrangement(comfy_table::ContentArrangement::Dynamic);
    // comfy-table auto-suppresses styling off a TTY, but doesn't honour
    // NO_COLOR itself — do it here so tables match the rest of the CLI.
    if std::env::var_os("NO_COLOR").is_some() {
        t.force_no_tty();
    }
    t
}

/// Accent + bold header cells (accent colour from the active theme).
pub fn header<const N: usize>(cols: [&str; N]) -> Vec<Cell> {
    cols.iter()
        .map(|c| {
            let cell = Cell::new(c).add_attribute(Attribute::Bold);
            match super::theme::theme()
                .accent
                .table_color(super::theme::truecolor_enabled())
            {
                Some(colour) => cell.fg(colour),
                None => cell,
            }
        })
        .collect()
}

fn sem_colour(sem: Sem) -> Option<Color> {
    let t = super::theme::theme();
    let tc = super::theme::truecolor_enabled();
    match sem {
        Sem::Success => t.success.table_color(tc),
        Sem::Error => t.error.table_color(tc),
        Sem::Warn => t.warn.table_color(tc),
        Sem::Accent => t.accent.table_color(tc),
        Sem::Muted | Sem::Neutral | Sem::Frame => None,
    }
}

/// Uncoloured value cell.
pub fn cell(text: impl Into<String>) -> Cell {
    Cell::new(text.into())
}

/// Value cell coloured by semantic role (Neutral/Muted = no colour).
pub fn cell_sem(text: impl Into<String>, sem: Sem) -> Cell {
    let c = Cell::new(text.into());
    match sem_colour(sem) {
        Some(col) => c.fg(col),
        None => c,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_renders_header_and_rows() {
        let mut t = table();
        t.set_header(vec!["NAME", "BRANCH"]);
        t.add_row(vec!["rateme", "main"]);
        let rendered = t.to_string();
        assert!(rendered.contains("NAME"), "{rendered}");
        assert!(rendered.contains("rateme"), "{rendered}");
        assert!(rendered.contains("main"), "{rendered}");
    }

    #[test]
    fn sem_colour_follows_the_theme() {
        let t = super::super::theme::theme();
        let tc = super::super::theme::truecolor_enabled();
        assert_eq!(sem_colour(Sem::Accent), t.accent.table_color(tc));
        assert_eq!(sem_colour(Sem::Success), t.success.table_color(tc));
        assert_eq!(sem_colour(Sem::Warn), t.warn.table_color(tc));
        assert_eq!(sem_colour(Sem::Error), t.error.table_color(tc));
        assert_eq!(sem_colour(Sem::Neutral), None);
        assert_eq!(sem_colour(Sem::Muted), None);
    }
}
