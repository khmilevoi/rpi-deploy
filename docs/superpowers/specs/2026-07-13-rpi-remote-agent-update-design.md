# Signed client-triggered agent update (`rpi upgrade`)

Date: 2026-07-13 (reconciled and re-approved 2026-07-14)
Status: approved design; implementation planning pending
Supersedes: the original SHA256/GitHub-TLS update design in this file

## Purpose

`rpi upgrade` updates the board from a client without granting the long-lived
agent root authority. Release authenticity is verified with Sigstore on both
the client and board; the privileged operation consumes only a verified opaque
staging ID, performs the same rootless setup/migration transaction as a local
`sudo rpi agent setup`, and can roll back the binary and host state.

This design is subordinate to
`2026-07-13-rpi-security-hardening-design.md`. If the documents differ, the
security invariants in that umbrella design win.

## Reconciliation with current code

The current implementation on `master` already has useful command and swap
plumbing:

- `crates/bin/src/cli/upgrade.rs` resolves a version and invokes
  `ssh -t ... sudo rpi agent update --version <X>`;
- `crates/bin/src/agent/update.rs` obtains a binary through npm or a direct
  GitHub download, then calls the existing self-install/setup path;
- `crates/bin/src/agent/release.rs` verifies an archive against same-release
  `SHA256SUMS` and accepts production base/API URL environment overrides;
- `scripts/postinstall.js` downloads the same archive and falls back to a source
  build, including automatic rustup installation;
- `scripts/install.sh` is a `curl | sh`-style bootstrap using SHA256SUMS;
- `.github/workflows/release.yml` publishes archives and npm but uses mutable
  Action/toolchain/package references and does not sign release artifacts.

Those mechanics match the superseded design, not the approved security model.
A checksum proves integrity only relative to the checksum file; replacing both
is undetectable. npm/source fallback also turns a verification or availability
failure into execution of a different supply path. The target below replaces,
rather than wraps, those trust decisions.

## Goals

- One client command updates a selected board to an exact authenticated
  version.
- No permanent root helper, setuid binary, generic root RPC, or agent sudo right.
- No binary bytes share the PTY used for a sudo password.
- Client and board independently verify artifact, Sigstore identity, issuer,
  platform, digest, and version.
- Remote downgrade is impossible.
- Binary replacement, setup, migration, service restart, verification, and
  rollback form one resumable transaction.
- The normal protocol supports at least the previous protocol minor so an older
  client can stage a compatible update.

## Non-goals

- Updating the client binary itself; npm/platform packages remain the client
  update channel.
- A permanent unattended root service.
- A supported `curl | sh` or checksum-only bootstrap.
- Silently repairing arbitrary/manual host modifications.
- Updating boards older than the first release containing the stable update
  staging capability. They need one manual signed-package update.
- Providing a general file-upload or arbitrary privileged-command API.

## Trust roots and invariants

An update requires all of these independent checks:

1. The client pins the board's SSH Ed25519 host key.
2. The stdio control session authenticates the agent challenge.
3. The staging request is signed by an unrevoked API identity with
   `agent:update:stage`; this is an owner capability by default.
4. The release bundle chains to the embedded Sigstore trusted root and matches
   the exact GitHub Actions OIDC issuer and release workflow identity.
5. The signed artifact/manifest names an exact semantic version, target triple,
   digest, byte size, and supported protocol range.
6. The fixed root apply command accepts only an opaque stage ID under its own
   private staging root and repeats verification from bytes.
7. A root-owned monotonic version floor rejects an older version.

A stolen deploy-only CI key cannot stage an update. A stolen owner API key
without board SSH/sudo cannot apply one. A stolen SSH key without an API key
cannot create a valid stage. Anyone who already controls board root is outside
the threat model.

The root apply path never accepts a release URL, arbitrary filesystem path,
shell fragment, package name, npm channel, or source-build flag.

## Release and npm format

Each release publishes, for every supported target:

```text
rpi-v<VERSION>-<TARGET>.tar.gz        # or .zip on Windows
rpi-v<VERSION>-<TARGET>.sigstore.json
release-manifest.json
release-manifest.sigstore.json
```

The manifest includes exact version, target, artifact filename, SHA-256, size,
protocol min/max, minimum setup schema, and release timestamp. Both manifest and
artifacts are keyless-signed by the release workflow. Verification pins:

- repository and exact workflow certificate identity;
- GitHub Actions OIDC issuer;
- Sigstore trusted root and bundle/transparency evidence;
- digest and signed manifest fields.

When the Sigstore trusted root changes, release metadata carries the required
threshold-signed TUF root rotation chain. An installed verifier accepts a new
root only by walking that chain from its embedded root; an artifact bundle
cannot replace its own trust anchor.

