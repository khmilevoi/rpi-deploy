# Per-command service override Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let a `[commands]` entry in `rpi.toml` declare which compose service it runs in, defaulting to `ingress.service`, so `rpi command <name>` can exec into a non-ingress container (e.g. `server`) instead of always the ingress one.

**Architecture:** Introduce a `CommandSpec { argv, service: Option<String> }` domain type with hand-written, backward-compatible serde (a bare argv array when `service` is `None`; a struct only when `Some`). `ProjectConfig.commands` moves from `BTreeMap<String, Vec<String>>` to `BTreeMap<String, CommandSpec>`. `RunCommand::execute` resolves the exec target as `spec.service.as_deref().unwrap_or(&config.service)`. A new TOML table form (`[commands.<name>]` with `run` + `service`) populates the field.

**Tech Stack:** Rust (workspace of `pi-domain`, `pi-application`, `pi-infrastructure`, `pi` bin), serde / serde_json, rusqlite, tokio, mockall.

## Global Constraints

- `rpi.toml` schema stays `schema = 1`; the table form is additive.
- No SQLite migration: the existing `commands TEXT` column keeps storing `serde_json::to_string(&config.commands)`; backward compatibility comes from `CommandSpec`'s serde, not a schema change.
- Wire/JSON for a service-less command MUST stay byte-identical to today (bare argv array), so mismatched CLI/agent versions keep working for existing commands.
- No upfront validation that `service` names a real compose service (consistent with the un-validated `ingress.service`); a bad name surfaces as docker's exec error.
- Follow existing patterns; prefix shell commands with `rtk` per CLAUDE.md. End commit messages with the `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>` trailer.
- Run `cargo test -p pi` for focused work and `cargo test --workspace` before finishing a task that spans crates.

---

### Task 1: `CommandSpec` domain type with backward-compatible serde

Introduce the type in the domain crate, standalone (no field changes yet), with serde that round-trips both the legacy bare-array shape and the new struct shape.

**Files:**
- Modify: `crates/domain/Cargo.toml` (add serde dependency)
- Modify: `crates/domain/src/entities.rs` (add `CommandSpec`, its serde, constructors; new test module)

**Interfaces:**
- Produces:
  - `pub struct CommandSpec { pub argv: Vec<String>, pub service: Option<String> }` (derives `Debug, Clone, PartialEq, Eq`)
  - `impl CommandSpec { pub fn new(argv: Vec<String>) -> CommandSpec }` (service = `None`)
  - `impl From<Vec<String>> for CommandSpec` (service = `None`)
  - `Serialize`/`Deserialize` for `CommandSpec`: serializes to a bare array when `service` is `None`, to `{"argv":[...],"service":"..."}` when `Some`; deserializes from either shape.

- [ ] **Step 1: Add serde to the domain crate**

Edit `crates/domain/Cargo.toml`, adding under `[dependencies]` (after the `mockall` line):

```toml
serde = { workspace = true }
```

- [ ] **Step 2: Write the failing serde tests**

Append to `crates/domain/src/entities.rs` (end of file):

```rust
#[cfg(test)]
mod command_spec_tests {
    use super::CommandSpec;

    #[test]
    fn legacy_array_json_deserializes_to_no_service() {
        let spec: CommandSpec = serde_json::from_str(r#"["node","seed.js"]"#).unwrap();
        assert_eq!(spec, CommandSpec::new(vec!["node".into(), "seed.js".into()]));
        assert_eq!(spec.service, None);
    }

    #[test]
    fn no_service_serializes_back_to_bare_array() {
        let spec = CommandSpec::new(vec!["node".into(), "seed.js".into()]);
        let json = serde_json::to_string(&spec).unwrap();
        assert_eq!(json, r#"["node","seed.js"]"#);
    }

    #[test]
    fn service_pinned_uses_struct_shape_and_roundtrips() {
        let spec = CommandSpec {
            argv: vec!["node".into(), "create-invite.cjs".into()],
            service: Some("server".into()),
        };
        let json = serde_json::to_string(&spec).unwrap();
        assert_eq!(
            json,
            r#"{"argv":["node","create-invite.cjs"],"service":"server"}"#
        );
        let back: CommandSpec = serde_json::from_str(&json).unwrap();
        assert_eq!(back, spec);
    }

    #[test]
    fn struct_shape_without_service_key_deserializes_to_none() {
        let spec: CommandSpec =
            serde_json::from_str(r#"{"argv":["node","x.js"]}"#).unwrap();
        assert_eq!(spec.service, None);
        assert_eq!(spec.argv, vec!["node".to_string(), "x.js".into()]);
    }

    #[test]
    fn from_vec_has_no_service() {
        let spec: CommandSpec = vec!["a".to_string()].into();
        assert_eq!(spec.service, None);
    }
}
```

