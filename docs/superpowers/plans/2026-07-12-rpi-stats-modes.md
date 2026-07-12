# rpi stats — history-backed graphs, human sizes, temperature — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give `rpi stats` an agent-side host time series so the CLI can render sparkline graphs (static mode) and a live `ratatui` TUI (`-w/--watch`), plus CPU temperature, human-readable IEC sizes, and a `rpi doctor` memory-cgroup check.

**Architecture:** The agent runs a 2s background sampler that keeps a 5-minute in-memory ring buffer of host samples (CPU%, mem, temp). `GET /v1/stats` now returns the latest sample assembled into `HostStats` plus the full `host_history`. The CLI draws sparklines from that history in static mode and a full-screen chart in watch mode. All new response fields are `#[serde(default)]` so a new CLI degrades gracefully against an old agent.

**Tech Stack:** Rust (workspace: domain / application / infrastructure / bin), `sysinfo`, `axum`, `tokio`, `comfy-table`, and new `ratatui` 0.30 + `crossterm` 0.29 for the TUI. Mocks via `mockall` behind the domain `mocks` feature.

## Global Constraints

- **Sampler interval: 2s. History window: 5 minutes** (~150 samples), evicted by age.
- **Realtime `--interval` default: 2s.** Realtime polls the full `/v1/stats` each interval.
- **Units are binary/IEC** (KiB/MiB/GiB/TiB, ÷1024).
- **History is in-memory only** — an agent restart resets the chart.
- **Old-agent compatibility via `#[serde(default)]`** on every new response field.
- **Temperature is CPU thermal zone only**, `Option<f64>` °C, `None` on non-Pi / old kernels → renders `n/a`.
- **No `color-eyre`.** The TUI restores the terminal via a custom RAII guard + panic hook.
- **`ratatui` 0.30.2, `crossterm` 0.29.0** (crossterm matched to what ratatui re-exports).
- **Every task must leave the whole workspace green** (`cargo build` + `cargo test` compile). Where a struct/trait/signature change would break an existing call site, the same task updates that call site.
- **CI gate (run before declaring the feature done):**
  `rtk cargo fmt --all -- --check && rtk cargo clippy --all-targets --locked -- -D warnings && rtk cargo test --locked`.
- Domain entities are plain (`#[derive(Debug, Clone, PartialEq)]`), no serde; serialization lives in `crates/bin/src/proto.rs` DTOs.

---

## File Structure

**Created:**
- `crates/infrastructure/src/temp.rs` — `ThermalZoneTempProbe` (reads `/sys/class/thermal`).
- `crates/infrastructure/src/metrics.rs` — pure `RingBuffer` + `HostMetricsSampler` + read handle.
- `crates/bin/src/cli/stats_render.rs` — pure CLI helpers: `human_bytes`, `sparkline`, `render_stats_static`.
- `crates/bin/src/cli/stats_tui.rs` — realtime TUI: pure `build_frame` + `stats_watch`.

**Modified:**
- `crates/domain/src/entities.rs` — `HostSample`, `HostStats.temp_celsius`, `StatsReport.host_history`.
- `crates/domain/src/contracts.rs` — `HostMetricsStore`, `TempProbe` traits.
- `crates/infrastructure/src/lib.rs` — register `temp`, `metrics` modules.
- `crates/infrastructure/src/stats.rs` — rewrite `CompositeStats` to read the ring buffer.
- `crates/infrastructure/src/probe.rs` — doctor `memory cgroup` check.
- `crates/bin/src/proto.rs` — `HostStatsDto.temp_celsius`, `HostSampleDto`, `StatsReportDto.host_history`.
- `crates/bin/src/agent/state.rs` — build temp probe + sampler; `AppState.metrics`; `build_state` returns `(AppState, HostMetricsSampler)`.
- `crates/bin/src/agent/run.rs` — call `sampler.start()` inside the runtime.
- `crates/bin/src/agent/http.rs` — `state_with` stub `HostMetricsStore` + stats-response test.
- `crates/bin/src/cli/mod.rs` — register `stats_render`, `stats_tui` modules.
- `crates/bin/src/cli/commands.rs` — rewrite `stats()` (watch/interval, `render_stats_static`, warns).
- `crates/bin/src/cli/api.rs` — extend the client stats-decode test.
- `crates/bin/src/main.rs` — `Cmd::Stats` gains `watch`, `interval`; forward them.
- `Cargo.toml` (workspace) + `crates/bin/Cargo.toml` — add `ratatui`, `crossterm`.

---

## Task 1: Domain — HostSample entity, temp field, history field, store + probe traits

**Files:**
- Modify: `crates/domain/src/entities.rs` (around lines 443–458)
- Modify: `crates/domain/src/contracts.rs` (imports + append new traits)
- Modify (green-keeper only): `crates/infrastructure/src/stats.rs:33,51`
- Test: `crates/domain/src/entities.rs` (`#[cfg(test)]`)

**Interfaces:**
- Produces:
  - `pub struct HostSample { pub at_ms: i64, pub cpu_percent: f64, pub mem_used_bytes: u64, pub mem_total_bytes: u64, pub temp_celsius: Option<f64> }` (`#[derive(Debug, Clone, PartialEq)]`)
  - `HostStats` gains `pub temp_celsius: Option<f64>`
  - `StatsReport` gains `pub host_history: Vec<HostSample>`
  - `pub trait HostMetricsStore: Send + Sync { fn latest(&self) -> Option<HostSample>; fn history(&self) -> Vec<HostSample>; }` → `MockHostMetricsStore` under `feature = "mocks"`
  - `pub trait TempProbe: Send + Sync { fn cpu_celsius(&self) -> Option<f64>; }` → `MockTempProbe` under `feature = "mocks"`

- [ ] **Step 1: Add `temp_celsius` to `HostStats` and the `HostSample` struct + `host_history` to `StatsReport`**

In `crates/domain/src/entities.rs`, replace the `HostStats` / `StatsReport` block:

```rust
/// Host metrics (sysinfo + DiskProbe).
#[derive(Debug, Clone, PartialEq)]
pub struct HostStats {
    pub cpu_percent: f64,
    pub mem_used_bytes: u64,
    pub mem_total_bytes: u64,
    pub disk_used_percent: u8,
    pub uptime_secs: u64,
    /// CPU temperature in °C; `None` on hosts without a readable thermal zone.
    pub temp_celsius: Option<f64>,
}

/// One background host sample retained in the agent's ring buffer.
/// Disk and uptime are intentionally not sampled per-tick — they are
/// assembled into the snapshot at request time.
#[derive(Debug, Clone, PartialEq)]
pub struct HostSample {
    pub at_ms: i64,
    pub cpu_percent: f64,
    pub mem_used_bytes: u64,
    pub mem_total_bytes: u64,
    pub temp_celsius: Option<f64>,
}

/// Full `rpi stats` payload.
#[derive(Debug, Clone, PartialEq)]
pub struct StatsReport {
    pub host: HostStats,
    pub projects: Vec<ProjectStats>,
    /// Recent host samples (oldest→newest) for CLI sparklines/charts.
    pub host_history: Vec<HostSample>,
}
```

- [ ] **Step 2: Add the two traits to `contracts.rs`**

In `crates/domain/src/contracts.rs`, extend the `use crate::entities::{…}` import to also include `HostSample`, then append at the end of the file:

```rust
/// Read side of the agent's host-metrics ring buffer (synchronous, no IO).
#[cfg_attr(feature = "mocks", automock)]
pub trait HostMetricsStore: Send + Sync {
    fn latest(&self) -> Option<HostSample>;
    fn history(&self) -> Vec<HostSample>;
}

/// CPU temperature probe (synchronous).
#[cfg_attr(feature = "mocks", automock)]
pub trait TempProbe: Send + Sync {
    fn cpu_celsius(&self) -> Option<f64>;
}
```

- [ ] **Step 3: Keep the workspace compiling — patch the one `HostStats`/`StatsReport` construction site**

