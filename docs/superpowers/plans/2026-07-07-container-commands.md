# Container Commands Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a `[commands]` section to rpi.toml and an `rpi command` CLI subcommand that runs deploy-time-registered admin commands inside the project's service container via `docker compose exec -T`.

**Architecture:** Server-side allowlist. Commands are declared in rpi.toml, sent to the agent inside `ProjectDto` at deploy time, and persisted in the sqlite project registry. Invocation sends only a command name plus extra argv items; the agent executes only stored commands (no generic exec endpoint). Output streams back over SSE with a terminal `exit` event carrying the exit code.

**Tech Stack:** Rust workspace (crates: `pi-domain`, `pi-application`, `pi-infrastructure`, `pi` bin), axum 0.8 (agent HTTP + SSE), clap 4 (CLI), rusqlite + rusqlite_migration, serde, tokio, mockall (tests), new dep: `shlex` (shell-word splitting, CLI side only).

**Spec:** `docs/superpowers/specs/2026-07-07-container-commands-design.md`

## Global Constraints

- Schema stays `1` — `[commands]` is optional, existing rpi.toml files must keep parsing.
- Command names must match `^[a-z0-9][a-z0-9_-]*$` (same rule as `is_valid_name` in `crates/bin/src/agent/http.rs:79`).
- No shell interpretation anywhere: argv arrays only, spawned via `Command::args`. String-form commands are split client-side with `shlex`.
- Container exec is always `docker compose exec -T <service> <argv...>` — non-TTY, stdin closed, service = `ingress.service`.
- Default agent-side command timeout: **600 seconds**; per-project override via `[timeouts] command = "..."` in rpi.toml.
- Wire compatibility both ways: new `ProjectDto` fields carry `#[serde(default)]` (BTreeMap) / are `Option` — old agent ignores them, old CLI leaves them empty.
- All shell commands in this repo are prefixed with `rtk` (see CLAUDE.md), e.g. `rtk cargo test`.
- Commit messages follow the repo style: `feat: ...`, `test: ...`, `docs: ...`, one commit per task, ending with the `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>` trailer.
- Work happens in the worktree `C:\Users\Khmil\RustProjects\pi\.claude\worktrees\feat-container-commands` (branch `worktree-feat-container-commands`). All paths below are relative to that root.

## File Structure

| File | Change |
| --- | --- |
| `Cargo.toml` (workspace) | add `shlex = "1"` to `[workspace.dependencies]` |
| `crates/domain/src/entities.rs` | `ProjectConfig` + `commands`, `command_timeout_secs` |
| `crates/domain/src/contracts.rs` | `ContainerRuntime::exec` |
| `crates/infrastructure/src/process.rs` | `run_streamed_code` (exit-code-returning variant) |
| `crates/infrastructure/src/docker.rs` | `exec_tail` helper + `exec` impl |
| `crates/infrastructure/src/sqlite.rs` | migration: `commands`, `command_timeout_secs` columns |
| `crates/infrastructure/src/repo.rs` | persist/load the new fields |
| `crates/application/src/command.rs` | **new** — `RunCommand` use case |
| `crates/application/src/lib.rs` | `pub mod command;` |
| `crates/bin/Cargo.toml` | add `shlex = { workspace = true }` |
| `crates/bin/src/proto.rs` | `ProjectDto.commands`, `TimeoutsDto.command_secs`, `CommandsResponse`, `CommandRunRequest` |
| `crates/bin/src/cli/rpitoml.rs` | parse + validate `[commands]`, `[timeouts].command` |
| `crates/bin/src/agent/http.rs` | 2 routes, handlers, `sse_exit`, deploy-time validation, tests |
| `crates/bin/src/agent/state.rs` | wire `RunCommand` into `AppState` |
| `crates/bin/src/cli/api.rs` | `list_commands`, `run_command` client methods |
| `crates/bin/src/cli/commands.rs` | `command()` CLI entry |
| `crates/bin/src/main.rs` | `Cmd::Command` clap variant + dispatch + parse tests |
| `README.md`, `plugins/rpi/skills/rpi-toml/SKILL.md`, `plugins/rpi/skills/rpi-cli/SKILL.md` | docs |

---

### Task 1: Domain — `ProjectConfig` fields

**Files:**
- Modify: `crates/domain/src/entities.rs` (~line 136, `ProjectConfig`)
- Modify: every `ProjectConfig { ... }` struct literal the compiler flags (proto.rs, rpitoml.rs, repo.rs incl. tests, application tests)

**Interfaces:**
- Produces: `ProjectConfig.commands: BTreeMap<String, Vec<String>>` and `ProjectConfig.command_timeout_secs: Option<u64>` — every later task relies on these exact names/types.

- [ ] **Step 1: Add the fields**

In `crates/domain/src/entities.rs`, `ProjectConfig` (after the `timeouts` field). `BTreeMap` is already imported at the top of the file.

```rust
    /// Stage timeout overrides ([timeouts] from rpi.toml). Not persisted in DB.
    pub timeouts: StageTimeoutOverrides,
    /// Declared admin commands ([commands] in rpi.toml), name -> argv.
    /// Persisted in the registry: `rpi command` must run what was deployed,
    /// not what the local file currently says.
    pub commands: BTreeMap<String, Vec<String>>,
    /// [timeouts].command override in seconds. Persisted alongside commands
    /// because it is needed at invocation time, not deploy time.
    pub command_timeout_secs: Option<u64>,
```

- [ ] **Step 2: Fix every construction site**

Run: `rtk cargo check --workspace`

The compiler lists every `ProjectConfig { ... }` literal missing the fields. In production code (`crates/bin/src/proto.rs` `From<ProjectDto>`, `crates/bin/src/cli/rpitoml.rs` `to_project_config`, `crates/infrastructure/src/repo.rs` `row_to_project`) add temporary placeholder values — Tasks 2–4 replace them with real mappings:

```rust
            commands: BTreeMap::new(),
            command_timeout_secs: None,
```

(`use std::collections::BTreeMap;` where missing; in `repo.rs`/`proto.rs` prefer `Default::default()` if `BTreeMap` is not already imported.)

In test fixtures (e.g. `cfg()` in `crates/infrastructure/src/repo.rs` tests, application-layer test fixtures) use:

```rust
            commands: Default::default(),
            command_timeout_secs: None,
```

- [ ] **Step 3: Verify the workspace is green**

Run: `rtk cargo test --workspace`
Expected: PASS (no behavior changed, only struct extension)

- [ ] **Step 4: Commit**

```bash
rtk git add -A && rtk git commit -m "feat(domain): add commands and command_timeout_secs to ProjectConfig"
```

---

### Task 2: rpi.toml — parse and validate `[commands]` + `[timeouts].command`

**Files:**
- Modify: `Cargo.toml` (workspace root)
- Modify: `crates/bin/Cargo.toml`
- Modify: `crates/bin/src/cli/rpitoml.rs`
- Test: `crates/bin/src/cli/rpitoml.rs` (inline `mod tests`)

**Interfaces:**
- Consumes: `ProjectConfig.commands` / `.command_timeout_secs` from Task 1.
- Produces: `RpiToml.commands: Option<BTreeMap<String, CommandValue>>`, `TimeoutsSection.command: Option<String>`; `to_project_config()` fills the two new `ProjectConfig` fields. `RpiToml::parse` rejects invalid names/empty commands/empty section/unbalanced quotes/bad durations.

- [ ] **Step 1: Add the shlex dependency**

`Cargo.toml` (workspace), in `[workspace.dependencies]` after `sysinfo`:

```toml
shlex = "1"
```

`crates/bin/Cargo.toml`, in `[dependencies]` after `inquire`:

