# RPI security hardening program

Date: 2026-07-13
Status: approved in conversation; written review pending

## Purpose

This document defines the security redesign for communication between the RPI
CLI and the board, project deployment, Docker isolation, secrets, persistent
data, migration of existing installations, and the release supply chain.

The current deployment model gives a trusted deployer nearly root-equivalent
power: the agent can reach a rootful Docker socket, repository-controlled
Compose can request host mounts or privileged execution, API requests are not
independently authenticated, and secrets can enter the build context. The goal
is to remove those accidental privilege-escalation paths while keeping RPI a
single-owner deployment tool that is practical on Raspberry Pi OS and Debian.

This is an umbrella design. Delivery is split into five independently planned
subprojects so that authentication, policy auditing, the breaking rootless
migration, supply-chain hardening, and adversarial validation can each be
implemented and reviewed at a manageable size.

## Threat model

### In scope

The board has one trusted owner and may be deployed to by the owner's machines
and CI. We defend against:

- theft of one client credential without treating every other client as
  compromised;
- a compromised CI runner, repository, dependency, Dockerfile, or Compose file;
- accidental or malicious Compose requests for host root, the Docker socket,
  host namespaces, devices, excess resources, or public ports;
- replay, request tampering, local tunnel-port capture, and accidental exposure
  of the agent API on TCP;
- SSH and Git host-key substitution;
- secrets leaking through the checkout, build context, logs, snapshots, or
  Compose interpolation;
- unsigned or substituted RPI/cloudflared release artifacts;
- resource exhaustion and disk exhaustion caused by a managed project;
- interrupted upgrades and failed data migration.

### Out of scope

- mutually hostile tenants sharing one board;
- containment equivalent to a VM or microVM boundary;
- a fully compromised kernel, root account, firmware, physical board, or RPI
  agent account;
- transparent encryption of the whole board; full-disk encryption is an OS
  deployment concern;
- a remote administrator service or permanent privileged broker;
- build-time secret injection in the first hardening release;
- implementing archive upload in this program. This design only creates the
  source abstraction that the later archive provider will use.

The deployer remains an administrator of projects managed by RPI, but no longer
receives an implicit path to host root through Docker. A malicious container may
still compromise its own project data and anything deliberately provided to it.

## Security invariants

The finished system must preserve all of these properties:

1. No long-lived RPI process runs as root or belongs to the rootful `docker`
   group.
2. Production containers run only through the `rpi-agent` user's rootless
   Docker daemon.
3. Repository content cannot mount host paths, mount a Docker socket, select
   host namespaces, add unbounded privileges, publish arbitrary host ports, or
   weaken agent-generated controls.
4. Every mutating or sensitive API request is attributable to a revocable
   client identity and is protected from tampering and replay.
5. The client authenticates the agent as well as the SSH server; capturing a
   local forwarded port is not sufficient to impersonate the agent.
6. Secrets never enter the Git checkout, build context, normalized Compose
   snapshot, image build arguments, or audit log.
7. Persistent database data may live on the board, but only in RPI-namespaced
   Docker named volumes or an explicitly designed future storage backend.
8. Deploy decisions are made against one immutable normalized Compose snapshot;
   all later build and lifecycle operations use that same snapshot and digest.
9. A fresh install never performs migration work. An existing rootful install
   is backed up and recoverable before destructive changes are allowed.
10. A security verification failure is fatal. There is no silent downgrade to
    rootful Docker, unsigned downloads, permissive host-key handling, or an
    unverified Compose file.

## Operator experience

RPI remains one installed CLI binary with two agent modes:

- `sudo rpi agent setup` is an idempotent, one-shot privileged installer and
  migrator. It may install OS prerequisites, create users and directories,
  configure subordinate IDs, install units, migrate data, and restart services.
- `rpi agent run` is the long-lived unprivileged runtime entry point used by the
  installed systemd unit. Operators do not start it by hand.

Initial installation and package upgrades have the same user-facing sequence:

```text
npm install -g <rpi-package>
sudo rpi agent setup
```

`setup` installs or updates the unit and restarts it when required. It is safe to
run repeatedly. An update that does not require a migration still performs
preflight, reconciles configuration, and returns quickly. The user does not run
both `setup` and `run`.

The production setup path is supported on current Raspberry Pi OS and Debian.
Other distributions may use explicitly documented development/manual paths, but
the production installer fails before mutation when it cannot prove the needed
rootless Docker, systemd, cgroup v2, filesystem, and package-manager semantics.

Primary operational commands are:

```text
rpi security audit
rpi security migrate-config
sudo rpi agent setup --dry-run
sudo rpi agent setup
sudo rpi agent migration status
sudo rpi agent migration finalize
rpi doctor
```

## Target architecture

### Process and privilege boundary

The `rpi-agent` system account owns `/var/lib/rpi`, its rootless Docker data,
managed source staging, encrypted secrets, runtime secret generations, policy
read access, and project volumes. It is not in the rootful `docker` group.

Rootless Docker runs as a user systemd service for `rpi-agent`, with lingering
enabled. The RPI agent remains a hardened system service so its Unix socket can
be exposed predictably at `/run/rpi/agent.sock` to the configured local access
group. It runs with the `rpi-agent` UID and connects only to that user's rootless
Docker socket under `/run/user/<uid>/docker.sock`.

The runtime refuses to start if any of the following are true:

- the configured Docker endpoint is rootful or cannot prove rootless mode;
- the cgroup driver is `none`, cgroup v2 is unavailable, or required systemd
  resource controllers are not delegated;
- the production API is configured on TCP;
- key files, policy files, state directories, or sockets have unsafe ownership
  or modes;
- authentication or policy configuration cannot be loaded.

There is no permanent setuid helper, root daemon, or generic root RPC. All
privileged mutation is confined to the explicitly invoked setup command.

### Layered defense against host capture

Rootless Docker alone is not treated as sufficient. A container mounting `/`
could still read or modify files available to `rpi-agent`, including project
data and credentials. The containment therefore has three layers:

1. Rootless Docker prevents the daemon and container root user from becoming
   host root through normal Docker authority.
2. Compose policy rejects all repository-controlled bind mounts, Docker
   sockets, host namespaces, devices, privileged mode, and policy weakening.
3. The agent account and systemd unit receive only the host paths and kernel
   features needed for RPI operation.

A repository cannot bypass these layers by changing Compose between validation
and execution because execution consumes the validated immutable snapshot.

## Authenticated control plane

### Client identities

Each operator machine or CI system has a separate Ed25519 client identity. The
private seed is generated and stored on that client with owner-only permissions;
the board stores only its public key, stable client ID, label, creation time, and
revocation state. Client API identities are distinct from per-project Git deploy
keys. This separation is useful even for one owner: a stolen CI key can be
revoked without interrupting laptops, and the audit log can identify which
machine authorized an operation.

Fresh setup creates a one-time enrollment code, displays it once, and expires it
after ten minutes. `rpi setup` uses the code to enroll the first client. Further
clients are invited locally with commands such as:

```text
sudo rpi agent client invite ci
sudo rpi agent client revoke <client-id>
```

An invite is single-use, short-lived, rate-limited, and recorded in audit. No
private client key is transported to or generated permanently on the board.

### Request authentication

Every authenticated request signs a canonical byte representation containing:

```text
protocol-version
client-id
HTTP method
normalized path and query
timestamp
nonce
SHA-256(request body)
```

The agent verifies the signature, requires a timestamp within 60 seconds of its
clock, and atomically rejects an already-seen nonce for the same identity. Nonce
records live at least through the accepted timestamp window and are bounded per
client. Streaming/SSE requests are signed when opened; their stream lifetime and
event budget are separately bounded.

Only version discovery, health suitable for tunnel startup, and enrollment are
available before authentication. Enrollment additionally requires its one-time
code and strict rate limits. Public health output contains no project or host
details.

### Agent authentication and SSH transport

