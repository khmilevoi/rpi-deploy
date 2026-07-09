---
name: release
description: Use when cutting, publishing, planning, or troubleshooting a release of rpi-deploy — version bump, git tag, GitHub Release binaries, npm publish.
---

# Releasing rpi-deploy

Pushing a tag `vX.Y.Z` triggers `.github/workflows/release.yml`, which does everything downstream automatically: version checks → 3-target binary build → GitHub Release with SHA256SUMS and generated notes → npm publish (OIDC, no token). Your job is one correct commit plus one correct tag. Both real release mistakes in this repo's history were made *before* the tag: a version-sync miss (fix commit `492012d`) and a stale README status line — steps 2 and 3 exist because of them.

The version lives in three files that must agree, plus the tag: `Cargo.toml` (`[workspace.package] version` — the only Cargo.toml to touch; crates inherit via `version.workspace = true`), `package.json`, and `Cargo.lock` (regenerated, never hand-edited). CI compares the tag byte-for-byte against `"v" + package.json version`.

## Release checklist (one commit, then one tag)

1. **Clean start**: `rtk git status` clean, `rtk git pull` — you release exactly `origin/master` HEAD.
2. **Bump versions**: `Cargo.toml` `[workspace.package] version` and `package.json` `version` to the same `X.Y.Z`; then `cargo update --workspace` and check `rtk git diff Cargo.lock` shows only the four `pi`/`pi-*` version lines. Stale lockfile = guaranteed CI failure (`--locked` everywhere).
3. **Update docs — this is part of the release commit, not optional polish**:
   - `README.md` "Status: vX.Y (...)" line near the top: new version + one-phrase feature summary, and fold the shipped features into the surrounding status paragraph / Supported features list (see how v0.7 prebuilt binaries is described there).
   - If the release changes behavior users must migrate through, add `docs/migration-*.md` (precedent: `migration-v0.5-to-v0.6.md`).
   - **Post-release, separate repo**: check whether this release changed anything the landing page shows (feature list, quick start, install instructions, CLI output look). If yes, update `rpi-deploy-site` and redeploy it (`rpi deploy` from that repo's root). Local checkout is a sibling directory: `C:\Users\Khmil\RustProjects\rpi-deploy-site` — `cd` there and `git pull` first; only `git clone git@github.com:khmilevoi/rpi-deploy-site.git` as a fallback if that directory doesn't exist. It's a separate repository, so this is a post-release follow-up, not part of the release commit — the npm version badge on the page updates itself and needs no action.
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

## If the release workflow fails after the tag

Fix on master, then delete and re-create the tag: `git tag -d vX.Y.Z && git push origin :refs/tags/vX.Y.Z`, delete the partial GitHub Release if one exists, re-tag. **Only until npm publish has succeeded** — published npm versions are immutable; after that, ship `X.Y.Z+1` instead.
