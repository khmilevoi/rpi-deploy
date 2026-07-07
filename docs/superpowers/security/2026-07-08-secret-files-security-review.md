# Security Review: Secret Files (`rpi secrets`)

Scope: the `[env]` → `[secrets]` migration (encrypted env bundles + arbitrary
secret files sent from a developer machine to the remote agent and
materialized into the deploy workdir). Commits `40dde4f5` through `49db5c43`
on `worktree-feat-secret-files`.

> **Update (re-review 2026-07-08):** Both findings below are now **remediated**
> in the working tree. A read-side symlink guard, `secretpath::resolve_within_root`
> (canonicalize + `starts_with(root)`), is applied to `[secrets].files`, the
> explicit `[secrets].env`, and the default `.env` in `collect_secrets`, and
> `validate_rel_path` is now applied to `[secrets].env` in `RpiToml::parse`.
> A full re-review of the rest of the PR (write-side symlink/manifest guards,
> the agent secrets PUT/GET handler, project-name→path handling, secret masking
> and `Debug` redaction) surfaced **no additional vulnerabilities** above the
> reporting bar.

## Vuln 1: Path Traversal / Arbitrary File Read: `crates/bin/src/cli/commands.rs:137-146`

* Status: **Fixed** — `collect_secrets` now resolves every file (and the env
  file) via `secretpath::resolve_within_root` before reading, rejecting any
  path whose canonical target escapes the project root.
* Severity: High
* Description: `collect_secrets` reads every path listed in `[secrets].files`
  with `std::fs::read(root.join(rel))`. The only gate is
  `secretpath::validate_rel_path` (`crates/infrastructure/src/secretpath.rs`),
  which is a **string-only** check (rejects `..`, absolute paths, backslashes,
  colons, NUL) and never calls `symlink_metadata` or otherwise inspects the
  filesystem. `std::fs::read` follows symlinks transparently, so a path that
  passes validation as a string (e.g. `certs/server.pem`) can, on disk, be a
  symlink to anywhere the invoking user can read (an SSH private key, cloud
  credentials, another project's `.env`). The write side of this same feature
  (`crates/infrastructure/src/secretsfile.rs`) was hardened against exactly
  this symlink-escape pattern in three follow-up commits in this same PR
  (`feat(infra): SecretsWriter materializes secret files with symlink guard`,
  `fix(infra): close nested-symlink gap in secret file write guard`,
  `fix(infra): guard stale secret file deletion against symlink escape`) — but
  no equivalent guard was added on the CLI's read-before-upload path.
* Exploit Scenario: An attacker with the ability to get a commit merged into
  a project's repo (malicious PR, compromised contributor, tampered
  dependency vendored into the checkout) adds or repurposes a path already
  declared under `[secrets].files` (e.g. `certs/server.pem`) as a git-tracked
  symlink pointing at `/home/victim/.ssh/id_rsa` or any other absolute path.
  When a legitimate developer or CI job later runs `rpi secrets send` from
  that checkout, the tool follows the symlink, base64-encodes the target
  file's bytes, and uploads them over the SSH tunnel to the remote agent,
  where they are stored (age-encrypted) and, on the next deploy, written back
  into the project's Docker Compose workdir — moving the stolen bytes into a
  different trust domain entirely, with no interaction from the victim beyond
  running a routine deploy command.
* Recommendation: Before reading each file in `collect_secrets` (and ideally
  reuse the same helper the write side already has), walk the path's
  directory components with `symlink_metadata` and reject if any component —
  including the leaf — is a symlink, mirroring the guard already implemented
  in `FsSecretsWriter::write_files_blocking`.

## Vuln 2: Path Traversal / Arbitrary File Read: `crates/bin/src/cli/rpitoml.rs` (`SecretsSection.env` unvalidated)

* Status: **Fixed** — `RpiToml::parse` now runs `validate_rel_path` on
  `parsed.secrets.env`, and `collect_secrets` additionally resolves it through
  `resolve_within_root` (defense in depth against symlink escape).
* Severity: High
* Description: `[secrets].files` entries are validated via
  `secretpath::validate_rel_path` in `RpiToml::parse`
  (`crates/bin/src/cli/rpitoml.rs:154-161`), but the sibling `[secrets].env`
  field (`Option<String>` naming the local env file) is never validated
  anywhere in that function or file. `collect_secrets`
  (`crates/bin/src/cli/commands.rs:120-124`) then does
  `root.join(name)` / `std::fs::read_to_string`, where `root` is `"."`. Per
  documented `Path::join` semantics, joining with an absolute string
  discards the base entirely, and joining with a string containing `..`
  walks out of the project root — no symlink or special git tooling needed,
  just a plain string in `rpi.toml`. This is a strictly simpler variant of
  Vuln 1, on a field the codebase's own author clearly intended to constrain
  (they wrote `validate_rel_path` and applied it to `files`, just not to the
  sibling `env` field — an inconsistency, not a deliberate trust boundary).
* Exploit Scenario: An attacker who can get a one-line change merged into a
  project's `rpi.toml` (e.g. hidden among unrelated changes in a PR) sets
  `[secrets]\nenv = "/home/victim/.aws/credentials"` or
  `env = "../other-project/.env"`. Any operator who later runs
  `rpi secrets send` from that checkout has the named file read, parsed with
  the permissive `KEY=VALUE` dotenv parser, and its contents uploaded to the
  remote agent — exfiltrating a file completely outside the project
  directory to a remote party with no symlink or repo tooling required.
* Recommendation: Apply `secretpath::validate_rel_path` (plus the
  symlink-component check from Vuln 1's fix) to `parsed.secrets.env` in
  `RpiToml::parse`, the same way it is already applied to
  `parsed.secrets.files`.
