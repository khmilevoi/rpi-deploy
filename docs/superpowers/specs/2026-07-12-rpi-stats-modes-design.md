# rpi stats — history-backed graphs, human-readable sizes, temperature

Date: 2026-07-12
Status: approved

## Context

`rpi stats` today renders a single instant snapshot: two `comfy-table` tables
(host row + per-service rows) produced by one round-trip to `GET /v1/stats`.
Three problems motivate this work:

1. **Output looks wrong.** Per-service memory shows `0/0 bytes` even for live
   containers (e.g. `valkey` at 0.2% CPU). Root cause is host-side: the
   Raspberry Pi kernel ships with the memory cgroup controller disabled, so
   `docker stats` reports `0B / 0B` for memory while CPU is fine. Our parser
   faithfully turns `0B / 0B` into `(0, 0)`.
2. **Sizes are unreadable.** Memory prints as raw bytes
   (`1249918976/8454029312 bytes`).
3. **No time dimension and no temperature.** The user wants graphs over time in
   two flavours — a live-updating **realtime** view and a one-shot **static**
   view — plus CPU temperature.

The graph requirement means the agent must retain a short **time series**, which
today it does not: every `/v1/stats` call is a fresh snapshot.

## Scope decisions (settled)

- **History lives in the agent** (ring buffer + background sampler), not the CLI.
  This makes the static view instant and meaningful (fetch → draw → exit) and
  keeps the realtime chart smooth regardless of CLI poll cadence. Chosen over a
  CLI-collects-its-own-window approach.
- **Host graphs only for now.** Time-series charts cover host CPU%, MEM%, and
  TEMP. Per-service data stays a current-values table. Full per-service graphs
  are deferred to a later dashboard.
- **Two modes, one command:**
  - `rpi stats [project]` → **static**: host panel with unicode sparklines built
    from agent history + services table. Prints and exits. Replaces today's
    instant output.
  - `rpi stats [project] -w/--watch [--interval N]` → **realtime**: full-screen
    `ratatui` TUI, refresh every `N` seconds (default 2), quit on
    `q`/`Esc`/`Ctrl-C`.
  - `rpi stats … --json` → snapshot JSON as today, plus the temperature field.
- **Temperature** from the Linux thermal zone file
  (`/sys/class/thermal/thermal_zone*/temp`, millidegrees). `Option<f64>` °C, CPU
  zone only. `None` on non-Pi hosts / old kernels → renders `n/a`.
- **Memory sampling stays host-only.** The background sampler reads sysinfo +
  temperature every 2s; it does NOT shell out to `docker stats` (too expensive
  at that cadence). Per-service metrics remain fetched on demand at request time.
- **`rpi doctor` gains a memory-cgroup check** (do it now, not deferred).
- **Units are binary/IEC** (KiB/MiB/GiB/TiB, 1024).
- **History is in-memory only** — not persisted; an agent restart resets the
  chart.
- **Old-agent compatibility via `#[serde(default)]`** on the new response fields;
  a new CLI against an old agent degrades to a snapshot (no sparklines,
  `temp n/a`) and prints a one-time warning.

## Defaults locked in

- Sampler interval: **2s**. History window: **5 minutes** (~150 samples),
  evicted by age.
- Realtime `--interval` default: **2s**.
- Realtime polls the full `/v1/stats` (services included) each interval; no
  separate lightweight host endpoint (add `/v1/stats/host` later if load shows
  it is needed).

## Domain (crates/domain)

### Entities (`entities.rs`)

- `HostStats` gains `temp_celsius: Option<f64>`.
- New `HostSample`:
  ```rust
  pub struct HostSample {
      pub at_ms: i64,          // unix millis of the sample
      pub cpu_percent: f64,
      pub mem_used_bytes: u64,
      pub mem_total_bytes: u64,
      pub temp_celsius: Option<f64>,
  }
  ```
  Disk and uptime are intentionally not sampled per-tick (disk changes slowly,
  uptime is derivable); they are assembled into the snapshot at request time.
- `StatsReport` gains `host_history: Vec<HostSample>`.

### Contracts (`contracts.rs`)

Both new traits carry `#[cfg_attr(feature = "mocks", automock)]`.

- `HostMetricsStore` — read side of the ring buffer (synchronous, no IO):
  ```rust
  pub trait HostMetricsStore: Send + Sync {
      fn latest(&self) -> Option<HostSample>;
      fn history(&self) -> Vec<HostSample>;
  }
  ```
- `TempProbe` — CPU temperature (synchronous):
  ```rust
  pub trait TempProbe: Send + Sync {
      fn cpu_celsius(&self) -> Option<f64>;
  }
  ```