```toml
shlex = { workspace = true }
```

- [ ] **Step 2: Write the failing tests**

Append inside `mod tests` in `crates/bin/src/cli/rpitoml.rs`:

```rust
    #[test]
    fn commands_section_parses_string_and_array_forms() {
        let toml = SAMPLE.replace(
            "[healthcheck]",
            "[commands]\ncreate-invite = \"node scripts/create-invite.js --admin\"\nmigrate = [\"npx\", \"prisma\", \"migrate\", \"deploy\"]\nbackup = \"sh -c 'pg_dump mydb | gzip > /b.gz'\"\n\n[healthcheck]",
        );
        let config = RpiToml::parse(&toml).unwrap().to_project_config();
        assert_eq!(
            config.commands.get("create-invite").unwrap(),
            &vec!["node".to_string(), "scripts/create-invite.js".into(), "--admin".into()]
        );
        assert_eq!(
            config.commands.get("migrate").unwrap(),
            &vec!["npx".to_string(), "prisma".into(), "migrate".into(), "deploy".into()]
        );
        assert_eq!(
            config.commands.get("backup").unwrap(),
            &vec!["sh".to_string(), "-c".into(), "pg_dump mydb | gzip > /b.gz".into()],
            "quoted segment must stay one argv item"
        );
    }

    #[test]
    fn missing_commands_section_means_no_commands() {
        let config = RpiToml::parse(SAMPLE).unwrap().to_project_config();
        assert!(config.commands.is_empty());
        assert_eq!(config.command_timeout_secs, None);
    }

    #[test]
    fn empty_commands_section_is_rejected() {
        let toml = SAMPLE.replace("[healthcheck]", "[commands]\n\n[healthcheck]");
        let err = RpiToml::parse(&toml).unwrap_err().to_string();
        assert!(err.contains("[commands]"), "got: {err}");
    }

    #[test]
    fn invalid_command_name_is_rejected() {
        for bad in ["\"Bad Name\" = \"run\"", "\"-x\" = \"run\"", "\"UP\" = \"run\""] {
            let toml = SAMPLE.replace("[healthcheck]", &format!("[commands]\n{bad}\n\n[healthcheck]"));
            let err = RpiToml::parse(&toml).unwrap_err().to_string();
            assert!(err.contains("command name"), "{bad}: got: {err}");
        }
    }

    #[test]
    fn empty_command_values_are_rejected() {
        for bad in ["x = \"\"", "x = []", "x = [\"\"]"] {
            let toml = SAMPLE.replace("[healthcheck]", &format!("[commands]\n{bad}\n\n[healthcheck]"));
            assert!(RpiToml::parse(&toml).is_err(), "{bad} must be rejected");
        }
    }

    #[test]
    fn unbalanced_quotes_in_command_are_rejected() {
        let toml = SAMPLE.replace(
            "[healthcheck]",
            "[commands]\nx = \"sh -c 'oops\"\n\n[healthcheck]",
        );
        let err = RpiToml::parse(&toml).unwrap_err().to_string();
        assert!(err.contains("quote"), "got: {err}");
    }

    #[test]
    fn command_timeout_is_parsed_and_validated() {
        let toml = SAMPLE.replace(
            "[healthcheck]",
            "[timeouts]\ncommand = \"30m\"\n\n[healthcheck]",
        );
        let config = RpiToml::parse(&toml).unwrap().to_project_config();
        assert_eq!(config.command_timeout_secs, Some(1800));

        let bad = SAMPLE.replace(
            "[healthcheck]",
            "[timeouts]\ncommand = \"soon\"\n\n[healthcheck]",
        );
        assert!(RpiToml::parse(&bad).is_err());
    }
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `rtk cargo test -p pi rpitoml`
Expected: FAIL — `commands` field does not exist / parse does not validate

- [ ] **Step 4: Implement**

In `crates/bin/src/cli/rpitoml.rs`:

Add import at top:

```rust
use std::collections::BTreeMap;
```

Add to the `RpiToml` struct (after `env`):

```rust
    #[serde(default)]
    pub commands: Option<BTreeMap<String, CommandValue>>,
```

Add `command` to `TimeoutsSection`:

```rust
#[derive(Debug, Default, Deserialize)]
pub struct TimeoutsSection {
    pub fetch: Option<String>,
    pub build: Option<String>,
    pub up: Option<String>,
    /// Budget for one `rpi command` run on the agent.
    pub command: Option<String>,
}
```

Add the value type and helpers (near `validate_expect`):

```rust
/// [commands] value: a shell-word string or an explicit argv array (§spec).
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum CommandValue {
    Line(String),
    Argv(Vec<String>),
}

fn is_valid_command_name(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some('a'..='z' | '0'..='9'))
        && chars.all(|c| matches!(c, 'a'..='z' | '0'..='9' | '_' | '-'))
}

