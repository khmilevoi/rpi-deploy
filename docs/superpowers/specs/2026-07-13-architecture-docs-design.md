# Architecture documentation: Mermaid diagrams + maintenance skills

Date: 2026-07-13

## Problem

The project owner does not read Rust. Today the only ways to understand how rpi
works are the README (user-facing, not structural) and the code itself. There
is no place that answers "how is the system built" and "how does process X
actually flow" without opening `.rs` files.

## Goal

Create `docs/architecture/` — a set of English-language markdown documents with
Mermaid diagrams that explain the system at the component and process level,
plus the infrastructure to keep them from rotting:

- a repo skill with conventions for creating/updating the diagrams,
- a repo skill that audits the diagrams against the code on demand,
- a CLAUDE.md rule making diagram updates part of finishing any task that
  changes covered behavior or structure.

Success criterion: the owner can answer "what talks to what" and "what happens
when I run `rpi <cmd>`" from `docs/architecture/` alone.

Non-goals:

- **No class/trait-level diagrams.** Detail stops at crates, modules-as-roles,
  and process flows. Class diagrams would rot on every refactor and require
  reading-Rust context to interpret.
- **No diagram autogeneration** (cargo-modules, cargo-depgraph). Generated
  graphs surface code identifiers — exactly what the owner does not want to
  read — and cannot express processes.
- **No CI validation of Mermaid syntax in v1.** mermaid-cli drags in puppeteer;
  too heavy and brittle, especially on Windows. Diagrams are verified by
  rendering preview when edited. Revisit only if broken diagrams actually ship.
- **No e2e tests.** Documentation-only change; there is no runtime surface.

## Structure

```
docs/architecture/
├── README.md          # map: what lives where, suggested reading order,
│                      #   pointer to the architecture-diagrams skill
├── overview.md        # system context: operator machine (CLI) ↔ SSH ↔ Pi
│                      #   (agent) ↔ Docker, plus external actors: GitHub,
│                      #   Cloudflare, npm
├── crates.md          # workspace layers: domain ← application ←
│                      #   infrastructure ← bin; request path: CLI command →
│                      #   SSH tunnel → HTTP → use case → contract → infra
├── storage.md         # what the agent keeps on the Pi: sqlite DB, repo
│                      #   checkouts, age-encrypted secrets, config files,
│                      #   systemd unit
└── flows/
    ├── deploy.md        # sequence: fetch→build→start→health→route→gc;
    │                    #   stateDiagram of the deploy queue (latest-wins,
    │                    #   --cancel)
    ├── connect.md       # SSH tunnel to unix socket, handshake, CLI↔agent
    │                    #   version compatibility gating
    ├── secrets.md       # encrypt → send → store (age) → inject into checkout
    │                    #   at deploy time
    ├── ingress.md       # Cloudflare Tunnel: create/adopt, DNS, publishing a
    │                    #   service
    ├── agent-setup.md   # bootstrapping the Pi: rpi agent setup, install.sh,
    │                    #   systemd unit
    ├── agent-update.md  # rpi upgrade / rpi agent update
    ├── commands.md      # rpi command: exec inside the service container, SSE
    │                    #   output streaming
    ├── observability.md # rpi logs / stats / doctor — how data gets from the
    │                    #   Pi to the terminal
    └── gc.md            # what is cleaned and when (images, checkouts,
                         #   history)
```

Diagram type by purpose:

| Purpose | Mermaid type |
|---|---|
| Context, components, data location | `flowchart` |
| "Who calls whom" process flows | `sequenceDiagram` |
| Deploy queue lifecycle | `stateDiagram-v2` |

## Document format

Every document follows one template, in order:

1. **Title + one paragraph** stating what the document explains, in plain
   English, no Rust terminology.
2. **Mermaid diagram(s).** Nodes are named by role ("Deploy use case",
   "SSH tunnel"), never by code identifiers. Soft cap of ~20 nodes per
   diagram; past that, split the diagram rather than grow it.
3. **Prose walkthrough** — numbered steps of what happens, including failure
   branches (health check fails, agent older than CLI, deploy superseded, …).
4. **`Source anchors`** — closing section: a bullet list of the code files the
   document describes, each with a short role note
   (`crates/application/src/deploy.rs — pipeline orchestration`). This is the
   only place code paths appear. Anchors are what the audit skill diffs
   against and what the conventions skill uses to answer "which docs does my
   change touch".

Language: everything in English, matching the rest of the repo's docs. Code
identifiers, command names, and file paths appear as-is.

GitHub and IDE markdown preview render Mermaid natively; the owner needs no
tooling to view the diagrams.

## Skills

Two repo-local skills under `.claude/skills/`, following the existing
`add-cli-command` / `release` conventions (single `SKILL.md`, dense,
checklist-style).

### `architecture-diagrams`

Trigger (description): any task that changes code behavior or structure, and
any task that creates or edits files under `docs/architecture/`.

Contents:

- **Code-area → doc map**: which source areas invalidate which documents
  (e.g. `crates/bin/src/agent/update.rs` → `flows/agent-update.md`;
  `crates/application/src/deploy.rs`, `scheduler.rs` → `flows/deploy.md`;
  `crates/infrastructure/src/cloudflare*.rs` → `flows/ingress.md`; a new crate
  or module → `crates.md` and possibly a new flow doc).
- **Document template** — the four sections above, spelled out.
- **Mermaid style rules** — role-based node names, node cap, diagram type by
  purpose, no code identifiers inside diagrams.
- **Definition of done** — diagram updated, prose walkthrough consistent with
  the diagram, `Source anchors` updated, diagram renders in preview.

### `architecture-audit`

Trigger (description): on request — checking, syncing, or auditing the
architecture docs against the code.

Procedure:

1. For each document under `docs/architecture/`: read its `Source anchors`,
   read those files, compare diagram + prose against reality.
2. Fix any drift found (edit the doc, not just report).
3. Orphan check both ways: source areas with no covering document (new
   modules), and anchors pointing at files that no longer exist.
4. Report: list of discrepancies found and what was changed.

## CLAUDE.md rule

Add one bullet to the existing "Before finishing any task" block in the
project `CLAUDE.md`, same register as the fmt/clippy/test items: if the change
alters behavior or structure covered by `docs/architecture/`, update the
affected documents (see the `architecture-diagrams` skill) before finishing.

## Acceptance

- All 13 documents exist and every diagram renders (verified via markdown
  preview during authoring).
- `architecture-audit` is run once right after the initial authoring pass and
  reports zero drift — this doubles as the acceptance test for the skill
  itself.
- The CLAUDE.md rule and both skills are committed alongside the docs.