The agent owns a separate Ed25519 signing identity. Enrollment pins its public
key into the client's board profile, and the enrollment transcript is signed.
On every new control connection the client sends a random challenge and
requires a valid agent signature before sending an authenticated request. Thus a
local process that wins or captures the forwarded port cannot impersonate the
board.

Production API transport remains the Unix socket. The CLI reaches it through an
SSH tunnel bound only to `127.0.0.1`; SSH itself allocates the ephemeral local
port so there is no probe-close-rebind race. The SSH process uses strict host-key
checking, `ForwardAgent=no`, and `ExitOnForwardFailure=yes`. The board profile
pins the SSH host key after explicit enrollment confirmation.

`PI_AGENT_URL` is a development facility. Release builds accept it only for a
Unix socket or an explicitly enabled loopback development endpoint. A production
agent refuses TCP listeners, including loopback TCP, so an accidental firewall
change cannot expose it.

The signed protocol version is not negotiated down after authentication. An
incompatible client or agent fails with an upgrade instruction instead of
retrying an older unsigned request format.

### Audit

Authenticated operations record `request_id`, client ID, operation, project ID,
policy/snapshot digest when relevant, result code, and latency. Request bodies,
environment values, secrets, authorization material, and command output are not
logged by default.

Audit records form a hash chain and rotate under a bounded retention policy. The
chain detects ordinary truncation or editing but is not presented as tamperproof
against compromise of the agent account. A remote audit sink is outside this
program.

## Immutable source contract

Deployment is separated from source acquisition. All source providers must
produce the same internal value:

```text
PreparedSource {
    workdir: canonical private staging directory,
    digest: content identity used throughout the deploy,
    provenance: authenticated human-readable origin,
}
```

The deploy pipeline receives only `PreparedSource` and does not branch on Git or
archive details after acquisition. The staging tree is immutable to the deploy
operation and inaccessible to project containers.

### Git provider

The initial provider deploys an exact 40-character commit SHA. A branch or tag is
resolved to a SHA before the board is asked to deploy; the agent fetches and
checks that exact object and records provenance as `git:<canonical-repo>@<sha>`.
No deployment follows a moving branch after authorization.

When a Git deployment originates from a local checkout, the CLI rejects
uncommitted or untracked source changes because they are not represented by the
authorized commit. Sending local-only content belongs to the future signed
archive provider, not to a permissive Git flag in production.

Git SSH never uses `StrictHostKeyChecking=accept-new`. GitHub fingerprints are
shipped from an authenticated RPI release and updated as release data. An
arbitrary Git host is usable only after a local administrator adds its expected
fingerprint. A changed key fails closed and names the trust command needed to
review it.

Private repositories receive one read-only Ed25519 deploy key per RPI project.
Public HTTPS repositories do not get a key. Private keys live outside checkouts
under `/var/lib/rpi/keys/<stable-project-id>/`, with directory mode `0700` and
file mode `0600`. CLI flows show, rotate, and revoke the public key. `rpi rm`
removes the board's local key; when GitHub CLI is available locally, the CLI may
offer to remove the remote deploy key without ever sending a GitHub token to the
board.

A local policy may additionally require signed commits and an allowed signer.
This is independent of SSH transport and per-project repository authorization.

### Future archive provider

This program implements only the common source contract and Git provider. It
must not publish a half-supported archive API.

A later archive provider will stream a deterministic `tar.zst` over SSH rather
than load it wholly into memory. The client will exclude `.git`, `.env`, RPI
secret/service state, and configured secret inputs; precompute a manifest and
SHA-256; and sign the archive metadata with the normal client identity. The
agent will enforce compressed/uncompressed/file-count/time quotas and reject
absolute paths, `..`, device nodes, FIFOs, escaping hard links, and escaping
symlinks before content-addressed staging. Provenance will be
`archive:sha256:<digest>`.

Archive trust uses the client signature plus content digest. It does not require
a Git deploy key, Git known-host entry, or commit signature. After preparation,
the archive follows exactly the same Compose, secrets, resource, and health
pipeline as Git.