/// String form is split client-side with shell-word rules (quotes only —
/// no variables, pipes or redirects; a shell must be spelled out as
/// `sh -c '...'`). Array form is taken verbatim.
fn command_argv(name: &str, value: &CommandValue) -> anyhow::Result<Vec<String>> {
    let argv = match value {
        CommandValue::Argv(items) => items.clone(),
        CommandValue::Line(line) => shlex::split(line).ok_or_else(|| {
            anyhow::anyhow!("rpi.toml [commands].{name}: unbalanced quote")
        })?,
    };
    if argv.is_empty() || argv.iter().any(|a| a.is_empty()) {
        anyhow::bail!("rpi.toml [commands].{name}: command must not be empty");
    }
    Ok(argv)
}
```

In `RpiToml::parse`, extend the timeouts validation array and add commands validation before `Ok(parsed)`:

```rust
        for (field, value) in [
            ("fetch", &parsed.timeouts.fetch),
            ("build", &parsed.timeouts.build),
            ("up", &parsed.timeouts.up),
            ("command", &parsed.timeouts.command),
        ] {
```

```rust
        if let Some(commands) = &parsed.commands {
            if commands.is_empty() {
                anyhow::bail!("rpi.toml [commands] is empty - declare a command or remove the section");
            }
            for (name, value) in commands {
                if !is_valid_command_name(name) {
                    anyhow::bail!(
                        "rpi.toml [commands]: command name '{name}' must match ^[a-z0-9][a-z0-9_-]*$"
                    );
                }
                command_argv(name, value)?;
            }
        }
```

In `to_project_config`, replace the Task 1 placeholders:

```rust
            commands: self
                .commands
                .as_ref()
                .map(|map| {
                    map.iter()
                        .map(|(name, value)| {
                            let argv =
                                command_argv(name, value).expect("validated in RpiToml::parse");
                            (name.clone(), argv)
                        })
                        .collect()
                })
                .unwrap_or_default(),
            command_timeout_secs: self
                .timeouts
                .command
                .as_deref()
                .and_then(|t| parse_duration_secs(t).ok()),
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `rtk cargo test -p pi rpitoml`
Expected: PASS (all new tests + existing ones)

- [ ] **Step 6: Commit**

```bash
rtk git add -A && rtk git commit -m "feat(cli): parse and validate [commands] and [timeouts].command in rpi.toml"
```

---

### Task 3: Wire protocol — `ProjectDto` fields + new DTOs

**Files:**
- Modify: `crates/bin/src/proto.rs`
- Test: `crates/bin/src/proto.rs` (inline `mod tests` — create it if absent)

**Interfaces:**
- Consumes: `ProjectConfig.commands` / `.command_timeout_secs` (Task 1).
- Produces: `ProjectDto.commands: BTreeMap<String, Vec<String>>` (`#[serde(default)]`), `TimeoutsDto.command_secs: Option<u64>`, `CommandsResponse { commands: BTreeMap<String, Vec<String>> }`, `CommandRunRequest { args: Vec<String> }` — Tasks 7 and 8 import these exact names.

- [ ] **Step 1: Write the failing test**

At the bottom of `crates/bin/src/proto.rs` (add the module if there is none):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_project_dto_without_commands_deserializes_empty() {
        let json = serde_json::json!({
            "name": "rateme", "repo": "r", "branch": "main",
            "compose": "docker-compose.yml", "service": "web",
            "port": 3000, "hostname": null
        });
        let dto: ProjectDto = serde_json::from_value(json).unwrap();
        assert!(dto.commands.is_empty());
        let config: pi_domain::entities::ProjectConfig = dto.into();
        assert!(config.commands.is_empty());
        assert_eq!(config.command_timeout_secs, None);
    }

    #[test]
    fn commands_roundtrip_config_to_dto_and_back() {
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
            command_timeout_secs: Some(1800),
        };
        config
            .commands
            .insert("migrate".into(), vec!["npx".into(), "prisma".into()]);

        let dto: ProjectDto = (&config).into();
        let json = serde_json::to_value(&dto).unwrap();
        let back: ProjectDto = serde_json::from_value(json).unwrap();
        let roundtripped: pi_domain::entities::ProjectConfig = back.into();
        assert_eq!(roundtripped.commands, config.commands);
        assert_eq!(roundtripped.command_timeout_secs, Some(1800));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `rtk cargo test -p pi proto`
Expected: FAIL — no `commands` field on `ProjectDto`

- [ ] **Step 3: Implement**

In `crates/bin/src/proto.rs`:

`TimeoutsDto` gains a field (serde defaults `Option` to `None` when missing):

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeoutsDto {
    pub fetch_secs: Option<u64>,
    pub build_secs: Option<u64>,
    pub up_secs: Option<u64>,
    pub command_secs: Option<u64>,
}
```

`ProjectDto` gains (after `timeouts`):

```rust
    #[serde(default)]
    pub commands: BTreeMap<String, Vec<String>>,
```

`impl From<ProjectDto> for ProjectConfig` — extract `command_secs` before `dto.timeouts` is moved, and replace the Task 1 placeholders:

```rust
impl From<ProjectDto> for ProjectConfig {
    fn from(dto: ProjectDto) -> ProjectConfig {
        let command_timeout_secs = dto.timeouts.as_ref().and_then(|t| t.command_secs);
        ProjectConfig {
            // ... existing field mappings unchanged ...
            commands: dto.commands,
            command_timeout_secs,
        }
    }
}
```

`impl From<&ProjectConfig> for ProjectDto`:

```rust
            commands: config.commands.clone(),
            timeouts: Some(TimeoutsDto {
                fetch_secs: config.timeouts.fetch_secs,
                build_secs: config.timeouts.build_secs,
                up_secs: config.timeouts.up_secs,
                command_secs: config.command_timeout_secs,
            }),
```

New DTOs (near `LifecycleResponse`):

```rust
/// GET /v1/projects/{name}/commands — deployed [commands], name -> argv.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandsResponse {
    pub commands: BTreeMap<String, Vec<String>>,
}

/// POST /v1/projects/{name}/commands/{command} body: extra argv items
/// appended to the declared command (never replacing the program).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandRunRequest {
    #[serde(default)]
    pub args: Vec<String>,
}
```

Note: `TimeoutsDto` construction sites flagged by the compiler (e.g. `rpi.toml`→DTO path in `cli/commands.rs` if any, tests) get `command_secs: None` or the real value where available.

- [ ] **Step 4: Run tests to verify they pass**

Run: `rtk cargo test -p pi proto && rtk cargo check --workspace`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
rtk git add -A && rtk git commit -m "feat(proto): carry [commands] and command timeout in ProjectDto; add command DTOs"
```

---

### Task 4: Persistence — sqlite migration + repo mapping

**Files:**
- Modify: `crates/infrastructure/src/sqlite.rs` (migrations, ~line 36)
- Modify: `crates/infrastructure/src/repo.rs` (SELECT const line 13, `row_to_project`, `upsert`)
- Test: both files' inline `mod tests`

**Interfaces:**
- Consumes: `ProjectConfig.commands` / `.command_timeout_secs` (Task 1).
- Produces: `SqliteProjectRepo` persists and returns both fields through `upsert`/`get`/`list`. Commands are stored as a JSON object string in a `commands TEXT NOT NULL DEFAULT '{}'` column; timeout in `command_timeout_secs INTEGER` (NULL = agent default).

- [ ] **Step 1: Write the failing tests**

In `crates/infrastructure/src/repo.rs` tests (reuse the existing `repo(&dir, min, max)` and `cfg(name)` helpers found in that module):

```rust
    #[tokio::test]
    async fn persists_commands_and_command_timeout() {
        let dir = tempfile::tempdir().unwrap();
        let repo = repo(&dir, 8000, 8999);
        let mut config = cfg("a");
        config
            .commands
            .insert("migrate".into(), vec!["npx".into(), "prisma".into()]);
        config.command_timeout_secs = Some(1800);

        repo.upsert(&config).await.unwrap();
        let loaded = repo.get("a").await.unwrap().unwrap();
        assert_eq!(loaded.config.commands, config.commands);
        assert_eq!(loaded.config.command_timeout_secs, Some(1800));

        // update path: replacing commands persists the new set
        config.commands.clear();
        config
            .commands
            .insert("seed".into(), vec!["node".into(), "seed.js".into()]);
        config.command_timeout_secs = None;
        repo.upsert(&config).await.unwrap();
        let loaded = repo.get("a").await.unwrap().unwrap();
        assert_eq!(loaded.config.commands.len(), 1);
        assert!(loaded.config.commands.contains_key("seed"));
        assert_eq!(loaded.config.command_timeout_secs, None);
    }
```

In `crates/infrastructure/src/sqlite.rs` tests (mirror `migration_adds_expose_column_defaulting_private`):

```rust
    #[tokio::test]
    async fn migration_adds_commands_columns_with_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("state.db")).unwrap();
        db.call(|c| {
            c.execute(
                "INSERT INTO projects
                 (name, repo, branch, compose_path, service, container_port, host_port, created_at)
                 VALUES ('a', 'repo-a', 'main', 'docker-compose.yml', 'web', 3000, 8000, 1)",
                [],
            )
            .map_err(storage_err)?;
            Ok(())
        })
        .await
        .unwrap();
        let (commands, timeout): (String, Option<i64>) = db
            .call(|c| {
                c.query_row(
                    "SELECT commands, command_timeout_secs FROM projects WHERE name = 'a'",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .map_err(storage_err)
            })
            .await
            .unwrap();
        assert_eq!(commands, "{}");
        assert_eq!(timeout, None);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `rtk cargo test -p pi-infrastructure repo && rtk cargo test -p pi-infrastructure sqlite`
Expected: FAIL — column does not exist / fields not persisted

- [ ] **Step 3: Implement**

`crates/infrastructure/src/sqlite.rs` — append a third migration entry after the `expose` one:

```rust
        M::up(
            r#"
        ALTER TABLE projects ADD COLUMN commands TEXT NOT NULL DEFAULT '{}';
        ALTER TABLE projects ADD COLUMN command_timeout_secs INTEGER;
        "#,
        ),
```

`crates/infrastructure/src/repo.rs`:

SELECT const:

```rust
const SELECT: &str = "SELECT name, repo, branch, compose_path, service, container_port, hostname, host_port, created_at, expose, commands, command_timeout_secs FROM projects";
```

`row_to_project` — replace the Task 1 placeholders (columns 10 and 11):

```rust
            commands: serde_json::from_str(&row.get::<_, String>(10)?).unwrap_or_default(),
            command_timeout_secs: row.get(11)?,
