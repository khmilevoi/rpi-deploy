# Stats -w Dashboard TUI Redesign Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rebuild the `rpi stats -w` realtime TUI as a card-based dashboard matching the approved mockup, and propagate real container status into the services table.

**Architecture:** A new pure view-model module (`stats_view.rs`) maps a `StatsReportDto` into dashboard data (DISK/UPTIME strip, three metric cards, service rows with a status pill and memory bar) and is unit-tested without a terminal. `stats_tui.rs` becomes rendering-only: it draws the view-model with `ratatui` + `tui-big-text`, choosing a Rich/Compact/Tiny layout by terminal size. A small backend change carries container `state`/`health` from `docker compose ps` (already fetched) through the domain entity and DTO so the pill shows real status.

**Tech Stack:** Rust, `ratatui = "0.30"`, `crossterm = "0.29"`, new `tui-big-text = "0.8"` (font8x8 block glyphs), `tokio`.

## Global Constraints

- Workspace pins `ratatui = "0.30"`, `crossterm = "0.29"`. `tui-big-text 0.8.8` is compatible (depends on `ratatui-core ^0.1` + `ratatui-widgets ^0.3`, the split crates ratatui 0.30 re-exports).
- Units are binary/IEC (KiB/MiB/GiB, ÷1024) via existing `stats_render::human_bytes`.
- New response/DTO fields are backward compatible via `#[serde(default)]` — a new CLI against an old agent must still deserialize.
- Exact hex colors only under truecolor (`COLORTERM=truecolor|24bit`); otherwise the existing `Paint` downgrade to nearest xterm-256 applies. Route every color through the `output` module's truecolor-aware helpers — never raw `Color::Rgb` unconditionally.
- Mockup palette: accent `#C51A4A` (= existing theme accent `Rgb(197,26,74)`), mem `#75A928` (`Rgb(117,169,40)`), temp `#D96AE0` (`Rgb(217,106,224)`), muted `#84838a`, border `#38373d`, track `#242327`, bar-fill `#59585f`, zebra bg `#101013`, page bg `#0b0b0d`.
- Command surface unchanged: `stats_watch(api, project, interval)` keeps its signature, refresh loop, and quit keys (`q`/`Esc`/`Ctrl-C`).
- Per repo `CLAUDE.md`, the final gate is `cargo fmt --all -- --check`, `cargo clippy --all-targets --locked -- -D warnings`, `cargo test --locked` (all green on Linux CI). Use `rtk cargo …` when running these.
- The mockup's window chrome (`● ● ●` + title bar) is NOT part of the TUI — never render it.

---

### Task 1: Propagate container state + health into `ServiceStats` (domain + infra)

**Files:**
- Modify: `crates/domain/src/entities.rs:427-432` (`ServiceStats`)
- Modify: `crates/infrastructure/src/docker.rs:143-183` (`parse_stats_json`) and its tests (~`crates/infrastructure/src/docker.rs:449-483`)
- Modify: `crates/infrastructure/src/stats.rs:117-122` (test literal)

**Interfaces:**
- Produces: `ServiceStats { service: String, cpu_percent: f64, mem_used_bytes: u64, mem_limit_bytes: u64, state: String, health: Option<String> }`. `state` is the docker compose `State` string (e.g. `"running"`); `health` is the optional healthcheck string. Empty `state` means "unknown / not reported".

- [ ] **Step 1: Update the `parse_stats_json` test to expect state/health**

In `crates/infrastructure/src/docker.rs`, the existing test `parse_stats_json_joins_services_by_container_name` builds `ps` NDJSON with `State` and joins with stats JSON. Extend its assertions (add after the existing service/cpu/mem asserts):

```rust
// state + health now flow through from the ps JSON
assert_eq!(out[0].state, "running");
assert_eq!(out[0].health, None);
```

If the test's `ps` fixture lines lack `State`, they already include it (`"State":"running"`). Add a `Health` to one line to cover the option, e.g. change the `web` ps line to:
```rust
r#"{"Name":"rateme-web-1","Service":"web","State":"running","Health":"healthy"}"#,
```
and assert:
```rust
assert_eq!(out.iter().find(|s| s.service == "web").unwrap().health.as_deref(), Some("healthy"));
```

