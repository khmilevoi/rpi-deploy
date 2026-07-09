/// "60s" | "2m" | "2h" | bare seconds -> seconds.
pub(crate) fn parse_duration_secs(s: &str) -> Result<u64, String> {
    let s = s.trim();
    let (digits, mult) = if let Some(d) = s.strip_suffix('m') {
        (d, 60)
    } else if let Some(d) = s.strip_suffix('h') {
        (d, 3600)
    } else if let Some(d) = s.strip_suffix('s') {
        (d, 1)
    } else {
        (s, 1)
    };
    digits
        .trim()
        .parse::<u64>()
        .map(|n| n * mult)
        .map_err(|_| format!("invalid duration '{s}' (expected like \"60s\", \"2m\" or \"2h\")"))
}

/// Wall-clock elapsed time for the deploy stamp. Under a minute: one decimal
/// second (`12.4s`). A minute or more: `1m03s` (seconds zero-padded).
// Allow removed in Task 4 once `deploy_stamp` calls this from the live deploy path.
#[allow(dead_code)]
pub(crate) fn format_elapsed(d: std::time::Duration) -> String {
    let secs = d.as_secs_f64();
    if secs < 60.0 {
        format!("{secs:.1}s")
    } else {
        let total = secs.round() as u64;
        format!("{}m{:02}s", total / 60, total % 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_duration_secs_supports_s_m_h_and_bare_numbers() {
        assert_eq!(parse_duration_secs("60s").unwrap(), 60);
        assert_eq!(parse_duration_secs("2m").unwrap(), 120);
        assert_eq!(parse_duration_secs("2h").unwrap(), 7200);
        assert_eq!(parse_duration_secs("90").unwrap(), 90);
        assert!(parse_duration_secs("soon").is_err());
    }

    #[test]
    fn format_elapsed_sub_minute_uses_one_decimal_second() {
        use std::time::Duration;
        assert_eq!(format_elapsed(Duration::from_millis(12_400)), "12.4s");
        assert_eq!(format_elapsed(Duration::from_millis(500)), "0.5s");
    }

    #[test]
    fn format_elapsed_over_a_minute_uses_m_and_zero_padded_s() {
        use std::time::Duration;
        assert_eq!(format_elapsed(Duration::from_secs(63)), "1m03s");
        assert_eq!(format_elapsed(Duration::from_secs(600)), "10m00s");
    }
}
