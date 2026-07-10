# Landing page audit — auditor briefs

Three read-only auditors, each with a narrow lens. You are one of them — read only the
shared context plus your own section. Report discrepancies; do not edit any file.

## Shared context

Site repo (`rpi-deploy-site`): no build step, no framework.

- `src/index.html` — the whole page: hero (terminal mock, install pill, hero-meta line),
  how-it-works (3 cards), features grid (8 cards), quick start (4 steps), dogfood aside, footer.
- `src/copy.js` — copy buttons; the *full* copied payload lives in each button's `data-copy`
  attribute in the HTML and can differ from the visible snippet (that's intentional).
- `src/assets/og.png` — generated screenshot of the hero (`npm run og`, `scripts/generate-og.mjs`);
  stale whenever the hero changes. `src/styles.css` — cosmetic, out of scope.

Pi repo (rpi-deploy itself) — sources of truth:

- `package.json` / `Cargo.toml` `[workspace.package]` — the current released version.
- `README.md` — the "Status: vX.Y (...)" line, the status paragraph narrating recent
  versions, and the "Supported features" list.
- `.github/workflows/release.yml` — which prebuilt-binary targets actually exist.
- `crates/bin/src/output/` (esp. `pipeline.rs`) — what `rpi deploy` output really looks like.
- `rpi.toml` schema: the config structs in `crates/` (search for the deserialization of
  `schema`, `[project]`, `[ingress]`) and real `rpi.toml` files in the ecosystem as living examples.

The shields.io npm badge (`img.shields.io/npm/v/rpi-deploy`) updates itself — never flag it.
Every *literal* number and claim in the HTML is hand-written and can drift.

## Report format (all auditors)

For each discrepancy:

- **Where**: `src/index.html:<line>` (site repo)
- **Page says**: quote it
- **Reality**: what is true now, with the evidence path in the pi repo
- **Fix**: one-line suggested change

End the report with the list of items you checked that were clean — so an empty findings
list is distinguishable from a skipped check. If you could not verify something (e.g. no
built binary to run), say so explicitly instead of guessing.

## Auditor 1 — facts and numbers

Every literal claim on the page vs current reality. Work through:

1. Version strings: `grep -nE '[0-9]+\.[0-9]+\.[0-9]+' src/index.html`. Known trap:
   quick-start step 1 shows an install transcript ending in `✓ rpi <version>` — it must
   equal the current version in the pi repo's `package.json`. This exact string once sat
   five releases stale.
2. The install command (`npm install -g rpi-deploy`) — still the recommended install path
   per the pi README.
3. The hero-meta line ("MIT · prebuilt binaries · Linux / macOS / Windows") — check each
   claim separately: license file, and the *actual* build matrix in
   `.github/workflows/release.yml` plus the npm postinstall fallback (`scripts/` in the pi
   repo). If a platform gets a source build rather than a prebuilt binary, the page
   should not imply otherwise.
4. `<title>`, `<meta name="description">`, and all `og:*` / twitter tags — product claims
   ("one command", "no registry, no Kubernetes, no YAML pipelines", "over plain SSH")
   still true; og:url still the live domain.
5. Footer: license, GitHub / npm links resolve to the right places.

## Auditor 2 — CLI output fidelity

**Principle: the page's terminal blocks are transcriptions of real `rpi` output, not
marketing mocks.** Every marker, glyph, colour, stage name, and wording must be
reproducible from the rendering code. When the CLI's output changes, the page changes
with it, and any line the tool never prints is an invention to remove — not creative
licence. You cannot run a real deploy without a Pi, so audit against the rendering code
below and say so in your report.

Sources of truth (pi repo) — read these, don't guess:

- `crates/bin/src/output/theme.rs` — the active theme. Default is `raspberry`: accent
  `#C51A4A`, success green `#75A928`, warn amber `#d4a017`, and **marker `▸`** — *not*
  `●`, which is the retired `classic` theme. The marker is always painted accent.
- `crates/bin/src/output/banner.rs` — the deploy banner printed at the top of every
  interactive `rpi deploy`: a 5-row density-ramp triangle (`░▒▓▓█`, row widths
  2·4·6·4·2) with a per-row pink `#F06CA0` → raspberry `#C51A4A` sweep, wordmark `r p i`
  (bold) + `deploy · <project>`. Also `deploy_stamp`:
  `deployed ✓ <project>  →  <url> · <n> services (<elapsed>)`.
- `crates/bin/src/output/pipeline.rs` — staged collapse: each finished stage prints
  `✓ <stage> (<elapsed>)` (green `✓`, plain stage name, muted elapsed); `✗ <stage>` on
  failure, `· <stage> skipped` when skipped; lines emitted between stages print plain.
- `crates/application/src/deploy.rs` `run_stages` — the real stage order and plain lines.
  Stages: **fetch** → **build** → **start** → **health** → **route** (only when
  `[ingress].hostname` is set) → **gc** (always, last). Plain lines: `fetched <sha>` after
  fetch; `secrets injected (<k> keys, <f> files)` **only when the project has `[secrets]`**;
  a `project '<name>': host port <n>` line is emitted first but lands in the pre-stage pane
  that is cleared on the first stage event, so it does **not** appear in the final output.
- `crates/bin/src/cli/commands.rs` `deploy()` — the two status lines
  `▸ agent <version> (api <api>)` and `▸ deployment <uuid> started; streaming logs:`. The
  agent `version` is `CARGO_PKG_VERSION` (e.g. `0.17.1`, no `v`); `api` is the string
  `v1` (`crates/bin/src/agent/http.rs`); `<uuid>` is a v4 UUID (`crates/infrastructure/src/sys.rs`).
- `crates/bin/src/output/mod.rs` — line shapes: `status()`/`info()` = `▸ <text>` (accent
  bold marker, untinted text); `success()` = `▸ <text>` with the text tinted green.

**Use this site's own deploy as the example** — the page is itself deployed by `rpi deploy`
(`rpi-deploy-site`, one `web` service, ingress `rpi.iiskelo.com`), and the dogfood aside
already says so, so the hero terminal should depict *this* project's real deploy, not a
fictional `my-app`. Canonical transcript (raspberry theme) the hero terminal must mirror:

```
$ rpi deploy
░░
▒▒▒▒    r p i
▓▓▓▓▓▓  deploy · rpi-deploy-site
▓▓▓▓
██
▸ agent 0.17.1 (api v1)
▸ deployment 3f9c21a4-8b7e-4c2a-9f1d-2e6a5b0c7d84 started; streaming logs:
✓ fetch (1.4s)
fetched 4f2a91c
✓ build (38.2s)
✓ start (2.1s)
✓ health (1.2s)
✓ route (0.6s)
✓ gc (0.3s)
▸ deployed ✓ rpi-deploy-site  →  https://rpi.iiskelo.com · 1 service (44.9s)
```

(No `secrets injected` line — `rpi-deploy-site`'s `rpi.toml` has no `[secrets]`. `route`
is present because a hostname is configured; `gc` is always last. `1 service` is singular —
`deploy_stamp` pluralises.) Colour map: `▸` raspberry; `✓` and the whole stamp line green;
elapsed `(…)` muted; banner rows pink→raspberry top-to-bottom; plain lines default fg.

**Refresh this against the current build every release** — don't trust this transcript or
the page blindly; the numbers, `api` string, stage set, and stamp shape drift as the CLI
evolves. The site deploys via `rpi deploy`, so `rpi` is installed and its commands can be
run locally in the site repo to see real rendering: `rpi --version`, `rpi ls`, `rpi status`
(force colour off-TTY with `CLICOLOR_FORCE=1 COLORTERM=truecolor`). Re-derive the transcript
from the code above each release and reconcile the page with it.

Check each against the code above:

1. Hero terminal (`.terminal-body` in `src/index.html`): markers are `▸` (never `●`);
   stages are fetch→build→start→health→route→gc in order, in the collapsed
   `✓ stage (elapsed)` form; the closing stamp matches `deploy_stamp` (glyph / project /
   `→` url / service count / elapsed). Project, url and service count are this site's real
   values. No line present that the code never emits.
2. Quick-start step 4 (`rpi deploy` mini): real glyphs — `✓ build (…)`, `▸ deployed ✓ …`
   — with no invented spinner line.
3. Quick-start step 2 (`rpi setup`): the closing line is a `▸ <green text>` success, not a
   `✓ …` (the `✓` is only a stamp/stage glyph, never a message marker).
4. Quick-start step 1: install shape; the version check uses a real command
   (`rpi --version` → `rpi X.Y.Z`), not an invented `✓ rpi <ver>` stamp. (Auditor 1 owns
   the number itself.)

## Auditor 3 — features and quick start

Capabilities and configuration shown on the page vs what the tool does today.

1. Features grid (8 cards) vs the pi README "Supported features" list and the status
   paragraph narrating recent versions. Two directions:
   - Each card's claim still accurate (queue semantics, secrets, tunnel ingress, health
     checks, port allocation, logs/stats/lifecycle, one-off commands, prebuilt installs).
   - Any flagship capability shipped since the page was written that's missing? The grid
     is curated — don't demand a card per subcommand; flag only features a user would
     choose the tool for (e.g. a new deploy pipeline view, theming, doctor diagnostics).
2. How-it-works cards: CLI on your machine / systemd agent that clones + builds + allocates
   a stable port / reachable via Cloudflare Tunnel or your own ingress — still the true
   architecture.
3. rpi.toml: both the visible snippet in quick-start step 3 *and* the full example inside
   that button's `data-copy` attribute must be valid against the current schema (field
   names, `schema = 1`, `[project]`/`[source]`/`[ingress]` shape). Check against the
   config structs in the pi repo's `crates/` and its own `rpi.toml` files.
4. Quick-start sequence (install → agent setup on the Pi + `rpi setup` on the machine →
   write rpi.toml → `rpi deploy`) — still the real minimal path. If the tool has grown a
   shorter path (e.g. `rpi init` scaffolding the toml), report it as an option; the main
   agent decides whether the page changes.