```

`upsert` UPDATE branch:

```rust
                    tx.execute(
                        "UPDATE projects SET repo=?2, branch=?3, compose_path=?4, service=?5, container_port=?6, hostname=?7, expose=?8, commands=?9, command_timeout_secs=?10 WHERE name=?1",
                        params![
                            &config.name,
                            &config.repo,
                            &config.branch,
                            &config.compose_path,
                            &config.service,
                            config.container_port,
                            &config.hostname,
                            config.expose.as_str(),
                            serde_json::to_string(&config.commands)
                                .unwrap_or_else(|_| "{}".into()),
                            config.command_timeout_secs,
                        ],
                    )
```

`upsert` INSERT branch:

```rust
                    tx.execute(
                        "INSERT INTO projects (name, repo, branch, compose_path, service, container_port, hostname, host_port, created_at, expose, commands, command_timeout_secs)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, unixepoch(), ?9, ?10, ?11)",
                        params![
                            &config.name,
                            &config.repo,
                            &config.branch,
                            &config.compose_path,
                            &config.service,
                            config.container_port,
                            &config.hostname,
                            port,
                            config.expose.as_str(),
                            serde_json::to_string(&config.commands)
                                .unwrap_or_else(|_| "{}".into()),
                            config.command_timeout_secs,
                        ],
                    )
```

(`serde_json` is already a dependency of `pi-infrastructure`.)

- [ ] **Step 4: Run tests to verify they pass**

Run: `rtk cargo test -p pi-infrastructure`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
rtk git add -A && rtk git commit -m "feat(infra): persist project commands and command timeout in sqlite registry"
```

---

### Task 5: Runtime — `ContainerRuntime::exec` via `docker compose exec -T`

**Files:**
- Modify: `crates/domain/src/contracts.rs` (`ContainerRuntime`, ~line 43)
- Modify: `crates/infrastructure/src/process.rs`
- Modify: `crates/infrastructure/src/docker.rs`
- Test: inline `mod tests` in `process.rs` and `docker.rs`

**Interfaces:**
- Consumes: `ComposeStack`, `LogSink`, `run_streamed`/`forward_lines` internals.
- Produces:
  - `ContainerRuntime::exec(&self, stack: &ComposeStack, service: &str, argv: &[String], log: Arc<dyn LogSink>) -> Result<i32, DomainError>` (async; `MockContainerRuntime` gains `expect_exec` automatically via automock).
  - `pi_infrastructure::process::run_streamed_code(cmd: Command, log: Arc<dyn LogSink>) -> Result<i32, String>`.

- [ ] **Step 1: Write the failing tests**

`crates/infrastructure/src/process.rs` tests:

```rust
    #[tokio::test]
    async fn run_streamed_code_returns_zero_on_success() {
        let sink = Arc::new(VecSink(Mutex::new(vec![])));
        let mut cmd = tokio::process::Command::new("git");
        cmd.arg("--version");
        assert_eq!(run_streamed_code(cmd, sink).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn run_streamed_code_returns_nonzero_code_as_ok() {
        let sink = Arc::new(VecSink(Mutex::new(vec![])));
        let mut cmd = tokio::process::Command::new("git");
        cmd.arg("definitely-not-a-git-command");
        let code = run_streamed_code(cmd, sink).await.unwrap();
        assert_ne!(code, 0, "nonzero exit is data, not an error");
    }
```

`crates/infrastructure/src/docker.rs` tests (style of `compose_args_shape`):

```rust
    #[test]
    fn exec_tail_shape() {
        let argv = strings(&["node", "scripts/create-invite.js", "--email", "x@y.com"]);
        assert_eq!(
            exec_tail("web", &argv),
            vec!["exec", "-T", "web", "node", "scripts/create-invite.js", "--email", "x@y.com"]
        );
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `rtk cargo test -p pi-infrastructure process && rtk cargo test -p pi-infrastructure docker`
Expected: FAIL — functions do not exist

- [ ] **Step 3: Implement**

`crates/infrastructure/src/process.rs` — add the code-returning variant and re-express `run_streamed` through it (message must keep containing "exited with" for existing tests):

```rust
/// Like `run_streamed`, but a nonzero exit is data, not an error: returns the
/// exit code. `Err` is reserved for spawn/wait failures. Killed-by-signal
/// (no code) logs a line and maps to 1. Dropping the future kills the child.
pub async fn run_streamed_code(mut cmd: Command, log: Arc<dyn LogSink>) -> Result<i32, String> {
    let label = format!("{:?}", cmd.as_std());
    cmd.stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null());
    cmd.kill_on_drop(true);
    let mut child = cmd.spawn().map_err(|e| format!("spawn {label}: {e}"))?;

    let stdout = child.stdout.take().ok_or("child stdout not captured")?;
    let stderr = child.stderr.take().ok_or("child stderr not captured")?;
    tokio::join!(
        forward_lines(stdout, Arc::clone(&log)),
        forward_lines(stderr, Arc::clone(&log))
    );

    let status = child
        .wait()
        .await
        .map_err(|e| format!("wait {label}: {e}"))?;
    match status.code() {
        Some(code) => Ok(code),
        None => {
            log.line("process terminated by signal");
            Ok(1)
        }
    }
}

pub async fn run_streamed(cmd: Command, log: Arc<dyn LogSink>) -> Result<(), String> {
    let label = format!("{:?}", cmd.as_std());
    match run_streamed_code(cmd, log).await? {
        0 => Ok(()),
        code => Err(format!("{label} exited with code {code}")),
    }
}
```

(The old body of `run_streamed` is deleted; its four existing tests must still pass.)

`crates/domain/src/contracts.rs` — add to `ContainerRuntime` after `down`:

```rust
    /// `docker compose exec -T <service> <argv...>` inside the running
    /// service container ([commands], §spec). Returns the process exit code;
    /// nonzero is data, not an error. Dropping the future kills the compose
    /// exec client (best effort — the in-container process may survive).
    async fn exec(
        &self,
        stack: &ComposeStack,
        service: &str,
        argv: &[String],
        log: Arc<dyn LogSink>,
    ) -> Result<i32, DomainError>;
```

`crates/infrastructure/src/docker.rs` — helper near `logs_args`:

```rust
pub(crate) fn exec_tail<'a>(service: &'a str, argv: &'a [String]) -> Vec<&'a str> {
    let mut tail = vec!["exec", "-T", service];
    tail.extend(argv.iter().map(String::as_str));
    tail
}
```

Impl in `impl ContainerRuntime for DockerComposeRuntime` (import `run_streamed_code` in the existing `use crate::process::...` line):

```rust
    async fn exec(
        &self,
        stack: &ComposeStack,
        service: &str,
        argv: &[String],
        log: Arc<dyn LogSink>,
    ) -> Result<i32, DomainError> {
        log.line(&format!("docker compose exec -T {service} ..."));
        run_streamed_code(self.compose(stack, &exec_tail(service, argv)), log)
            .await
            .map_err(DomainError::Runtime)
    }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `rtk cargo test -p pi-infrastructure && rtk cargo check --workspace`
Expected: PASS (automock regenerates `MockContainerRuntime` with `expect_exec`; existing mock users compile because unset expectations only fail when called)

- [ ] **Step 5: Commit**

```bash
rtk git add -A && rtk git commit -m "feat(runtime): ContainerRuntime::exec via docker compose exec -T with exit code"
```

---

### Task 6: Application — `RunCommand` use case

