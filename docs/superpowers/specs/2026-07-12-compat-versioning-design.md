# Unified CLI ↔ agent compatibility and feature versioning

Date: 2026-07-12
Status: approved

## Context

Compatibility between the CLI and the agent is currently handled in scattered,
ad-hoc ways:

- `version_mismatch_warning` (`cli/commands.rs`) compares binary versions, but
  only `rpi deploy` calls it.
- Bare-404 sniffing ("404 with no JSON `{"error"}` body means old agent")
  is duplicated per endpoint: `commands_not_found`, the secrets 404 helper,
  and `source_check` returning `Ok(None)` — each with its own policy and its
  own error text.
- Endpoints added after an agent was deployed (e.g. `/v1/stats` against an
  older agent) surface as a raw `404 Not Found` with no guidance.

Goal: one encapsulated abstraction that knows which features the other side
supports, applies a declared per-feature policy when one is missing, and owns
all user-facing version/compatibility notices — so call sites never
hand-roll version checks again.

## Scope decisions (settled)

- **CLI ↔ agent only.** `rpi.toml` schema versioning stays a separate
  mechanism (different lifecycle: a file in the user's repo, not a pair of
  binaries).
- **Semantic features, not endpoints.** Units of compatibility are named
  capabilities ("secrets", "commands", "source-check", "stats"); one feature
  may cover several routes.
- **Policy lives in the registry, not at call sites.** Each feature declares
  how its absence is handled: `Required` (error + update hint), `Degradable`
  (one-shot warning banner + fallback path), `Silent` (skip quietly).
- **CLI does the checking.** The agent stays passive and backward-compatible;
  it only advertises. When the agent is *newer* than the CLI, the CLI shows an
  "update the CLI" banner based on version comparison.
- **Capability handshake is the source of truth** (agent self-describes), with
  a frozen version matrix as fallback for agents that predate the handshake
  field. A centralized bare-404 interpretation remains only as a race safety
  net.

## Feature versioning model

Feature versions are part of the capability name — the advertised set stays
flat:

```json
{ "version": "0.22.0", "api": "v1",
  "features": ["secrets", "secrets/2", "commands", "source-check"] }
```

Conventions:

- `name` with no suffix means generation 1 (`secrets` ≡ `secrets/1`).
- A feature version bumps **only on a breaking change**. Additive changes
  (new optional fields) do not bump anything — serde tolerance covers them.
- While the agent can serve both generations it advertises both strings;
  dropping support for the old behaviour means dropping the old string.
- Version ranges therefore come free as set membership: "supports secrets
  v1–v2" is `{"secrets", "secrets/2"}`. No semver parsing, no range matching.
- The CLI picks the best mutually supported generation via
  `session.pick(&[Feature::SecretsV2, Feature::Secrets])`.

Rejected alternatives: a `name → max version` map (breaks as soon as an agent
drops v1 — forces min/max range machinery) and LSP-style structured
capabilities (overkill for a single-consumer CLI; revisit per-feature if a
capability ever needs parameters).

## Architecture

The CLI and the agent are one binary (`crates/bin` has both `cli/` and
`agent/`), so a single shared module is the source of truth for both sides —
"agent advertises X, CLI checks Y" drift is impossible by construction.

### New module `crates/bin/src/compat.rs`

**`Feature`** — enum of all semantic features. Each variant knows everything
about itself:

```rust
pub enum Feature {
    Secrets,       // "secrets"
    Commands,      // "commands"
    SourceCheck,   // "source-check"
    Stats,         // "stats"
    // future: SecretsV2 → "secrets/2"
}

impl Feature {
    fn capability(&self) -> &'static str;  // handshake string
    fn label(&self) -> &'static str;       // human name for messages
    fn policy(&self) -> Policy;
    fn since(&self) -> &'static str;       // min agent version, for hints
    pub fn advertised() -> &'static [Feature]; // what THIS binary serves as agent
}
```

**`Policy`** — `Required` | `Degradable` | `Silent`.

**`CompatSession`** — CLI-side, one per command run:

```rust
pub struct CompatSession {
    agent_version: String,
    capabilities: BTreeSet<String>,
    warned: RefCell<HashSet<&'static str>>, // banner dedup per run
}
```

API: `supports(Feature) -> bool`, `require(Feature) -> Result<()>`,
`gate(Feature) -> Result<bool>` (applies the declared policy),
`pick(&[Feature]) -> Option<Feature>`.

### Agent side

The `/v1/version` handler adds `features: Feature::advertised()` to its
response. The list is generated from the enum — nothing is enumerated by
hand. `VersionInfo` in `proto.rs` gains
`#[serde(default)] pub features: Vec<String>` so responses from old agents
(no field) still parse.

### CLI side

Commands currently do `SshTunnel::open` → `ApiClient::new` individually. A
shared connect helper returns `(ApiClient, CompatSession)`; the handshake
(`GET /v1/version`) happens **always and exactly once** per command. Side
effects of the handshake are the version-skew banners:

- agent older than CLI → the existing warning (moves out of
  `commands.rs:775` into `compat.rs`);
- agent newer than CLI → new banner: update the CLI.

### Legacy fallback

