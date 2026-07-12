use std::io;

use crossterm::event::{Event, EventStream, KeyCode, KeyEvent, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use futures::StreamExt;
use ratatui::prelude::*;
use ratatui::widgets::{Axis, Block, Borders, Chart, Dataset, GraphType, Paragraph, Row, Table};

use crate::cli::api::ApiClient;
use crate::cli::stats_render::human_bytes;
use crate::output;
use crate::proto::StatsReportDto;

/// Terminal-independent view model derived from a stats response. Kept pure so
/// it is unit-testable without a real terminal.
pub struct StatsFrame {
    pub cpu_points: Vec<(f64, f64)>,
    pub mem_points: Vec<(f64, f64)>,
    pub temp_points: Vec<(f64, f64)>,
    pub service_rows: Vec<[String; 4]>,
    pub host_summary: String,
}

pub fn build_frame(report: &StatsReportDto) -> StatsFrame {
    let mut cpu_points = Vec::new();
    let mut mem_points = Vec::new();
    let mut temp_points = Vec::new();
    for (i, s) in report.host_history.iter().enumerate() {
        let x = i as f64;
        cpu_points.push((x, s.cpu_percent));
        let mem_pct = if s.mem_total_bytes > 0 {
            s.mem_used_bytes as f64 / s.mem_total_bytes as f64 * 100.0
        } else {
            0.0
        };
        mem_points.push((x, mem_pct));
        if let Some(t) = s.temp_celsius {
            temp_points.push((x, t));
        }
    }

    let service_rows = report
        .projects
        .iter()
        .flat_map(|p| {
            p.services.iter().map(move |s| {
                let mem = if s.mem_limit_bytes == 0 {
                    "n/a".to_string()
                } else {
                    format!(
                        "{}/{}",
                        human_bytes(s.mem_used_bytes),
                        human_bytes(s.mem_limit_bytes)
                    )
                };
                [
                    p.project.clone(),
                    s.service.clone(),
                    format!("{:.1}%", s.cpu_percent),
                    mem,
                ]
            })
        })
        .collect();

    let h = &report.host;
    let temp = match h.temp_celsius {
        Some(c) => format!("{c:.1}°C"),
        None => "n/a".into(),
    };
    let host_summary = format!(
        "CPU {:.1}%   MEM {}/{}   TEMP {}   DISK {}%",
        h.cpu_percent,
        human_bytes(h.mem_used_bytes),
        human_bytes(h.mem_total_bytes),
        temp,
        h.disk_used_percent
    );

    StatsFrame {
        cpu_points,
        mem_points,
        temp_points,
        service_rows,
        host_summary,
    }
}

/// Restores the terminal on drop (normal exit, `?`, or panic-unwind).
struct TerminalGuard;

impl TerminalGuard {
    fn enter() -> io::Result<TerminalGuard> {
        enable_raw_mode()?;
        if let Err(e) = execute!(io::stdout(), EnterAlternateScreen) {
            let _ = disable_raw_mode();
            return Err(e);
        }
        Ok(TerminalGuard)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
    }
}

fn install_panic_hook() {
    let original = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
        original(info);
    }));
}

pub async fn stats_watch(
    api: ApiClient,
    project: Option<String>,
    interval: u64,
) -> anyhow::Result<()> {
    install_panic_hook();
    let _guard = TerminalGuard::enter()?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;

    let mut events = EventStream::new();
    let mut ticker = tokio::time::interval(std::time::Duration::from_secs(interval.max(1)));
    let mut last: Option<StatsReportDto> = None;
    let mut status = String::from("connecting…");

    // Prime the first frame immediately.
    match api.stats(project.as_deref()).await {
        Ok(r) => {
            last = Some(r);
            status.clear();
        }
        Err(e) => status = format!("reconnecting… ({e})"),
    }

    loop {
        terminal.draw(|f| draw(f, last.as_ref(), &status))?;

        tokio::select! {
            _ = ticker.tick() => {
                match api.stats(project.as_deref()).await {
                    Ok(r) => { last = Some(r); status.clear(); }
                    Err(e) => { status = format!("reconnecting… ({e})"); }
                }
            }
            maybe_event = events.next() => {
                match maybe_event {
                    Some(Ok(Event::Key(KeyEvent { code, modifiers, .. }))) => {
                        let quit = matches!(code, KeyCode::Char('q') | KeyCode::Esc)
                            || (code == KeyCode::Char('c') && modifiers.contains(KeyModifiers::CONTROL));
                        if quit {
                            break;
                        }
                    }
                    Some(Err(_)) | None => break, // terminal event stream ended
                    _ => {}
                }
            }
        }
    }
    Ok(())
}

