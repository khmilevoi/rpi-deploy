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

/// Cyan + bold header cells.
pub fn header<const N: usize>(cols: [&str; N]) -> Vec<Cell> {
    cols.iter()
        .map(|c| Cell::new(c).fg(Color::Cyan).add_attribute(Attribute::Bold))
        .collect()
}

fn sem_colour(sem: Sem) -> Option<Color> {
    match sem {
        Sem::Success => Some(Color::Green),
        Sem::Error => Some(Color::Red),
        Sem::Warn => Some(Color::Yellow),
        Sem::Accent => Some(Color::Cyan),
        Sem::Muted | Sem::Neutral => None,
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
}
