# cloudflared setup safety hardening — Design

## 0. Context & motivation

The 2026-07-09 adoption work (`…-rpi-cloudflared-adoption-design.md`, merged to
master) made `setup --with-cloudflared` adopt an existing `config.yml` instead of
overwriting it, injected `XDG_RUNTIME_DIR` into the deploy-time cloudflared
restart, and added a `rpi doctor` check for "hostname declared while ingress
disabled". A real prod incident (a hand-built `myboard` tunnel overwritten →
`board.iiskelo.com` → HTTP 530) confirmed the failure class the adoption work
addresses; it happened only because the Pi still runs the **pre-adoption
binary**.

This batch closes the residual gaps of the same class — *the tool must never
silently overwrite/disrupt an existing tunnel, and must fail loudly when it
cannot proceed safely* — plus two adjacent robustness/security fixes surfaced by
the same review. Four independent parts:

- **A. Foreign-tunnel guard** — refuse the fresh-install path when a cloudflared
  the tool didn't create is already running.
- **B. Secure token input** — accept the API token from a file/stdin/env, not
  only argv (which leaks via `ps`/history/`journald`).
- **C. Doctor half-state checks** — flag "ingress configured but connector down"
  and "declared hostname with no route".
- **D. DBUS in the user-scoped systemctl env** — belt-and-suspenders alongside
  the existing `XDG_RUNTIME_DIR` injection.

Parts are independent (different files/seams) and can be implemented and reviewed
task-by-task. No wire/protocol/schema changes; no new crate dependency
(`serde_yaml` already present in both crates touched).

---

## A. Foreign-tunnel guard

### A.1 Problem

`cloudflared_bootstrap_full` (`crates/bin/src/agent/setup.rs`) takes the
fresh-install path whenever `/var/lib/rpi/cloudflared/config.yml` is absent: it
mints a new tunnel via the Cloudflare API, writes `config.yml`, then
`cloudflared_user_service` **unconditionally overwrites** the `cloudflared` user
unit (`CLOUDFLARED_UNIT_PATH`) and **restarts `user@<uid>.service`**. If a
cloudflared the tool didn't create is already running (a hand-run
`cloudflared tunnel run`, or a tunnel whose config lives off the standard path),
that fresh path disrupts it silently. Adoption covers the "config.yml present"
case; this is the leftover.

### A.2 Key invariant

The fresh branch runs **only** when `config.yml` is absent, i.e. rpi has never
created a tunnel here — so **any cloudflared running at that moment is foreign**.
No ownership bookkeeping needed. If `config.yml` exists, the adoption branch runs
first and this guard never fires.

### A.3 Detection

Helper over the existing `Sys` trait:

```
enum CloudflaredState { Running, NotRunning, Undetermined }
async fn cloudflared_running(sys: &dyn Sys) -> CloudflaredState
```