In `crates/infrastructure/src/stats.rs`, the existing `report()` builds both structs; add the new fields so the crate still compiles (Task 6 rewrites this method fully):

`HostStats { … uptime_secs: System::uptime(), temp_celsius: None }` and change the return to `Ok(StatsReport { host, projects: project_stats, host_history: Vec::new() })`.

- [ ] **Step 4: Write the failing test for `HostSample`**

Add to the `tests` module in `crates/domain/src/entities.rs`:

```rust
#[test]
fn host_sample_holds_all_fields() {
    let s = HostSample {
        at_ms: 1_700_000_000_000,
        cpu_percent: 12.5,
        mem_used_bytes: 1024,
        mem_total_bytes: 4096,
        temp_celsius: Some(42.0),
    };
    assert_eq!(s.at_ms, 1_700_000_000_000);
    assert_eq!(s.temp_celsius, Some(42.0));
    assert_eq!(s.clone(), s);
}
```

- [ ] **Step 5: Run the domain + infrastructure builds/tests**

Run: `rtk cargo test -p pi-domain --locked`
Expected: PASS (new test compiles and passes).
Run: `rtk cargo build -p pi-infrastructure --locked`
Expected: PASS (green-keeper patch works).

- [ ] **Step 6: Verify the mocks feature generates the new mocks**

Run: `rtk cargo build -p pi-domain --features mocks --locked`
Expected: PASS — `MockHostMetricsStore` and `MockTempProbe` now exist.

- [ ] **Step 7: Commit**

```bash
rtk git add crates/domain/src/entities.rs crates/domain/src/contracts.rs crates/infrastructure/src/stats.rs
rtk git commit -m "feat(domain): HostSample, host temp/history, metrics-store + temp-probe traits"
```

---

## Task 2: Protocol DTOs — temp, host sample, host history (additive, back-compatible)

**Files:**
- Modify: `crates/bin/src/proto.rs` (imports; `HostStatsDto`; new `HostSampleDto`; `StatsReportDto`)
- Test: `crates/bin/src/proto.rs` (`#[cfg(test)]`)

**Interfaces:**
- Consumes: `HostSample`, `HostStats.temp_celsius`, `StatsReport.host_history` (Task 1).
- Produces:
  - `HostStatsDto.temp_celsius: Option<f64>` (`#[serde(default)]`)
  - `pub struct HostSampleDto { pub at_ms: i64, pub cpu_percent: f64, pub mem_used_bytes: u64, pub mem_total_bytes: u64, pub temp_celsius: Option<f64> }` + `From<HostSample>`
  - `StatsReportDto.host_history: Vec<HostSampleDto>` (`#[serde(default)]`)

- [ ] **Step 1: Write the failing compatibility + roundtrip test**

Add to the `tests` module in `crates/bin/src/proto.rs`:

```rust
#[test]
fn old_agent_stats_body_without_history_or_temp_decodes_to_defaults() {
    // An old agent never emits `temp_celsius` or `host_history`.
    let json = r#"{"host":{"cpu_percent":5.0,"mem_used_bytes":100,"mem_total_bytes":200,"disk_used_percent":10,"uptime_secs":42},"projects":[]}"#;
    let dto: StatsReportDto = serde_json::from_str(json).unwrap();
    assert_eq!(dto.host.temp_celsius, None);
    assert!(dto.host_history.is_empty());
}

#[test]
fn stats_report_dto_roundtrips_with_history_and_temp() {
    let report = StatsReport {
        host: HostStats {
            cpu_percent: 9.0,
            mem_used_bytes: 10,
            mem_total_bytes: 20,
            disk_used_percent: 3,
            uptime_secs: 7,
            temp_celsius: Some(50.5),
        },
        projects: vec![],
        host_history: vec![HostSample {
            at_ms: 1,
            cpu_percent: 1.0,
            mem_used_bytes: 4,
            mem_total_bytes: 8,
            temp_celsius: None,
        }],
    };
    let dto: StatsReportDto = report.into();
    let json = serde_json::to_string(&dto).unwrap();
    let back: StatsReportDto = serde_json::from_str(&json).unwrap();
    assert_eq!(back.host.temp_celsius, Some(50.5));
    assert_eq!(back.host_history.len(), 1);
    assert_eq!(back.host_history[0].mem_total_bytes, 8);
}
```

Add `HostSample` to the `use pi_domain::entities::{…}` import at the top of `proto.rs`.

- [ ] **Step 2: Run the test to verify it fails**

Run: `rtk cargo test -p pi --locked old_agent_stats_body_without_history_or_temp_decodes_to_defaults`
Expected: FAIL to compile (`temp_celsius` / `host_history` fields do not exist yet).

- [ ] **Step 3: Add `temp_celsius` to `HostStatsDto` + its `From`**

In `crates/bin/src/proto.rs`, add the field and map it:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostStatsDto {
    pub cpu_percent: f64,
    pub mem_used_bytes: u64,
    pub mem_total_bytes: u64,
    pub disk_used_percent: u8,
    pub uptime_secs: u64,
    #[serde(default)]
    pub temp_celsius: Option<f64>,
}

impl From<HostStats> for HostStatsDto {
    fn from(h: HostStats) -> HostStatsDto {
        HostStatsDto {
            cpu_percent: h.cpu_percent,
            mem_used_bytes: h.mem_used_bytes,
            mem_total_bytes: h.mem_total_bytes,
            disk_used_percent: h.disk_used_percent,
            uptime_secs: h.uptime_secs,
            temp_celsius: h.temp_celsius,
        }
    }
}
```

- [ ] **Step 4: Add `HostSampleDto` + `From<HostSample>`**

Insert near `HostStatsDto`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostSampleDto {
    pub at_ms: i64,
    pub cpu_percent: f64,
    pub mem_used_bytes: u64,
    pub mem_total_bytes: u64,
    #[serde(default)]
    pub temp_celsius: Option<f64>,
}

impl From<HostSample> for HostSampleDto {
    fn from(s: HostSample) -> HostSampleDto {
        HostSampleDto {
            at_ms: s.at_ms,
            cpu_percent: s.cpu_percent,
            mem_used_bytes: s.mem_used_bytes,
            mem_total_bytes: s.mem_total_bytes,
            temp_celsius: s.temp_celsius,
        }
    }
}
```

- [ ] **Step 5: Add `host_history` to `StatsReportDto` + its `From`**

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatsReportDto {
    pub host: HostStatsDto,
    pub projects: Vec<ProjectStatsDto>,
    #[serde(default)]
    pub host_history: Vec<HostSampleDto>,
}

impl From<StatsReport> for StatsReportDto {
    fn from(r: StatsReport) -> StatsReportDto {
        StatsReportDto {
            host: r.host.into(),
            projects: r.projects.into_iter().map(Into::into).collect(),
            host_history: r.host_history.into_iter().map(Into::into).collect(),
        }
    }
}
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `rtk cargo test -p pi --locked stats`
Expected: PASS (both new tests plus existing proto tests).

- [ ] **Step 7: Commit**

```bash
rtk git add crates/bin/src/proto.rs
rtk git commit -m "feat(proto): additive temp + host_history stats DTO fields"
```

---

## Task 3: Infrastructure — ThermalZoneTempProbe

**Files:**
- Create: `crates/infrastructure/src/temp.rs`
- Modify: `crates/infrastructure/src/lib.rs` (add `pub mod temp;`)
- Test: `crates/infrastructure/src/temp.rs` (`#[cfg(test)]`)

**Interfaces:**
- Consumes: `pi_domain::contracts::TempProbe` (Task 1).
- Produces: `ThermalZoneTempProbe::new(root: &std::path::Path) -> Arc<ThermalZoneTempProbe>` implementing `TempProbe`.

- [ ] **Step 1: Write the failing tests**

