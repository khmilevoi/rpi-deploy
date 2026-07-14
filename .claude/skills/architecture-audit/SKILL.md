---
name: architecture-audit
description: Use when asked to check, sync, or audit docs/architecture/ against the code — verifies every diagram and walkthrough against its Source anchors and fixes drift.
---

# Architecture Audit

Full sweep of `docs/architecture/` against the code. The outcome is fixed
docs, not a report of suggestions. Conventions and the code-area→doc map
live in the `architecture-diagrams` skill — read it first.

## Procedure

1. **Inventory.** List every `.md` under `docs/architecture/` (including
   `flows/`). `README.md` is checked for link integrity and listing
   completeness only (it deliberately has no anchors).
2. **Per document:**
   - Read its `Source anchors`. Anchor pointing at a missing file → drift.
   - Read every anchored file. Compare against the diagram AND the prose
     walkthrough: stage names/order, participants, states, failure branches.
   - Fix any mismatch by editing the doc (diagram, walkthrough, and anchors
     together — all three must stay consistent). Follow the template and
     Mermaid style rules from `architecture-diagrams`.
3. **Orphan check, both directions:**
   - Docs → code: anchors referencing deleted/renamed files.
   - Code → docs: rows of the code-area→doc map in `architecture-diagrams`
     whose code area has files not reflected in the mapped doc; plus new
     modules under `crates/*/src/` and new `cli/`/`agent/` files matching no
     map row at all. A genuinely new area may need a new `flows/<name>.md`
     — create it from the template, and add a map row to
     `architecture-diagrams` in the same change.
4. **Report.** One list: `doc — what drifted — what was changed`. If nothing
   drifted, say "zero drift" explicitly.
5. **Commit** doc fixes as `docs(architecture): sync <doc> with code` (one
   commit for the sweep is fine).

## What counts as drift

- Diagram or walkthrough contradicts current code behavior.
- Anchor list stale (missing files that shaped the doc; listing dead paths).
- Walkthrough failure branches that no longer exist (or new prominent ones
  missing).
- README index missing a doc or linking a dead file.

Cosmetic differences (wording, layout) are not drift — do not churn docs.
