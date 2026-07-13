use std::io;

use crossterm::event::{Event, EventStream, KeyCode, KeyEvent, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use futures::StreamExt;
use ratatui::prelude::*;
use ratatui::widgets::{
    Axis, Block, BorderType, Borders, Cell, Chart, Dataset, GraphType, Paragraph, Row, Table,
};
use tui_big_text::{BigText, PixelSize};

use crate::cli::api::ApiClient;
use crate::cli::stats_view::{build_view, layout_mode, LayoutMode, MetricCard, StatsView};
use crate::output;
use crate::proto::StatsReportDto;

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
                    Some(Err(_)) | None => break,
                    _ => {}
                }
            }
        }
    }
    Ok(())
}

/// Truecolor-aware grey palette matching the mockup; under non-truecolor we use
/// named fallbacks so nothing renders as a jarring wrong hue.
struct Palette {
    border: Color,
    muted: Color,
    fill: Color,
    zebra: Color,
    track: Color,
    bg: Option<Color>,
}

impl Palette {
    fn current() -> Palette {
        if output::truecolor_enabled() {
            Palette {
                border: Color::Rgb(56, 55, 61),   // #38373d
                muted: Color::Rgb(132, 131, 138), // #84838a
                fill: Color::Rgb(89, 88, 95),     // #59585f
                zebra: Color::Rgb(16, 16, 19),    // #101013
                track: Color::Rgb(36, 35, 39),    // #242327
                bg: Some(Color::Rgb(11, 11, 13)), // #0b0b0d
            }
        } else {
            Palette {
                border: Color::DarkGray,
                muted: Color::Gray,
                fill: Color::Gray,
                zebra: Color::Black,
                track: Color::DarkGray,
                bg: None, // keep the terminal background
            }
        }
    }
}

fn draw(f: &mut Frame, report: Option<&StatsReportDto>, status: &str) {
    let pal = Palette::current();
    if let Some(bg) = pal.bg {
        f.render_widget(Block::default().style(Style::default().bg(bg)), f.area());
    }

    let Some(report) = report else {
        f.render_widget(Paragraph::new(status.to_string()), f.area());
        return;
    };
    let view = build_view(report);
    let mode = layout_mode(f.area());

    match mode {
        LayoutMode::Tiny => draw_tiny(f, &view, status, &pal),
        _ => draw_dashboard(f, &view, status, &pal, mode),
    }
}

fn draw_dashboard(f: &mut Frame, view: &StatsView, status: &str, pal: &Palette, mode: LayoutMode) {
    let cards_h: u16 = if mode == LayoutMode::Rich { 10 } else { 6 };
    let [strip, cards, services, help] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(cards_h),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .areas(f.area());

    draw_strip(f, strip, view, status, pal);
    draw_cards(f, cards, view, pal, mode);
    draw_services(f, services, view, pal);
    draw_help(f, help, pal);
}

fn draw_strip(f: &mut Frame, area: Rect, view: &StatsView, status: &str, pal: &Palette) {
    let muted = Style::default().fg(pal.muted);
    let mut spans = vec![
        Span::styled("DISK ", muted),
        Span::styled(format!("{}%", view.disk_percent), Style::default().bold()),
        Span::styled("   UPTIME ", muted),
        Span::styled(view.uptime.clone(), Style::default().bold()),
    ];
    if !status.is_empty() {
        spans.push(Span::styled(format!("   [{status}]"), muted));
    }
    let block = Block::default()
        .borders(Borders::BOTTOM)
        .border_style(Style::default().fg(pal.border));
    f.render_widget(Paragraph::new(Line::from(spans)).block(block), area);
}

fn draw_cards(f: &mut Frame, area: Rect, view: &StatsView, pal: &Palette, mode: LayoutMode) {
    let cols = Layout::horizontal([Constraint::Fill(1); 3]).split(area);
    let cpu_c = output::accent_ratatui_color().unwrap_or(Color::Red);
    let mem_c = output::mem_ratatui_color().unwrap_or(Color::Green);
    let temp_c = output::temp_ratatui_color().unwrap_or(Color::Magenta);
    draw_card(f, cols[0], &view.cpu, cpu_c, pal, mode);
    draw_card(f, cols[1], &view.mem, mem_c, pal, mode);
    draw_card(f, cols[2], &view.temp, temp_c, pal, mode);
}