Create `crates/infrastructure/src/temp.rs` with the test module first:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write_zone(root: &std::path::Path, idx: usize, zone_type: &str, millideg: &str) {
        let dir = root.join("sys/class/thermal").join(format!("thermal_zone{idx}"));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("type"), zone_type).unwrap();
        fs::write(dir.join("temp"), millideg).unwrap();
    }

    #[test]
    fn prefers_the_cpu_zone_and_parses_millidegrees() {
        let root = tempfile::tempdir().unwrap();
        write_zone(root.path(), 0, "gpu-thermal\n", "40000\n");
        write_zone(root.path(), 1, "cpu-thermal\n", "48250\n");
        let probe = ThermalZoneTempProbe::new(root.path());
        assert_eq!(probe.cpu_celsius(), Some(48.25));
    }

    #[test]
    fn falls_back_to_zone0_when_no_cpu_zone() {
        let root = tempfile::tempdir().unwrap();
        write_zone(root.path(), 0, "soc\n", "55000\n");
        let probe = ThermalZoneTempProbe::new(root.path());
        assert_eq!(probe.cpu_celsius(), Some(55.0));
    }

    #[test]
    fn none_when_tree_absent() {
        let root = tempfile::tempdir().unwrap();
        let probe = ThermalZoneTempProbe::new(root.path());
        assert_eq!(probe.cpu_celsius(), None);
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `rtk cargo test -p pi-infrastructure --locked temp::`
Expected: FAIL to compile (`ThermalZoneTempProbe` undefined).

- [ ] **Step 3: Implement `ThermalZoneTempProbe`**

Prepend to `crates/infrastructure/src/temp.rs`:

```rust
use std::path::{Path, PathBuf};
use std::sync::Arc;

use pi_domain::contracts::TempProbe;

/// Reads CPU temperature from `<root>/sys/class/thermal/thermal_zone*/`.
/// `root` is injected (mirrors `SysinfoDiskProbe::new`) so tests can point at
/// a temp dir with fake zones; production passes `/`.
pub struct ThermalZoneTempProbe {
    root: PathBuf,
}

impl ThermalZoneTempProbe {
    pub fn new(root: &Path) -> Arc<ThermalZoneTempProbe> {
        Arc::new(ThermalZoneTempProbe {
            root: root.to_path_buf(),
        })
    }

    fn read_millideg(dir: &Path) -> Option<f64> {
        let raw = std::fs::read_to_string(dir.join("temp")).ok()?;
        let milli: f64 = raw.trim().parse().ok()?;
        Some(milli / 1000.0)
    }
}

impl TempProbe for ThermalZoneTempProbe {
    fn cpu_celsius(&self) -> Option<f64> {
        let base = self.root.join("sys/class/thermal");
        let mut zone0: Option<PathBuf> = None;
        for entry in std::fs::read_dir(&base).ok()?.flatten() {
            let dir = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if !name.starts_with("thermal_zone") {
                continue;
            }
            if name == "thermal_zone0" {
                zone0 = Some(dir.clone());
            }
            let zone_type = std::fs::read_to_string(dir.join("type")).unwrap_or_default();
            if zone_type.to_lowercase().contains("cpu") {
                return Self::read_millideg(&dir);
            }
        }
        zone0.and_then(|d| Self::read_millideg(&d))
    }
}
```

Add `pub mod temp;` to `crates/infrastructure/src/lib.rs` (alphabetical: after `pub mod stats;`, before `pub mod sys;`).

- [ ] **Step 4: Run the tests to verify they pass**

Run: `rtk cargo test -p pi-infrastructure --locked temp::`
Expected: PASS (all three).

- [ ] **Step 5: Commit**

```bash
rtk git add crates/infrastructure/src/temp.rs crates/infrastructure/src/lib.rs
rtk git commit -m "feat(infra): ThermalZoneTempProbe reads CPU thermal zone"
```

---

## Task 4: Infrastructure — pure RingBuffer

**Files:**
- Create: `crates/infrastructure/src/metrics.rs` (RingBuffer only in this task)
- Modify: `crates/infrastructure/src/lib.rs` (add `pub mod metrics;`)
- Test: `crates/infrastructure/src/metrics.rs` (`#[cfg(test)]`)

**Interfaces:**
- Consumes: `pi_domain::entities::HostSample` (Task 1).
- Produces (crate-internal): `RingBuffer::new(window: std::time::Duration) -> RingBuffer`, `push(&mut self, HostSample)`, `latest(&self) -> Option<HostSample>`, `snapshot(&self) -> Vec<HostSample>`. Eviction drops samples older than `window` relative to the newest sample's `at_ms`.

- [ ] **Step 1: Write the failing tests**

Create `crates/infrastructure/src/metrics.rs`:

```rust
use std::collections::VecDeque;
use std::time::Duration;

use pi_domain::entities::HostSample;

/// Age-evicted ring buffer of host samples. Pure — no clock, no IO, no tokio.
/// Eviction is relative to the newest sample's timestamp so tests are
/// deterministic.
pub struct RingBuffer {
    samples: VecDeque<HostSample>,
    window_ms: i64,
}

impl RingBuffer {
    pub fn new(window: Duration) -> RingBuffer {
        RingBuffer {
            samples: VecDeque::new(),
            window_ms: window.as_millis() as i64,
        }
    }

    pub fn push(&mut self, sample: HostSample) {
        let cutoff = sample.at_ms - self.window_ms;
        self.samples.push_back(sample);
        while let Some(front) = self.samples.front() {
            if front.at_ms < cutoff {
                self.samples.pop_front();
            } else {
                break;
            }
        }
    }

    pub fn latest(&self) -> Option<HostSample> {
        self.samples.back().cloned()
    }

    pub fn snapshot(&self) -> Vec<HostSample> {
        self.samples.iter().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(at_ms: i64) -> HostSample {
        HostSample {
            at_ms,
            cpu_percent: 1.0,
            mem_used_bytes: 1,
            mem_total_bytes: 2,
            temp_celsius: None,
        }
    }

    #[test]
    fn evicts_samples_older_than_the_window() {
        let mut rb = RingBuffer::new(Duration::from_secs(300)); // 300_000 ms
        rb.push(sample(0));
        rb.push(sample(100_000));
        rb.push(sample(200_000));
        rb.push(sample(400_000)); // cutoff = 100_000 → drops at_ms 0
        let snap = rb.snapshot();
        assert_eq!(
            snap.iter().map(|s| s.at_ms).collect::<Vec<_>>(),
            vec![100_000, 200_000, 400_000]
        );
    }

    #[test]
    fn latest_returns_the_newest_sample() {
        let mut rb = RingBuffer::new(Duration::from_secs(300));
        assert_eq!(rb.latest(), None);
        rb.push(sample(1));
        rb.push(sample(2));
        assert_eq!(rb.latest().unwrap().at_ms, 2);
    }

    #[test]
    fn snapshot_is_oldest_to_newest() {
        let mut rb = RingBuffer::new(Duration::from_secs(300));
        rb.push(sample(10));
        rb.push(sample(20));
        rb.push(sample(30));
        assert_eq!(
            rb.snapshot().iter().map(|s| s.at_ms).collect::<Vec<_>>(),
            vec![10, 20, 30]
        );
    }
}
```

Add `pub mod metrics;` to `crates/infrastructure/src/lib.rs` (alphabetical: after `pub mod history;`... actually after `pub mod hostnet;`, before `pub mod migrations;`).

- [ ] **Step 2: Run the tests to verify they pass**

Run: `rtk cargo test -p pi-infrastructure --locked metrics::`
Expected: PASS (all three).

- [ ] **Step 3: Commit**

```bash
rtk git add crates/infrastructure/src/metrics.rs crates/infrastructure/src/lib.rs
rtk git commit -m "feat(infra): pure age-evicted RingBuffer for host samples"
```

---

## Task 5: Infrastructure — HostMetricsSampler (sysinfo + temp + tokio loop)

**Files:**
- Modify: `crates/infrastructure/src/metrics.rs` (append sampler + read handle)
- Test: `crates/infrastructure/src/metrics.rs` (`#[cfg(test)]`)

**Interfaces:**
- Consumes: `RingBuffer` (Task 4), `TempProbe` (Task 1), `sysinfo::System`.
- Produces:
  - `HostMetricsSampler::new(system: sysinfo::System, temp: Arc<dyn TempProbe>, window: Duration, interval: Duration) -> HostMetricsSampler` — takes **one immediate sample** synchronously; does **not** spawn.
  - `HostMetricsSampler::handle(&self) -> Arc<dyn HostMetricsStore>` — cloneable read handle.
  - `HostMetricsSampler::start(self)` — spawns the tokio sampling loop.

- [ ] **Step 1: Write the failing tests**

Add to `crates/infrastructure/src/metrics.rs`:

```rust
#[cfg(test)]
mod sampler_tests {
    use super::*;
    use std::sync::Arc;
    use sysinfo::System;

    struct FixedTemp(Option<f64>);
    impl pi_domain::contracts::TempProbe for FixedTemp {
        fn cpu_celsius(&self) -> Option<f64> {
            self.0
        }
    }

    #[test]
    fn construction_takes_one_immediate_sample() {
        let sampler = HostMetricsSampler::new(
            System::new(),
            Arc::new(FixedTemp(Some(41.0))),
            Duration::from_secs(300),
            Duration::from_secs(2),
        );
        let handle = sampler.handle();
        let latest = handle.latest().expect("pre-seeded sample present");
        assert!(latest.mem_total_bytes > 0, "sysinfo memory read");
        assert_eq!(latest.temp_celsius, Some(41.0));
        assert_eq!(handle.history().len(), 1);
    }

    #[tokio::test]
    async fn started_loop_appends_more_samples() {
        let sampler = HostMetricsSampler::new(
            System::new(),
            Arc::new(FixedTemp(None)),
            Duration::from_secs(300),
            Duration::from_millis(20),
        );
        let handle = sampler.handle();
        sampler.start();
        tokio::time::sleep(Duration::from_millis(80)).await;
        assert!(handle.history().len() >= 2, "sampler appended samples");
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `rtk cargo test -p pi-infrastructure --locked sampler_tests`
Expected: FAIL to compile (`HostMetricsSampler` undefined).

- [ ] **Step 3: Implement the sampler + handle**

Append to `crates/infrastructure/src/metrics.rs` (add the extra imports at the top of the file: `use std::sync::{Arc, Mutex};` and `use pi_domain::contracts::{HostMetricsStore, TempProbe};` and `use sysinfo::System;`):

```rust
/// Owns the ring buffer behind a std `Mutex` (critical section is a
/// push/evict with no `.await` held) plus the loop's sysinfo + temp state.
pub struct HostMetricsSampler {
    buf: Arc<Mutex<RingBuffer>>,
    system: System,
    temp: Arc<dyn TempProbe>,
    interval: Duration,
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn take_sample(system: &mut System, temp: &Arc<dyn TempProbe>) -> HostSample {
    system.refresh_cpu_usage();
    system.refresh_memory();
    HostSample {
        at_ms: now_ms(),
        cpu_percent: f64::from(system.global_cpu_usage()),
        mem_used_bytes: system.used_memory(),
        mem_total_bytes: system.total_memory(),
        temp_celsius: temp.cpu_celsius(),
    }
}

impl HostMetricsSampler {
    pub fn new(
        mut system: System,
        temp: Arc<dyn TempProbe>,
        window: Duration,
        interval: Duration,
    ) -> HostMetricsSampler {
        let mut ring = RingBuffer::new(window);
        // Immediate seed so `latest()` is Some before the first request.
        ring.push(take_sample(&mut system, &temp));
        HostMetricsSampler {
            buf: Arc::new(Mutex::new(ring)),
            system,
            temp,
            interval,
        }
    }

    pub fn handle(&self) -> Arc<dyn HostMetricsStore> {
        Arc::new(HostMetricsHandle {
            buf: Arc::clone(&self.buf),
        })
    }

    pub fn start(self) {
        let HostMetricsSampler {
            buf,
            mut system,
            temp,
            interval,
        } = self;
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            ticker.tick().await; // first tick fires immediately — skip (already seeded)
            loop {
                ticker.tick().await;
                let sample = take_sample(&mut system, &temp);
                if let Ok(mut ring) = buf.lock() {
                    ring.push(sample);
                }
            }
        });
    }
}

