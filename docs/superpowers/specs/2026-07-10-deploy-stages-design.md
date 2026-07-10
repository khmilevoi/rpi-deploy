# Deploy pipeline view — stage events (package B)

Date: 2026-07-10
Status: approved

## Context

Package B of the two-part CLI brand effort (see
`2026-07-09-cli-brand-visuals-design.md`, "Non-goals"). Package A delivered the
deploy banner and result stamp; this package makes the deploy stream show the
pipeline itself: each agent-side stage as a timed, collapsing log pane, plus a
service count in the final stamp. Unlike package A this changes the agent→CLI
SSE protocol (additively) and the agent's log plumbing.

## Scope decisions (settled)

- **UX: collapsing panes.** Each stage renders its own live log pane (the
  existing `LogPane` box). When a stage completes, its pane collapses to a
  one-line summary (`✓ build (48.3s)`) and the next stage opens a new pane
  below. The CLI is purely reactive — it does not need to know the stage plan
  upfront (docker-buildx style), so new agent stages appear automatically.
- **Six announced stages:** `fetch`, `build`, `start`, `health`, `route`, `gc`.
  Instant steps (project registration, secrets injection, override write) are
  not stages; their lines print as plain lines. `route` is announced only when
  the project has a hostname. `gc` failure does not fail the deploy — it
  reports `skipped`.
- **Failure output: dump only the failed stage.** The failed stage's pane turns
  red and its full captured lines are printed below it; earlier (successful)
  stages stay collapsed. The agent DB keeps the 400-line tail as before.
- **Service count in the stamp** is included in this package:
  `▸ deployed ✓  myboard  →  <url> · 2 services   (64.2s)`.

## Protocol (SSE, additive — no API version bump)

Stream: `GET /v1/deployments/{id}/logs`. Two new event types alongside the
existing `log` and `finished`:

- `event: stage` — JSON payload:
  - `{"stage":"build","status":"started"}`
  - `{"stage":"build","status":"ok","elapsed_ms":48231}`
  - statuses: `started` | `ok` | `failed` | `skipped`. `elapsed_ms` is present
    on `ok`/`failed`/`skipped`, absent on `started`.
  - wire stage names: `fetch`, `build`, `start`, `health`, `route`, `gc`. The
    CLI renders unknown stage names as-is and ignores events with unknown
    statuses or unparsable JSON — forward compatibility for future stages.
- `event: summary` — JSON `{"services":2}`, sent once after the health gate
  passes (count from `runtime.ps(project)`), before `finished`. If `ps` fails
  the event is simply not sent (non-fatal).
- `event: finished` — **unchanged** (bare status string). Old CLIs compare it
  as a string; it must stay a plain token.

Compatibility matrix:

| CLI \ agent | old agent | new agent |
|---|---|---|
| old CLI | today | ignores `stage`/`summary` (verified: `follow_logs` has a `_ => {}` arm) — today's view |
| new CLI | no stage events → single-pane fallback, byte-for-byte today's view | pipeline view |

## Agent: domain + application

- `LogSink` (`crates/domain/src/contracts.rs`) gains two methods:
  `fn stage(&self, ev: &StageEvent)` and `fn summary(&self, services: usize)`.
  New domain entity:
  `StageEvent { stage: String, status: StageStatus, elapsed_ms: Option<u64> }`
  with `enum StageStatus { Started, Ok, Failed, Skipped }`.
- Sink chain forwards it: `MaskingSink` passes through unchanged (stage names
  carry no secrets), `TailSink` forwards **and** records a plain boundary line
  into its tail buffer (`▸ build ok (48.3s)`; plain `{secs:.1}s` formatting —
  `crate::duration` lives in the bin crate and is not available here) so the
  DB `log_tail` stays readable with no schema change. `CollectSink` (test
  support) records events.
- `DeployProject::run_stages` emits `started` before and `ok`/`failed` (with
  measured `elapsed_ms`) after each of the six stages. The existing `staged()`
  timeout wrapper is the natural seam — it already takes the stage name.
  `health` is wrapped the same way (it keeps its own internal timeout).
- Stage display names are unified with the wire names: the timeout error text
  becomes `timeout: start` (was `up`). Config/DTO field names (`up_secs` in
  rpi.toml, `TimeoutsDto`) are wire compatibility and **do not change**.
- `route`: emitted around `ingress.upsert` when a hostname is configured;
  `IngressOutcome::Skipped` (ingress disabled on the agent) maps to status
  `skipped`. No hostname → the stage is never announced.
- `gc`: an `Err` from the gc stage maps to `skipped` (deploy still succeeds),
  matching today's "gc skipped: {err}" line.
