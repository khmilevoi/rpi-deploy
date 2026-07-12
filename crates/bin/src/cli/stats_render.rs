use crate::output;
use crate::proto::StatsReportDto;

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

/// Assemble the whole static `rpi stats` view as a String (colours applied
/// only on a TTY via comfy-table; plain text under tests/pipes).
pub fn render_stats_static(report: &StatsReportDto) -> String {
    use std::fmt::Write as _;

    let h = &report.host;
    let mem_pct = if h.mem_total_bytes > 0 {
        h.mem_used_bytes as f64 / h.mem_total_bytes as f64 * 100.0
    } else {
        0.0
    };
    let temp_cell = match h.temp_celsius {
        Some(c) => format!("{c:.1}°C"),
        None => "n/a".to_string(),
    };

    let mut host_table = output::table();
    host_table.set_header(output::header(["CPU", "MEM", "TEMP", "DISK", "UPTIME"]));
    host_table.add_row(vec![
        output::cell_sem(
            format!("{:.1}%", h.cpu_percent),
            output::usage_sem(h.cpu_percent),
        ),
        output::cell_sem(
            format!(
                "{}/{} ({:.0}%)",
                human_bytes(h.mem_used_bytes),
                human_bytes(h.mem_total_bytes),
                mem_pct
            ),
            output::usage_sem(mem_pct),
        ),
        output::cell(temp_cell),
        output::cell_sem(
            format!("{}%", h.disk_used_percent),
            output::usage_sem(h.disk_used_percent as f64),
        ),
        output::cell(crate::cli::commands::human_duration(h.uptime_secs)),
    ]);

    let mut out = String::new();
    let _ = writeln!(out, "{host_table}");

    if !report.host_history.is_empty() {
        let width = 60;
        let cpu: Vec<f64> = report.host_history.iter().map(|s| s.cpu_percent).collect();
        let _ = writeln!(out, "CPU%  {}", sparkline(&cpu, width));
        let temps: Vec<f64> = report
            .host_history
            .iter()
            .filter_map(|s| s.temp_celsius)
            .collect();
        if !temps.is_empty() {
            let _ = writeln!(out, "TEMP  {}", sparkline(&temps, width));
        }
    }

    if !report.projects.is_empty() {
        let mut services = output::table();
        services.set_header(output::header(["PROJECT", "SERVICE", "CPU", "MEM"]));
        for p in &report.projects {
            for s in &p.services {
                let mem = if s.mem_limit_bytes == 0 {
                    "n/a".to_string()
                } else {
                    let pct = s.mem_used_bytes as f64 / s.mem_limit_bytes as f64 * 100.0;
                    format!(
                        "{}/{} ({:.0}%)",
                        human_bytes(s.mem_used_bytes),
                        human_bytes(s.mem_limit_bytes),
                        pct
                    )
                };
                services.add_row(vec![
                    output::cell(p.project.clone()),
                    output::cell(s.service.clone()),
                    output::cell_sem(
                        format!("{:.1}%", s.cpu_percent),
                        output::usage_sem(s.cpu_percent),
                    ),
                    output::cell(mem),
                ]);
            }
        }
        let _ = writeln!(out, "{services}");
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::{
        HostSampleDto, HostStatsDto, ProjectStatsDto, ServiceStatsDto, StatsReportDto,
    };

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

    fn report(temp: Option<f64>, history: Vec<HostSampleDto>, mem_limit: u64) -> StatsReportDto {
        StatsReportDto {
            host: HostStatsDto {
                cpu_percent: 12.5,
                mem_used_bytes: 1024 * 1024 * 1024,
                mem_total_bytes: 8 * 1024 * 1024 * 1024,
                disk_used_percent: 40,
                uptime_secs: 3661,
                temp_celsius: temp,
            },
            projects: vec![ProjectStatsDto {
                project: "app".into(),
                services: vec![ServiceStatsDto {
                    service: "valkey".into(),
                    cpu_percent: 0.2,
                    mem_used_bytes: 0,
                    mem_limit_bytes: mem_limit,
                }],
                last_deploy: None,
            }],
            host_history: history,
        }
    }

    fn sample(cpu: f64, temp: Option<f64>) -> HostSampleDto {
        HostSampleDto {
            at_ms: 1,
            cpu_percent: cpu,
            mem_used_bytes: 1,
            mem_total_bytes: 2,
            temp_celsius: temp,
        }
    }

    #[test]
    fn static_view_shows_na_for_missing_temp_and_zero_mem_limit() {
        let out = render_stats_static(&report(None, vec![], 0));
        assert!(out.contains("n/a"), "temp n/a and mem n/a: {out}");
        assert!(out.contains("1.0 GiB"), "human bytes used: {out}");
    }

    #[test]
    fn static_view_renders_sparkline_rows_when_history_present() {
        let history = vec![sample(0.0, Some(40.0)), sample(10.0, Some(45.0))];
        let out = render_stats_static(&report(Some(45.0), history, 1024 * 1024 * 512));
        assert!(out.contains("CPU"), "cpu sparkline label: {out}");
        assert!(
            out.chars().any(|c| ('▁'..='█').contains(&c)),
            "sparkline glyph present: {out}"
        );
        assert!(out.contains("45.0"), "temp shown: {out}");
    }
}
