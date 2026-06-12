/// "60s" | "2m" | bare seconds -> seconds. Shared by pi.toml and agent.toml.
pub(crate) fn parse_duration_secs(s: &str) -> Result<u64, String> {
    let s = s.trim();
    let (digits, mult) = if let Some(d) = s.strip_suffix('m') {
        (d, 60)
    } else if let Some(d) = s.strip_suffix('s') {
        (d, 1)
    } else {
        (s, 1)
    };
    digits
        .trim()
        .parse::<u64>()
        .map(|n| n * mult)
        .map_err(|_| format!("invalid duration '{s}' (expected like \"60s\" or \"2m\")"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_duration_secs_supports_s_m_and_bare_numbers() {
        assert_eq!(parse_duration_secs("60s").unwrap(), 60);
        assert_eq!(parse_duration_secs("2m").unwrap(), 120);
        assert_eq!(parse_duration_secs("90").unwrap(), 90);
        assert!(parse_duration_secs("soon").is_err());
    }
}