## Compose policy and immutable execution

### Validation pipeline

Each deployment runs these stages in order:

1. Acquire an exact immutable `PreparedSource`.
2. Scan raw Compose inputs before interpolation, includes, builds, or secrets.
3. Resolve allowed Compose files, anchors, merges, variables, and profiles in a
   sealed environment that contains no secret values.
4. Canonicalize paths and reject symlink or traversal escapes.
5. Validate the complete normalized effective model against local policy.
6. Inject agent-owned resource, ingress, namespace, volume, and secret controls.
7. Serialize one final normalized snapshot, atomically store it, and compute its
   digest.
8. Materialize the selected secret generation outside the source tree.
9. Build and start using only the stored snapshot after rechecking its digest.
10. Run health checks, commit state, and append the audit result.

Subsequent restart, stop, logs, command, and removal operations refer to the
stored project/snapshot identity rather than re-reading repository Compose.

### Denied repository capabilities

The raw and normalized scans reject:

- every bind mount, whether absolute, relative, short syntax, long syntax, or
  reached through a symlink;
- the rootful or rootless Docker/Podman/containerd sockets and equivalent daemon
  control endpoints;
- `privileged`, host `network_mode`, host `pid`, host `ipc`, host `userns`, and
  joins to another container's namespaces;
- `devices`, `device_cgroup_rules`, and `volumes_from`;
- dangerous added capabilities, `seccomp=unconfined`, `apparmor=unconfined`, and
  other security-option escapes;
- external volumes, explicit global volume names, unsupported volume drivers,
  and references to another project's volumes;
- repository-declared host port publication;
- build contexts, Dockerfiles, configs, extends/includes, and file references
  outside the canonical prepared source;
- attempts to replace agent-generated overrides, resource ceilings, ingress,
  labels, secret targets, Compose project name, or snapshot files.

Build contexts are checked independently of the Compose file location. Both the
context root and Dockerfile must remain inside the prepared source after link
resolution.

### Allowed capabilities

Projects may use:

- RPI-namespaced local named volumes;
- `tmpfs` mounts within configured limits;
- private/internal Compose networks;
- container-side `expose` without host publication;
- non-secret config files located inside the prepared source;
- agent-generated read-only secret mounts;
- the one agent-managed ingress binding authorized for the project;
- resource requests that do not exceed local maxima.

The agent assigns a stable internal project ID and Compose project name. Volume
names derive from that stable ID plus the logical Compose volume name; project
renaming does not silently create an empty database volume.

### Local exceptions

Repository content and remote clients cannot request a dangerous override. The
only exception source is `/etc/rpi/security.toml`, owned by root, readable by the
agent group, and mode `0640`. An exception binds all of:

- stable project ID;
- exact policy rule/capability;
- exact normalized Compose digest;
- optional narrow parameters such as one device path and access mode.

Changing Compose invalidates the permission. There is no remote
`--allow-dangerous` flag. A typical justified exception is one specific media
device; a host root bind, container engine socket, privileged mode, or policy
override remains non-exemptible.

### Rollout

The first compatibility release computes the complete policy decision and final
snapshot but reports existing violations through `rpi security audit` and deploy
warnings where legacy behavior must temporarily continue. It also ships schema
2 secrets and migration tooling.

The next explicitly announced breaking release makes rootless Docker, the policy
decision, resource ceilings, immutable snapshots, and schema 2 secrets
mandatory. The audit release must provide machine-readable findings and an exact
safe remediation for every enforcement rule. New dangerous features are denied
immediately; a compatibility allowance exists only for a project and Compose
digest inventoried at upgrade time. Any source or Compose change invalidates the
allowance, so the audit period cannot be used to introduce a new dangerous
capability.

## Persistent databases and volumes

Databases such as PostgreSQL may remain on the board. Their data belongs in a
project-namespaced Docker named volume, not a host bind such as
`/srv/postgres:/var/lib/postgresql/data`.