fn draw_card(
    f: &mut Frame,
    area: Rect,
    card: &MetricCard,
    color: Color,
    pal: &Palette,
    mode: LayoutMode,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(pal.border));
    let inner = block.inner(area);
    f.render_widget(block, area);

    // label / value / mini-chart
    let value_h: u16 = if mode == LayoutMode::Rich { 4 } else { 1 };
    let [label_a, value_a, chart_a] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(value_h),
        Constraint::Fill(1),
    ])
    .areas(inner);

    f.render_widget(
        Paragraph::new(Span::styled(card.label, Style::default().fg(pal.muted))),
        label_a,
    );
    draw_value(f, value_a, card, color, mode);
    draw_mini_chart(f, chart_a, card, color, pal);
}

/// Big block digits under Rich; bold text otherwise. The unit suffix always
/// renders as normal text (the font8x8 glyph set has no `°`).
fn draw_value(f: &mut Frame, area: Rect, card: &MetricCard, color: Color, mode: LayoutMode) {
    if mode == LayoutMode::Rich {
        let big = BigText::builder()
            .pixel_size(PixelSize::Quadrant)
            .style(Style::default().fg(color))
            .lines(vec![card.value.clone().into()])
            .build();
        // Big value on the left, unit small to its right on the last row.
        let [big_a, unit_a] =
            Layout::horizontal([Constraint::Min(0), Constraint::Length(4)]).areas(area);
        f.render_widget(big, big_a);
        f.render_widget(
            Paragraph::new(Span::styled(card.unit, Style::default().fg(color)))
                .alignment(Alignment::Left),
            unit_a,
        );
        return;
    }
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(card.value.clone(), Style::default().fg(color).bold()),
            Span::styled(card.unit, Style::default().fg(color)),
        ])),
        area,
    );
}

fn draw_mini_chart(f: &mut Frame, area: Rect, card: &MetricCard, color: Color, pal: &Palette) {
    if card.series.len() < 2 {
        return;
    }
    let x_max = (card.series.len() as f64 - 1.0).max(1.0);
    let (mut y_min, mut y_max) = card
        .series
        .iter()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), &(_, y)| {
            (lo.min(y), hi.max(y))
        });
    if (y_max - y_min).abs() < f64::EPSILON {
        y_min -= 1.0;
        y_max += 1.0;
    }
    let pad = (y_max - y_min) * 0.1;
    let dataset = Dataset::default()
        .marker(symbols::Marker::Braille)
        .graph_type(GraphType::Line)
        .style(Style::default().fg(color))
        .data(&card.series);
    let axis_style = Style::default().fg(pal.track);
    let chart = Chart::new(vec![dataset])
        .x_axis(Axis::default().bounds([0.0, x_max]).style(axis_style))
        .y_axis(
            Axis::default()
                .bounds([y_min - pad, y_max + pad])
                .style(axis_style),
        );
    f.render_widget(chart, area);
}

/// Eighth-block bar of `ratio` (0..=1) over `width` cells.
fn bar(ratio: f64, width: usize) -> String {
    const EIGHTHS: [char; 9] = [' ', '▏', '▎', '▍', '▌', '▋', '▊', '▉', '█'];
    let total_eighths = (ratio.clamp(0.0, 1.0) * (width * 8) as f64).round() as usize;
    let full = total_eighths / 8;
    let rem = total_eighths % 8;
    let mut s = String::new();
    for _ in 0..full.min(width) {
        s.push('█');
    }
    if full < width && rem > 0 {
        s.push(EIGHTHS[rem]);
    }
    while s.chars().count() < width {
        s.push(' ');
    }
    s
}