/// Cloneable read handle over the shared ring buffer.
struct HostMetricsHandle {
    buf: Arc<Mutex<RingBuffer>>,
}

impl HostMetricsStore for HostMetricsHandle {
    fn latest(&self) -> Option<HostSample> {
        self.buf.lock().ok().and_then(|r| r.latest())
    }

    fn history(&self) -> Vec<HostSample> {
        self.buf.lock().map(|r| r.snapshot()).unwrap_or_default()
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `rtk cargo test -p pi-infrastructure --locked metrics`
Expected: PASS (ring buffer + sampler tests).

- [ ] **Step 5: Commit**

```bash
rtk git add crates/infrastructure/src/metrics.rs
rtk git commit -m "feat(infra): HostMetricsSampler with immediate seed + tokio loop"
```

---

## Task 6: CompositeStats rewrite + agent wiring (sampler through the agent)

This task changes `CompositeStats::new`'s signature, so it updates **all three** call sites (unit test in `stats.rs`, production `state.rs`, fixture `http.rs`) plus `build_state`'s return type and `run.rs` in the same commit to keep the build green.

**Files:**
- Modify: `crates/infrastructure/src/stats.rs` (rewrite `CompositeStats` + tests)
- Modify: `crates/infrastructure/src/metrics.rs` (add `HostMetricsSampler::with_defaults`)
- Modify: `crates/bin/src/agent/state.rs` (temp probe + sampler; `AppState.metrics`; `build_state` returns a tuple)
- Modify: `crates/bin/src/agent/run.rs` (destructure the tuple; `sampler.start()`)
- Modify: `crates/bin/src/agent/http.rs` (`state_with` stub `HostMetricsStore`; new stats-response test)

**Interfaces:**
- Consumes: `HostMetricsStore` (Task 1), `HostMetricsSampler`/`handle()` (Task 5), `ThermalZoneTempProbe` (Task 3), `StatsReportDto`/`HostSampleDto` (Task 2).
- Produces:
  - `CompositeStats::new(runtime: Arc<dyn ContainerRuntime>, disk: Arc<dyn DiskProbe>, metrics: Arc<dyn HostMetricsStore>) -> Arc<CompositeStats>`
  - `AppState` gains `pub metrics: Arc<dyn HostMetricsStore>`
  - `build_state(config, log_dir_available) -> anyhow::Result<(AppState, pi_infrastructure::metrics::HostMetricsSampler)>`

- [ ] **Step 1: Rewrite `CompositeStats` and its unit test in `stats.rs`**

Replace the whole body of `crates/infrastructure/src/stats.rs` with:

```rust
use std::sync::Arc;

use async_trait::async_trait;
use pi_domain::contracts::{ContainerRuntime, DiskProbe, HostMetricsStore, StatsProvider};
use pi_domain::entities::{HostStats, ProjectStats, StatsReport};
use pi_domain::error::DomainError;
use sysinfo::System;

pub struct CompositeStats {
    runtime: Arc<dyn ContainerRuntime>,
    disk: Arc<dyn DiskProbe>,
    metrics: Arc<dyn HostMetricsStore>,
}

impl CompositeStats {
    pub fn new(
        runtime: Arc<dyn ContainerRuntime>,
        disk: Arc<dyn DiskProbe>,
        metrics: Arc<dyn HostMetricsStore>,
    ) -> Arc<CompositeStats> {
        Arc::new(CompositeStats {
            runtime,
            disk,
            metrics,
        })
    }
}

#[async_trait]
impl StatsProvider for CompositeStats {
    async fn report(&self, projects: Vec<String>) -> Result<StatsReport, DomainError> {
        let host = match self.metrics.latest() {
            Some(latest) => HostStats {
                cpu_percent: latest.cpu_percent,
                mem_used_bytes: latest.mem_used_bytes,
                mem_total_bytes: latest.mem_total_bytes,
                disk_used_percent: self.disk.used_percent().unwrap_or(0),
                uptime_secs: System::uptime(),
                temp_celsius: latest.temp_celsius,
            },
            // Defensive: sampler pre-seeds one sample, so this is unexpected.
            None => {
                let mut sys = System::new();
                sys.refresh_memory();
                HostStats {
                    cpu_percent: 0.0,
                    mem_used_bytes: sys.used_memory(),
                    mem_total_bytes: sys.total_memory(),
                    disk_used_percent: self.disk.used_percent().unwrap_or(0),
                    uptime_secs: System::uptime(),
                    temp_celsius: None,
                }
            }
        };

        let mut project_stats = Vec::new();
        for project in projects {
            let services = self.runtime.stats(&project).await.unwrap_or_default();
            project_stats.push(ProjectStats {
                project,
                services,
                last_deploy: None,
            });
        }

        Ok(StatsReport {
            host,
            projects: project_stats,
            host_history: self.metrics.history(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pi_domain::contracts::{MockContainerRuntime, MockDiskProbe, MockHostMetricsStore};
    use pi_domain::entities::{HostSample, ServiceStats};

    fn sample() -> HostSample {
        HostSample {
            at_ms: 1_000,
            cpu_percent: 12.5,
            mem_used_bytes: 2048,
            mem_total_bytes: 8192,
            temp_celsius: Some(47.0),
        }
    }

    #[tokio::test]
    async fn host_is_assembled_from_latest_sample_plus_disk_and_uptime() {
        let mut runtime = MockContainerRuntime::new();
        runtime.expect_stats().returning(|_| Ok(vec![]));
        let mut disk = MockDiskProbe::new();
        disk.expect_used_percent().returning(|| Ok(37));
        let mut metrics = MockHostMetricsStore::new();
        metrics.expect_latest().returning(|| Some(sample()));
        metrics.expect_history().returning(|| vec![sample(), sample()]);

        let stats = CompositeStats::new(Arc::new(runtime), Arc::new(disk), Arc::new(metrics));
        let report = stats.report(vec![]).await.unwrap();

        assert_eq!(report.host.cpu_percent, 12.5);
        assert_eq!(report.host.mem_used_bytes, 2048);
        assert_eq!(report.host.disk_used_percent, 37);
        assert_eq!(report.host.temp_celsius, Some(47.0));
        assert!(report.host.uptime_secs > 0);
        assert_eq!(report.host_history.len(), 2);
    }

    #[tokio::test]
    async fn per_service_zero_mem_limit_is_preserved() {
        let mut runtime = MockContainerRuntime::new();
        runtime.expect_stats().returning(|_| {
            Ok(vec![ServiceStats {
                service: "valkey".into(),
                cpu_percent: 0.2,
                mem_used_bytes: 0,
                mem_limit_bytes: 0,
            }])
        });
        let mut disk = MockDiskProbe::new();
        disk.expect_used_percent().returning(|| Ok(0));
        let mut metrics = MockHostMetricsStore::new();
        metrics.expect_latest().returning(|| Some(sample()));
        metrics.expect_history().returning(Vec::new);

        let stats = CompositeStats::new(Arc::new(runtime), Arc::new(disk), Arc::new(metrics));
        let report = stats.report(vec!["p".into()]).await.unwrap();

        let svc = &report.projects[0].services[0];
        assert_eq!(svc.mem_limit_bytes, 0);
        assert_eq!(svc.mem_used_bytes, 0);
    }
}
```

- [ ] **Step 2: Verify the infrastructure crate builds + tests pass**

Run: `rtk cargo test -p pi-infrastructure --locked stats::`
Expected: PASS.

- [ ] **Step 3: Wire the sampler into `state.rs`**

The `bin` crate does not depend on `sysinfo`, so first add a `sysinfo`-hiding
constructor to `HostMetricsSampler` in `crates/infrastructure/src/metrics.rs`
(alongside `new`, in the same `impl` block):

```rust
    /// Convenience constructor that owns a fresh `sysinfo::System`, so callers
    /// (the agent's `build_state`) need not depend on `sysinfo` directly.
    pub fn with_defaults(
        temp: Arc<dyn TempProbe>,
        window: Duration,
        interval: Duration,
    ) -> HostMetricsSampler {
        HostMetricsSampler::new(System::new(), temp, window, interval)
    }
```

Then, in `crates/bin/src/agent/state.rs`:

1. Add imports:
```rust
use pi_domain::contracts::{HostMetricsStore, TempProbe};
use pi_infrastructure::metrics::HostMetricsSampler;
use pi_infrastructure::temp::ThermalZoneTempProbe;
use std::path::Path;
use std::time::Duration;
```
2. Add the field to `AppState`:
```rust
    pub metrics: Arc<dyn HostMetricsStore>,
```
3. Build the sampler. Replace the `stats_provider`/`stats` lines (currently 140–141) with:
```rust
    let temp_probe: Arc<dyn TempProbe> = ThermalZoneTempProbe::new(Path::new("/"));
    let sampler = HostMetricsSampler::with_defaults(
        temp_probe,
        Duration::from_secs(300),
        Duration::from_secs(2),
    );
    let metrics = sampler.handle();
    let stats_provider = CompositeStats::new(runtime.clone(), disk.clone(), Arc::clone(&metrics));
    let stats = GetStats::new(projects.clone(), Arc::clone(&history), stats_provider);
```
4. Change the function signature:
```rust
pub fn build_state(
    config: &AgentConfig,
    log_dir_available: bool,
) -> anyhow::Result<(AppState, HostMetricsSampler)> {
```
5. Add `metrics,` to the `AppState { … }` literal, and change the final `Ok(AppState { … })` to `Ok((AppState { … }, sampler))`.

- [ ] **Step 4: Update `run.rs` to destructure and start the sampler**

In `crates/bin/src/agent/run.rs` line 49, replace:
```rust
    let state = build_state(&config, log_dir_available)?;
```
with:
```rust
    let (state, sampler) = build_state(&config, log_dir_available)?;
    sampler.start(); // spawn the 2s host-metrics loop inside the tokio runtime
```

- [ ] **Step 5: Give the `http.rs` test fixture a stub `HostMetricsStore`**

In `crates/bin/src/agent/http.rs` test module:

1. Add a stub near the top of the `tests` module:
```rust
    struct StubMetrics;
    impl pi_domain::contracts::HostMetricsStore for StubMetrics {
        fn latest(&self) -> Option<pi_domain::entities::HostSample> {
            Some(pi_domain::entities::HostSample {
                at_ms: 1,
                cpu_percent: 12.5,
                mem_used_bytes: 1024,
                mem_total_bytes: 4096,
                temp_celsius: Some(42.0),
            })
        }
        fn history(&self) -> Vec<pi_domain::entities::HostSample> {
            vec![
                pi_domain::entities::HostSample {
                    at_ms: 1,
                    cpu_percent: 10.0,
                    mem_used_bytes: 1000,
                    mem_total_bytes: 4096,
                    temp_celsius: Some(40.0),
                },
                pi_domain::entities::HostSample {
                    at_ms: 2,
                    cpu_percent: 12.5,
                    mem_used_bytes: 1024,
                    mem_total_bytes: 4096,
                    temp_celsius: Some(42.0),
                },
            ]
        }
    }
```
2. In `state_with`, replace the `stats_provider`/`stats` lines (873–874) with:
```rust
        let metrics: Arc<dyn pi_domain::contracts::HostMetricsStore> = Arc::new(StubMetrics);
        let stats_provider =
            CompositeStats::new(Arc::clone(&runtime), disk.clone(), Arc::clone(&metrics));
        let stats = GetStats::new(projects.clone(), Arc::clone(&history), stats_provider);
```
3. Add `metrics,` to the `AppState { … }` literal returned by `state_with`.

- [ ] **Step 6: Write the failing agent stats-response test**

Add to the `http.rs` test module (uses the existing `router`, `state_with`, `request`, `ok_source`, `ok_runtime` helpers):

```rust
    #[tokio::test]
    async fn stats_endpoint_returns_history_and_temp() {
        let dir = tempfile::tempdir().unwrap();
        let app = router(state_with(
            dir.path(),
            Arc::new(ok_source()),
            Arc::new(ok_runtime()),
        ));
        let (status, body) = request(
            app,
            axum::http::Request::builder()
                .uri("/v1/stats")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["host"]["temp_celsius"], 42.0);
        assert_eq!(body["host"]["cpu_percent"], 12.5);
        assert!(body["host_history"].as_array().unwrap().len() >= 2);
    }
```

- [ ] **Step 7: Run the whole bin crate's tests**

Run: `rtk cargo test -p pi --locked`
Expected: PASS (fixture compiles, new stats test passes, existing tests unaffected).

- [ ] **Step 8: Full workspace check**

Run: `rtk cargo build --locked && rtk cargo test --locked`
Expected: PASS.

- [ ] **Step 9: Commit**

```bash
rtk git add crates/infrastructure/src/stats.rs crates/infrastructure/src/metrics.rs crates/bin/src/agent/state.rs crates/bin/src/agent/run.rs crates/bin/src/agent/http.rs
rtk git commit -m "feat(agent): serve host metrics from the background sampler ring buffer"
```

---

## Task 7: Doctor — memory-cgroup check

**Files:**
- Modify: `crates/infrastructure/src/probe.rs` (pure helper + `diagnostics()` wiring + tests)
- Test: `crates/infrastructure/src/probe.rs` (`#[cfg(test)]`)

**Interfaces:**
- Consumes: `pi_domain::entities::DiagnosticCheck`.
- Produces (module-private): `fn memory_cgroup_check(controllers: Option<String>, v1_present: bool) -> DiagnosticCheck`.

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module in `crates/infrastructure/src/probe.rs`:

```rust
    #[test]
    fn memory_cgroup_v2_passes_when_controllers_list_memory() {
        let check = memory_cgroup_check(Some("cpuset cpu io memory pids\n".into()), false);
        assert!(check.passed);
        assert!(check.hint.is_none());
    }

    #[test]
    fn memory_cgroup_v2_fails_when_memory_absent_and_no_v1() {
        let check = memory_cgroup_check(Some("cpuset cpu io pids\n".into()), false);
        assert!(!check.passed);
        assert!(check
            .hint
            .as_deref()
            .unwrap()
            .contains("cgroup_enable=memory cgroup_memory=1"));
    }

    #[test]
    fn memory_cgroup_v1_passes_when_dir_present() {
        let check = memory_cgroup_check(None, true);
        assert!(check.passed);
    }

    #[test]
    fn memory_cgroup_fails_when_neither_present() {
        let check = memory_cgroup_check(None, false);
        assert!(!check.passed);
        assert_eq!(check.name, "memory cgroup");
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `rtk cargo test -p pi-infrastructure --locked memory_cgroup`
Expected: FAIL to compile (`memory_cgroup_check` undefined).

- [ ] **Step 3: Implement the pure helper**

Add a free function in `crates/infrastructure/src/probe.rs` (place it above `impl HostSystemProbe` or at module scope after the imports):

```rust
/// Pure decision for the doctor `memory cgroup` check. `controllers` is the
/// contents of `/sys/fs/cgroup/cgroup.controllers` (cgroup v2), `v1_present`
/// is whether `/sys/fs/cgroup/memory` exists (cgroup v1).
fn memory_cgroup_check(controllers: Option<String>, v1_present: bool) -> DiagnosticCheck {
    let v2_ok = controllers
        .as_deref()
        .map(|c| c.split_whitespace().any(|t| t == "memory"))
        .unwrap_or(false);
    let passed = v2_ok || v1_present;
    DiagnosticCheck {
        name: "memory cgroup".into(),
        passed,
        detail: if passed {
            "memory accounting enabled".into()
        } else {
            "memory cgroup controller disabled — per-container memory reports 0".into()
        },
        hint: (!passed).then(|| {
            "enable cgroup memory accounting: add 'cgroup_enable=memory cgroup_memory=1' to \
             /boot/cmdline.txt (or firmware/cmdline.txt) and reboot"
                .into()
        }),
    }
}
```

- [ ] **Step 4: Call it from `diagnostics()` (Linux only)**

In `diagnostics()`, immediately before the final `DiagnosticReport { checks }`, add:

```rust
        #[cfg(target_os = "linux")]
        {
            let controllers = std::fs::read_to_string("/sys/fs/cgroup/cgroup.controllers").ok();
            let v1_present = std::path::Path::new("/sys/fs/cgroup/memory").exists();
            checks.push(memory_cgroup_check(controllers, v1_present));
        }
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `rtk cargo test -p pi-infrastructure --locked memory_cgroup`
Expected: PASS (all four).

- [ ] **Step 6: Commit**

```bash
rtk git add crates/infrastructure/src/probe.rs
rtk git commit -m "feat(doctor): memory-cgroup accounting check with cmdline hint"
```

---

## Task 8: CLI — pure `human_bytes` + `sparkline` helpers

**Files:**
- Create: `crates/bin/src/cli/stats_render.rs` (`human_bytes`, `sparkline`)
- Modify: `crates/bin/src/cli/mod.rs` (add `pub mod stats_render;`)
- Test: `crates/bin/src/cli/stats_render.rs` (`#[cfg(test)]`)

**Interfaces:**
- Produces:
  - `pub fn human_bytes(n: u64) -> String` — `B` below 1024, else one decimal in KiB/MiB/GiB/TiB (÷1024).
  - `pub fn sparkline(values: &[f64], width: usize) -> String` — run of `▁▂▃▄▅▆▇█`; empty→"", single/`min==max`→mid block, otherwise scales min→max; uses at most the newest `width` values.

- [ ] **Step 1: Write the failing tests**

Create `crates/bin/src/cli/stats_render.rs`:

```rust
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
```

- [ ] **Step 2: Run to verify it fails**

Run: `rtk cargo test -p pi --locked stats_render`
Expected: FAIL to compile (`human_bytes` / `sparkline` undefined). Add `pub mod stats_render;` to `crates/bin/src/cli/mod.rs` now so the module is discovered.

- [ ] **Step 3: Implement the helpers**

Prepend to `crates/bin/src/cli/stats_render.rs`:

```rust
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
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `rtk cargo test -p pi --locked stats_render`
Expected: PASS (all five).

- [ ] **Step 5: Commit**

```bash
rtk git add crates/bin/src/cli/stats_render.rs crates/bin/src/cli/mod.rs
rtk git commit -m "feat(cli): human_bytes + sparkline pure render helpers"
```

---

## Task 9: CLI — `render_stats_static`

**Files:**
- Modify: `crates/bin/src/cli/stats_render.rs` (add `render_stats_static`)
- Modify: `crates/bin/src/cli/commands.rs` (make `human_duration` `pub(crate)` — visibility only)
- Test: `crates/bin/src/cli/stats_render.rs` (`#[cfg(test)]`)

**Interfaces:**
- Consumes: `human_bytes`, `sparkline` (Task 8); `crate::proto::StatsReportDto` (Task 2); `crate::output` table/cell helpers; `crate::cli::commands::human_duration` (made `pub(crate)` here).
- Produces: `pub fn render_stats_static(report: &crate::proto::StatsReportDto) -> String` — host panel (CPU%, MEM `used/total (X%)`, TEMP, DISK, UPTIME) + CPU%/TEMP sparkline lines from `host_history`, then a services table with `n/a` where `mem_limit_bytes == 0`. Pure (no stdout, no warns).

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module in `crates/bin/src/cli/stats_render.rs`:

```rust
    use crate::proto::{
        HostSampleDto, HostStatsDto, ProjectStatsDto, ServiceStatsDto, StatsReportDto,
    };

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
```

- [ ] **Step 2: Run to verify it fails**

Run: `rtk cargo test -p pi --locked render_stats_static`
Expected: FAIL to compile (`render_stats_static` undefined).

- [ ] **Step 3: Implement `render_stats_static`**

Add to `crates/bin/src/cli/stats_render.rs` (import the output helpers at the top of the file):

```rust
use crate::output;
use crate::proto::StatsReportDto;

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
    // uptime cell above uses crate::cli::commands::human_duration (see Step 3).

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
```

To avoid duplicating the uptime formatter, reuse the existing one in
`commands.rs`. Change its declaration (currently `fn human_duration(secs: u64)`
around line 789 of `crates/bin/src/cli/commands.rs`) to
`pub(crate) fn human_duration(secs: u64)` — a visibility-only change, no logic
edit. `render_stats_static` then calls `crate::cli::commands::human_duration`.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `rtk cargo test -p pi --locked render_stats_static`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
rtk git add crates/bin/src/cli/stats_render.rs
rtk git commit -m "feat(cli): render_stats_static host panel + sparklines + services table"
```

---

## Task 10: CLI — dependencies + realtime TUI module

**REQUIRED SUB-SKILL for the implementer:** invoke `ratatui-tui` before writing the terminal wiring, and (per the repo's context7 rule) fetch current `ratatui` 0.30 docs for `Chart`/`Sparkline`/`Dataset` if any API detail is uncertain. The **pure `build_frame` function and its test are the graded deliverable**; the terminal wiring is thin and verified by running `rpi stats -w`.

**Files:**
- Modify: `Cargo.toml` (workspace `[workspace.dependencies]`)
- Modify: `crates/bin/Cargo.toml` (`[dependencies]`)
- Create: `crates/bin/src/cli/stats_tui.rs` (pure `build_frame` + `stats_watch`)
- Modify: `crates/bin/src/cli/mod.rs` (add `pub mod stats_tui;`)
- Test: `crates/bin/src/cli/stats_tui.rs` (`#[cfg(test)]`)

**Interfaces:**
- Consumes: `crate::cli::api::ApiClient` (`stats(project: Option<&str>)`), `crate::proto::StatsReportDto`, `human_bytes` (Task 8).
- Produces:
  - `pub struct StatsFrame { pub cpu_points: Vec<(f64,f64)>, pub mem_points: Vec<(f64,f64)>, pub temp_points: Vec<(f64,f64)>, pub service_rows: Vec<[String;4]>, pub host_summary: String }`
  - `pub fn build_frame(report: &StatsReportDto) -> StatsFrame`
  - `pub async fn stats_watch(api: crate::cli::api::ApiClient, project: Option<String>, interval: u64) -> anyhow::Result<()>`

- [ ] **Step 1: Add the dependencies**

In workspace `Cargo.toml` `[workspace.dependencies]`, add:
```toml
ratatui = "0.30"
crossterm = { version = "0.29", features = ["event-stream"] }
```
In `crates/bin/Cargo.toml` `[dependencies]`, add:
```toml
ratatui = { workspace = true }
crossterm = { workspace = true }
```

Run: `rtk cargo build -p pi --locked`
Expected: PASS (deps resolve; nothing uses them yet).

- [ ] **Step 2: Write the failing `build_frame` test**

Create `crates/bin/src/cli/stats_tui.rs` with the test module:

```rust
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
```

- [ ] **Step 3: Run to verify it fails**

Run: `rtk cargo test -p pi --locked stats_tui`
Expected: FAIL to compile (`build_frame` undefined). Add `pub mod stats_tui;` to `crates/bin/src/cli/mod.rs` now.

- [ ] **Step 4: Implement the pure `build_frame`**

Prepend to `crates/bin/src/cli/stats_tui.rs`:

```rust
use crate::cli::stats_render::human_bytes;
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
```

- [ ] **Step 5: Run the pure test to verify it passes**

Run: `rtk cargo test -p pi --locked stats_tui`
Expected: PASS (both `build_frame` tests).

- [ ] **Step 6: Implement the terminal wiring (`stats_watch`)**

Append to `crates/bin/src/cli/stats_tui.rs`. This uses a custom RAII guard + panic hook (no `color-eyre`) and an async `select!` over a poll timer and a crossterm `EventStream`. Verify widget API against `ratatui` 0.30 via the `ratatui-tui` skill; adjust import paths/builders if the crate’s surface differs.

```rust
use std::io::{self, Stdout};

use crossterm::event::{Event, EventStream, KeyCode, KeyEvent, KeyModifiers};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::execute;
use futures::StreamExt;
use ratatui::prelude::*;
use ratatui::widgets::{Axis, Block, Borders, Chart, Dataset, GraphType, Paragraph, Row, Table};

use crate::cli::api::ApiClient;

/// Restores the terminal on drop (normal exit, `?`, or panic-unwind).
struct TerminalGuard;

impl TerminalGuard {
    fn enter() -> io::Result<TerminalGuard> {
        enable_raw_mode()?;
        execute!(io::stdout(), EnterAlternateScreen)?;
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
    // for a Pi: idle ~40, throttle ~80).
    let datasets = vec![
        Dataset::default()
            .name("cpu%")
            .graph_type(GraphType::Line)
            .data(&frame.cpu_points)
            .cyan(),
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
        .x_axis(Axis::default().bounds([0.0, x_max]))
        .y_axis(Axis::default().bounds([0.0, 100.0]).labels(["0", "50", "100"]));
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
```

Add `use crate::proto::StatsReportDto;` to the top of the file if not already imported by the pure section.

- [ ] **Step 7: Build + run the full suite; then smoke-test the binary compiles**

Run: `rtk cargo build -p pi --locked && rtk cargo test -p pi --locked stats_tui`
Expected: PASS. (Live TUI behaviour is verified in the feature-level check at the end against a dev agent.)

- [ ] **Step 8: Commit**

```bash
rtk git add Cargo.toml crates/bin/Cargo.toml crates/bin/src/cli/stats_tui.rs crates/bin/src/cli/mod.rs
rtk git commit -m "feat(cli): realtime stats TUI (ratatui) with pure frame builder"
```

---

## Task 11: CLI — `stats` command (static + watch) and `main.rs` wiring

**Files:**
- Modify: `crates/bin/src/cli/commands.rs` (rewrite `stats()`)
- Modify: `crates/bin/src/main.rs` (`Cmd::Stats` args + match arm)
- Test: `crates/bin/src/main.rs` (`#[cfg(test)]` clap tests) or `crates/bin/src/cli/commands.rs`

**Interfaces:**
- Consumes: `render_stats_static` (Task 9), `stats_watch` (Task 10), `ApiClient::stats`, `output::warn`.
- Produces: `pub async fn stats(project: Option<String>, json: bool, watch: bool, interval: u64, connect: ConnectOpts) -> anyhow::Result<()>`; `Cmd::Stats { project, json, watch, interval, connect }`.

- [ ] **Step 1: Rewrite `commands::stats`**

Replace the whole `pub async fn stats(...)` in `crates/bin/src/cli/commands.rs` (lines 378–446) with:

```rust
pub async fn stats(
    project: Option<String>,
    json: bool,
    watch: bool,
    interval: u64,
    connect: ConnectOpts,
) -> anyhow::Result<()> {
    let profile = connect.resolve()?;
    let tunnel = SshTunnel::open(&profile).await?;
    let api = ApiClient::new(tunnel.base_url.clone());

    if watch {
        // Keep the SSH tunnel alive for the duration of the watch loop.
        let _tunnel = tunnel;
        return crate::cli::stats_tui::stats_watch(api, project, interval).await;
    }

    let resp = api.stats(project.as_deref()).await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&resp)?);
        return Ok(());
    }

    print!("{}", crate::cli::stats_render::render_stats_static(&resp));

    if resp.host_history.is_empty() {
        output::warn("no host history from the agent - update the agent on the Pi to see graphs");
    }
    if resp
        .projects
        .iter()
        .flat_map(|p| &p.services)
        .any(|s| s.mem_limit_bytes == 0)
    {
        output::warn(
            "per-service memory shows n/a: enable cgroup memory accounting on the Pi \
             (run `rpi doctor` for the fix)",
        );
    }
    Ok(())
}
```

- [ ] **Step 2: Update `main.rs` `Cmd::Stats` definition + match arm**

In `crates/bin/src/main.rs`, change the `Stats` variant (lines 113–120) to:

```rust
    /// Live CPU/memory/disk/temperature metrics (add -w for a live graph)
    Stats {
        project: Option<String>,
        #[arg(long)]
        json: bool,
        /// Full-screen live-updating view; quit with q/Esc/Ctrl-C
        #[arg(short = 'w', long)]
        watch: bool,
        /// Refresh interval in seconds for --watch
        #[arg(long, default_value_t = 2)]
        interval: u64,
        #[command(flatten)]
        connect: cli::config::ConnectOpts,
    },
```

And the match arm (lines 352–356):

```rust
        Cmd::Stats {
            project,
            json,
            watch,
            interval,
            connect,
        } => cli::commands::stats(project, json, watch, interval, connect).await,
```

- [ ] **Step 3: Write the failing clap parse tests**

If `main.rs` has no test module, add one; otherwise extend it. The `Cli`/`Cmd` types must be reachable (they already are in `main.rs`). Add:

```rust
#[cfg(test)]
mod stats_cli_tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn stats_watch_with_interval() {
        let cli = Cli::parse_from(["rpi", "stats", "-w", "--interval", "5"]);
        match cli.command {
            Cmd::Stats {
                watch,
                interval,
                json,
                project,
                ..
            } => {
                assert!(watch);
                assert_eq!(interval, 5);
                assert!(!json);
                assert_eq!(project, None);
            }
            _ => panic!("expected Stats"),
        }
    }

    #[test]
    fn stats_json_and_positional_project() {
        let cli = Cli::parse_from(["rpi", "stats", "rateme", "--json"]);
        match cli.command {
            Cmd::Stats {
                json,
                project,
                watch,
                interval,
                ..
            } => {
                assert!(json);
                assert_eq!(project.as_deref(), Some("rateme"));
                assert!(!watch);
                assert_eq!(interval, 2); // default
            }
            _ => panic!("expected Stats"),
        }
    }
}
```

Confirm the top-level parser type name (`Cli`) and the field holding the subcommand (`command`) match `main.rs`; adjust the test to the actual names if they differ.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `rtk cargo test -p pi --locked stats_cli_tests`
Expected: PASS.

- [ ] **Step 5: Full workspace check**

Run: `rtk cargo build --locked && rtk cargo test --locked`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
rtk git add crates/bin/src/cli/commands.rs crates/bin/src/main.rs
rtk git commit -m "feat(cli): stats static view + -w/--watch flags wired through main"
```

---

## Task 12: CLI — ApiClient stats-decode test for the extended DTO

**Files:**
- Modify: `crates/bin/src/cli/api.rs` (test module — reuse `spawn_app`)
- Test: `crates/bin/src/cli/api.rs` (`#[cfg(test)]`)

**Interfaces:**
- Consumes: `ApiClient::stats` (existing), `StatsReportDto`/`HostSampleDto` (Task 2), `spawn_app` (existing test helper).

- [ ] **Step 1: Write the failing decode test**

Add to the `tests` module in `crates/bin/src/cli/api.rs` (a handler that returns an extended body plus the assertion):

```rust
    async fn stats_extended() -> impl IntoResponse {
        axum::Json(serde_json::json!({
            "host": {
                "cpu_percent": 12.5,
                "mem_used_bytes": 1024,
                "mem_total_bytes": 4096,
                "disk_used_percent": 40,
                "uptime_secs": 3600,
                "temp_celsius": 47.5
            },
            "projects": [],
            "host_history": [
                {"at_ms": 1, "cpu_percent": 10.0, "mem_used_bytes": 1000, "mem_total_bytes": 4096, "temp_celsius": 45.0},
                {"at_ms": 2, "cpu_percent": 12.5, "mem_used_bytes": 1024, "mem_total_bytes": 4096, "temp_celsius": 47.5}
            ]
        }))
    }

    #[tokio::test]
    async fn stats_decodes_temp_and_history() {
        let app = Router::new().route("/v1/stats", get(stats_extended));
        let client = ApiClient::new(spawn_app(app).await);
        let resp = client.stats(None).await.unwrap();
        assert_eq!(resp.host.temp_celsius, Some(47.5));
        assert_eq!(resp.host_history.len(), 2);
        assert_eq!(resp.host_history[1].cpu_percent, 12.5);
    }
```

Ensure `get` is imported in the test module (it already imports `use axum::routing::{get, post};`).

- [ ] **Step 2: Run to verify it fails, then passes**

Run: `rtk cargo test -p pi --locked stats_decodes_temp_and_history`
Expected: PASS immediately — `ApiClient::stats` already decodes `StatsReportDto`, and the extended fields exist from Task 2. (If it fails to compile, a needed import is missing — add it.) This test is a regression guard for the wire contract.

- [ ] **Step 3: Commit**

```bash
rtk git add crates/bin/src/cli/api.rs
rtk git commit -m "test(cli): ApiClient::stats decodes temp + host_history"
```

---

## Final verification (feature-level, after all tasks)

- [ ] **CI gate — the exact commands CI runs:**

```bash
rtk cargo fmt --all -- --check
rtk cargo clippy --all-targets --locked -- -D warnings
rtk cargo test --locked
```
Expected: all clean. If `fmt` reports a diff, run `rtk cargo fmt --all` and commit.

- [ ] **Drive the real flow (verification-before-completion):**
  1. Start a dev agent locally (see the `rpi-cli` skill / `Cmd::Agent` with a `tcp` bind in `agent.toml`).
  2. `rpi stats` → static view shows the host panel with human sizes, a `TEMP` column (`n/a` on a dev box with no thermal zone), CPU%/TEMP sparkline rows once ~2 samples exist, and any zero-limit service memory as `n/a`.
  3. `rpi stats -w` → full-screen chart updates every 2s; `q`/`Esc`/`Ctrl-C` restores the terminal cleanly (no leftover raw mode).
  4. `rpi stats --json` → JSON includes `temp_celsius` and `host_history`.
  5. `rpi doctor` on Linux → includes a `memory cgroup` check.

---

## Self-review notes (coverage against the spec)

- Domain entities/contracts → Task 1. Proto additive DTOs + compat → Task 2 (compat matrix "new CLI vs old agent" covered by the defaults test). `ThermalZoneTempProbe` → Task 3. Ring buffer → Task 4. Sampler (immediate seed, `start()` loop, handle) → Task 5. `CompositeStats` rewrite + `state.rs`/`run.rs`/`http.rs` wiring + agent http test → Task 6. Doctor memory-cgroup → Task 7. `human_bytes`/`sparkline` → Task 8. `render_stats_static` → Task 9. Deps + realtime TUI (RAII guard + panic hook, pure `build_frame`) → Task 10. `stats` command + `main.rs` flags + clap tests → Task 11. `ApiClient::stats` decode test → Task 12.
- Non-goals (per-service graphs, history persistence, `/v1/stats/host`, cgroup auto-fix) are respected — no task implements them.
- Type consistency: `HostSample`, `HostMetricsStore::{latest,history}`, `CompositeStats::new(runtime, disk, metrics)`, `build_state -> (AppState, HostMetricsSampler)`, `stats(project, json, watch, interval, connect)`, and `render_stats_static`/`build_frame`/`human_bytes`/`sparkline` names are used identically across tasks.