Named volumes survive deploys, restarts, agent/package upgrades, image changes,
and ordinary `rpi rm`. They are deleted only by the explicit destructive command
`rpi rm --volumes`, which lists affected volumes and requires the existing
destructive-action confirmation contract. Garbage collection never removes
named volumes or migration backups.

RPI inventories volume ownership by stable project ID and records the volume
driver and creation origin. External/global volumes and non-local drivers are
not automatically migrated; preflight reports them as blockers before downtime.

## Secrets schema 2

### Configuration

Schema 2 makes service scope explicit and separates environment-file inputs
from file secrets. A representative configuration is:

```toml
[secrets.env]
file = ".env"
services = ["web", "worker"]

[secrets.files.tls-cert]
source = "certs/server.pem"
services = ["web"]
target = "/run/secrets/server.pem"
mode = "0400"
```

The CLI canonicalizes every input path, rejects symlink escapes and special
files, encrypts the selected bytes for the board, and transmits no unrelated
checkout content. Environment keys and file-secret names are validated, but
secret values are never included in diagnostics.

### Storage and injection

The agent stores encrypted secret generations outside source trees. At deploy it
decrypts the selected generation into a private runtime directory outside the
checkout and build context. Environment values are injected only into their
declared services by an agent-generated override. File secrets are exposed only
to declared services as read-only Compose secrets at their declared targets.

Secret values are unavailable to Compose interpolation, Dockerfile `ARG`/`ENV`,
image build contexts, normalized snapshots, and ordinary logs. Build-time
secrets are rejected in this program and require a separate design.

Applying secrets is generation-based and atomic: stage a new encrypted/runtime
generation, deploy and health-check it, then retire the old runtime generation.
A failed health check returns to the previous usable generation.

The security boundary is stated honestly: the `rpi-agent` account and its
rootless Docker daemon can access runtime secrets, and compromise of both the
encrypted store and its age identity permits decryption. Schema 2 prevents
accidental build/repository leakage; it is not a hardware secret vault.

### Schema 1 transition

The audit release detects all schema 1 configurations. `rpi security
migrate-config` automatically rewrites unambiguous cases and prints an exact
schema 2 snippet for ambiguous service scope or target paths. It never guesses
which services should receive a secret.

The breaking release rejects schema 1 and removes all checkout materialization.
Before enforcement, `rpi security audit` must prove that no managed project
depends on legacy `.env` or secret files being written into its source tree.

## Rootless installation and migration

### Fresh installation fast path

`sudo rpi agent setup` first classifies the host. With no managed rootful RPI
installation it uses the clean path:

1. Validate Raspberry Pi OS/Debian version, systemd, cgroup v2, disk, and
   architecture.
2. Install pinned OS prerequisites through apt.
3. Create `rpi-agent`, its home `/var/lib/rpi`, access group, and private state
   directories.
4. Allocate non-overlapping `/etc/subuid` and `/etc/subgid` ranges of at least
   65,536 IDs.
5. Install and verify rootless Docker for `rpi-agent`, enable lingering, and
   enable its user service.
6. Prove the daemon is rootless and all required cgroup resource controls work.
7. Install the hardened agent system service and production Unix socket.
8. Create the one-time first-client enrollment code and start the agent.

It does not create a backup, migration journal, maintenance window, or rootful
Docker dependency. An unrelated rootful daemon and foreign Docker resources are
left untouched.

### Existing installation preflight

An existing managed rootful installation enters migration mode under a global
setup lock. Before stopping anything, setup:

- inventories managed projects, snapshots, containers, networks, and named
  volumes without adopting unrelated Docker resources;
- runs the complete future policy and schema audit;
- blocks unsupported bind/external volumes, storage drivers, devices, or
  unresolved configuration;
- validates rootless prerequisites, subordinate ID ranges, cgroup delegation,
  pinned backup helper availability, and target ownership mapping;
- estimates source, archive, and restored-volume space plus a safety margin;
- creates an atomic, resumable migration journal containing every intended
  action and previous unit configuration.