- [ ] **Step 2: Run the test to verify it fails to compile / fails**

Run: `rtk cargo test -p pi-infrastructure parse_stats_json_joins_services_by_container_name`
Expected: FAIL — `ServiceStats` has no field `state` (compile error).

- [ ] **Step 3: Add the fields to the domain entity**

In `crates/domain/src/entities.rs`, replace the `ServiceStats` struct (lines 425-432):

```rust
/// Live container metrics of one compose service (`rpi stats`, v0.4 design §4).
#[derive(Debug, Clone, PartialEq)]
pub struct ServiceStats {
    pub service: String,
    pub cpu_percent: f64,
    pub mem_used_bytes: u64,
    pub mem_limit_bytes: u64,
    /// Docker compose `State` string (e.g. "running"); empty when not reported.
    pub state: String,
    /// Docker healthcheck state ("healthy"/"unhealthy"/"starting"), None when
    /// the service declares no healthcheck.
    pub health: Option<String>,
}
```

- [ ] **Step 4: Populate state/health in `parse_stats_json`**

In `crates/infrastructure/src/docker.rs`, change the `services` map in `parse_stats_json` (lines 143-152) to carry state/health, and set them on the pushed `ServiceStats`. Replace the map build:

```rust
    // Name -> (service, state, health) from the ps JSON.
    let services: HashMap<String, (String, String, Option<String>)> = json_lines(ps_output)
        .iter()
        .filter_map(|v| {
            let state = v.get("State").and_then(|s| s.as_str()).unwrap_or("").to_string();
            let health = v
                .get("Health")
                .and_then(|h| h.as_str())
                .filter(|h| !h.is_empty())
                .map(str::to_string);
            Some((
                v.get("Name")?.as_str()?.to_string(),
                (v.get("Service")?.as_str()?.to_string(), state, health),
            ))
        })
        .collect();
```

Then in the stats loop, replace the lookup + push. The current code does `let Some(service) = services.get(name)` and later `service: service.clone()`. Change to destructure and set the new fields (around lines 159-183):

```rust
        let Some((service, state, health)) = services.get(name) else {
            continue;
        };
```
and in the `out.push(ServiceStats { … })` literal add:
```rust
            state: state.clone(),
            health: health.clone(),
```

- [ ] **Step 5: Fix the `stats.rs` test literal**

In `crates/infrastructure/src/stats.rs`, the `ServiceStats { … }` literal (lines 117-122) must add the two fields:

```rust
            Ok(vec![ServiceStats {
                service: "valkey".into(),
                cpu_percent: 0.2,
                mem_used_bytes: 0,
                mem_limit_bytes: 0,
                state: "running".into(),
                health: None,
            }])
```

- [ ] **Step 6: Run infra tests to verify they pass**

Run: `rtk cargo test -p pi-infrastructure`
Expected: PASS (all infra tests, including the extended parse test).

- [ ] **Step 7: Commit**

```bash
rtk git add crates/domain/src/entities.rs crates/infrastructure/src/docker.rs crates/infrastructure/src/stats.rs
rtk git commit -m "feat(stats): carry container state+health into ServiceStats"
```

---

### Task 2: Add state/health to `ServiceStatsDto` with backward compat (proto)

**Files:**
- Modify: `crates/bin/src/proto.rs:177-194` (`ServiceStatsDto` + `From` impl), add a test in the `proto.rs` test module
- Modify: `crates/bin/src/cli/stats_render.rs:199-205` (test DTO literal)
- Modify: `crates/bin/src/cli/stats_tui.rs:291-296` (test DTO literal — temporary; replaced in Task 5)

**Interfaces:**
- Produces: `ServiceStatsDto { service, cpu_percent, mem_used_bytes, mem_limit_bytes, state: String, health: Option<String> }`. `#[serde(default)]` on `state` and `health` so old-agent payloads omitting them deserialize (empty state, no health).

- [ ] **Step 1: Write the backward-compat deserialization test**

