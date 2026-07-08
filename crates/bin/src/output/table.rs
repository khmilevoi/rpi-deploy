pub fn table() -> comfy_table::Table {
    let mut t = comfy_table::Table::new();
    t.load_preset(comfy_table::presets::UTF8_FULL)
        .apply_modifier(comfy_table::modifiers::UTF8_ROUND_CORNERS)
        .set_content_arrangement(comfy_table::ContentArrangement::Dynamic);
    t
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