**Files:**
- Create: `crates/application/src/command.rs`
- Modify: `crates/application/src/lib.rs` (add `pub mod command;` in alphabetical position)
- Test: `crates/application/src/command.rs` (inline `mod tests`)

**Interfaces:**
- Consumes: `ProjectRepository::get`, `Source::workdir`, `OverrideStore::path`, `ContainerRuntime::exec` (Task 5), `ProjectConfig.commands`/`.command_timeout_secs`.
- Produces (Task 7 uses these exact signatures):
  - `pi_application::command::DEFAULT_COMMAND_TIMEOUT_SECS: u64` (= 600)
  - `RunCommand::new(projects, runtime, source, overrides) -> Arc<RunCommand>`
  - `RunCommand::list(&self, project: &str) -> Result<BTreeMap<String, Vec<String>>, DomainError>`
  - `RunCommand::resolve(&self, project: &str, command: &str) -> Result<(), DomainError>`
  - `RunCommand::execute(&self, project: &str, command: &str, extra_args: &[String], log: Arc<dyn LogSink>) -> Result<i32, DomainError>`

- [ ] **Step 1: Write the file with failing tests**

`crates/application/src/command.rs`:

```rust
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use pi_domain::contracts::{ContainerRuntime, LogSink, OverrideStore, ProjectRepository, Source};
use pi_domain::entities::{ComposeStack, Project};
use pi_domain::error::DomainError;

/// Agent-side budget for one command run when the project sets no
/// [timeouts].command override.
pub const DEFAULT_COMMAND_TIMEOUT_SECS: u64 = 600;

/// Runs a [commands] entry inside the project's service container (§spec).
/// Allowlist semantics: only commands registered at deploy time exist here —
/// there is deliberately no way to execute an arbitrary argv.
pub struct RunCommand {
    projects: Arc<dyn ProjectRepository>,
    runtime: Arc<dyn ContainerRuntime>,
    source: Arc<dyn Source>,
    overrides: Arc<dyn OverrideStore>,
}

impl RunCommand {
    pub fn new(
        projects: Arc<dyn ProjectRepository>,
        runtime: Arc<dyn ContainerRuntime>,
        source: Arc<dyn Source>,
        overrides: Arc<dyn OverrideStore>,
    ) -> Arc<RunCommand> {
        Arc::new(RunCommand {
            projects,
            runtime,
            source,
            overrides,
        })
    }

    /// Deployed commands of a project — `rpi command` list mode.
    pub async fn list(&self, project: &str) -> Result<BTreeMap<String, Vec<String>>, DomainError> {
        Ok(self.registered(project).await?.config.commands)
    }

    /// Existence check so the HTTP layer can 404 before opening the SSE
    /// stream (same idea as StreamLogs::ensure_project).
    pub async fn resolve(&self, project: &str, command: &str) -> Result<(), DomainError> {
        self.lookup(project, command).await.map(|_| ())
    }

    /// Runs declared argv + extra args in the service container.
    /// Returns the in-container exit code; nonzero is data, not an error.
    pub async fn execute(
        &self,
        project: &str,
        command: &str,
        extra_args: &[String],
        log: Arc<dyn LogSink>,
    ) -> Result<i32, DomainError> {
        let (registered, mut argv) = self.lookup(project, command).await?;
        argv.extend(extra_args.iter().cloned());
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
            self.runtime
                .exec(&stack, &registered.config.service, &argv, log),
        )
        .await
        .map_err(|_| DomainError::Timeout {
            stage: "command".into(),
            secs,
        })?
    }

    async fn registered(&self, project: &str) -> Result<Project, DomainError> {
        self.projects
            .get(project)
            .await?
            .ok_or_else(|| DomainError::NotFound(format!("project {project}")))
    }

    async fn lookup(
        &self,
        project: &str,
        command: &str,
    ) -> Result<(Project, Vec<String>), DomainError> {
        let registered = self.registered(project).await?;
        match registered.config.commands.get(command) {
            Some(argv) => {
                let argv = argv.clone();
                Ok((registered, argv))
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::CollectSink;
    use pi_domain::contracts::{
        MockContainerRuntime, MockOverrideStore, MockProjectRepository, MockSource,
    };
    use pi_domain::entities::ProjectConfig;
    use std::path::PathBuf;

    fn project(name: &str) -> Project {
        let mut config = ProjectConfig {
            name: name.into(),
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
            vec!["node".into(), "scripts/create-invite.js".into()],
        );
        Project {
            config,
            host_port: 8000,
            created_at: 1,
        }
    }

    fn deps_with(
        runtime: MockContainerRuntime,
        proj: Project,
    ) -> Arc<RunCommand> {
        let mut projects = MockProjectRepository::new();
        projects
            .expect_get()
            .returning(move |_| Ok(Some(proj.clone())));
        let mut source = MockSource::new();
        source
            .expect_workdir()
            .returning(|name| PathBuf::from("/data").join(name));
        let mut overrides = MockOverrideStore::new();
        overrides
            .expect_path()
            .returning(|name| PathBuf::from("/overrides").join(name));
        RunCommand::new(
            Arc::new(projects),
            Arc::new(runtime),
            Arc::new(source),
            Arc::new(overrides),
        )
    }

    #[tokio::test]
    async fn executes_declared_argv_plus_extra_args_in_service() {
        let mut runtime = MockContainerRuntime::new();
        runtime
            .expect_exec()
            .withf(|stack, service, argv, _| {
                stack.project_name == "rateme"
                    && service == "web"
                    && argv
                        == [
                            "node",
                            "scripts/create-invite.js",
                            "--email",
                            "x@y.com",
                        ]
            })
            .returning(|_, _, _, _| Ok(42));

        let run = deps_with(runtime, project("rateme"));
        let code = run
            .execute(
                "rateme",
                "create-invite",
                &["--email".into(), "x@y.com".into()],
                CollectSink::new(),
            )
            .await
            .unwrap();
        assert_eq!(code, 42, "exit code propagates untouched");
    }

    #[tokio::test]
    async fn unknown_project_is_not_found() {
        let mut projects = MockProjectRepository::new();
        projects.expect_get().returning(|_| Ok(None));
        let run = RunCommand::new(
            Arc::new(projects),
            Arc::new(MockContainerRuntime::new()),
            Arc::new(MockSource::new()),
            Arc::new(MockOverrideStore::new()),
        );
        let err = run
            .execute("ghost", "x", &[], CollectSink::new())
            .await
            .unwrap_err();
        assert!(matches!(err, DomainError::NotFound(_)));
    }

    #[tokio::test]
    async fn unknown_command_lists_available_names() {
        let run = deps_with(MockContainerRuntime::new(), project("rateme"));
        let err = run
            .execute("rateme", "nope", &[], CollectSink::new())
            .await
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("available"), "got: {msg}");
        assert!(msg.contains("create-invite"), "got: {msg}");
    }

    #[tokio::test]
    async fn list_returns_deployed_commands() {
        let run = deps_with(MockContainerRuntime::new(), project("rateme"));
        let commands = run.list("rateme").await.unwrap();
        assert!(commands.contains_key("create-invite"));
    }

    #[tokio::test]
    async fn timeout_kills_the_run_and_reports_stage_command() {
        struct HangingRuntime;
        #[async_trait::async_trait]
        impl ContainerRuntime for HangingRuntime {
            async fn build(&self, _: &ComposeStack, _: Arc<dyn LogSink>) -> Result<(), DomainError> { unimplemented!() }
            async fn up(&self, _: &ComposeStack, _: Arc<dyn LogSink>) -> Result<(), DomainError> { unimplemented!() }
            async fn ps(&self, _: &str) -> Result<Vec<pi_domain::entities::ServiceState>, DomainError> { unimplemented!() }
            async fn prune_images(&self, _: Arc<dyn LogSink>) -> Result<(), DomainError> { unimplemented!() }
            async fn prune_builder(&self, _: Arc<dyn LogSink>) -> Result<(), DomainError> { unimplemented!() }
            async fn logs(&self, _: &str, _: usize, _: bool, _: Arc<dyn LogSink>) -> Result<(), DomainError> { unimplemented!() }
            async fn stats(&self, _: &str) -> Result<Vec<pi_domain::entities::ServiceStats>, DomainError> { unimplemented!() }
            async fn lifecycle(&self, _: &ComposeStack, _: pi_domain::entities::LifecycleAction, _: Arc<dyn LogSink>) -> Result<(), DomainError> { unimplemented!() }
            async fn down(&self, _: &ComposeStack, _: bool, _: Arc<dyn LogSink>) -> Result<(), DomainError> { unimplemented!() }
            async fn exec(&self, _: &ComposeStack, _: &str, _: &[String], _: Arc<dyn LogSink>) -> Result<i32, DomainError> {
                tokio::time::sleep(Duration::from_secs(60)).await;
                Ok(0)
            }
        }

        let mut proj = project("rateme");
        proj.config.command_timeout_secs = Some(0);
        let mut projects = MockProjectRepository::new();
        projects
            .expect_get()
            .returning(move |_| Ok(Some(proj.clone())));
        let mut source = MockSource::new();
        source
            .expect_workdir()
            .returning(|name| PathBuf::from("/data").join(name));
        let mut overrides = MockOverrideStore::new();
        overrides
            .expect_path()
            .returning(|name| PathBuf::from("/overrides").join(name));
        let run = RunCommand::new(
            Arc::new(projects),
            Arc::new(HangingRuntime),
            Arc::new(source),
            Arc::new(overrides),
        );

        let err = run
            .execute("rateme", "create-invite", &[], CollectSink::new())
            .await
            .unwrap_err();
        assert!(
            matches!(err, DomainError::Timeout { ref stage, .. } if stage == "command"),
            "got: {err}"
        );
    }
}
```