Release CI uses immutable commit SHAs for third-party Actions, pinned Rust/Node
toolchains, locked dependencies, least-privilege job permissions, and npm
trusted publishing/provenance. Release publication fails unless all platform
artifacts, bundles, manifest entries, and version fields form one complete set.

npm uses a small provenance-bearing meta package plus platform-specific
packages constrained by `os`/`cpu`. The selected package already contains the
binary and bundle. Installation does not download GitHub assets, install rustup,
or compile source. Unsupported platform, missing package, or verification
failure is fatal; there is no fallback channel.

`scripts/install.sh` is removed from supported distribution. A future no-npm
bootstrap requires a separately designed signed OS repository or an operator
who already has a trusted Sigstore verifier; a pipe-to-shell bootstrap cannot
establish the required root of trust.

## Client command

```text
rpi upgrade --server <profile>
rpi upgrade --server <profile> --version 0.22.0
rpi upgrade --server <profile> --version latest
```

No `--version` means the client's own version. `latest` is resolved by the
client to a concrete signed manifest before confirmation and never reaches the
root command as a moving value. Standard connection/profile flags and `--yes`
are supported. There is no remote `--allow-downgrade`.

The command performs:

1. Resolve the board profile and verify the pinned SSH host key.
2. Open the stdio proxy and complete agent challenge plus protocol/capability
   negotiation, using the independently versioned signed `upgrade-v1` protocol
   when the normal API ranges do not overlap.
3. Read current version, target architecture, migration state, and version
   floor through the minimal signed status operation.
4. Resolve an exact target and reject an obvious downgrade or incompatible
   platform/protocol before download.
5. Download the manifest, artifact, and bundles on the client.
6. Verify Sigstore identity/issuer, manifest signature, artifact digest/size,
   target, exact version, and protocol metadata locally.
7. Create an authenticated update staging session, stream the bytes, and commit
   the measured digest. The agent verifies them again and returns a random
   opaque stage ID plus expiry.
8. Display `current -> target`, signer identity, expected restart, applicable
   migration/downtime, and confirmation. `--yes` is an explicit automation
   choice, not a policy bypass.
9. Open a separate SSH session with a TTY and invoke only:

   ```text
   sudo -- /usr/local/bin/rpi agent update --apply-staged <stage-id> --yes
   ```

10. Stream privileged progress, reconnect through the normal proxy, require the
    new version/health handshake, and report the transaction result.

Artifact upload and the sudo password deliberately use separate SSH sessions.
This avoids mixing binary framing with a terminal and avoids SCP/SFTP, a remote
download performed as root, or password capture by the CLI.

## Stable update staging capability

Update staging is a narrow control-plane capability, not a general filesystem
upload:

- only an owner or explicitly authorized `agent:update:stage` identity may use
  it;
- the session is bound to client ID, board identity, exact target, declared
  maximum size, nonce, and a short TTL;
- bytes are written under an agent-owned mode-`0700` staging root using random
  names and no path supplied by the client;
- the agent computes size/digest while streaming and does not extract or execute
  the archive;
- a signed commit must match measured size/digest and verified manifest;
- the stage descriptor records verified signer identity, protocol metadata, and
  file identities;
- stage IDs are 256-bit random values encoded with a strict fixed alphabet;
- failed, expired, used, or uncommitted stages are inert GC targets.

The agent's verification is an early rejection and audit event. It is not a
substitute for the privileged verifier because the staging directory is writable
by the unprivileged agent until the root transaction opens and locks its files.

## Privileged apply transaction

`rpi agent update --apply-staged <id>` must run as root. It:

1. Acquires the global setup/update lock and writes a root-owned, fsynced update
   journal before mutation.
2. Resolves the ID only inside the fixed staging root; opens descriptor and
   payload with no-follow/beneath semantics; requires regular files, one link,
   expected owner, and no group/world write bit. While hashing, it copies bytes
   into a newly created root-owned transaction directory, fsyncs that copy, and
   never reopens the agent-writable payload.
3. Recomputes every byte size and digest from the root-owned copy and repeats
   Sigstore manifest/artifact verification with trust policy embedded in the
   installed root-owned binary.
4. Verifies board architecture, executable format, exact target version,
   protocol compatibility, and the root-owned version floor.
5. Copies the current binary and relevant unit/config state into the transaction
   rollback directory, fsyncs, and atomically installs the verified new binary.
6. Invokes the newly installed binary to continue the root-owned transaction;
   the old process must not apply new-version setup logic on its behalf.
7. Runs the same classification, preflight, confirmation, rootless setup, and
   resumable migration state machine as `sudo rpi agent setup`.
8. Restarts the agent and requires a bounded local Unix-socket health/version
   check before committing the new version floor.
9. Marks the stage consumed and schedules its cleanup.