fn draw_services(f: &mut Frame, area: Rect, view: &StatsView, pal: &Palette) {
    let muted = Style::default().fg(pal.muted);
    let accent = output::accent_ratatui_color().unwrap_or(Color::Red);

    let header = Row::new(
        ["PROJECT", "SERVICE", "CPU", "MEM", "STATUS"]
            .into_iter()
            .map(|h| Cell::from(Span::styled(h, muted))),
    );

    let rows = view.services.iter().enumerate().map(|(i, s)| {
        let project = Line::from(vec![
            Span::styled("▸ ", Style::default().fg(accent)),
            Span::raw(s.project.clone()),
        ]);
        let mem_cell: Line = match s.mem_ratio {
            Some(ratio) => Line::from(vec![
                Span::raw(format!("{} ", s.mem)),
                Span::styled(bar(ratio, 8), Style::default().fg(pal.fill)),
            ]),
            None => Line::from(Span::styled(s.mem.clone(), muted)),
        };
        let status_cell: Line = match output::sem_ratatui_color(s.sem) {
            Some(c) if !s.state.is_empty() => Line::from(vec![
                Span::styled("● ", Style::default().fg(c)),
                Span::styled(s.state.clone(), Style::default().fg(c)),
            ]),
            _ => Line::from(Span::styled("—", muted)),
        };
        let row = Row::new(vec![
            Cell::from(project),
            Cell::from(s.service.clone()),
            Cell::from(Span::styled(s.cpu.clone(), muted)),
            Cell::from(mem_cell),
            Cell::from(status_cell),
        ]);
        if i % 2 == 1 {
            row.style(Style::default().bg(pal.zebra))
        } else {
            row
        }
    });

    let table = Table::new(
        rows,
        [
            Constraint::Fill(13),
            Constraint::Fill(15),
            Constraint::Fill(7),
            Constraint::Fill(16),
            Constraint::Fill(9),
        ],
    )
    .header(header)
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(pal.border))
            .title(" services "),
    );
    f.render_widget(table, area);
}

fn draw_help(f: &mut Frame, area: Rect, pal: &Palette) {
    let accent = output::accent_ratatui_color().unwrap_or(Color::Red);
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("q/Esc/Ctrl-C", Style::default().fg(accent).bold()),
            Span::styled(" quit", Style::default().fg(pal.muted)),
        ])),
        area,
    );
}

/// Very small terminals: a one-line host summary + the services table only, so
/// the dashboard never overflows or panics.
fn draw_tiny(f: &mut Frame, view: &StatsView, status: &str, pal: &Palette) {
    let [summary, services] =
        Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(f.area());
    let mut line = format!(
        "CPU {}{}  MEM {}{}  TEMP {}{}  DISK {}%",
        view.cpu.value,
        view.cpu.unit,
        view.mem.value,
        view.mem.unit,
        view.temp.value,
        view.temp.unit,
        view.disk_percent
    );
    if !status.is_empty() {
        line.push_str(&format!("  [{status}]"));
    }
    f.render_widget(Paragraph::new(line).bold(), summary);
    draw_services(f, services, view, pal);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;

    use crate::proto::{
        HostSampleDto, HostStatsDto, ProjectStatsDto, ServiceStatsDto, StatsReportDto,
    };

    fn sample() -> HostSampleDto {
        HostSampleDto {
            at_ms: 0,
            cpu_percent: 10.0,
            mem_used_bytes: 25,
            mem_total_bytes: 100,
            temp_celsius: Some(44.0),
        }
    }

    fn report() -> StatsReportDto {
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
                services: vec![ServiceStatsDto {
                    service: "valkey".into(),
                    cpu_percent: 0.1,
                    mem_used_bytes: 16,
                    mem_limit_bytes: 1024,
                    state: "running".into(),
                    health: None,
                }],
                last_deploy: None,
            }],
            host_history: vec![sample(), sample()],
        }
    }

    #[test]
    fn bar_fills_proportionally() {
        assert_eq!(bar(0.0, 4), "    ");
        assert_eq!(bar(1.0, 4), "████");
        assert_eq!(bar(0.5, 4), "██  ");
    }

    #[test]
    fn dashboard_renders_without_panic_at_full_size() {
        let backend = TestBackend::new(120, 40);
        let mut terminal = Terminal::new(backend).unwrap();
        let r = report();
        terminal.draw(|f| draw(f, Some(&r), "")).unwrap();
        let buf = terminal.backend().buffer().clone();
        let text: String = buf.content().iter().map(|c| c.symbol()).collect();
        assert!(text.contains("services"), "services panel title present");
        assert!(text.contains("quit"), "help line present");
    }

    #[test]
    fn tiny_layout_renders_without_panic() {
        let backend = TestBackend::new(80, 8);
        let mut terminal = Terminal::new(backend).unwrap();
        let r = report();
        terminal.draw(|f| draw(f, Some(&r), "")).unwrap();
        let buf = terminal.backend().buffer().clone();
        let text: String = buf.content().iter().map(|c| c.symbol()).collect();
        assert!(text.contains("DISK"), "tiny summary present");
    }
}