Add to `crates/application/src/lib.rs` (alphabetical):

```rust
pub mod command;
```

Note: the `HangingRuntime` test stub uses `#[async_trait::async_trait]`. If `async-trait` is not already available to `pi-application` tests (check `crates/application/Cargo.toml`), add to its `[dev-dependencies]`: `async-trait = { workspace = true }`.

- [ ] **Step 2: Run tests to verify they pass**

Run: `rtk cargo test -p pi-application command`
Expected: PASS (implementation was written together with tests above; if anything fails, fix the implementation, not the assertions)

- [ ] **Step 3: Commit**

```bash
rtk git add -A && rtk git commit -m "feat(app): RunCommand use case - allowlisted container commands with timeout"
```

---

### Task 7: Agent HTTP — routes, SSE `exit` event, wiring, deploy validation

**Files:**
- Modify: `crates/bin/src/agent/http.rs`
- Modify: `crates/bin/src/agent/state.rs`
- Test: `crates/bin/src/agent/http.rs` (inline `mod tests`)

**Interfaces:**
- Consumes: `RunCommand` (Task 6), `CommandsResponse`/`CommandRunRequest` (Task 3), existing `sse_log`, `ChannelSink`, `AbortOnDrop`, `is_valid_name`, `ApiError`.
- Produces:
  - `GET /v1/projects/{name}/commands` → 200 `CommandsResponse`, 404 unknown project.
  - `POST /v1/projects/{name}/commands/{command}` body `CommandRunRequest` → SSE stream of `log` events + terminal `exit` event with the exit code; 404 (JSON error) before the stream when project/command is unknown.
  - `AppState.commands: Arc<RunCommand>`.
  - `create_deployment` rejects invalid command names/argv with 400.

- [ ] **Step 1: Write the failing tests**

Append to `mod tests` in `crates/bin/src/agent/http.rs`:

```rust
    fn deploy_body_with_commands(name: &str) -> serde_json::Value {
        let mut body = deploy_body(name);
        body["project"]["commands"] = serde_json::json!({
            "create-invite": ["node", "scripts/create-invite.js"]
        });
        body
    }

    /// Deploys and polls until the deployment reaches `success`.
    async fn deploy_and_wait(app: &Router, body: &serde_json::Value) {
        let (status, json) = request(app.clone(), post_json("/v1/deployments", body)).await;
        assert_eq!(status, StatusCode::ACCEPTED);
        let id = json["deployment_id"].as_str().unwrap().to_string();
        for _ in 0..100 {
            let (_, d) = request(app.clone(), get_req(&format!("/v1/deployments/{id}"))).await;
            if d["status"] == "success" {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        panic!("deployment did not reach success");
    }

    async fn request_text(
        app: Router,
        req: axum::http::Request<axum::body::Body>,
    ) -> (StatusCode, String) {
        let resp = app.oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        (status, String::from_utf8_lossy(&bytes).into_owned())
    }

    #[tokio::test]
    async fn list_commands_returns_deployed_commands() {
        let dir = tempfile::tempdir().unwrap();
        let app = router(state_with(
            dir.path(),
            Arc::new(ok_source()),
            Arc::new(ok_runtime()),
        ));
        deploy_and_wait(&app, &deploy_body_with_commands("rateme")).await;

        let (status, json) = request(app.clone(), get_req("/v1/projects/rateme/commands")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            json["commands"]["create-invite"],
            serde_json::json!(["node", "scripts/create-invite.js"])
        );

        let (status, _) = request(app, get_req("/v1/projects/ghost/commands")).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn run_command_streams_output_and_exit_code() {
        let dir = tempfile::tempdir().unwrap();
        let mut runtime = ok_runtime();
        runtime
            .expect_exec()
            .withf(|_, service, argv, _| {
                service == "web"
                    && argv == ["node", "scripts/create-invite.js", "--email", "x@y.com"]
            })
            .returning(|_, _, _, log| {
                log.line("invite created");
                Ok(0)
            });
        let app = router(state_with(
            dir.path(),
            Arc::new(ok_source()),
            Arc::new(runtime),
        ));
        deploy_and_wait(&app, &deploy_body_with_commands("rateme")).await;

        let (status, body) = request_text(
            app,
            post_json(
                "/v1/projects/rateme/commands/create-invite",
                &serde_json::json!({ "args": ["--email", "x@y.com"] }),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("event: log"), "got: {body}");
        assert!(body.contains("invite created"), "got: {body}");
        assert!(body.contains("event: exit"), "got: {body}");
        assert!(body.contains("data: 0"), "got: {body}");
    }

    #[tokio::test]
    async fn run_unknown_command_is_404_with_available_names() {
        let dir = tempfile::tempdir().unwrap();
        let app = router(state_with(
            dir.path(),
            Arc::new(ok_source()),
            Arc::new(ok_runtime()),
        ));
        deploy_and_wait(&app, &deploy_body_with_commands("rateme")).await;

        let (status, json) = request(
            app,
            post_json(
                "/v1/projects/rateme/commands/nope",
                &serde_json::json!({ "args": [] }),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let msg = json["error"].as_str().unwrap();
        assert!(msg.contains("create-invite"), "got: {msg}");
    }

    #[tokio::test]
    async fn deploy_rejects_invalid_command_names() {
        let dir = tempfile::tempdir().unwrap();
        let app = router(state_with(
            dir.path(),
            Arc::new(ok_source()),
            Arc::new(ok_runtime()),
        ));
        let mut body = deploy_body("rateme");
        body["project"]["commands"] = serde_json::json!({ "Bad Name": ["run"] });
        let (status, _) = request(app.clone(), post_json("/v1/deployments", &body)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);

        let mut body = deploy_body("rateme");
        body["project"]["commands"] = serde_json::json!({ "x": [] });
        let (status, _) = request(app, post_json("/v1/deployments", &body)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `rtk cargo test -p pi http`
Expected: FAIL — routes/state field do not exist (compile error)

- [ ] **Step 3: Implement**

`crates/bin/src/agent/state.rs`:

- Import: `use pi_application::command::RunCommand;`
- `AppState` gains (after `lifecycle`):

```rust
    pub commands: Arc<RunCommand>,