If the handshake response has no `features` field, the capability set is
reconstructed from a **frozen** table mapping already-shipped features to the
agent version that introduced them, compared against the reported `version`.
The table is pinned once during implementation — each `since` value is the
first git tag whose tree contains the feature's route (`git tag --contains`
on the introducing commit) — and never grows: every future feature is
advertised via the handshake only, so the table dies out as old agents
disappear.

## Initial feature registry

| Feature | Capability | Policy | Rationale |
|---|---|---|---|
| `Secrets` | `secrets` | Required | Fails today with an ad-hoc "update the agent" 404 message; keeps failing, but with the unified text incl. required version |
| `Commands` | `commands` | Required | Same |
| `SourceCheck` | `source-check` | Silent | Spec 2026-07-10 deliberately degrades silently so first deploys are never blocked; flipping to Degradable later is a one-line registry change |
| `Stats` | `stats` | Required | Today an old agent yields a raw `404 Not Found`; becomes an actionable hint |

`since` values are pinned from git history during implementation (see Legacy
fallback).

## Error handling and races

- **Agent replaced between handshake and call** (upgraded/rolled back
  mid-command): the bare-404 interpretation stays as a safety net, but
  centralized — one `ApiClient` helper `expect_feature(resp, Feature)` turns
  a bodyless 404 into the *same* error `require()` would have produced. The
  ad-hoc helpers (`commands_not_found`, the secrets 404 helper, the
  `source_check` special case) are replaced by it. A 404 *with* a JSON
  `{"error"}` body remains a domain not-found and passes through unchanged.
- **Hint direction** follows the version comparison: agent older than CLI and
  feature missing → "update the agent on the Pi (requires ≥ X)"; agent newer
  and feature missing (old generation dropped) → "this CLI is too old for
  agent vX — update the CLI".
- **Broken handshake**: a bare 404 on `/v1/version` keeps the existing
  "agent does not expose /v1 — incompatible agent" error; transport errors
  propagate as today.
- **Banner dedup**: every banner (version skew, degradable warnings) prints
  at most once per command run, tracked in `CompatSession::warned`.

## What gets deleted

`version_mismatch_warning`, `commands_not_found`, the secrets 404 helper, and
`source_check`'s inline 404 handling all dissolve into `compat.rs` +
`expect_feature`. Call sites shrink to `session.require(...)` /
`session.supports(...)` / `session.pick(...)`.

## Testing

1. **Unit (`compat.rs`)**: legacy-matrix reconstruction by version; `pick()`
   returns the best available generation; the three policies produce
   error/banner/silence respectively; banner dedup; hint direction (old agent
   vs new agent).
2. **Router tests** (axum mocks, like the existing ones in `api.rs`):
   handshake with a `features` field, without one (legacy path), and the
   bare-404 safety net yielding the unified error.
3. **Drift guard**: a test asserts the agent's `/v1/version` response equals
   `Feature::advertised()` — shipping a feature without registering it fails
   CI.
4. **Existing tests** for `version_mismatch_warning`, `commands_not_found`,
   etc. migrate to the new module without losing scenario coverage.

### Cross-version e2e

The e2e image is built once from the current tree and `.dockerignore`
excludes `.git`, so the old version is prepared outside the docker build:

- **Legacy binary**: `run.mjs` runs `git archive <LEGACY_TAG>` into a
  generated tarball inside the build context (e.g. `tests/e2e/.legacy-src.tar`,
  gitignored). A new `legacy-builder` Dockerfile stage unpacks and builds it
  into `/usr/local/bin/rpi-legacy` next to the current `rpi`. `git archive`
  of a tag is deterministic, so the layer caches; the double Rust build is
  paid once per cold cache. (Downloading GitHub Release artifacts was
  rejected: external dependency, CI flake surface, and not all tags have
  artifacts.)
- **Agent selection**: `target-entrypoint.sh` gains a per-scenario override
  modeled on the existing per-scenario `agent.toml` — a file `agent-bin` in
  the scenario folder naming the binary path; default stays
  `/usr/local/bin/rpi`. The `client` CLI is always the current build.
- **Scenario `compat-legacy-agent`** (legacy agent + current CLI) asserts:
  1. deploy succeeds — legacy fallback reconstructed capabilities,
     source-check silently skipped (Silent policy exercised);
  2. the version-skew banner appears in deploy output;
  3. a command whose feature the legacy tag lacks exits non-zero with the
     unified "requires agent ≥ X — update the agent" message.
- **Tag choice** is pinned during implementation: the newest tag that (a)
  runs under the harness (parses `agent.default.toml`, serves the
  unix-socket healthcheck) and (b) lacks at least one Required feature
  (candidates: v0.13.0–v0.14.0, pre commands/secrets, if v0.19.1 already has
  everything current).
- **Not covered by e2e**: the "agent newer than CLI" direction (nothing newer
  than the current build exists at test time) — covered by router tests with
  mocked `/v1/version` responses advertising unknown capabilities and a
  higher version.

## Out of scope

- `rpi.toml` schema versioning (separate mechanism, unchanged).
- npm wrapper / landing / install-script versioning.
- Full symmetry (agent adapting behaviour to the CLI's version per request).
- Structured, parameterized capabilities — flat strings until a real need
  appears.
