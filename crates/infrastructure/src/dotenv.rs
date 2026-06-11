//! Dotenv parse/serialize shared by the CLI (`pi env send` reads a local
//! file) and the agent (bundle payload <-> workdir `.env`) (§10, §12).

use pi_domain::entities::EnvBundle;

/// `KEY=VALUE` lines; skips blanks and `#` comments; strips an optional
/// `export ` prefix and one pair of matching single/double quotes.
pub fn parse(text: &str) -> Result<EnvBundle, String> {
    let mut bundle = EnvBundle::default();
    for (i, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line);
        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| format!("line {}: expected KEY=VALUE", i + 1))?;
        let key = key.trim();
        if !is_valid_key(key) {
            return Err(format!("line {}: invalid key '{key}'", i + 1));
        }
        bundle.vars.insert(key.to_string(), unquote(value.trim()).to_string());
    }
    Ok(bundle)
}

/// `[A-Za-z_][A-Za-z0-9_]*` - also used to validate PUT /env payloads.
pub fn is_valid_key(key: &str) -> bool {
    let mut chars = key.chars();
    matches!(chars.next(), Some('A'..='Z' | 'a'..='z' | '_'))
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Deterministic KEY=VALUE serialization (BTreeMap order, trailing newline).
pub fn serialize(bundle: &EnvBundle) -> String {
    let mut out = String::new();
    for (key, value) in &bundle.vars {
        out.push_str(key);
        out.push('=');
        out.push_str(value);
        out.push('\n');
    }
    out
}

fn unquote(value: &str) -> &str {
    for quote in ['"', '\''] {
        if value.len() >= 2 && value.starts_with(quote) && value.ends_with(quote) {
            return &value[1..value.len() - 1];
        }
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_comments_export_quotes_and_equals_in_value() {
        let text =
            "# comment\n\nDB_URL=postgres://u:p@db/x?a=b\nexport TOKEN=\"abc=def\"\nNAME='single'\n";
        let bundle = parse(text).unwrap();
        assert_eq!(bundle.vars["DB_URL"], "postgres://u:p@db/x?a=b");
        assert_eq!(bundle.vars["TOKEN"], "abc=def");
        assert_eq!(bundle.vars["NAME"], "single");
        assert_eq!(bundle.vars.len(), 3);
    }

    #[test]
    fn rejects_invalid_keys_and_lines_without_equals() {
        assert!(parse("1BAD=x").unwrap_err().contains("line 1"));
        assert!(parse("OK=1\nno-equals-here").unwrap_err().contains("line 2"));
        assert!(is_valid_key("_OK_2"));
        assert!(!is_valid_key("BAD-DASH"));
        assert!(!is_valid_key(""));
    }

    #[test]
    fn serialize_then_parse_roundtrips() {
        let mut bundle = EnvBundle::default();
        bundle.vars.insert("B".into(), "2".into());
        bundle.vars.insert("A".into(), "1".into());
        let text = serialize(&bundle);
        assert_eq!(text, "A=1\nB=2\n");
        assert_eq!(parse(&text).unwrap(), bundle);
    }
}