This test module uses `serde_json`, which is dev-available; add it as a dev-dependency if the crate lacks one (Step 3 note).

- [ ] **Step 3: Run the tests to verify they fail to compile**

Run: `rtk cargo test -p pi-domain command_spec`
Expected: compile error — `CommandSpec` not found (and possibly `serde_json` unresolved).

If `serde_json` is unresolved, add to `crates/domain/Cargo.toml`:

```toml
[dev-dependencies]
serde_json = { workspace = true }
```

- [ ] **Step 4: Implement `CommandSpec` and its serde**

Add near the `ProjectConfig` definition in `crates/domain/src/entities.rs` (before `ProjectConfig`), and add the serde imports at the top of the file.

At the top of `crates/domain/src/entities.rs` add:

```rust
use serde::de::{Deserializer, Error as _};
use serde::ser::Serializer;
use serde::{Deserialize, Serialize};
```

Then add the type:

```rust
/// One deployed `[commands]` entry: the declared argv plus an optional compose
/// service to exec into. `service = None` means the project's ingress service
/// (ProjectConfig.service).
///
/// Serde is hand-written for backward compatibility: a service-less command
/// serializes to a bare argv array — byte-identical to the pre-service format
/// stored in the registry and sent over the CLI<->agent protocol — while a
/// service-pinned command uses a struct. Deserialization accepts both shapes,
/// so existing rows and older-CLI payloads decode with `service = None`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandSpec {
    pub argv: Vec<String>,
    pub service: Option<String>,
}

impl CommandSpec {
    pub fn new(argv: Vec<String>) -> CommandSpec {
        CommandSpec {
            argv,
            service: None,
        }
    }
}

impl From<Vec<String>> for CommandSpec {
    fn from(argv: Vec<String>) -> CommandSpec {
        CommandSpec::new(argv)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum CommandSpecRepr {
    Argv(Vec<String>),
    Full {
        argv: Vec<String>,
        #[serde(default)]
        service: Option<String>,
    },
}

impl Serialize for CommandSpec {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match &self.service {
            None => self.argv.serialize(serializer),
            Some(service) => CommandSpecRepr::Full {
                argv: self.argv.clone(),
                service: Some(service.clone()),
            }
            .serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for CommandSpec {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<CommandSpec, D::Error> {
        match CommandSpecRepr::deserialize(deserializer)? {
            CommandSpecRepr::Argv(argv) => Ok(CommandSpec {
                argv,
                service: None,
            }),
            CommandSpecRepr::Full { argv, service } => {
                if argv.is_empty() {
                    return Err(D::Error::custom("command argv must not be empty"));
                }
                Ok(CommandSpec { argv, service })
            }
        }
    }
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `rtk cargo test -p pi-domain command_spec`
Expected: PASS (5 tests).

- [ ] **Step 6: Commit**

```bash
rtk git add crates/domain/Cargo.toml crates/domain/src/entities.rs
rtk git commit -m "feat(domain): add CommandSpec with backward-compatible serde"
```

---

### Task 2: Migrate `ProjectConfig.commands` to `BTreeMap<String, CommandSpec>`

Flip the field type and update every call site to compile and preserve current behavior (all commands resolve to the ingress service because parsing still yields `service = None`). This is one atomic change: Rust will not compile until all sites are updated. Also wire in the service-resolution line in `execute` (inert until Task 3 produces `Some`) and prove it with a test.

**Files:**
- Modify: `crates/domain/src/entities.rs:165` (field type)
- Modify: `crates/application/src/command.rs` (lookup/execute/list + tests)
- Modify: `crates/infrastructure/src/repo.rs` (test constructors)
- Modify: `crates/bin/src/proto.rs` (DTO field types + tests)
- Modify: `crates/bin/src/cli/commands.rs:367` (list display uses `spec.argv`)
- Modify: `crates/bin/src/agent/http.rs:117` (deploy validation uses `spec.argv`)

**Interfaces:**
- Consumes: `CommandSpec` from Task 1.
- Produces:
  - `ProjectConfig.commands: BTreeMap<String, CommandSpec>`
  - `RunCommand::execute` execs into `spec.service.as_deref().unwrap_or(&registered.config.service)`
  - `RunCommand::list(&self, project) -> Result<BTreeMap<String, CommandSpec>, DomainError>`
  - `ProjectDto.commands` and `CommandsResponse.commands` typed `BTreeMap<String, CommandSpec>`

- [ ] **Step 1: Add the service-pinning execute test (failing)**

In `crates/application/src/command.rs`, inside `mod tests`, add after `executes_declared_argv_plus_extra_args_in_service`:

```rust
#[tokio::test]
async fn execs_into_pinned_service_when_set() {
    let mut runtime = MockContainerRuntime::new();
    runtime
        .expect_exec()
        .withf(|_, service, _, _| service == "server")
        .returning(|_, _, _, _| Ok(0));

    let mut proj = project("rateme");
    proj.config.commands.insert(
        "create-invite".into(),
        CommandSpec {
            argv: vec!["node".into(), "create-invite.cjs".into()],
            service: Some("server".into()),
        },
    );
    let run = deps_with(runtime, proj);
    let code = run
        .execute("rateme", "create-invite", &[], CollectSink::new())
        .await
        .unwrap();
    assert_eq!(code, 0);
}
```

Also update the existing `project(name)` helper in that test module: change

```rust
config.commands.insert(
    "create-invite".into(),
    vec!["node".into(), "scripts/create-invite.js".into()],
);
```

to

```rust
config.commands.insert(
    "create-invite".into(),
    CommandSpec::new(vec!["node".into(), "scripts/create-invite.js".into()]),
);
```

and add `use pi_domain::entities::CommandSpec;` to the test module's `use` block (next to `use pi_domain::entities::ProjectConfig;`).

- [ ] **Step 2: Run to verify it fails to compile**

Run: `rtk cargo test -p pi 2>&1 | head -40`
Expected: compile errors across the workspace (field type mismatch) — this is the change we make next.

- [ ] **Step 3: Change the field type**

In `crates/domain/src/entities.rs`, change the `commands` field of `ProjectConfig`:

```rust
    /// Declared admin commands ([commands] in rpi.toml), name -> spec.
    /// Persisted in the registry: `rpi command` must run what was deployed,
    /// not what the local file currently says.
    pub commands: BTreeMap<String, CommandSpec>,
