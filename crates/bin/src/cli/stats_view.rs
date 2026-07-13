//! Terminal-independent view-model for the `rpi stats -w` dashboard. Kept pure
//! so the whole mapping (uptime formatting, memory-bar scaling, status role,
//! layout-mode selection) is unit-testable without a real terminal.

use ratatui::layout::Rect;

use crate::cli::stats_render::human_bytes;
use crate::output::{self, Sem};
use crate::proto::StatsReportDto;

/// One metric card (cpu / mem / temp): a large current value plus a mini series.
pub struct MetricCard {
    pub label: &'static str,
    /// Numeric part only (drawn large); e.g. "0.9", "11.2", "48.5", or "n/a".
    pub value: String,
    /// Unit suffix drawn small beside the value; "%", "°C", or "".
    pub unit: &'static str,
    /// (x, y) history points for the mini line chart.
    pub series: Vec<(f64, f64)>,
}

/// One row of the services table.
pub struct ServiceRow {
    pub project: String,
    pub service: String,
    pub cpu: String,
    /// Memory used, human-readable ("192.4 MiB"), or "n/a" when no limit.
    pub mem: String,
    /// 0.0..=1.0 relative to the heaviest service; None when memory is n/a.
    pub mem_ratio: Option<f64>,
    /// Docker state string ("running"); empty when the agent didn't report it.
    pub state: String,
    /// Semantic role for the status pill. Neutral when state is unknown.
    pub sem: Sem,
}

