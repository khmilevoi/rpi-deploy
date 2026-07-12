/// Format a byte count as IEC units (÷1024), one decimal above bytes.
pub fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    if n < 1024 {
        return format!("{n} B");
    }
    let mut val = n as f64;
    let mut unit = 0usize;
    while val >= 1024.0 && unit < UNITS.len() - 1 {
        val /= 1024.0;
        unit += 1;
    }
    format!("{val:.1} {}", UNITS[unit])
}

/// Render `values` as a unicode sparkline of block glyphs. Uses at most the
/// newest `width` values; empty input or zero width → empty string; a single
/// value or a flat series renders a mid-height block.
pub fn sparkline(values: &[f64], width: usize) -> String {
    const BLOCKS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    if values.is_empty() || width == 0 {
        return String::new();
    }
    let slice = if values.len() > width {
        &values[values.len() - width..]
    } else {
        values
    };
    let min = slice.iter().cloned().fold(f64::INFINITY, f64::min);
    let max = slice.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let range = max - min;
    slice
        .iter()
        .map(|&v| {
            let idx = if range <= f64::EPSILON {
                BLOCKS.len() / 2
            } else {
                (((v - min) / range) * (BLOCKS.len() - 1) as f64).round() as usize
            };
            BLOCKS[idx.min(BLOCKS.len() - 1)]
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_bytes_boundaries() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(1023), "1023 B");
        assert_eq!(human_bytes(1024), "1.0 KiB");
        assert_eq!(human_bytes(1024 * 1024), "1.0 MiB");
        assert_eq!(human_bytes(1024 * 1024 * 1024), "1.0 GiB");
        assert_eq!(human_bytes(1024_u64.pow(4)), "1.0 TiB");
        assert_eq!(human_bytes(1536), "1.5 KiB");
    }

    #[test]
    fn sparkline_empty_is_blank() {
        assert_eq!(sparkline(&[], 10), "");
        assert_eq!(sparkline(&[1.0, 2.0], 0), "");
    }

    #[test]
    fn sparkline_single_and_flat_use_mid_block() {
        assert_eq!(sparkline(&[5.0], 10), "▅");
        assert_eq!(sparkline(&[3.0, 3.0, 3.0], 10), "▅▅▅");
    }

    #[test]
    fn sparkline_scales_min_to_max() {
        assert_eq!(sparkline(&[0.0, 5.0, 10.0], 10), "▁▅█");
    }

    #[test]
    fn sparkline_keeps_the_newest_values_when_over_width() {
        // width 2 → drops the oldest (0.0); remaining [5.0,10.0] scale to ▁█
        assert_eq!(sparkline(&[0.0, 5.0, 10.0], 2), "▁█");
    }
}