```

- [ ] **Step 4: Update `RunCommand` (lookup / execute / list)**

In `crates/application/src/command.rs`:

Change `list` to return specs:

```rust
    /// Deployed commands of a project — `rpi command` list mode.
    pub async fn list(
        &self,
        project: &str,
    ) -> Result<BTreeMap<String, pi_domain::entities::CommandSpec>, DomainError> {
        Ok(self.registered(project).await?.config.commands)
    }
```

Change `execute` to resolve the service and pull argv from the spec:

```rust
    pub async fn execute(
        &self,
        project: &str,
        command: &str,
        extra_args: &[String],
        log: Arc<dyn LogSink>,
    ) -> Result<i32, DomainError> {
        let (registered, spec) = self.lookup(project, command).await?;
        let mut argv = spec.argv.clone();
        argv.extend(extra_args.iter().cloned());
        let service = spec
            .service
            .as_deref()
            .unwrap_or(&registered.config.service)
            .to_string();
        let workdir = self.source.workdir(project);
        let compose_file = workdir.join(&registered.config.compose_path);
        let override_file = self.overrides.path(project);
        let stack = ComposeStack {
            project_name: registered.config.name.clone(),
            workdir,
            compose_file,
            override_file,
        };
        let secs = registered
            .config
            .command_timeout_secs
            .unwrap_or(DEFAULT_COMMAND_TIMEOUT_SECS);
        tokio::time::timeout(
            Duration::from_secs(secs),
            self.runtime.exec(&stack, &service, &argv, log),
        )
        .await
        .map_err(|_| DomainError::Timeout {
            stage: "command".into(),
            secs,
        })?
    }