## Infrastructure (crates/infrastructure)

### `ThermalZoneTempProbe` (new, e.g. `temp.rs`)

- Reads `<root>/sys/class/thermal/thermal_zone*/`. For each zone read `type`;
  prefer the zone whose type contains `cpu` (e.g. `cpu-thermal`), else fall back
  to `thermal_zone0`. Read `temp` (millidegrees) → °C.
- Returns `None` on any failure (missing dir on Windows/dev, unreadable files).
- Root path injected (mirrors `SysinfoDiskProbe::new(path)`) so tests point at a
  temp dir with fake `thermal_zone{0,1}/{type,temp}` files.

### `HostMetricsSampler` (new, e.g. `metrics.rs`)

- Owns the ring buffer: `Arc<Mutex<VecDeque<HostSample>>>` (std `Mutex`; the
  critical section is a push/evict with no `.await` held).
- Ring-buffer logic is a **pure struct** (`push(sample)`, evict older than the
  window, `latest()`, `snapshot()`), unit-tested independently of tokio.
- Construction takes a persistent `sysinfo::System`, a `TempProbe`, the window
  and interval. On construction it takes **one immediate sample** so `latest()`
  is `Some` before the first request.
- `start()` spawns the tokio loop (every `interval`: refresh sysinfo CPU+mem,
  read temp, push). Returns a cloneable handle implementing `HostMetricsStore`.
- Between refreshes sysinfo computes CPU% from the delta, so the 200ms
  double-sample trick in the current `CompositeStats` is dropped.

### `CompositeStats` (`stats.rs`, rewritten)

New dependencies: `metrics: Arc<dyn HostMetricsStore>`, keeps `disk` and
`runtime`. `report()`:

- `latest = metrics.latest()` → assemble `HostStats` (cpu/mem/temp from the
  sample) + `disk.used_percent()` + `System::uptime()`. If the buffer is somehow
  empty, fall back to a fresh sysinfo read (defensive; sampler pre-seeds one).
- `host_history = metrics.history()`.
- Per-service stats unchanged (on-demand docker via `runtime.stats`).

### Doctor memory-cgroup check (`probe.rs`)

Add a `memory cgroup` check to `HostSystemProbe::diagnostics()`:

- cgroup v2: pass if `/sys/fs/cgroup/cgroup.controllers` contains `memory`.
- cgroup v1: pass if `/sys/fs/cgroup/memory` exists.
- Neither → fail, detail explains memory accounting is off, hint:
  `enable cgroup memory accounting: add 'cgroup_enable=memory cgroup_memory=1' to /boot/cmdline.txt (or firmware/cmdline.txt) and reboot`.
- Implemented as a pure helper `memory_cgroup_check(controllers: Option<String>, v1_present: bool) -> DiagnosticCheck`, unit-tested; `diagnostics()` reads the files (std::fs) and calls it. On non-Linux the check is skipped.

## Wiring (crates/bin/src/agent/state.rs)

- Build `ThermalZoneTempProbe` (root `/`).
- Build `HostMetricsSampler` (sysinfo `System`, temp probe, 2s / 5min). The
  constructor takes one immediate sample synchronously and does **not** spawn —
  so `build_state` stays synchronous and runtime-agnostic. Store the sampler
  handle in `AppState` (as `Arc<dyn HostMetricsStore>`) and pass it into
  `CompositeStats::new(...)` alongside `runtime` and `disk`.
- The tokio sampling loop is started by a separate `start()` call made once from
  the agent's async entrypoint (`main`) after `build_state` returns, so
  `tokio::spawn` runs inside the runtime. Tests that never call `start()` still
  see the pre-seeded `latest()` sample.
- Test fixture `state_with...` in `agent/http.rs` gets a stub `HostMetricsStore`
  (fixed sample + history) — no sampler, no spawn.

## Protocol (crates/bin/src/proto.rs) — additive, no API version bump

- `HostStatsDto` gains `#[serde(default)] temp_celsius: Option<f64>`.
- New `HostSampleDto { at_ms, cpu_percent, mem_used_bytes, mem_total_bytes, temp_celsius }`.
- `StatsReportDto` gains `#[serde(default)] host_history: Vec<HostSampleDto>`.
- `#[serde(default)]` is the compatibility mechanism: a **new CLI decoding an old
  agent's response** (which lacks these fields) gets empty history / `None` temp
  and does not error.

Compatibility matrix:

| CLI \ agent | old agent | new agent |
|---|---|---|
| old CLI | today | ignores extra JSON fields — today's view |
| new CLI | empty history + `temp n/a` + one-time warn, snapshot only | full static/realtime |