In `crates/bin/src/proto.rs` `#[cfg(test)]` module, add:

```rust
#[test]
fn service_stats_dto_defaults_missing_state_and_health() {
    let json = r#"{"service":"web","cpu_percent":0.0,"mem_used_bytes":10,"mem_limit_bytes":0}"#;
    let dto: ServiceStatsDto = serde_json::from_str(json).unwrap();
    assert_eq!(dto.state, "");
    assert_eq!(dto.health, None);
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `rtk cargo test -p pi-bin service_stats_dto_defaults_missing_state_and_health`
Expected: FAIL — compile error (fields don't exist) or missing-field deser error.

- [ ] **Step 3: Add the DTO fields + map them in `From`**

In `crates/bin/src/proto.rs`, replace the struct (lines 177-183):

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceStatsDto {
    pub service: String,
    pub cpu_percent: f64,
    pub mem_used_bytes: u64,
    pub mem_limit_bytes: u64,
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub health: Option<String>,
}
```

and extend the `From<ServiceStats>` impl (lines 187-192) to copy `state`/`health`:

```rust
        ServiceStatsDto {
            service: s.service,
            cpu_percent: s.cpu_percent,
            mem_used_bytes: s.mem_used_bytes,
            mem_limit_bytes: s.mem_limit_bytes,
            state: s.state,
            health: s.health,
        }
```

- [ ] **Step 4: Fix the two existing DTO test literals so the build stays green**

In `crates/bin/src/cli/stats_render.rs` (lines 199-205) and `crates/bin/src/cli/stats_tui.rs` (lines 291-296), each `ServiceStatsDto { … }` literal gains:

```rust
                    state: "running".into(),
                    health: None,
```
(add both fields before the closing brace of each literal).

- [ ] **Step 5: Run the affected tests to verify they pass**

Run: `rtk cargo test -p pi-bin`
Expected: PASS (proto compat test + existing stats_render/stats_tui tests compile and pass).

- [ ] **Step 6: Commit**

```bash
rtk git add crates/bin/src/proto.rs crates/bin/src/cli/stats_render.rs crates/bin/src/cli/stats_tui.rs
rtk git commit -m "feat(stats): add state/health to ServiceStatsDto (serde default for old agents)"
```

---

### Task 3: Truecolor-aware ratatui color helpers (output module)