```

Change `lookup` to return the spec instead of a cloned argv:

```rust
    async fn lookup(
        &self,
        project: &str,
        command: &str,
    ) -> Result<(Project, pi_domain::entities::CommandSpec), DomainError> {
        let registered = self.registered(project).await?;
        match registered.config.commands.get(command) {
            Some(spec) => {
                let spec = spec.clone();
                Ok((registered, spec))
            }
            None => {
                let available: Vec<&str> = registered
                    .config
                    .commands
                    .keys()
                    .map(String::as_str)
                    .collect();
                Err(DomainError::NotFound(if available.is_empty() {
                    format!(
                        "command '{command}' (project has no deployed [commands]; declare it in rpi.toml and run `rpi deploy`)"
                    )
                } else {
                    format!("command '{command}' (available: {})", available.join(", "))
                }))
            }
        }
    }
```

Add `CommandSpec` to the file-level import so signatures can drop the `pi_domain::entities::` prefix if preferred: change the top `use pi_domain::entities::{ComposeStack, Project};` to `use pi_domain::entities::{CommandSpec, ComposeStack, Project};` and use `CommandSpec` in the signatures above.

- [ ] **Step 5: Update proto DTOs**

In `crates/bin/src/proto.rs`, add `CommandSpec` to the domain import (line 5-8 block):

```rust
use pi_domain::entities::{
    AgentOverview, CommandSpec, Deployment, DiagnosticCheck, DiagnosticReport, ExposeMode,
    HealthcheckConfig, HostStats, ProjectConfig, ProjectStats, ServiceStats, StageTimeoutOverrides,
    StatsReport,
};
```

Change `ProjectDto.commands` (line 48):

```rust
    #[serde(default)]
    pub commands: BTreeMap<String, CommandSpec>,
```

Change `CommandsResponse.commands` (line 306):

```rust
pub struct CommandsResponse {
    pub commands: BTreeMap<String, CommandSpec>,
}
```

The `From` bodies at lines 83 (`commands: dto.commands`) and 111 (`commands: config.commands.clone()`) already type-check unchanged.

Update the proto test at line 555-557:

```rust
        config
            .commands
            .insert("migrate".into(), CommandSpec::new(vec!["npx".into(), "prisma".into()]));
```

Add a proto test proving service survives the DTO round-trip (append inside `mod tests`):

```rust
    #[test]
    fn service_pinned_command_survives_dto_roundtrip() {
        let mut config = pi_domain::entities::ProjectConfig {
            name: "rateme".into(),
            repo: "r".into(),
            branch: "main".into(),
            compose_path: "docker-compose.yml".into(),
            service: "web".into(),
            container_port: 3000,
            hostname: None,
            expose: Default::default(),
            healthcheck: Default::default(),
            timeouts: Default::default(),
            commands: Default::default(),
            command_timeout_secs: None,
        };
        config.commands.insert(
            "create-invite".into(),
            CommandSpec {
                argv: vec!["node".into(), "x.cjs".into()],
                service: Some("server".into()),
            },
        );
        let dto: ProjectDto = (&config).into();
        let json = serde_json::to_value(&dto).unwrap();
        let back: ProjectDto = serde_json::from_value(json).unwrap();
        let roundtripped: pi_domain::entities::ProjectConfig = back.into();
        assert_eq!(
            roundtripped.commands.get("create-invite").unwrap().service.as_deref(),
            Some("server")
        );
    }
```

- [ ] **Step 6: Update the CLI list display**

In `crates/bin/src/cli/commands.rs`, replace the loop at line 367-369:

```rust
            for (cmd, spec) in &resp.commands {
                println!("{cmd}  ->  {}", spec.argv.join(" "));
            }