pub struct StatsView {
    pub disk_percent: u8,
    pub uptime: String,
    pub cpu: MetricCard,
    pub mem: MetricCard,
    pub temp: MetricCard,
    pub services: Vec<ServiceRow>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LayoutMode {
    Rich,
    Compact,
    Tiny,
}

/// Pick a layout by terminal size so the tall rich dashboard degrades instead
/// of overflowing. Thresholds are height-first (cards are the tall part).
pub fn layout_mode(area: Rect) -> LayoutMode {
    if area.height < 10 {
        LayoutMode::Tiny
    } else if area.height < 20 || area.width < 70 {
        LayoutMode::Compact
    } else {
        LayoutMode::Rich
    }
}

/// Days-aware uptime: "4d 6h" / "6h 5m" / "3m".
pub fn format_uptime(secs: u64) -> String {
    let days = secs / 86_400;
    let hours = (secs % 86_400) / 3600;
    let minutes = (secs % 3600) / 60;
    if days > 0 {
        format!("{days}d {hours}h")
    } else if hours > 0 {
        format!("{hours}h {minutes}m")
    } else {
        format!("{minutes}m")
    }
}

pub fn build_view(report: &StatsReportDto) -> StatsView {
    let h = &report.host;

    let cpu_points: Vec<(f64, f64)> = report
        .host_history
        .iter()
        .enumerate()
        .map(|(i, s)| (i as f64, s.cpu_percent))
        .collect();
    let mem_points: Vec<(f64, f64)> = report
        .host_history
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let pct = if s.mem_total_bytes > 0 {
                s.mem_used_bytes as f64 / s.mem_total_bytes as f64 * 100.0
            } else {
                0.0
            };
            (i as f64, pct)
        })
        .collect();
    let temp_points: Vec<(f64, f64)> = report
        .host_history
        .iter()
        .enumerate()
        .filter_map(|(i, s)| s.temp_celsius.map(|t| (i as f64, t)))
        .collect();

    let mem_pct = if h.mem_total_bytes > 0 {
        h.mem_used_bytes as f64 / h.mem_total_bytes as f64 * 100.0
    } else {
        0.0
    };
    let (temp_value, temp_unit): (String, &'static str) = match h.temp_celsius {
        Some(c) => (format!("{c:.1}"), "°C"),
        None => ("n/a".to_string(), ""),
    };

    // Memory bar is scaled to the heaviest service (by used bytes) among rows
    // that report a limit — reads as "who eats the most", matching the mockup
    // (whose bars are relative, not fractions of the 7.9 GiB container limit).
    let max_used = report
        .projects
        .iter()
        .flat_map(|p| &p.services)
        .filter(|s| s.mem_limit_bytes > 0)
        .map(|s| s.mem_used_bytes)
        .max()
        .unwrap_or(0);

    let services = report
        .projects
        .iter()
        .flat_map(|p| {
            p.services.iter().map(move |s| {
                let (mem, mem_ratio) = if s.mem_limit_bytes == 0 {
                    ("n/a".to_string(), None)
                } else {
                    let ratio = if max_used > 0 {
                        (s.mem_used_bytes as f64 / max_used as f64).clamp(0.0, 1.0)
                    } else {
                        0.0
                    };
                    (human_bytes(s.mem_used_bytes), Some(ratio))
                };
                let sem = if s.state.is_empty() {
                    Sem::Neutral
                } else {
                    output::status_sem(&s.state, s.health.as_deref())
                };
                ServiceRow {
                    project: p.project.clone(),
                    service: s.service.clone(),
                    cpu: format!("{:.1}%", s.cpu_percent),
                    mem,
                    mem_ratio,
                    state: s.state.clone(),
                    sem,
                }
            })
        })
        .collect();

    StatsView {
        disk_percent: h.disk_used_percent,
        uptime: format_uptime(h.uptime_secs),
        cpu: MetricCard {
            label: "cpu%",
            value: format!("{:.1}", h.cpu_percent),
            unit: "%",
            series: cpu_points,
        },
        mem: MetricCard {
            label: "mem%",
            value: format!("{mem_pct:.1}"),
            unit: "%",
            series: mem_points,
        },
        temp: MetricCard {
            label: "temp°C",
            value: temp_value,
            unit: temp_unit,
            series: temp_points,
        },
        services,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::{
        HostSampleDto, HostStatsDto, ProjectStatsDto, ServiceStatsDto, StatsReportDto,
    };

    fn svc(service: &str, used: u64, limit: u64, state: &str) -> ServiceStatsDto {
        ServiceStatsDto {
            service: service.into(),
            cpu_percent: 0.0,
            mem_used_bytes: used,
            mem_limit_bytes: limit,
            state: state.into(),
            health: None,
        }
    }

    fn report(services: Vec<ServiceStatsDto>, history: Vec<HostSampleDto>) -> StatsReportDto {
        StatsReportDto {
            host: HostStatsDto {
                cpu_percent: 0.9,
                mem_used_bytes: 900,
                mem_total_bytes: 8000,
                disk_used_percent: 16,
                uptime_secs: 4 * 86_400 + 6 * 3600,
                temp_celsius: Some(48.5),
            },
            projects: vec![ProjectStatsDto {
                project: "myboard".into(),
                services,
                last_deploy: None,
            }],
            host_history: history,
        }
    }

    #[test]
    fn format_uptime_days_hours_minutes() {
        assert_eq!(format_uptime(4 * 86_400 + 6 * 3600), "4d 6h");
        assert_eq!(format_uptime(6 * 3600 + 5 * 60), "6h 5m");
        assert_eq!(format_uptime(3 * 60), "3m");
    }

    #[test]
    fn layout_mode_by_size() {
        assert_eq!(layout_mode(Rect::new(0, 0, 120, 40)), LayoutMode::Rich);
        assert_eq!(layout_mode(Rect::new(0, 0, 120, 16)), LayoutMode::Compact);
        assert_eq!(layout_mode(Rect::new(0, 0, 60, 40)), LayoutMode::Compact);
        assert_eq!(layout_mode(Rect::new(0, 0, 120, 8)), LayoutMode::Tiny);
    }

    #[test]
    fn cards_carry_current_values_and_history() {
        let v = build_view(&report(vec![], vec![]));
        assert_eq!(v.cpu.value, "0.9");
        assert_eq!(v.cpu.unit, "%");
        assert_eq!(v.mem.value, "11.2"); // 900/8000*100
        assert_eq!(v.temp.value, "48.5");
        assert_eq!(v.temp.unit, "°C");
        assert_eq!(v.disk_percent, 16);
        assert_eq!(v.uptime, "4d 6h");
    }

    #[test]
    fn mem_bar_scales_to_heaviest_service_and_na_without_limit() {
        let v = build_view(&report(
            vec![
                svc("big", 200, 1000, "running"),
                svc("small", 50, 1000, "running"),
                svc("nolimit", 0, 0, "running"),
            ],
            vec![],
        ));
        assert_eq!(v.services[0].mem, "200 B");
        assert_eq!(v.services[0].mem_ratio, Some(1.0)); // heaviest
        assert_eq!(v.services[1].mem_ratio, Some(0.25)); // 50/200
        assert_eq!(v.services[2].mem, "n/a");
        assert_eq!(v.services[2].mem_ratio, None);
    }

    #[test]
    fn status_role_from_state_and_unknown_is_neutral() {
        let v = build_view(&report(
            vec![svc("up", 10, 100, "running"), svc("old", 10, 100, "")],
            vec![],
        ));
        assert_eq!(v.services[0].sem, Sem::Success);
        assert_eq!(v.services[0].state, "running");
        assert_eq!(v.services[1].sem, Sem::Neutral); // empty state (old agent)
    }
}
