---
name: release
description: Use when cutting, publishing, planning, or troubleshooting a release of rpi-deploy — choosing the version bump (patch vs minor vs major), git tag, GitHub Release binaries, npm publish, and the post-release landing page audit.
---

# Releasing rpi-deploy

Pushing a tag `vX.Y.Z` triggers `.github/workflows/release.yml`, which does everything downstream automatically: version checks → 3-target binary build → GitHub Release with SHA256SUMS and generated notes → npm publish (OIDC, no token). Your job is one correct commit plus one correct tag. Both real release mistakes in this repo's history were made *before* the tag: a version-sync miss (fix commit `492012d`) and a stale README status line — steps 2 and 3 exist because of them.

The version lives in three files that must agree, plus the tag: `Cargo.toml` (`[workspace.package] version` — the only Cargo.toml to touch; crates inherit via `version.workspace = true`), `package.json`, and `Cargo.lock` (regenerated, never hand-edited). CI compares the tag byte-for-byte against `"v" + package.json version`.

## Choosing the bump (patch / minor / major)

Decide from the actual unreleased commits, never from memory of what "feels" shipped:

```
rtk git log --oneline "$(git describe --tags --abbrev=0)..HEAD"
```

The project is pre-1.0; the rules as practiced here:

- **Minor** (`0.X.Y → 0.(X+1).0`) — at least one commit gives users something new to do or see: any `feat:` commit, a new command or flag, a new config field, new output rendering, or a deprecation (the old way still works but warns — that's new behavior, not a fix). Precedents: v0.14.0 (output theming), v0.17.0 (token-via-file setup, new doctor ingress checks).
- **Patch** (`0.X.Y → 0.X.(Y+1)`) — only `fix:` / `docs:` / `chore:` / `ci:` commits: corrections to behavior that already existed, nothing users can newly do. Precedents: v0.9.1 (self-install fix), v0.17.1 (doctor false-failure fix).
- **Major** — not used before 1.0. A breaking change (rpi.toml schema, removed flag or command, agent-protocol incompatibility) still ships as a **minor** bump, but requires a `docs/migration-*.md` (precedent: `migration-v0.5-to-v0.6.md`) and a README callout. Moving to 1.0.0 is a deliberate API-stability decision — only on the user's explicit call, never yours.

Tiebreakers: mixed `feat` + `fix` → minor (feat wins). "Is this rendering change a feature?" — if a user looking at the terminal can tell the difference, yes → minor. If genuinely torn, name the two candidate versions and your reasoning to the user before bumping.

## Release checklist (one commit, then one tag)

1. **Clean start**: `rtk git status` clean, `rtk git pull` — you release exactly `origin/master` HEAD.
2. **Bump versions**: `Cargo.toml` `[workspace.package] version` and `package.json` `version` to the same `X.Y.Z`; then `cargo update --workspace` and check `rtk git diff Cargo.lock` shows only the four `pi`/`pi-*` version lines. Stale lockfile = guaranteed CI failure (`--locked` everywhere).
3. **Update docs — this is part of the release commit, not optional polish**:
   - `README.md` "Status: vX.Y (...)" line near the top: new version + one-phrase feature summary, and fold the shipped features into the surrounding status paragraph / Supported features list (see how v0.7 prebuilt binaries is described there).
   - If the release changes behavior users must migrate through, add `docs/migration-*.md` (precedent: `migration-v0.5-to-v0.6.md`).
   - The landing page lives in a separate repo and is a **post-release follow-up** — run the "Landing page audit" section below after the tag; never fold it into the release commit.
4. **Local gate** (mirrors CI's `check` job and `ci.yml`; catch it here, not in CI):
   ```
   node scripts/check-version.js        # must print: check-version: ok (X.Y.Z)
   rtk cargo fmt --all -- --check
   rtk cargo clippy --all-targets --locked -- -D warnings
   rtk cargo test --locked
   node --test "scripts/**/*.test.js"   # postinstall tests; CI runs these too
   npm pack --dry-run                   # tarball must include bin/, scripts/, crates/, Cargo.toml, Cargo.lock
   ```
5. **Commit and push**: `chore: release X.Y.Z` with `Cargo.toml package.json Cargo.lock README.md` (+ any docs). Wait for the `ci` workflow to go green: `rtk gh run list --workflow ci --limit 1`.
6. **Optional dry run** (recommended after toolchain/dependency changes): `gh workflow run release.yml --ref master` builds all 3 targets (Windows MSVC, x86_64/aarch64 musl) but skips release + publish.
7. **Tag and push**: `rtk git tag -a vX.Y.Z -m "vX.Y.Z" && rtk git push origin vX.Y.Z`. Lowercase `v`, full three-part version — the check job rejects anything else.

## After the tag (automatic — do not do these by hand)

check (versions+tests) → build (3 archives named `rpi-vX.Y.Z-<triple>.*`) → GitHub Release (`--generate-notes`, SHA256SUMS) → npm publish. Notes can be polished on GitHub afterwards.

## Post-release verification

```
rtk gh run list --workflow release --limit 1   # 4 jobs green
gh release view vX.Y.Z                         # 3 archives + SHA256SUMS
npm view rpi-deploy version                    # X.Y.Z
```

The `npx rpi-deploy@X.Y.Z --version` check must run in a throwaway Docker container, never directly on the dev machine — a local machine can have a global `rpi-deploy` install or npx cache that shadows the version resolution and silently passes/fails against stale state instead of the real published package:

```
docker run --rm node:20-slim npx -y rpi-deploy@X.Y.Z --version   # must print rpi X.Y.Z, and install must be fast (prebuilt binary), not a multi-minute cargo build
```

## Landing page audit (after every release, in subagents)

The landing (`rpi-deploy-site`, live at https://rpi.iiskelo.com) once sat five releases stale — quick-start step 1 still printed `rpi 0.12.0` when v0.17.1 was current — because this step used to say "check whether this release changed anything the landing shows", and that check got answered from memory ("probably not") instead of by reading the page. Drift accumulates across releases, so the audit is unconditional: run it even when this release "obviously" changed nothing user-visible — the drift you find is usually from earlier releases.

1. **Sync the site repo.** Local checkout is a sibling directory: `C:\Users\Khmil\RustProjects\rpi-deploy-site` — `cd` there and `rtk git pull` first; only `git clone git@github.com:khmilevoi/rpi-deploy-site.git` as a fallback if the directory doesn't exist.
2. **Spawn three read-only auditor subagents in parallel** (one message; Explore-type agents fit — they must not edit anything). Narrow lenses are the point: one broad "check the landing" pass skims and misses, three auditors each reading their sources end to end don't. Each prompt must be self-contained: the site repo path, the pi repo path, the absolute path to this skill's `references/landing-audit.md` with which section to follow, and the report format defined there.
   - **Auditor 1 — facts and numbers**: every literal claim on the page (version strings, platform list, license, install command, meta/OG descriptions) vs current reality.
   - **Auditor 2 — CLI output fidelity**: the hero terminal mock and quick-start output snippets vs what the CLI actually renders today.
   - **Auditor 3 — features and quick start**: feature grid, how-it-works cards, rpi.toml example, and quick-start step sequence vs current capabilities and schema.
3. **Apply the confirmed fixes yourself** in the site repo (auditors only report). If the hero terminal block changed, regenerate the OG image with `npm run og` — it is a screenshot of the hero, so it goes stale together with it.
4. **Deploy and verify**: commit, push, then `rpi deploy` from the site repo root; check the live page reflects the fixes. The npm version *badge* is the only element that updates itself — every other number and claim on the page is hand-written.

## If the release workflow fails after the tag

Fix on master, then delete and re-create the tag: `git tag -d vX.Y.Z && git push origin :refs/tags/vX.Y.Z`, delete the partial GitHub Release if one exists, re-tag. **Only until npm publish has succeeded** — published npm versions are immutable; after that, ship `X.Y.Z+1` instead.