fn draw(f: &mut Frame, report: Option<&StatsReportDto>, status: &str) {
    let Some(report) = report else {
        f.render_widget(Paragraph::new(status.to_string()), f.area());
        return;
    };
    let frame = build_frame(report);

    let [summary, charts, services, help] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length((frame.service_rows.len() as u16) + 3),
        Constraint::Length(1),
    ])
    .areas(f.area());

    let header = if status.is_empty() {
        frame.host_summary.clone()
    } else {
        format!("{}   [{}]", frame.host_summary, status)
    };
    f.render_widget(Paragraph::new(header).bold(), summary);

    // CPU% chart (0..100). MEM% shares the axis; TEMP overlays on the same
    // panel with its own dataset (°C read against the same 0..100 grid is fine
    // for a Pi: idle ~40, throttle ~80). CPU is the primary series, so it wears
    // the brand accent; mem/temp stay distinct secondary hues on the shared
    // panel.
    let cpu_dataset = {
        let d = Dataset::default()
            .name("cpu%")
            .graph_type(GraphType::Line)
            .data(&frame.cpu_points);
        match output::accent_ratatui_color() {
            Some(c) => d.fg(c),
            None => d.cyan(),
        }
    };
    let datasets = vec![
        cpu_dataset,
        Dataset::default()
            .name("mem%")
            .graph_type(GraphType::Line)
            .data(&frame.mem_points)
            .green(),
        Dataset::default()
            .name("temp°C")
            .graph_type(GraphType::Line)
            .data(&frame.temp_points)
            .magenta(),
    ];
    let x_max = frame.cpu_points.len().max(1) as f64 - 1.0;
    let chart = Chart::new(datasets)
        .block(Block::default().borders(Borders::ALL).title(" host "))
        .x_axis(Axis::default().bounds([0.0, x_max.max(1.0)]))
        .y_axis(
            Axis::default()
                .bounds([0.0, 100.0])
                .labels(["0", "50", "100"]),
        );
    f.render_widget(chart, charts);

    let rows: Vec<Row> = frame
        .service_rows
        .iter()
        .map(|r| Row::new(r.clone()))
        .collect();
    let table = Table::new(
        rows,
        [
            Constraint::Length(16),
            Constraint::Length(16),
            Constraint::Length(8),
            Constraint::Fill(1),
        ],
    )
    .header(Row::new(["PROJECT", "SERVICE", "CPU", "MEM"]).bold())
    .block(Block::default().borders(Borders::ALL).title(" services "));
    f.render_widget(table, services);

    f.render_widget(
        Paragraph::new(Line::from(vec![
            " q/Esc/Ctrl-C ".bold().cyan(),
            "quit".dim(),
        ])),
        help,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::{
        HostSampleDto, HostStatsDto, ProjectStatsDto, ServiceStatsDto, StatsReportDto,
    };

    fn sample(cpu: f64, mem_used: u64, mem_total: u64, temp: Option<f64>) -> HostSampleDto {
        HostSampleDto {
            at_ms: 0,
            cpu_percent: cpu,
            mem_used_bytes: mem_used,
            mem_total_bytes: mem_total,
            temp_celsius: temp,
        }
    }

    fn report(history: Vec<HostSampleDto>, mem_limit: u64) -> StatsReportDto {
        StatsReportDto {
            host: HostStatsDto {
                cpu_percent: 5.0,
                mem_used_bytes: 50,
                mem_total_bytes: 100,
                disk_used_percent: 10,
                uptime_secs: 5,
                temp_celsius: Some(44.0),
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

    #[test]
    fn build_frame_maps_history_to_chart_points() {
        let history = vec![
            sample(10.0, 25, 100, Some(40.0)),
            sample(20.0, 50, 100, Some(42.0)),
        ];
        let frame = build_frame(&report(history, 1024));
        assert_eq!(frame.cpu_points.len(), 2);
        assert_eq!(frame.cpu_points[0], (0.0, 10.0));
        assert_eq!(frame.cpu_points[1], (1.0, 20.0));
        // mem% = used/total*100
        assert_eq!(frame.mem_points[1], (1.0, 50.0));
        assert_eq!(frame.temp_points.len(), 2);
        assert_eq!(frame.temp_points[1], (1.0, 42.0));
    }

    #[test]
    fn build_frame_marks_zero_mem_limit_service_na() {
        let frame = build_frame(&report(vec![], 0));
        assert_eq!(frame.service_rows[0][0], "app");
        assert_eq!(frame.service_rows[0][1], "valkey");
        assert_eq!(frame.service_rows[0][3], "n/a");
        assert!(frame.temp_points.is_empty(), "no history → no temp points");
    }
}