If preflight fails, no managed stack is stopped and no active configuration is
changed. Migration normally needs free space for both a backup copy and a
restored rootless copy while the original data remains intact; `--dry-run`
reports the measured requirement.

### Maintenance, backup, and restore

After successful preflight, setup opens a maintenance window and gracefully
stops all managed stacks. For every managed local named volume it uses a pinned,
digest-addressed helper image to create an encrypted archive that preserves
numeric UID/GID, modes, links, and timestamps. Each archive has a manifest with
stable project/volume identity, driver, byte counts, and cryptographic checksum.
Setup verifies the archive before continuing.

The migration backup is encrypted with the board's RPI age identity and stored
in a root-owned mode-`0600` migration directory. This protects discarded or
copied archives from casual disclosure, not from a board-root compromise.

Rootless UID mapping is handled during restore: container UID 0 maps to the host
`rpi-agent` UID, and container UID `n >= 1` maps through the assigned subordinate
range. Restore runs inside the destination user namespace rather than blindly
chowning files from the host. The restored manifest and checksums are verified
before a project is started.

Projects start from validated immutable snapshots. Health checks and a database
smoke check where configured must pass before migration is declared operational.
The original rootful volumes, verified archives, old unit definition, and old
daemon configuration remain available for rollback at this stage. After the
rootless cutover passes, setup removes `rpi-agent` from the rootful Docker group;
the journal retains enough previous configuration for the privileged setup
command to restore that membership during rollback. The running rootless agent
never keeps simultaneous authority over both daemons.

### Failure, resumption, and finalization

Every journal step is idempotent and records intent before action and completion
after verification. Re-running setup resumes from the last verified boundary.

On failure after downtime, setup stops new rootless stacks, restores the previous
agent unit and endpoint, restarts the original rootful stacks, verifies their
health, and retains backups plus diagnostics. Crucially, membership in the
rootful Docker group and old configuration are not removed until rootless
validation succeeds.

`sudo rpi agent migration finalize` is a separate explicit cleanup. It rechecks
all project health and backup manifests, then may remove old managed rootful
volumes, migration archives, and obsolete access. It never removes foreign
Docker resources. Until finalization, `migration status` explains retained disk
usage and rollback availability.

## Runtime resource and exposure policy

Rootless Docker must use cgroup v2 with systemd delegation. A Docker cgroup
driver of `none`, ignored limit probe, or missing required controller is fatal.
Setup tests actual enforcement with a disposable container rather than trusting
configuration output alone.

The initial local defaults are:

```text
per-service default: memory 512 MiB, CPU 1.0, PIDs 256
per-service maximum: memory 2 GiB, CPU 4.0, PIDs 1024
```

Local root policy may lower defaults/maxima to fit the board. A project may
request values up to the maxima but cannot disable limits. Setup rejects defaults
that are impossible for the detected host and offers measured safe values.

Control-plane concurrency is bounded: one deploy per project, a small global
build limit, one command per project, a global command limit, bounded argv and
output, command deadlines, log-tail limits, SSE/client limits, request-body
limits, and authentication/enrollment rate limits. Exact numeric budgets belong
in the trust-plane and runtime implementation plans and are surfaced by
`rpi doctor`.

Deploy preflight accounts for build context, expected image, snapshot, backup,
and volume growth. A high disk watermark blocks new deploys. At the critical
watermark the agent stops the fastest-growing managed project if necessary to
protect the host, records the reason, and does not delete data. Logs and deploy
history rotate under bounds; images/build cache may be garbage-collected only
when not referenced. Volumes, secrets needed for rollback, and backups are never
automatic GC targets. Filesystem project quotas are used when supported;
otherwise accounting and emergency-stop limitations are disclosed by doctor.

Ingress is agent-owned. Repository Compose can expose container ports internally
but cannot publish them. LAN exposure requires an exact local policy permission
bound to project and snapshot digest. Database ports are never made public merely
because the repository requests them.

## Service hardening