- `summary`: after the health gate passes, `run_stages` calls
  `self.runtime.ps(&config.name)` and emits the service count via
  `LogSink::summary`, carried through the same sink chain.
- Cancellation drops the `run_stages` future, so no `failed` stage event is
  emitted; the CLI closes any open pane when `finished` arrives (statuses
  other than the current stage's completion are handled by the pane-close
  rule, see CLI section).

## Hub and HTTP

- `DeployEvent` (`crates/infrastructure/src/events.rs`) becomes
  `Line(String) | Stage(StageEvent) | Summary(usize) | Finished(DeploymentStatus)`.
  The hub backlog changes from lines to events (`VecDeque<DeployEvent>`,
  same cap of 1000 entries — a deploy adds at most ~14 stage/summary events),
  so reconnects and queued deploys replay the full pane structure in order.
- `HubSink` implements the new `LogSink` methods by broadcasting the
  corresponding variants.
- `agent/http.rs` `deployment_logs` maps hub events to SSE:
  `sse_log` / `sse_stage` (serde-serialized JSON) / `sse_summary` /
  `sse_finished`. Other SSE routes (`project_logs`, `agent_logs`, command
  exec) are untouched.

## CLI

- `ApiClient::follow_logs` changes its callback from `FnMut(&str)` to
  `FnMut(DeployStreamEvent)` where
  `DeployStreamEvent::Line(&str) | Stage { stage, status, elapsed_ms } | Summary { services }`.
  `stage`/`summary` payloads that fail to parse are ignored. Only `deploy`
  uses `follow_logs`; `stream_sse` (commands, logs) is untouched.
- New orchestrator `output::Pipeline` (new file `output/pipeline.rs`) built on
  top of the existing `LogPane`:
  - Starts in **legacy mode**: lines go into a `deploy '<name>'` pane exactly
    as today. If the stream never carries a stage event (old agent), behaviour
    is byte-for-byte today's, including `finish_ok`/`finish_err` semantics.
  - First `stage started` event: the legacy pane collapses silently (cleared,
    no summary line) and pipeline mode begins.
  - `started` → open a pane labelled with the stage name (max 10 visible
    lines, as today). Lines route to the open pane; lines arriving between
    panes print as plain lines.
  - `ok` → clear the pane, print `✓ build (48.3s)` (green check, dim elapsed,
    via the existing `Sem` styles; elapsed rendered by `crate::duration`).
  - `skipped` → clear the pane (if one is open — `route skipped` arrives as a
    completion without lines), print a dim `· route skipped (0.4s)`, elapsed
    included when the payload carries it.
  - `failed` → recolour the pane red in place (existing `finish_err` frame),
    print `— build log —` followed by that stage's full captured lines, then
    a red `✗ build (12.1s)` line. Earlier stages stay collapsed.
  - `finished` with a pane still open (cancel/supersede/agent restart) →
    the pane collapses with the neutral/red treatment matching the final
    status, no elapsed.
  - Unicode/ASCII: `✓`/`✗`/`·`/`▸` degrade through the same
    `console::Emoji`-based gate package A uses (`✓`→`ok`, `✗`→`x`).
- Non-interactive output (pipe/CI): lines stream as today; stage completions
  print boundary lines `▸ build ok (48.3s)` (nothing on `started`).
- `deploy_stamp` gains `services: Option<usize>`; the segment `· 2 services`
  appears only when a `summary` event was received.
- Failure path keeps today's contract: warnings reprinted, exit code 1.

## Testing

- `application::deploy`: extended `CollectSink` asserts the ordered stage
  event sequence for the happy path (`fetch → build → start → health → route →
  gc` with `started`/`ok` pairs); `route` absent without hostname; `skipped`
  for disabled ingress and for gc failure; `failed` (with `elapsed_ms`) on
  stage error and on stage timeout; no stage events after cancellation.
- `infrastructure::events`: mixed backlog (lines + stage + summary) replays in
  order; cap still enforced.
- `agent/http`: SSE stream carries `stage`/`summary` events with the exact
  JSON shape; `finished` still a bare token.
- `cli`: `follow_logs` surfaces typed events and ignores malformed payloads;
  `Pipeline` rendering — legacy fallback, silent collapse of the pre-stage
  pane, collapse lines, failed-stage red dump limited to that stage's lines,
  non-interactive boundary lines; `deploy_stamp` with and without services.
- Compat: a legacy stream (only `log`/`finished`) renders identically to
  today's output.

## Non-goals

- Persisting per-stage timings in the DB or exposing them via other routes.
- A ticking elapsed timer in the live pane header (`LogPane` redraws only on
  new lines; not worth a timer thread).
- Stage events for `rpi command`, `rpi logs`, or any stream other than deploy.
- Changing `finished` payload shape or bumping the API version.