**Files:**
- Modify: `crates/bin/src/output/mod.rs` (add pub fns near `accent_ratatui_color`, ~line 179-183; add tests in the file's `#[cfg(test)]` module)

**Interfaces:**
- Produces:
  - `output::truecolor_enabled() -> bool`
  - `output::mem_ratatui_color() -> Option<ratatui::style::Color>` (green `Rgb(117,169,40)`)
  - `output::temp_ratatui_color() -> Option<ratatui::style::Color>` (magenta `Rgb(217,106,224)`)
  - `output::sem_ratatui_color(sem: Sem) -> Option<ratatui::style::Color>` (Success/Warn/Error/Accent from theme; None for Muted/Neutral/Frame)

- [ ] **Step 1: Write the tests**

In `crates/bin/src/output/mod.rs` `#[cfg(test)]` module add:

```rust
#[test]
fn metric_colors_are_exact_rgb_under_truecolor() {
    use ratatui::style::Color;
    assert_eq!(theme::Paint::Rgb(117, 169, 40).ratatui_color(true), Some(Color::Rgb(117, 169, 40)));
    assert_eq!(theme::Paint::Rgb(217, 106, 224).ratatui_color(true), Some(Color::Rgb(217, 106, 224)));
}

#[test]
fn sem_ratatui_color_maps_status_roles() {
    // running -> Success -> some color; unknown neutral -> None
    assert!(sem_ratatui_color(Sem::Success).is_some());
    assert_eq!(sem_ratatui_color(Sem::Neutral), None);
    assert_eq!(sem_ratatui_color(Sem::Muted), None);
}
```

- [ ] **Step 2: Run to verify failure**

Run: `rtk cargo test -p pi-bin sem_ratatui_color_maps_status_roles`
Expected: FAIL — `sem_ratatui_color` not found.

- [ ] **Step 3: Implement the helpers**

In `crates/bin/src/output/mod.rs`, after `accent_ratatui_color` (line 183) add:

```rust
/// True when 24-bit color should be emitted (colors on + truecolor terminal).
/// Public wrapper so `ratatui` callers can build a matching grey palette.
pub fn truecolor_enabled() -> bool {
    theme::truecolor_enabled()
}

/// Memory-series green (`#75A928`) for `ratatui`; exact under truecolor,
/// nearest xterm-256 otherwise.
pub fn mem_ratatui_color() -> Option<ratatui::style::Color> {
    theme::Paint::Rgb(117, 169, 40).ratatui_color(theme::truecolor_enabled())
}

/// Temperature-series magenta (`#D96AE0`) for `ratatui`.
pub fn temp_ratatui_color() -> Option<ratatui::style::Color> {
    theme::Paint::Rgb(217, 106, 224).ratatui_color(theme::truecolor_enabled())
}

/// Semantic role -> `ratatui` color, following the active theme; `None` for
/// non-colored roles (Muted/Neutral/Frame). Mirrors `table.rs::sem_colour`.
pub fn sem_ratatui_color(sem: Sem) -> Option<ratatui::style::Color> {
    let t = theme::theme();
    let tc = theme::truecolor_enabled();
    match sem {
        Sem::Success => t.success.ratatui_color(tc),
        Sem::Warn => t.warn.ratatui_color(tc),
        Sem::Error => t.error.ratatui_color(tc),
        Sem::Accent => t.accent.ratatui_color(tc),
        Sem::Muted | Sem::Neutral | Sem::Frame => None,
    }
}
```

Note: `theme::Paint` must be visible here. `Paint` is `pub` in `theme.rs`; reference it as `theme::Paint`.

- [ ] **Step 4: Run to verify pass**

Run: `rtk cargo test -p pi-bin metric_colors_are_exact_rgb_under_truecolor sem_ratatui_color_maps_status_roles`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
rtk git add crates/bin/src/output/mod.rs
rtk git commit -m "feat(output): truecolor-aware ratatui color helpers for stats dashboard"
```

---

### Task 4: Pure dashboard view-model module (`stats_view.rs`)

**Files:**
- Create: `crates/bin/src/cli/stats_view.rs`
- Modify: `crates/bin/src/cli/mod.rs` (add `mod stats_view;` next to the other `mod stats_*;` declarations)

**Interfaces:**
- Consumes: `crate::proto::StatsReportDto`, `crate::cli::stats_render::human_bytes`, `crate::output::{self, Sem}`, `ratatui::layout::Rect`.
- Produces:
  - `StatsView { disk_percent: u8, uptime: String, cpu: MetricCard, mem: MetricCard, temp: MetricCard, services: Vec<ServiceRow> }`
  - `MetricCard { label: &'static str, value: String, unit: &'static str, series: Vec<(f64, f64)> }`
  - `ServiceRow { project: String, service: String, cpu: String, mem: String, mem_ratio: Option<f64>, state: String, sem: Sem }`
  - `enum LayoutMode { Rich, Compact, Tiny }`
  - `fn build_view(report: &StatsReportDto) -> StatsView`
  - `fn layout_mode(area: Rect) -> LayoutMode`
  - `fn format_uptime(secs: u64) -> String`

- [ ] **Step 1: Write the module with failing tests first**

Create `crates/bin/src/cli/stats_view.rs` with the test module only referencing the not-yet-written API (write the full file in Step 3; here add the module declaration so tests can run). First add to `crates/bin/src/cli/mod.rs`:

```rust
mod stats_view;
```
(place it beside the existing `mod stats_tui;` / `mod stats_render;` lines.)

- [ ] **Step 2: Run to verify failure**

Run: `rtk cargo test -p pi-bin stats_view`
Expected: FAIL — file/module empty, symbols missing.

- [ ] **Step 3: Write the full module**

Write `crates/bin/src/cli/stats_view.rs`:

```rust
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
```

- [ ] **Step 4: Run the module tests to verify they pass**

Run: `rtk cargo test -p pi-bin stats_view`
Expected: PASS (all `stats_view::tests`).

- [ ] **Step 5: Commit**

```bash
rtk git add crates/bin/src/cli/stats_view.rs crates/bin/src/cli/mod.rs
rtk git commit -m "feat(stats): pure dashboard view-model (cards, service rows, layout mode)"
```

---

### Task 5: Render the dashboard in `stats_tui.rs` (+ `tui-big-text` dependency)

**Files:**
- Modify: `Cargo.toml` (workspace `[workspace.dependencies]`: add `tui-big-text = "0.8"`)
- Modify: `crates/bin/Cargo.toml` (add `tui-big-text = { workspace = true }`)
- Rewrite: `crates/bin/src/cli/stats_tui.rs` (replace `StatsFrame`/`build_frame`/`draw` with the dashboard renderer; the pure mapping now lives in `stats_view.rs`)

**Interfaces:**
- Consumes: `crate::cli::stats_view::{StatsView, MetricCard, ServiceRow, LayoutMode, build_view, layout_mode}`, `crate::output` color helpers from Task 3, `tui_big_text::{BigText, PixelSize}`.
- Produces: unchanged public `stats_watch(api, project, interval)`.

- [ ] **Step 1: Add the dependency**

In `Cargo.toml` under `[workspace.dependencies]` (near the `ratatui`/`crossterm` lines, ~43-44):
```toml
tui-big-text = "0.8"
```
In `crates/bin/Cargo.toml` (near line 36-37):
```toml
tui-big-text = { workspace = true }
```

- [ ] **Step 2: Verify the dependency resolves against ratatui 0.30**

Run: `rtk cargo build -p pi-bin`
Expected: builds (may warn about unused `tui-big-text` until Step 3 wires it; no error).

- [ ] **Step 3: Rewrite `stats_tui.rs`**

Replace the entire contents of `crates/bin/src/cli/stats_tui.rs` with:

```rust
use std::io;

use crossterm::event::{Event, EventStream, KeyCode, KeyEvent, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use futures::StreamExt;
use ratatui::prelude::*;
use ratatui::widgets::{Block, BorderType, Borders, Cell, Chart, Dataset, GraphType, Paragraph, Row, Table};
use tui_big_text::{BigText, PixelSize};

use crate::cli::api::ApiClient;
use crate::cli::stats_view::{build_view, layout_mode, LayoutMode, MetricCard, ServiceRow, StatsView};
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
    track: Color,
    fill: Color,
    zebra: Color,
    bg: Option<Color>,
}

impl Palette {
    fn current() -> Palette {
        if output::truecolor_enabled() {
            Palette {
                border: Color::Rgb(56, 55, 61),   // #38373d
                muted: Color::Rgb(132, 131, 138), // #84838a
                track: Color::Rgb(36, 35, 39),     // #242327
                fill: Color::Rgb(89, 88, 95),      // #59585f
                zebra: Color::Rgb(16, 16, 19),     // #101013
                bg: Some(Color::Rgb(11, 11, 13)),  // #0b0b0d
            }
        } else {
            Palette {
                border: Color::DarkGray,
                muted: Color::Gray,
                track: Color::DarkGray,
                fill: Color::Gray,
                zebra: Color::Black,
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

fn draw_card(f: &mut Frame, area: Rect, card: &MetricCard, color: Color, pal: &Palette, mode: LayoutMode) {
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
        if let Ok(big) = BigText::builder()
            .pixel_size(PixelSize::Quadrant)
            .style(Style::default().fg(color))
            .lines(vec![card.value.clone().into()])
            .build()
        {
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
    let (mut y_min, mut y_max) = card.series.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), &(_, y)| {
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
        .y_axis(Axis::default().bounds([y_min - pad, y_max + pad]).style(axis_style));
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
        view.cpu.value, view.cpu.unit, view.mem.value, view.mem.unit, view.temp.value, view.temp.unit, view.disk_percent
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
```

- [ ] **Step 4: Run the stats_tui tests to verify they pass**

Run: `rtk cargo test -p pi-bin stats_tui`
Expected: PASS (`bar_fills_proportionally`, `dashboard_renders_without_panic_at_full_size`, `tiny_layout_renders_without_panic`).

- [ ] **Step 5: Commit**

```bash
rtk git add Cargo.toml Cargo.lock crates/bin/Cargo.toml crates/bin/src/cli/stats_tui.rs
rtk git commit -m "feat(stats): card-based dashboard render for rpi stats -w"
```

---

### Task 6: Full workspace verification gate

**Files:** none (verification only; fix fallout inline if any).

- [ ] **Step 1: Format check**

Run: `rtk cargo fmt --all -- --check`
Expected: no diff. If it reports one, run `rtk cargo fmt --all` and re-commit.

- [ ] **Step 2: Clippy (deny warnings)**

Run: `rtk cargo clippy --all-targets --locked -- -D warnings`
Expected: no warnings. Common fixups if any: remove unused imports (e.g. `MetricCard`/`ServiceRow`/`StatsView` if a helper ended up not referencing a type), collapse `if i % 2 == 1` styling, or add `#[allow]` only as a last resort.

- [ ] **Step 3: Full test suite**

Run: `rtk cargo test --locked`
Expected: all crates green.

- [ ] **Step 4: Manual smoke via the verify skill (optional but preferred)**

Drive the real TUI against a local dev agent per the `rpi-cli` skill (`PI_AGENT_URL` to a running `rpi agent run`), or confirm rendering with the `run` skill. Observe: cards show current cpu/mem/temp with mini-lines, services table shows the status pill + mem bar, `q` quits and the terminal is restored.

- [ ] **Step 5: Commit any fixups**

```bash
rtk git add -A
rtk git commit -m "chore(stats): verification fixups"
```
(Skip if nothing changed.)

---

## Self-Review

**Spec coverage:**
- Status strip (DISK+UPTIME) → Task 4 (`build_view`, `format_uptime`) + Task 5 (`draw_strip`). ✓
- Three metric cards (big number + mini line, per-metric color) → Task 4 (`MetricCard`) + Task 5 (`draw_card`/`draw_value`/`draw_mini_chart`, `tui-big-text`). ✓
- Drop combined chart → Task 5 (no `Chart` over the whole area; only per-card minis). ✓
- Services table: `▸` marker, zebra, inline mem bar, Status pill → Task 5 (`draw_services`, `bar`). ✓
- Real status column (state/health) → Tasks 1–2 (entity + DTO) + Task 4 (`sem`) + Task 5 (pill). ✓
- Exact colors via truecolor with xterm-256 fallback → Task 3 helpers + Task 5 `Palette`. ✓
- Responsive Rich/Compact/Tiny → Task 4 (`layout_mode`) + Task 5 (`draw_dashboard`/`draw_tiny`). ✓
- Forced dark bg gated on truecolor → Task 5 (`Palette::bg`). ✓
- Old-agent compat → Task 2 (`#[serde(default)]`) + Task 4 (empty state → Neutral). ✓
- Tests: view-model units + `TestBackend` render + DTO round-trip → Tasks 2/4/5. ✓
- Final fmt/clippy/test gate → Task 6. ✓

**Placeholder scan:** No TBD/TODO; every code step shows full code. ✓

**Type consistency:** `build_view`/`layout_mode`/`format_uptime`, `StatsView`/`MetricCard`/`ServiceRow`/`LayoutMode`, and `output::{truecolor_enabled, mem_ratatui_color, temp_ratatui_color, sem_ratatui_color, accent_ratatui_color, status_sem, Sem}` names match across Tasks 3/4/5. `ServiceStats`/`ServiceStatsDto` field names (`state`, `health`) match across Tasks 1/2/4. ✓

**Note for the implementer:** the exact `PixelSize` (Quadrant) and card height (10/6) are tuned against real terminal widths during Task 5 Step 4 — if `Quadrant` digits overflow a third-width card at 120 cols, try `HalfHeight` or shrink to bold sooner; the `draw_value` fallback already covers the non-Rich path.
```