```

- In `build_state`, right after the `lifecycle` wiring (before `remove`, while `projects`/`runtime`/`source`/`overrides` are still clonable):

```rust
    let commands = RunCommand::new(
        projects.clone(),
        runtime.clone(),
        source.clone(),
        overrides.clone(),
    );
```

and add `commands,` to the `AppState { ... }` literal.

`crates/bin/src/agent/http.rs`:

- Extend the proto import with `CommandRunRequest, CommandsResponse`.
- Routes (after the lifecycle route):

```rust
        .route("/v1/projects/{name}/commands", get(list_commands))
        .route(
            "/v1/projects/{name}/commands/{command}",
            post(run_command),
        )
```

- Terminal event helper (near `sse_finished`):

```rust
/// Terminal event of a command run: the in-container exit code.
fn sse_exit(code: i32) -> Result<Event, Infallible> {
    Ok(Event::default().event("exit").data(code.to_string()))
}
```

- Handlers (near `lifecycle`):

```rust
async fn list_commands(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<CommandsResponse>, ApiError> {
    if !is_valid_name(&name) {
        return Err(ApiError(DomainError::Invalid(
            "project name must match ^[a-z0-9][a-z0-9_-]*$".into(),
        )));
    }
    let commands = state.commands.list(&name).await.map_err(ApiError)?;
    Ok(Json(CommandsResponse { commands }))
}

async fn run_command(
    State(state): State<AppState>,
    Path((name, command)): Path<(String, String)>,
    Json(req): Json<CommandRunRequest>,
) -> Result<Response, ApiError> {
    if !is_valid_name(&name) || !is_valid_name(&command) {
        return Err(ApiError(DomainError::Invalid(
            "project and command names must match ^[a-z0-9][a-z0-9_-]*$".into(),
        )));
    }
    // 404 with a JSON error before the SSE stream opens.
    state
        .commands
        .resolve(&name, &command)
        .await
        .map_err(ApiError)?;

    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let run = state.commands.clone();
    let args = req.args;
    let (task_name, task_cmd, task_args) = (name.clone(), command.clone(), args.clone());
    let started = std::time::Instant::now();
    let handle = tokio::spawn(async move {
        run.execute(&task_name, &task_cmd, &task_args, Arc::new(ChannelSink(tx)))
            .await
    });
    let stream = async_stream::stream! {
        // Client disconnect drops this stream -> guard aborts the task ->
        // the exec future is dropped -> kill_on_drop kills `docker compose
        // exec` (best effort; the in-container process may survive).
        let mut guard = AbortOnDrop(handle);
        while let Some(line) = rx.recv().await {
            yield sse_log(line);
        }
        let code = match (&mut guard.0).await {
            Ok(Ok(code)) => code,
            Ok(Err(e)) => {
                yield sse_log(format!("error: {e}"));
                1
            }
            Err(_) => 1,
        };
        tracing::info!(
            "command run: project={name} command={command} args={args:?} exit={code} duration={}s",
            started.elapsed().as_secs()
        );
        yield sse_exit(code);
    };
    Ok(Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response())
}
```

- In `create_deployment`, after the `container_port` check (defense in depth — the CLI already validates, but the agent must not trust the client):

```rust
    for (cmd_name, argv) in &config.commands {
        if !is_valid_name(cmd_name) {
            return Err(ApiError(DomainError::Invalid(format!(
                "command name '{cmd_name}' must match ^[a-z0-9][a-z0-9_-]*$"
            ))));
        }
        if argv.is_empty() || argv.iter().any(|a| a.is_empty()) {
            return Err(ApiError(DomainError::Invalid(format!(
                "command '{cmd_name}' must have a non-empty argv"
            ))));
        }
    }
```

- In the tests' `state_with` helper, wire the use case exactly like `lifecycle` (before `remove`):

```rust
        let commands = pi_application::command::RunCommand::new(
            projects.clone(),
            Arc::clone(&runtime),
            source.clone(),
            overrides.clone(),
        );
```

and add `commands,` to its `AppState { ... }` literal.

- [ ] **Step 4: Run tests to verify they pass**

Run: `rtk cargo test -p pi http`
Expected: PASS (all 4 new tests + existing router tests)

- [ ] **Step 5: Commit**

```bash
rtk git add -A && rtk git commit -m "feat(agent): command list/run endpoints with SSE exit event and deploy-time validation"
```

---

### Task 8: CLI — `rpi command` subcommand

**Files:**
- Modify: `crates/bin/src/cli/api.rs`
- Modify: `crates/bin/src/cli/commands.rs`
- Modify: `crates/bin/src/main.rs`
- Test: `crates/bin/src/main.rs` (inline `mod tests`)

**Interfaces:**
- Consumes: `CommandsResponse`/`CommandRunRequest` (Task 3), agent endpoints (Task 7), `SseParser`, `ConnectOpts`, `SshTunnel`, `RpiToml`.
- Produces: `rpi command` (list), `rpi command <name> [-- <args>...]` (run, exit code propagated to the shell).

- [ ] **Step 1: Write the failing clap tests**

In `mod tests` of `crates/bin/src/main.rs`:

```rust
    #[test]
    fn command_parses_name_and_trailing_args() {
        let cli = Cli::try_parse_from([
            "pi", "command", "create-invite", "--", "--email", "x@y.com",
        ])
        .unwrap();
        match cli.cmd {
            Cmd::Command { name, args, .. } => {
                assert_eq!(name.as_deref(), Some("create-invite"));
                assert_eq!(args, vec!["--email".to_string(), "x@y.com".into()]);
            }
            _ => panic!("expected command"),
        }
    }

    #[test]
    fn bare_command_means_list_mode() {
        let cli = Cli::try_parse_from(["pi", "command"]).unwrap();
        match cli.cmd {
            Cmd::Command { name, args, .. } => {
                assert_eq!(name, None);
                assert!(args.is_empty());
            }
            _ => panic!("expected command"),
        }
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `rtk cargo test -p pi --bin rpi command_parses`
Expected: FAIL — no `Cmd::Command` variant (compile error)

- [ ] **Step 3: Implement**

`crates/bin/src/main.rs` — new variant in `enum Cmd` (after `Logs`):

```rust
    /// Run a command declared in [commands] of rpi.toml inside the project container
    Command {
        /// Command name; omit to list commands deployed on the agent
        name: Option<String>,
        /// Extra arguments appended to the declared command (write them after --)
        #[arg(last = true)]
        args: Vec<String>,
        #[command(flatten)]
        connect: cli::config::ConnectOpts,
    },
```

Dispatch in `main` (after the `Cmd::Logs` arm):

```rust
        Cmd::Command {
            name,
            args,
            connect,
        } => cli::commands::command(name, args, connect).await,
```

`crates/bin/src/cli/api.rs` — extend the proto import with `CommandRunRequest, CommandsResponse`, then add two methods to `impl ApiClient`:

```rust
    /// 404 on this route can mean two very different things: an old agent
    /// without the feature (bare 404, no JSON body) or a domain "not found"
    /// (JSON error). Distinguish them for a usable message.
    async fn commands_not_found(resp: reqwest::Response) -> anyhow::Error {
        match resp
            .json::<serde_json::Value>()
            .await
            .ok()
            .and_then(|v| v["error"].as_str().map(str::to_string))
        {
            Some(msg) => anyhow::anyhow!("{msg}"),
            None => anyhow::anyhow!(
                "agent does not support [commands]; update rpi-agent on the Pi"
            ),
        }
    }

    pub async fn list_commands(&self, project: &str) -> anyhow::Result<CommandsResponse> {
        let resp = self
            .http
            .get(format!("{}/v1/projects/{project}/commands", self.base))
            .send()
            .await?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(Self::commands_not_found(resp).await);
        }
        Ok(extract_error(resp).await?.json().await?)
    }

    /// Streams command output; returns the in-container exit code.
    pub async fn run_command(
        &self,
        project: &str,
        command: &str,
        args: &[String],
        mut on_line: impl FnMut(&str),
    ) -> anyhow::Result<i32> {
        let resp = self
            .http
            .post(format!(
                "{}/v1/projects/{project}/commands/{command}",
                self.base
            ))
            .json(&CommandRunRequest {
                args: args.to_vec(),
            })
            .send()
            .await?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(Self::commands_not_found(resp).await);
        }
        let resp = extract_error(resp).await?;
        let mut stream = resp.bytes_stream();
        let mut parser = SseParser::default();
        let mut buf: Vec<u8> = Vec::new();
        while let Some(chunk) = stream.next().await {
            buf.extend_from_slice(&chunk?);
            let valid_up_to = match std::str::from_utf8(&buf) {
                Ok(_) => buf.len(),
                Err(e) if e.error_len().is_none() => e.valid_up_to(),
                Err(_) => buf.len(),
            };
            if valid_up_to == 0 {
                continue;
            }
            let text = String::from_utf8_lossy(&buf[..valid_up_to]).into_owned();
            buf.drain(..valid_up_to);
            for ev in parser.push(&text) {
                match ev.event.as_str() {
                    "log" => on_line(&ev.data),
                    "exit" => {
                        return ev.data.trim().parse::<i32>().map_err(|_| {
                            anyhow::anyhow!("agent sent invalid exit code '{}'", ev.data)
                        })
                    }
                    _ => {}
                }
            }
        }
        anyhow::bail!("command stream ended without an exit status (agent restarted?)")
    }
```

`crates/bin/src/cli/commands.rs` — new function (after `lifecycle`):

```rust
pub async fn command(
    name: Option<String>,
    args: Vec<String>,
    connect: ConnectOpts,
) -> anyhow::Result<()> {
    let rpitoml = RpiToml::load(Path::new("rpi.toml"))?;
    let project_name = rpitoml.project.name.clone();

    let profile = connect.resolve()?;
    let tunnel = SshTunnel::open(&profile).await?;
    let api = ApiClient::new(tunnel.base_url.clone());

    let Some(name) = name else {
        // List mode: the agent's answer is the deployed reality; the local
        // file only powers the "undeployed changes" hint.
        let resp = api.list_commands(&project_name).await?;
        if resp.commands.is_empty() {
            eprintln!(
                "no commands deployed for '{project_name}' - declare [commands] in rpi.toml and run `rpi deploy`"
            );
        } else {
            for (cmd, argv) in &resp.commands {
                println!("{cmd}  ->  {}", argv.join(" "));
            }
        }
        let local = rpitoml.to_project_config().commands;
        let undeployed: Vec<&str> = local
            .keys()
            .filter(|k| !resp.commands.contains_key(*k))
            .map(String::as_str)
            .collect();
        if !undeployed.is_empty() {
            eprintln!(
                "note: local rpi.toml declares undeployed command(s): {} - run `rpi deploy`",
                undeployed.join(", ")
            );
        }
        return Ok(());
    };

    let code = api
        .run_command(&project_name, &name, &args, |line| println!("{line}"))
        .await?;
    if code != 0 {
        eprintln!("command '{name}' exited with code {code}");
        drop(tunnel);
        std::process::exit(code);
    }
    eprintln!("command '{name}' finished (exit 0)");
    Ok(())
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `rtk cargo test -p pi`
Expected: PASS (clap tests + whole bin crate)

- [ ] **Step 5: Commit**

```bash
rtk git add -A && rtk git commit -m "feat(cli): rpi command - run and list deployed container commands"
```

---

### Task 9: Docs + final verification

**Files:**
- Modify: `README.md`
- Modify: `plugins/rpi/skills/rpi-toml/SKILL.md`
- Modify: `plugins/rpi/skills/rpi-cli/SKILL.md`

- [ ] **Step 1: README**

Find the rpi.toml reference section in `README.md` (search for `[healthcheck]` or `[timeouts]`) and add, in matching style:

1. A `[commands]` subsection to the config reference:

```markdown
### `[commands]` — admin commands (optional)

One-off admin commands runnable inside the service container with `rpi command`:

​```toml
[commands]
create-invite = "node scripts/create-invite.js"
migrate = ["npx", "prisma", "migrate", "deploy"]
backup = "sh -c 'pg_dump mydb | gzip > /data/backup.gz'"
​```

- Value: string (split with shell-word rules — quotes work, no variables/pipes/redirects) or an explicit argv array. Need a shell? Spell it out: `sh -c '...'`.
- Names must match `[a-z0-9][a-z0-9_-]*`.
- Commands are registered on the agent **at deploy time** and run in the `ingress.service` container via `docker compose exec -T`. The agent only executes deployed commands — there is no generic remote exec.
- Timeout: 10 minutes by default; override with `command = "30m"` in `[timeouts]`.
```

2. A CLI usage entry next to the other commands:

```markdown
​```bash
rpi command                                   # list commands deployed on the agent
rpi command create-invite                     # run a command in the service container
rpi command create-invite -- --email x@y.com  # extra args are appended to the declared argv
​```

The remote exit code becomes the `rpi` exit code. Ctrl+C detaches and best-effort
kills the run on the agent (the in-container process may survive — standard
`docker exec` behavior). A concurrent deploy is not blocked by a running command;
if it restarts the container mid-run, the command fails.
```

- [ ] **Step 2: rpi-toml skill**

In `plugins/rpi/skills/rpi-toml/SKILL.md`:
- Add rows to the Fields table:

```markdown
| `commands.<name>` | no | none | String (shell-word split, quotes only) or argv array. Name: `[a-z0-9][a-z0-9_-]*`. Registered at deploy, run via `rpi command`. |
| `timeouts.command` | no | `"600s"` | Budget for one `rpi command` run. |
```

- Add a `[commands]` example to the Minimal Shape or a new section, and mention in Validation Notes: empty `[commands]` section, empty argv, bad names and unbalanced quotes are rejected by `crates/bin/src/cli/rpitoml.rs`.

- [ ] **Step 3: rpi-cli skill**

In `plugins/rpi/skills/rpi-cli/SKILL.md`, add `rpi command` to the command list following the existing entry style: list mode (bare), run mode, `--` for extra args, exit-code propagation, "update rpi-agent" hint on old agents.

- [ ] **Step 4: Full verification**

Run: `rtk cargo test --workspace && rtk cargo clippy --workspace -- -D warnings && rtk cargo fmt --check`
Expected: all green. If `cargo fmt --check` flags files, run `rtk cargo fmt` and re-check.

- [ ] **Step 5: Commit**

```bash
rtk git add -A && rtk git commit -m "docs: document [commands] and rpi command in README and skills"
```