If verification fails, no host state changes. If setup, migration, restart, or
health verification fails, the journal restores the previous binary and
unit/config state; a migration that crossed downtime uses its own backup journal
to restore rootful project state. Re-running the command resumes or rolls back
from the last verified boundary.

Same-version updates verify and finish as a no-op unless setup reconciliation is
explicitly requested. A lower version is always rejected by `rpi upgrade` and
the apply command. Emergency downgrade requires a separate board-local root
recovery procedure that visibly adjusts the version floor; it is not expressible
through the remote update API.

## sudo behavior

The default opens a TTY, so a normal sudo password is entered directly into the
remote sudo prompt. The client never reads or stores it. The client first shows
the complete update/migration plan; after the operator confirms (or supplied
client-side `--yes`), the fixed root command receives `--yes` so setup does not
ask a second question. It does not bypass sudo or any preflight check.

No sudoers change is installed automatically. An operator who needs unattended
updates may create a narrow rule for the exact
`/usr/local/bin/rpi agent update --apply-staged ...` subcommand. This is safer
than generic agent-update or setup rights because the root command accepts no
URL/path and can install only a correctly signed, non-downgrade RPI artifact.
The residual risk is documented by `rpi doctor`.

## Protocol and bootstrap behavior

The control handshake advertises protocol ranges and capabilities. Normal
operations require overlap; signed `upgrade-v1` status/staging is independently
versioned and kept stable across normal protocol changes, while still using the
same identity, agent challenge, and replay protection. If target metadata says
the new agent cannot speak to the current client, update fails before staging.

The first release that implements this design is the update bootstrap boundary.
Boards on an older SHA256-based release receive one manual update using the new
provenance-bearing platform npm package followed by `sudo rpi agent setup`.
Thereafter `rpi upgrade` is the supported path. A completely unavailable agent
also uses the local signed-package recovery path; update staging is not a second
management daemon.

## Testability and production trust

Production binaries do not read `RPI_RELEASE_BASE_URL` or
`RPI_RELEASE_API_URL`. Tests inject a `ReleaseSource`, trusted fixture root, and
verifier through test-only constructors/build configuration. A production build
cannot be redirected to a local unsigned fixture by environment variables.

Unit/property tests cover:

- exact/latest resolution and canonical semantic versions;
- signed manifest/artifact verification, identity and issuer mismatch, corrupt
  bundle, digest/size mismatch, target mismatch, expiry, and missing target;
- protocol overlap and no-overlap behavior;
- stage ownership, no-follow/beneath handling, hard/symlink attacks, ID parsing,
  commit races, TTL, single-use semantics, and GC;
- version-floor comparison, same-version no-op, and downgrade rejection;
- journal boundaries and calling the newly installed binary for continuation.

Integration/E2E tests cover:

- client verification -> stdio staging -> separate SSH/TTY apply -> restart ->
  version handshake;
- a fake signer, substituted artifact/checksum, mutable `latest`, and production
  base-URL environment variables all failing closed;
- sudo cancellation and wrong password without stage mutation;
- process termination/power-loss injection before and after every binary/setup
  journal boundary;
- setup-triggered rootful-to-rootless migration success and rollback;
- real Debian/Raspberry Pi OS rootless systemd behavior, not only privileged
  Docker-in-Docker.

## Implementation mapping

Reuse:

- `self_install` atomic file primitives after adding fsync/versioned rollback;
- setup classification and the new migration journal/state machine;
- `ConnectOpts`/`ServerProfile`, SSH profile handling, prompts, and structured
  output;
- the new signed stdio control plane and generic two-phase staging primitive.

Replace or remove:

- replace `cli/upgrade.rs` tunnel/version/download logic with signed manifest,
  update staging, and fixed apply orchestration;
- replace `agent/update.rs` npm/GitHub acquisition with apply-by-stage-ID only;
- replace `agent/release.rs` SHA256/curl/environment behavior with embedded
  Sigstore policy and injectable test-only release sources;
- remove `scripts/install.sh` and postinstall network/source-build/rustup paths;
- split npm into provenance-bearing platform packages;
- change the release workflow to keyless signing, immutable CI references, and
  atomic artifact/package publication.

## Acceptance criteria

- A wrong Sigstore identity/issuer, missing bundle, changed artifact, target
  mismatch, unsafe stage path, or downgrade fails before binary replacement.
- The root command has no URL, arbitrary path, npm, shell, or source-build input.
- The client never handles a sudo password and never sends binary bytes through
  the sudo PTY.
- Killing the process at any journal boundary resumes safely or restores the
  prior binary and agent state.
- A successful update reports the exact signed version through a new authenticated
  agent handshake and advances the floor only after health succeeds.
- A deploy-only key, an owner API key without SSH/sudo, and an SSH key without an
  update-capable API identity cannot complete an update.
- npm installation performs no release download, rustup installation, or source
  compilation and publishes verifiable provenance for the selected platform
  package.