```

(Service display is added in Task 4; keep parity with today's output here.)

- [ ] **Step 7: Update the agent-side deploy validation**

In `crates/bin/src/agent/http.rs`, replace the loop starting at line 117:

```rust
    for (cmd_name, spec) in &config.commands {
        if !is_valid_name(cmd_name) {
            return Err(ApiError(DomainError::Invalid(format!(
                "command name '{cmd_name}' must match ^[a-z0-9][a-z0-9_-]*$"
            ))));
        }
        if spec.argv.is_empty() || spec.argv.iter().any(|a| a.is_empty()) {
            return Err(ApiError(DomainError::Invalid(format!(
```

Leave the closing of that `if` block and the rest of the loop body unchanged (only the `for` binding and the two `argv` references change from `argv` to `spec.argv`).

- [ ] **Step 8: Fix infrastructure repo test constructors**

In `crates/infrastructure/src/repo.rs`, the `persists_commands_and_command_timeout` test inserts argv vecs. Update both inserts:

```rust
        config
            .commands
            .insert("migrate".into(), pi_domain::entities::CommandSpec::new(vec!["npx".into(), "prisma".into()]));
```

and

```rust
        config
            .commands
            .insert("seed".into(), pi_domain::entities::CommandSpec::new(vec!["node".into(), "seed.js".into()]));
```

Add a legacy-row read test right after that test (proves an old array-shaped JSON row still loads):

```rust
    #[tokio::test]
    async fn loads_legacy_array_shaped_commands_row() {
        let dir = tempfile::tempdir().unwrap();
        let repo = repo(&dir, 8000, 8999);
        let mut config = cfg("legacy");
        config
            .commands
            .insert("migrate".into(), pi_domain::entities::CommandSpec::new(vec!["npx".into()]));
        repo.upsert(&config).await.unwrap();
        let loaded = repo.get("legacy").await.unwrap().unwrap();
        assert_eq!(
            loaded.config.commands.get("migrate").unwrap().service,
            None,
            "service-less command decodes with no service"
        );
    }
```

- [ ] **Step 9: Run the full test suite**

Run: `rtk cargo test --workspace`
Expected: PASS. In particular `execs_into_pinned_service_when_set`, `service_pinned_command_survives_dto_roundtrip`, `loads_legacy_array_shaped_commands_row`, and all pre-existing command/proto/repo tests are green.

- [ ] **Step 10: Commit**

```bash
rtk git add -A
rtk git commit -m "feat: carry per-command service through config, protocol, and exec"
```

---

### Task 3: Parse the `[commands]` table form in `rpi.toml`

Add the table form (`run` + optional `service`) to the CLI parser and map it into `CommandSpec`. Validate `service` is non-empty. Add matching defense-in-depth validation on the agent.

**Files:**
- Modify: `crates/bin/src/cli/rpitoml.rs` (`CommandValue`, `CommandRun`, `command_argv` → `command_spec`, parse validation, `to_project_config`, tests)
- Modify: `crates/bin/src/agent/http.rs` (reject empty `service`)

**Interfaces:**
- Consumes: `CommandSpec` from Task 1; `ProjectConfig.commands` from Task 2.
- Produces: `rpi.toml` `[commands.<name>]` table with `run` (string or array) and optional `service`; `to_project_config` yields `CommandSpec { argv, service }`.

- [ ] **Step 1: Write failing parser tests**

In `crates/bin/src/cli/rpitoml.rs` `mod tests`, add:

```rust
    #[test]
    fn commands_table_form_pins_service() {
        let toml = SAMPLE.replace(
            "[healthcheck]",
            "[commands.create-invite]\nrun = \"node create-invite.cjs\"\nservice = \"server\"\n\n[commands.seed]\nrun = [\"node\", \"seed.js\"]\n\n[healthcheck]",
        );
        let config = RpiToml::parse(&toml).unwrap().to_project_config();
        let invite = config.commands.get("create-invite").unwrap();
        assert_eq!(invite.argv, vec!["node".to_string(), "create-invite.cjs".into()]);
        assert_eq!(invite.service.as_deref(), Some("server"));
        let seed = config.commands.get("seed").unwrap();
        assert_eq!(seed.argv, vec!["node".to_string(), "seed.js".into()]);
        assert_eq!(seed.service, None, "table form without service => None");
    }

    #[test]
    fn shorthand_forms_have_no_service() {
        let toml = SAMPLE.replace(
            "[healthcheck]",
            "[commands]\nmigrate = \"npx prisma migrate deploy\"\n\n[healthcheck]",
        );
        let config = RpiToml::parse(&toml).unwrap().to_project_config();
        assert_eq!(config.commands.get("migrate").unwrap().service, None);
    }

    #[test]
    fn empty_service_string_is_rejected() {
        let toml = SAMPLE.replace(
            "[healthcheck]",
            "[commands.x]\nrun = \"node x.js\"\nservice = \"\"\n\n[healthcheck]",
        );
        let err = RpiToml::parse(&toml).unwrap_err().to_string();
        assert!(err.contains("service"), "got: {err}");
    }
```

Also update the existing `commands_section_parses_string_and_array_forms` test: its three assertions compare `config.commands.get(..).unwrap()` to `&vec![...]`. Change each to compare `.argv`, e.g.:

```rust
        assert_eq!(
            config.commands.get("create-invite").unwrap().argv,
            vec!["node".to_string(), "scripts/create-invite.js".into(), "--admin".into()]
        );
```

(apply the same `.argv` change to the `migrate` and `backup` assertions).

- [ ] **Step 2: Run to verify failure**

Run: `rtk cargo test -p pi commands_table_form_pins_service commands_section_parses empty_service`
Expected: compile error / FAIL — `run`/`service` table not accepted, `.argv` field access on `Vec`.

- [ ] **Step 3: Extend `CommandValue` and add `CommandRun`**

In `crates/bin/src/cli/rpitoml.rs`, replace the `CommandValue` enum (lines ~109-115):

```rust
/// [commands] value: a shell-word string, an explicit argv array, or a table
/// with `run` (string/array) plus an optional `service` (§spec).
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum CommandValue {
    Line(String),
    Argv(Vec<String>),
    Table {
        run: CommandRun,
        #[serde(default)]
        service: Option<String>,
    },
}

/// The `run` of a table-form command: same two shapes as the shorthand value.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum CommandRun {
    Line(String),
    Argv(Vec<String>),
}
```

- [ ] **Step 4: Resolve a `CommandValue` to `(argv, service)`**

Replace `command_argv` (lines ~123-136) with `command_spec`. Import `CommandSpec` at the top of the file: change `use pi_domain::entities::{ExposeMode, HealthcheckConfig, ProjectConfig, StageTimeoutOverrides};` to add `CommandSpec`.

```rust
fn argv_from_run(name: &str, line_or_argv: RunShape) -> anyhow::Result<Vec<String>> {
    let argv = match line_or_argv {
        RunShape::Argv(items) => items,
        RunShape::Line(line) => shlex::split(&line)
            .ok_or_else(|| anyhow::anyhow!("rpi.toml [commands].{name}: unbalanced quote"))?,
    };
    if argv.is_empty() || argv.iter().any(|a| a.is_empty()) {
        anyhow::bail!("rpi.toml [commands].{name}: command must not be empty");
    }
    Ok(argv)
}

/// Internal shape passed to `argv_from_run` from either the shorthand value or a
/// table's `run`.
enum RunShape {
    Line(String),
    Argv(Vec<String>),
}

/// String form is split client-side with shell-word rules (quotes only —
/// no variables, pipes or redirects; a shell must be spelled out as
/// `sh -c '...'`). Array form is taken verbatim.
fn command_spec(name: &str, value: &CommandValue) -> anyhow::Result<CommandSpec> {
    let (run, service) = match value {
        CommandValue::Line(line) => (RunShape::Line(line.clone()), None),
        CommandValue::Argv(items) => (RunShape::Argv(items.clone()), None),
        CommandValue::Table { run, service } => {
            let run = match run {
                CommandRun::Line(line) => RunShape::Line(line.clone()),
                CommandRun::Argv(items) => RunShape::Argv(items.clone()),
            };
            (run, service.clone())
        }
    };
    if let Some(service) = &service {
        if service.is_empty() {
            anyhow::bail!("rpi.toml [commands].{name}: service must not be empty");
        }
    }
    let argv = argv_from_run(name, run)?;
    Ok(CommandSpec { argv, service })
}
```

- [ ] **Step 5: Update `parse` validation and `to_project_config`**

In `RpiToml::parse`, the loop that calls `command_argv(name, value)?;` (line ~213) becomes:

```rust
                command_spec(name, value)?;
```

In `to_project_config`, replace the `commands` mapping (lines ~271-283):

```rust
            commands: self
                .commands
                .as_ref()
                .map(|map| {
                    map.iter()
                        .map(|(name, value)| {
                            let spec =
                                command_spec(name, value).expect("validated in RpiToml::parse");
                            (name.clone(), spec)
                        })
                        .collect()
                })
                .unwrap_or_default(),
```

- [ ] **Step 6: Reject empty `service` on the agent too**

In `crates/bin/src/agent/http.rs`, inside the `for (cmd_name, spec) in &config.commands` loop (from Task 2), after the argv-empty check block, add:

```rust
        if spec.service.as_deref().is_some_and(str::is_empty) {
            return Err(ApiError(DomainError::Invalid(format!(
                "command '{cmd_name}' service must not be empty"
            ))));
        }
```

- [ ] **Step 7: Run the tests**

Run: `rtk cargo test -p pi`
Expected: PASS, including the three new parser tests and the updated `commands_section_parses_string_and_array_forms`.

- [ ] **Step 8: Commit**

```bash
rtk git add -A
rtk git commit -m "feat(rpi.toml): [commands] table form with per-command service"
```

---

### Task 4: Surface the target service in `rpi command` list output

Show which service each deployed command runs in when listing (`rpi command` with no name). Extract a pure formatter so it is unit-testable.

**Files:**
- Modify: `crates/bin/src/cli/commands.rs` (list loop + `format_command_line` helper + test)

**Interfaces:**
- Consumes: `CommandsResponse.commands: BTreeMap<String, CommandSpec>` (Task 2), the deployed project's ingress `service`.
- Produces: `fn format_command_line(name: &str, spec: &CommandSpec) -> String`.

- [ ] **Step 1: Write the failing formatter test**

In `crates/bin/src/cli/commands.rs`, add a `#[cfg(test)] mod tests` (or extend an existing one) with:

```rust
#[cfg(test)]
mod command_list_tests {
    use super::format_command_line;
    use pi_domain::entities::CommandSpec;

    #[test]
    fn service_less_command_shows_argv_only() {
        let spec = CommandSpec::new(vec!["node".into(), "seed.js".into()]);
        assert_eq!(format_command_line("seed", &spec), "seed  ->  node seed.js");
    }

    #[test]
    fn service_pinned_command_shows_service_suffix() {
        let spec = CommandSpec {
            argv: vec!["node".into(), "x.cjs".into()],
            service: Some("server".into()),
        };
        assert_eq!(
            format_command_line("create-invite", &spec),
            "create-invite  ->  node x.cjs  [service: server]"
        );
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `rtk cargo test -p pi format_command_line`
Expected: compile error — `format_command_line` not found.

- [ ] **Step 3: Add the formatter and use it**

In `crates/bin/src/cli/commands.rs`, add near the top (module scope):

```rust
fn format_command_line(name: &str, spec: &pi_domain::entities::CommandSpec) -> String {
    let base = format!("{name}  ->  {}", spec.argv.join(" "));
    match &spec.service {
        Some(service) => format!("{base}  [service: {service}]"),
        None => base,
    }
}
```

Replace the list loop body (from Task 2) so it calls the helper:

```rust
            for (cmd, spec) in &resp.commands {
                println!("{}", format_command_line(cmd, spec));
            }
```

- [ ] **Step 4: Run the tests**

Run: `rtk cargo test -p pi format_command_line`
Expected: PASS (2 tests).

- [ ] **Step 5: Commit**

```bash
rtk git add crates/bin/src/cli/commands.rs
rtk git commit -m "feat(cli): show per-command service in rpi command list"
```

---

### Task 5: Documentation

Document the table form and `service` key in the README and the two skills.

**Files:**
- Modify: `README.md` (the `[commands]` section)
- Modify: `.claude/skills/rpi-toml/SKILL.md` (or the file that documents `[commands]`)
- Modify: `.claude/skills/rpi-cli/SKILL.md` ("Running Admin Commands")

**Interfaces:** none (docs only).

- [ ] **Step 1: Locate the docs to edit**

Run: `rtk grep -n "\[commands\]" README.md` and `rtk grep -rn "commands" .claude/skills/rpi-toml .claude/skills/rpi-cli`
Expected: line numbers of the existing `[commands]` documentation.

- [ ] **Step 2: Update `README.md`**

In the `[commands]` documentation, add the table form after the string/array examples:

````markdown
Commands run in the `[ingress].service` container by default. To run a command
in a different compose service, use the table form:

```toml
[commands.create-invite]
run     = "node dist/scripts/create-invite.cjs"   # string or array, same rules as the shorthand
service = "server"                                 # optional; omitted => ingress service
```

`rpi command` (list mode) shows the target service for commands that pin one.
````

- [ ] **Step 3: Update the `rpi-toml` skill**

Add the same table-form example and a one-line note ("`service` — optional compose service to exec into; defaults to `[ingress].service`") to the `[commands]` section of `.claude/skills/rpi-toml/SKILL.md`.

- [ ] **Step 4: Update the `rpi-cli` skill**

In "Running Admin Commands" of `.claude/skills/rpi-cli/SKILL.md`, add a sentence: commands run in `ingress.service` unless the `[commands.<name>]` table sets `service = "<other-service>"`, in which case they exec into that service.

- [ ] **Step 5: Sanity-check and commit**

Run: `rtk cargo build --workspace` (ensure nothing broke) then:

```bash
rtk git add README.md .claude/skills/rpi-toml/SKILL.md .claude/skills/rpi-cli/SKILL.md
rtk git commit -m "docs: per-command service override in [commands]"
```

---

### Task 6: Bump version and tag the release

Cut a release for the feature: bump the workspace version and create an annotated git tag. Do this last, after all functional tasks are merged and green.

**Files:**
- Modify: `Cargo.toml:6` (workspace version)
- Modify: `Cargo.lock` (refreshed by the build)

**Interfaces:** none.

**Version choice:** `0.9.1` → `0.10.0` — a minor bump, because this adds a backward-compatible feature (per-command `service`). Tag convention is `vX.Y.Z` (existing tags: `v0.9.0`, `v0.9.1`).

- [ ] **Step 1: Bump the workspace version**

In `Cargo.toml`, change line 6:

```toml
version = "0.10.0"
```

- [ ] **Step 2: Refresh the lockfile and verify the build**

Run: `rtk cargo build --workspace`
Expected: builds clean; `Cargo.lock` updates the `pi`/`pi-*` package versions to `0.10.0`.

- [ ] **Step 3: Commit the bump**

```bash
rtk git add Cargo.toml Cargo.lock
rtk git commit -m "chore: release 0.10.0 (per-command service override)"
```

- [ ] **Step 4: Create the annotated tag**

```bash
rtk git tag -a v0.10.0 -m "v0.10.0: per-command service override for [commands]"
```

Verify: `git tag --list v0.10.0` prints `v0.10.0`.

Note: pushing the commit and tag (`git push && git push origin v0.10.0`) is a separate, outward-facing step — do it only when the user asks.

---

## Self-Review Notes

- **Spec coverage:** TOML schema → Task 3; domain model + Option-A serde → Task 1; execution → Task 2 (Step 4); protocol DTOs + list service → Tasks 2 & 4; persistence (no migration) → Task 2 (Steps 8 legacy-row test); error handling (empty service parse + agent) → Task 3; testing → each task; documentation → Task 5; release (version bump + tag) → Task 6.
- **Type consistency:** `CommandSpec { argv, service }`, `CommandSpec::new(Vec<String>)`, `From<Vec<String>>`, `RunCommand::list -> BTreeMap<String, CommandSpec>`, `format_command_line(&str, &CommandSpec) -> String` are used consistently across tasks.
- **Backward compat:** service-less commands serialize to a bare array (Task 1 tests `no_service_serializes_back_to_bare_array`), so SQLite rows and the CLI↔agent protocol are unchanged for existing commands; legacy rows load (Task 2 `loads_legacy_array_shaped_commands_row`).