The agent systemd service runs as `rpi-agent` with a minimal environment and
private umask. The target unit uses, where compatible with its explicit writable
paths:

```text
CapabilityBoundingSet=
NoNewPrivileges=yes
ProtectSystem=strict
PrivateTmp=yes
ProtectKernelTunables=yes
ProtectKernelModules=yes
ProtectKernelLogs=yes
ProtectControlGroups=yes
RestrictSUIDSGID=yes
LockPersonality=yes
UMask=0077
```

`ReadWritePaths` is limited to RPI state, the agent Unix socket directory, and
the `rpi-agent` rootless runtime paths. Additional systemd restrictions are added
only after integration tests prove rootless Docker access, health checks, and
updates still function. The rootless Docker user unit is hardened separately
because its kernel and filesystem needs differ from the API agent.

## Source and binary supply chain

### Project images

The audit release reports mutable Compose image tags and Dockerfile `FROM` tags.
`rpi security lock-images` resolves them and writes `rpi.lock` containing exact
registry digests tied to the source identity. The enforcement release requires
the lock or an explicit digest and verifies that normalized Compose/build inputs
match it. Registry credentials, if later supported, follow the secret boundary
and never enter Dockerfiles or snapshots.

### RPI releases

Release assets and `SHA256SUMS` carry Sigstore keyless attestations. The npm
postinstall verifies the asset digest plus expected repository and workflow
identity before execution or installation. Signature, identity, or digest
failure aborts with no unsigned fallback.

An intentional source-build fallback may use the npm package's verified
integrity and must run Cargo with `--locked`; it is reported distinctly from a
verified prebuilt asset. It never activates because prebuilt verification
failed.

### cloudflared

RPI does not download a `latest` URL. Each supported cloudflared version,
architecture, digest, and available upstream attestation is pinned in signed RPI
release metadata. Setup verifies all available trust material before atomically
installing the binary. There is no `curl | sh`, permissive checksum, or silent
unsigned path.

## Errors and remediation

Security failures use stable machine-readable codes. Initial codes include:

```text
AUTH_SIGNATURE_INVALID
AUTH_REPLAY_DETECTED
AUTH_AGENT_IDENTITY_CHANGED
COMPOSE_BIND_MOUNT_DENIED
COMPOSE_DOCKER_SOCKET_DENIED
COMPOSE_DIGEST_CHANGED
ROOTLESS_CGROUP_UNAVAILABLE
MIGRATION_BACKUP_VERIFY_FAILED
SOURCE_HOST_UNTRUSTED
SOURCE_COMMIT_NOT_EXACT
IMAGE_DIGEST_REQUIRED
```

The human message states what was blocked, why it is dangerous, the safe repair,
the local root-only permission command when an exception class is supported, and
the request ID. It must not suggest bypassing signature, policy, or ownership
checks.

## Verification strategy

### Unit and property testing

- canonical request bytes, Ed25519 verification, clock windows, nonce races,
  revocation, enrollment expiry, and agent challenge verification;
- Compose raw/effective policy rules, merge/profile/interpolation cases,
  snapshot canonicalization, digest rechecks, and local exception matching;
- path canonicalization and symlink/hard-link escape handling on supported
  filesystems;
- volume namespace stability, UID/GID mapping, resource limit clamping, disk
  accounting, and stable error codes;
- schema 1 migration and schema 2 service-scoped secret generation;
- property/fuzz tests for signed requests, Compose path forms, and the archive
  extraction primitives that can safely be introduced before the archive API.

### Malicious fixture corpus

Fixtures cover privileged containers, `/` mounts in all Compose syntaxes,
Docker/rootless sockets, host namespaces, devices, unconfined security options,
external/global volumes, port publication, Compose include/extends escapes,
Dockerfile/context escapes, symlink swaps, override replacement, secret
interpolation, oversized inputs, and digest changes between validation and run.
Each fixture asserts a specific error code and no Docker mutation.

### Integration and migration testing

- real rootless Docker on supported Debian/Raspberry Pi OS environments with
  cgroup enforcement probes;