The agent handler `GET /v1/stats` and the `StatsQuery` are unchanged in shape;
the response simply carries the new fields.

## CLI (crates/bin/src/cli)

### Pure helpers (tested)

- `human_bytes(u64) -> String` → `B` / `KiB` / `MiB` / `GiB` / `TiB` (÷1024, one
  decimal above `B`). Replaces every `"{}/{} bytes"` site (host and services).
- `sparkline(values: &[f64], width: usize) -> String` → run of `▁▂▃▄▅▆▇█`.
  Handles empty (blank), single value, `min == max` (flat mid-block), and
  scales min→max across the block ramp.
- `render_stats_static(report: &StatsReportDto) -> String` (or writes to a
  buffer) — assembles the whole static view: host panel (CPU%, MEM
  `used/total (X%)`, TEMP, DISK, UPTIME) + CPU%/TEMP sparklines from
  `host_history`, then the services table with `n/a` where `mem_limit == 0`.

### `stats` command (`commands.rs`)

- Signature gains `watch: bool` and `interval: u64`.
- `--json` → unchanged early return (now includes temp).
- No `--watch` → build report, `print!(render_stats_static(&report))`. Emit
  `output::warn(...)` once when `host_history` is empty ("update the agent on the
  Pi to see graphs") and once when any service has `mem_limit == 0`
  (cgroup-memory hint). `temp None` → `n/a`.
- `--watch` → `stats_watch(api, project, interval)`.

### Realtime TUI (`stats_watch`, new module e.g. `cli/stats_tui.rs`)

- `ratatui` + `crossterm`. Enter alternate screen + raw mode behind an RAII
  guard that restores the terminal on drop **and on panic** (custom guard +
  panic hook; no `color-eyre` dependency).
- Loop: poll `api.stats(project)` every `interval`; non-blocking crossterm event
  read with a timeout drives both input and the redraw tick. Draw host block +
  `Chart`/`Sparkline` for CPU%/MEM%/TEMP from `host_history` + services table.
- Because each response carries the full 5-minute history, the first frame is
  already populated and the chart stays smooth at any poll rate.
- A failed poll shows a `reconnecting…` status and keeps the last frame; a dead
  SSH tunnel exits with an error after restoring the terminal.
- The frame-building logic (DTO → chart datasets / rows) is factored into a pure
  function so it is testable without a real terminal; the event/terminal wiring
  stays thin.

### main.rs

`Cmd::Stats` gains `#[arg(short = 'w', long)] watch: bool` and
`#[arg(long, default_value_t = 2)] interval: u64` with `///` help. The match arm
forwards them to `cli::commands::stats`.

## Dependencies

- Add `ratatui` and `crossterm` to the workspace `Cargo.toml` and
  `crates/bin/Cargo.toml`. Match `crossterm` to the version ratatui re-exports.

## Testing

- `TempProbe`: fake `thermal_zone{0,1}` → picks the `cpu` zone, parses
  millidegrees, `None` when the tree is absent.
- Ring buffer: window eviction by age, `latest()` returns newest, `snapshot()`
  ordering.
- `CompositeStats` (mock `HostMetricsStore` + `MockDiskProbe` +
  `MockContainerRuntime`): host assembled from latest sample + disk + uptime;
  `host_history` passed through; per-service `mem_limit == 0` preserved.
- Doctor: `memory_cgroup_check` for v2 (controllers contains / lacks `memory`)
  and v1 (dir present / absent).
- Proto (key compat test): decode an old-agent JSON body with no `host_history`
  / `temp_celsius` → empty / `None`; plus full roundtrip.
- CLI: `human_bytes` boundaries (1023→`B`, 1024→`1.0 KiB`, MiB/GiB/TiB);
  `sparkline` (empty, single, flat, scaled); `render_stats_static` (contains
  `n/a` for `None` temp and for zero-limit mem, sparkline row present when
  history exists, warn path when history empty).
- Agent http: `GET /v1/stats` returns history + temp via `state_with` fixture.
- `ApiClient::stats` decodes the extended DTO (extend the `spawn_app` test).
- clap parse: `stats -w --interval 5`, `stats --json`, `stats <project>`.
- TUI frame builder: pure DTO→dataset function (history → chart points, services
  → rows) without a terminal.

## Non-goals

- Per-service time-series graphs (future dashboard).
- Persisting history across agent restarts.
- A dedicated lightweight host-only polling endpoint (`/v1/stats/host`) — only if
  realtime load proves it necessary.
- Auto-fixing the memory cgroup (doctor only reports + hints).

## CI gate

`rtk cargo fmt --all -- --check && rtk cargo clippy --all-targets --locked -- -D warnings && rtk cargo test --locked`.