Implemented with `pgrep -x cloudflared`, mapped through `Sys::run`'s contract
(`Ok(stdout)` exit 0; `Err(stderr)` non-zero; `Err("spawn …")` when the program
can't be launched):

| `run("pgrep",["-x","cloudflared"])` | meaning | state |
|---|---|---|
| `Ok(_)` | a cloudflared process exists | `Running` |
| `Err(e)`, `e` starts with `"spawn "` | pgrep not installed | `Undetermined` |
| `Err(_)` otherwise | pgrep ran, no match | `NotRunning` |

Detecting a *running process*, not the unit file, is deliberate: rpi's own
no-token fallback (`setup.rs` ~884) scaffolds `CLOUDFLARED_UNIT_PATH` without
starting it, so a unit-file check would false-positive and break the legitimate
"scaffold now, finish with a token later" workflow.

### A.4 The guard

Placement **P1**: at the top of `cloudflared_bootstrap_full`'s fresh/else branch
(the `else` of `if sys.exists(CLOUDFLARED_CONFIG_PATH)`), **before** the
`if dry { … }` block and any tunnel creation. On refuse the wrapper's existing
`if rep.errors.len() > errs_before { return }` guard (`cloudflared_bootstrap`
~865) already skips `cloudflared_user_service`.

- `NotRunning` → proceed with the fresh install exactly as today.
- `Running` → real run: `rep.errors.push(<refuse msg>)`, `return false`; dry run:
  `rep.skipped.push("would refuse: …")`, `return false`.
- `Undetermined` → same as `Running` (refuse-to-be-safe), message names pgrep.

**Side effects on refuse:** the API token file may already be written by
`ensure_cloudflare_token` (harmless — operator's own token, reused next run);
`ensure_cloudflared_binary` only installs when cloudflared is absent, in which
case nothing is running and the guard returns `NotRunning`. Every destructive
action (create tunnel, write config.yml, overwrite unit, restart) happens after
the guard.

### A.5 Messages

Real run, `Running`:
> `a cloudflared tunnel is already running on this host, but rpi has no config.yml at /var/lib/rpi/cloudflared/config.yml to adopt — refusing to overwrite it and restart the tunnel. Stop the running cloudflared (then re-run to create a fresh rpi-managed tunnel), or move its config to /var/lib/rpi/cloudflared/config.yml so setup adopts it.`

Real run, `Undetermined`:
> `could not check for a running cloudflared (pgrep unavailable); refusing to proceed rather than risk overwriting an existing tunnel — verify manually that no cloudflared is running, or install pgrep, then re-run.`

Dry run: same substance via `rep.skipped` as `would refuse: …`.

### A.6 Non-goals

No `--force` override (the message gives two clean exits); no change to the
no-token fallback path (already non-destructive).

---

## B. Secure token input

### B.1 Problem

`--cf-token <token>` puts the secret in argv → leaks to `ps`, shell history, and
`journald` (`sudo … COMMAND=…`). Env `CLOUDFLARE_API_TOKEN` is already accepted
(`setup.rs:1087`), so a leak-free path exists, but there is no file/stdin input
and `--cf-token` is not flagged as unsafe.

### B.2 Resolution order (in `run_cmd`, `setup.rs`)

Most-secure first; the first that yields a non-empty token wins:

1. `--cf-token-file <path>` — read the secret from `<path>`; `path == "-"` reads
   **stdin**. Trim trailing whitespace/newline. An unreadable path, or an empty
   resolved token (empty file/stdin), is a **hard error**, not a fall-through.
2. `--cf-token <inline>` — **deprecated**: honored for back-compat, but surfaces
   an operator-visible warning (setup-report warning / stderr) that it leaks via
   `ps`/shell-history/`journald` and suggests `--cf-token-file`/`CLOUDFLARE_API_TOKEN`.
3. env `CLOUDFLARE_API_TOKEN` — existing fallback, unchanged.

`--cf-token-file` and `--cf-token` together → `--cf-token-file` wins (still warn
about the inline one). Resolution happens before `SetupOpts` is built; the
resolved token flows through the existing `cf_token: Option<String>` field, so
no downstream change.

### B.3 CLI (`main.rs`)

Add `--cf-token-file <PATH>` to the `setup` args next to `--cf-token`; update the
`--cf-token` help to note it is deprecated/unsafe and point at the alternatives.

### B.4 Non-goals

No change to how the token is stored on disk (`/var/lib/rpi/cloudflare/token`,
already 0640 rpi-agent). No secret redaction in unrelated logs.

---

## C. Doctor half-state checks

Two new checks in `HostSystemProbe::diagnostics()`
(`crates/infrastructure/src/probe.rs`), complementing the existing "ingress
routing" check (which fires when ingress is *disabled* with declared hostnames).
These fire when ingress is *active* but the data plane is broken — the half-state
a rolled-back deploy or a dead connector produces.

### C.1 (a) Connector-alive — cheap, no constructor change

When `ingress_active` is true but no cloudflared process runs
(`runner.run("pgrep", ["-x","cloudflared"])` errs with a non-"spawn " error),
push a failing check:

- name: `"cloudflared connector"`
- detail: `"ingress is configured but no cloudflared process is running"`
- hint: `"start it: sudo -u rpi-agent XDG_RUNTIME_DIR=/run/user/<uid> systemctl --user start cloudflared (or check its logs)"`

If pgrep itself is unavailable (`Err("spawn …")`), skip the check (can't tell —
don't emit a false failure). Reuses the same pgrep signal as Part A, but over
`ProbeRunner` (the probe's own trait) — a few duplicated lines across the two
layers, acceptable and noted.

### C.2 (b) Route-missing — needs the config path threaded in

`HostSystemProbe` gains a `cloudflared_config: Option<String>` field/constructor
param (the path from `config.cloudflared.config`, set in `agent/state.rs` where
`CloudflaredIngress` is built; `None` when ingress is disabled). When
`ingress_active` and the path is `Some` and readable
(`runner.run("cat", [path])`), parse its `ingress:` list (serde_yaml, already a
dep of pi-infrastructure) into the set of routed hostnames. For each registered
project (`self.projects.list()`) whose `hostname` is `Some(h)` and `h` is **not**
in the routed set, push a failing check:

- name: `"ingress route"`
- detail: `"hostname(s) declared with a running ingress but no route in config.yml: <h, …>"`
- hint: `"re-deploy the project(s) to (re)create the route, or check config.yml"`

Read/parse failure → skip silently (the connector check and the existing checks
still cover the host); do not emit a spurious failure on a transient read error.

### C.3 Constructor ripple

`HostSystemProbe::new` gains `cloudflared_config: Option<String>` (placed after
`ingress_active`, before `started_at`). Call sites: `agent/state.rs` (pass
`config.cloudflared.as_ref().map(|c| c.config.clone())` — `Some` only when
`[cloudflared]` is present) and the `http.rs` test call site (`None`). The
`#[allow(clippy::too_many_arguments)]` already on `new` stays.

### C.4 Non-goals

No Cloudflare-API DNS lookup in doctor — checks stay local, fast, and
token-free. The "CNAME points at a tunnel with no route/connector" condition is
caught from the agent side by (a)+(b) without an API call.

---

## D. DBUS in the user-scoped systemctl env

### D.1 Change

`restart_extra_env` (`crates/infrastructure/src/cloudflared.rs`) currently returns
a single `Option<(&'static str, String)>` injecting only `XDG_RUNTIME_DIR` when
the agent's env lacks it. Broaden it to inject **both**
`XDG_RUNTIME_DIR=/run/user/<uid>` and
`DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/<uid>/bus` together, gated on the
same condition (env lacks a **non-empty** `XDG_RUNTIME_DIR` — the existing
empty-string-counts-as-absent behavior is preserved). Return type becomes a
`Vec<(&'static str, String)>` (empty when the env is already set up);
`apply_restart_env` iterates it. uid stays runtime (`current_uid()` =
`libc::getuid()`), never hardcoded.

Also add `DBUS_SESSION_BUS_ADDRESS` to the `enable --now` invocation in
`cloudflared_user_service` (`setup.rs`), which already sets `XDG_RUNTIME_DIR` via
`id -u rpi-agent`.

### D.2 Rationale / non-goal

`XDG_RUNTIME_DIR` alone is normally sufficient (`systemctl --user` finds the bus
at `$XDG_RUNTIME_DIR/bus`); `DBUS_SESSION_BUS_ADDRESS` is cheap insurance for
non-standard setups. Not unifying the two execution contexts (in-process restart
vs `sudo -u rpi-agent … enable`) — they differ legitimately; both just gain the
DBUS var.

---

## Testing (all FakeSys / FakeRunner / mockall)

**A.** (1) fresh + running → refuse: `config.yml` absent, `pgrep` `Ok` → refuse
msg in `rep.errors`, zero writes, no tunnel-API call (`MockCloudflareApi` no
expectations), `cloudflared_user_service` unreached. (2) fresh + not running →
unchanged fresh install (existing tests pass). (3) config.yml exists + running →
still adopts (guard never fires). (4) undetermined (`Err("spawn …")`) → refuse,
zero writes. (5) dry-run + running → `would refuse` in `rep.skipped`, zero writes.
(6) `cloudflared_running` unit test: three `Sys::run` outcomes → three states.

**B.** (1) `--cf-token-file <path>` read + trimmed; (2) `--cf-token-file -` reads
stdin; (3) unreadable path → error; (4) `--cf-token` used → resolves + emits
deprecation warning; (5) file + inline both → file wins, warning still emitted;
(6) neither + env set → env used.

**C.** (a) ingress_active + pgrep err(non-spawn) → `"cloudflared connector"`
fails; ingress_active + pgrep Ok → no such failure; pgrep unavailable → check
skipped. (b) ingress_active + config with route for H → project H passes; project
H2 with no route → `"ingress route"` fails listing H2; unreadable/None config →
check skipped. Constructor call-site updates keep existing probe tests green.

**D.** `restart_extra_env(None, uid)` → both vars present; `Some("/run/user/x")`
→ empty; `Some("")` → both vars.

## Files touched

- `crates/bin/src/agent/setup.rs` — A (guard + `cloudflared_running`), B
  (token resolution), D (enable-path DBUS).
- `crates/bin/src/main.rs` — B (`--cf-token-file` arg + help).
- `crates/infrastructure/src/cloudflared.rs` — D (`restart_extra_env` →
  Vec + DBUS).
- `crates/infrastructure/src/probe.rs` — C (both checks + constructor param).
- `crates/bin/src/agent/state.rs`, `crates/bin/src/agent/http.rs` — C
  (pass `cloudflared_config` / `None` at the two call sites).