- clean install with no rootful Docker present and with unrelated rootful Docker
  present;
- authenticated Unix-socket-over-SSH requests, concurrent nonces, revocation,
  tunnel port capture, strict SSH/Git host-key failure, and clock skew;
- PostgreSQL and representative file-volume migrations with row/data checks
  before backup, after rootless restore, and after rollback;
- fault injection before and after every migration journal boundary, including
  power-loss-style interruption and repeated setup;
- schema 2 rotation, failed health rollback, checkout/build-context absence, and
  log redaction;
- signed RPI asset and pinned cloudflared verification, including wrong digest,
  wrong workflow identity, missing attestation, and offline failure;
- disk high/critical watermark behavior and proof that GC never deletes volumes
  or backups.

Each of the five subprojects receives a dedicated security review. The breaking
release requires a full adversarial end-to-end run and a documented recovery
drill on representative board hardware.

## Delivery decomposition

### 1. Trust plane

Implement per-client identities, enrollment/revocation, canonical signed
requests, replay defense, agent challenge/pinning, audited request IDs, strict
SSH transport behavior, and the `PreparedSource` contract. The Git adapter in
this phase always records the resolved immutable commit; strict rejection of
non-SHA client inputs and the remaining Git supply-chain rules land in phase 4.
This can ship without changing the container runtime.

### 2. Compose and secrets audit release

Implement raw/effective Compose normalization and policy decisions, immutable
snapshots, legacy inventory and warnings, schema 2 secret storage/injection,
`rpi security audit`, and `rpi security migrate-config`. Existing installations
receive remediation before enforcement.

### 3. Rootless breaking migration

Implement supported-platform setup, the clean-install fast path, rootless Docker
and cgroup verification, automatic volume backup/restore, resumable journal,
rollback/finalization, removal of rootful Docker-group authority, policy
enforcement, and schema 1 rejection.

### 4. Supply-chain and runtime hardening

Implement exact Git SHA and strict known-host enforcement, image locks,
attested RPI/cloudflared downloads, resource/disk/concurrency policy, systemd
hardening, and ingress ownership.

### 5. Adversarial E2E and rollout

Complete malicious fixtures, rootless/migration/recovery matrices, board
hardware drills, operator documentation, release gates, telemetry-free audit
guidance, and the breaking-release checklist.

The implementation plans are written and executed in this order. A later plan
may depend on interfaces from an earlier one, but each subproject must end in a
shippable, testable state with its own compatibility notes and rollback story.

## Acceptance criteria

This program is complete only when all of the following are demonstrated:

- a Compose file that requests `/:/host`, a Docker socket, privileged mode, host
  namespaces, or arbitrary host ports is rejected before build or secret access;
- a successful container escape from normal rootless namespace permissions
  cannot yield host root through RPI's Docker authority;
- stealing one client seed can be stopped by revoking that client without
  rotating other clients or project Git keys;
- an attacker controlling a forwarded local port cannot pass the pinned agent
  challenge;
- a private repository key grants read access to only its associated project and
  can be independently rotated;
- deployed secrets are absent from checkout, build context, immutable snapshot,
  image history, and ordinary logs;
- a PostgreSQL database in a named volume survives deploy, package update,
  rootless migration, restart, and rollback with verified data;
- clean setup performs no backup or rootful migration steps;
- interrupted setup resumes or rolls back without deleting the original managed
  volumes, and finalization never touches foreign Docker resources;
- rootless/cgroup, signature, host-key, digest, and policy failures all fail
  closed with stable remediation errors;
- future archive upload can implement a new source provider without bypassing or
  duplicating the post-acquisition security pipeline.

## Explicit non-decisions for later designs

This spec deliberately does not choose an archive command/API, archive UX,
remote audit backend, hardware secret store, full-disk encryption procedure,
multi-tenant boundary, VM isolation layer, or build-time secret protocol. Adding
any of these changes the threat model or public behavior and requires its own
brainstorming/design.
