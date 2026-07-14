---
name: architecture-diagrams
description: Use when changing code behavior or structure in this repo, or when creating or editing files under docs/architecture/ — conventions, document template, and update procedure for the Mermaid architecture docs.
---

# Architecture Diagrams

`docs/architecture/` explains the system at component/process level for a
reader who does not read Rust. English only; Mermaid inside GFM (GitHub
renders it natively). This skill defines the conventions and the update
procedure. Full design: `docs/superpowers/specs/2026-07-13-architecture-docs-design.md`.

## When code changes

1. Match the files you touched against the map below; update every affected
   doc **before finishing the task** (this is a CLAUDE.md requirement).
   1a. The map isn't exhaustive — a file can anchor more than one doc — so
       also grep `docs/architecture/` for each changed file's path and update
       every doc that anchors it, even ones the map above doesn't list.
2. Updating a doc means all three: diagram, numbered prose walkthrough, and
   `Source anchors` stay consistent with each other and the code.
3. No matching row but you added a crate/module/process or a new external
   actor: update `crates.md` (and `overview.md` for external actors), and
   create a new `flows/<name>.md` from the template if a new user-visible
   process appeared.

## Code-area → doc map

| Code area touched | Update |
|---|---|
| `crates/application/src/deploy.rs`, `scheduler.rs`; `crates/infrastructure/src/{git,repo,docker,health,probe,hostnet,overrides}.rs` | `flows/deploy.md` |
| `crates/bin/src/cli/{connect,ssh,tunnel,api,config}.rs`, `crates/bin/src/compat.rs` | `flows/connect.md` |
| `crates/application/src/{secrets,mask}.rs`; `crates/infrastructure/src/{secrets,secretsfile,secretpath,dotenv}.rs` | `flows/secrets.md` |
| `crates/infrastructure/src/{cloudflare,cloudflared}.rs` | `flows/ingress.md` |
| `crates/bin/src/agent/{setup,self_install,uninstall}.rs`, `scripts/install.sh`, `scripts/postinstall.js`, `bin/rpi.js` | `flows/agent-setup.md` |
| `crates/bin/src/agent/{update,release}.rs`, `crates/bin/src/cli/upgrade.rs` | `flows/agent-update.md` |
| `crates/application/src/command.rs`, `crates/bin/src/cli/sse.rs` | `flows/commands.md` |
| `crates/application/src/{logs,tail,stats,diagnostics}.rs`; `crates/infrastructure/src/{metrics,sys,events,stats}.rs`; `crates/bin/src/cli/stats_*.rs`; `crates/bin/src/agent/logfile.rs` | `flows/observability.md` |
| `crates/application/src/gc.rs`; `crates/infrastructure/src/{disk,history}.rs` | `flows/gc.md` |
| `Cargo.toml` workspace members, any `crates/*/src/lib.rs`, `crates/domain/src/contracts.rs` | `crates.md` |
| `crates/infrastructure/src/{sqlite,history,migrations,secretsfile,secretpath}.rs`; `crates/bin/src/agent/{config,state,migrate,migrate_ledger}.rs` | `storage.md` |
| `crates/bin/src/agent/http.rs` routes, `crates/bin/src/proto.rs`, `crates/bin/src/cli/commands.rs` | the flow doc of the affected command, plus `crates.md` if the request path itself changed |
| New external actor (registry, API, cloud service) | `overview.md` |

## Document template

Every doc has exactly these sections, in order:

1. **Title + one paragraph** stating what the document explains, in plain
   English, no Rust terminology.
2. **Mermaid diagram(s).**
3. **`## Walkthrough`** — numbered steps of what happens, including failure
   branches (what the user sees when a stage fails, a version is too old,
   a deploy is superseded, …).
4. **`## Source anchors`** — bullet list, one per code file the doc
   describes: `` `path` — role note``. This is the only place code paths
   appear in the doc.

## Mermaid style rules

- Nodes/participants named by role, never by code identifiers:
  "Deploy use case", not `DeployUseCase`; "Agent HTTP API", not `http.rs`.
- Diagram type by purpose: `flowchart` = context/components/data location;
  `sequenceDiagram` = who-calls-whom; `stateDiagram-v2` = lifecycles.
- Soft cap ~20 nodes per diagram — split rather than grow.
- No class/trait diagrams; detail stops at components and processes.

## Definition of done

- [ ] Diagram updated and consistent with the prose walkthrough.
- [ ] Failure branches in the walkthrough match current behavior.
- [ ] `Source anchors` list updated (added/removed files reflected).
- [ ] Every diagram passes: fences correct, `subgraph`/`alt`/`opt`/`loop`
      blocks closed with `end`, special characters in labels quoted
      (`id["label (text)"]`), ≤ ~20 nodes.
