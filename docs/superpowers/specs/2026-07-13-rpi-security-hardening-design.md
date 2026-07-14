# RPI security hardening program

Date: 2026-07-13 (reconciled with the codebase and re-approved 2026-07-14)
Status: approved design; implementation planning pending

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

## Reconciliation with current code

This target was rechecked against `master` at `e43c93b`. It is intentionally not
a description of current behavior. The relevant current gaps are:

- `crates/bin/src/agent/http.rs` registers sensitive routes without an
  authentication/authorization middleware;
- `crates/bin/src/cli/tunnel.rs` reserves then releases a TCP port before
  starting `ssh -L`, leaving a local bind race and a Windows portability issue;
- setup runs the agent against rootful Docker, adds `rpi-agent` to the `docker`
  group, and the cleanup fallback can mount a host parent into a root container;
- Compose is passed to Docker without a normalized security policy and
  automatically incorporates repository override files; lifecycle operations
  re-read the mutable checkout;
- decrypted secrets are written into that checkout before build;
- Git already has a useful `Source` seam and per-project key, but URL/protocol,
  host-key, key-directory mode, repository-change, and immutable-snapshot rules
  are incomplete;
- named volumes already survive ordinary removal, but there is no durable
  project/volume tombstone registry;
- migration has an applied-ID ledger, not a root-owned resumable backup/restore
  journal;
- `rpi upgrade`, `rpi agent update`, and `scripts/install.sh` now exist, but they
  implement the superseded SHA256/GitHub-TLS design, support mutable production
  base URLs and source/npm fallbacks, and do not provide Sigstore or
  anti-downgrade verification;
- the release workflow uses mutable Action/toolchain references, while
  cloudflared installation follows an unpinned `latest` asset without a release
  signature check.

Existing encrypted age storage, path/symlink guards, deploy serialization,
timeouts, body/secret size limits, disk GC threshold, self-install primitive,
and source abstraction are retained where they satisfy the new invariants.

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
- shipping archive upload in the first hardening program. Its two-phase
  staging/commit contract is fixed here so a later implementation cannot bypass
  source, policy, quota, or authentication checks.

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
5. The client authenticates the agent as well as the SSH server; there is no
   local forwarded TCP port to capture or race.
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

Initial board installation and a manual package upgrade use:

```text
npm install -g <rpi-package>
sudo rpi agent setup
```

`setup` installs or updates the unit and restarts it when required. It is safe to
run repeatedly. An update that does not require a migration still performs
preflight, reconciles configuration, and returns quickly. The user does not run
both `setup` and `run`.

Client enrollment is a separate, one-time client-side operation:

```text
rpi setup
rpi setup --host user@host --host-fingerprint SHA256:<fingerprint>
```

Without `--host`, the wizard asks for `[user@]host`. It pins the SSH host key,
creates the client API identity locally, invokes the fixed privileged
registration command over SSH, saves the board profile, and verifies the stdio
proxy. It is not repeated after agent upgrades.

Once a board supports remote updates, the normal update path is:

```text
rpi upgrade --server <profile> [--version <exact-or-latest>]
```

The client resolves `latest` before confirmation and the privileged step always
receives an exact version and a previously verified staged artifact.

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
rpi source-host list --server <profile>
rpi upgrade --server <profile>
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
private seed is generated and stored on that client with owner-only permissions
or a Windows owner-only ACL. The board stores only its public key, stable client
ID, label, scopes, creation time, and revocation state. Client API identities are
distinct from per-project Git deploy keys.

The first `rpi setup` registers its public key through a fixed SSH/TTY command
that invokes `sudo rpi agent client add --owner --public-key <strict-base64>`.
The non-secret key uses a validated fixed-alphabet argument, leaving the TTY
stdin exclusively to sudo. There is no unauthenticated enrollment endpoint,
temporary public mode, or private-key transport. The board-side
`sudo rpi agent setup` prints its Ed25519 SSH host fingerprint; the client
requires that independently obtained fingerprint before pinning an unknown
host.

The first identity is the board owner. Additional CI identities are created
with explicit project IDs and capabilities such as `deploy`, `logs`, or
`secrets:write`. New CI identities are least-privileged by default. Revocation
does not rotate other API identities or Git deploy keys. Source-host policy,
dangerous Compose exceptions, downgrade permission, and other host policy are
never writable through the normal agent API, including by an owner identity.

### Request authentication

Every authenticated request signs a canonical byte representation containing:

```text
protocol-version
session-id
agent-challenge
client-id
HTTP method
normalized path and query
timestamp
nonce
SHA-256(request body)
```

The agent verifies the signature, client scope, session binding, timestamp, and
body digest, then atomically rejects an already-seen nonce for that identity.
Nonce records live at least through the accepted timestamp window and are
bounded per client. Streaming logs are signed when opened; stream lifetime and
event budgets are separately bounded.

Archive uploads use a two-phase exception to the up-front body digest rule. A
signed request creates a short-lived upload session bound to one client,
project, maximum size, and nonce. The agent hashes bytes while writing only to
staging. A second signed commit request supplies the final size and digest; no
archive is unpacked before both values match. Incomplete sessions are inert and
garbage-collected.

Only a minimal health/version/capability handshake is available before request
authentication. It contains no project or host details. Enrollment remains a
privileged SSH/sudo operation rather than an API route.

### Agent authentication and SSH transport

The agent owns a separate Ed25519 signing identity. `rpi setup` pins its public
key into the board profile after the privileged registration step. On every new
control connection the client requires a signature over the fresh session ID
and challenge before sending project requests.

Production API transport remains the Unix socket. The CLI starts the fixed,
root-owned remote command `/usr/local/bin/rpi agent proxy`; that unprivileged
command copies SSH stdin/stdout to `/run/rpi/agent.sock`. HTTP travels over the
child process streams. No local or remote TCP listener, port selection, or
probe-close-rebind sequence exists. SSH uses strict host-key checking, a
dedicated managed `known_hosts`, `ForwardAgent=no`, and the pinned board key.

`PI_AGENT_URL` is a development facility. Release builds accept it only for a
Unix socket or an explicitly enabled test build. A production agent refuses TCP
listeners, including loopback TCP.

The handshake exchanges `protocol_min`, `protocol_max`, and capabilities. Normal
operations require an overlap and sign the selected version. At least the
previous protocol minor remains supported for update orchestration. With no
overlap, only minimal health/version and the stable upgrade staging capability
remain available; deploy, secrets, lifecycle, logs, and command operations fail
with an upgrade instruction. There is no fallback to an unsigned or less
authenticated request format.

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

The request may name an exact commit or a configured branch/tag. The agent
resolves a moving ref once, fetches that exact object, and records provenance as
`git:<canonical-repo>@<sha>`. From that point through lifecycle operations only
the resolved SHA and immutable snapshot are used. A root policy may require an
exact client-supplied SHA or a signed commit for higher-assurance projects.

When a Git deployment originates from a local checkout, the CLI rejects
uncommitted or untracked source changes because they are not represented by the
authorized commit. Sending local-only content belongs to the future signed
archive provider, not to a permissive Git flag in production.

Git accepts only `ssh` and credential-free `https` URLs whose host is in the
root-owned allowlist. It rejects `http`, `git://`, `file://`, local paths,
userinfo/tokens, option-like repository arguments, unsafe redirects, and
unexpected access to loopback/private/link-local addresses. The Git child uses
a sealed environment: terminal prompts and credential helpers are disabled,
system/user configuration is ignored, and only explicitly allowed protocols are
enabled.

Git SSH never uses `StrictHostKeyChecking=accept-new`. Built-in provider keys are
shipped as signed release data. Additional hosts are registered with:

```text
rpi source-host add git.example.com --server <profile> \
  --ssh-fingerprint SHA256:<fingerprint> [--allow-private-network]
```

The client invokes a fixed SSH/sudo board command. It scans the full key only to
compare it with the independently obtained fingerprint, then atomically stores
the key and root-owned policy. List, remove, and key-rotation commands use the
same privileged path. A changed key fails closed. Remote API identities cannot
expand this list.

Private repositories receive one read-only Ed25519 deploy key per RPI project.
Public HTTPS repositories do not get a key. Private keys live outside checkouts
under `/var/lib/rpi/keys/<stable-project-id>/`, with directory mode `0700` and
file mode `0600`. CLI flows show, rotate, and revoke the public key. `rpi rm`
removes the board's local key; when GitHub CLI is available locally, the CLI may
offer to remove the remote deploy key without ever sending a GitHub token to the
board.

A local policy may additionally require signed commits and an allowed signer.
This is independent of SSH transport and per-project repository authorization.

Every deployment uses a fresh staging directory. Changing a project's repository
cannot accidentally reuse an old checkout or its `origin`; successful staging is
promoted atomically to a content-addressed snapshot. Key directories are mode
`0700` and key files are mode `0600` before Git starts.

### Future archive provider

This program implements the common contract and Git provider; archive shipping
is deferred, but its protocol boundary is fixed. A later client creates a signed
upload session through the same SSH stdio proxy, streams `tar.zst` without
loading it wholly into memory, and finishes with a separately signed commit
containing final size and SHA-256. The agent computes both values independently
while writing to private staging.

The agent enforces compressed/uncompressed/file-count/time quotas and rejects
absolute paths, `..`, device nodes, FIFOs, hard links, and symlinks that can
escape staging. It does not extract before commit verification. Incomplete or
expired sessions are inert GC targets. Provenance is
`archive:sha256:<digest>`.

Archive trust uses the client signature plus content digest. It does not require
a Git deploy key, Git known-host entry, or commit signature. After preparation,
the archive follows exactly the same Compose, secrets, resource, and health
pipeline as Git.

## Compose policy and immutable execution

### Validation pipeline

Each deployment runs these stages in order:

1. Acquire an exact immutable `PreparedSource`.
2. Clear `COMPOSE_FILE`, project `.env`, credential, and ambient interpolation
   inputs; select only the explicitly declared project Compose file.
3. Scan raw Compose before interpolation, then resolve anchors, merges,
   variables, and profiles in a sealed environment containing no secret values.
4. Canonicalize paths and reject symlink or traversal escapes.
5. Validate the complete normalized effective model against local policy.
6. Inject agent-owned resource, ingress, namespace, volume, and secret controls.
7. Serialize one final normalized snapshot, atomically store it, and compute its
   digest.
8. Build from the immutable source with no runtime secret generation present.
9. Materialize the selected secret generation outside the source tree, then
   create/start from the stored snapshot after rechecking its digest.
10. Run health checks, atomically select the active snapshot/generation, and
    append the audit result.

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
  labels, secret targets, Compose project name, or snapshot files;
- implicit repository `docker-compose.override.yml`, ambient `.env`, `COMPOSE_*`
  environment, or undeclared Compose file chains;
- project/service/volume/command identifiers that fail anchored length/character
  rules, begin with `-`, contain path/control syntax, or could be parsed as a
  Docker/Compose option.

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
- resource requests that do not exceed local maxima;
- an agent-generated read-only root filesystem, bounded writable `tmpfs`, and
  explicitly declared managed named-volume targets;
- normal internet egress through the agent-generated, digest-pinned project
  gateway; access to host, private, and link-local networks remains denied.

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

Fresh installations enforce the complete policy immediately. Existing
installations run the same engine as a mandatory migration preflight. Current
containers are not stopped merely to produce the report, but every new deploy
is evaluated in enforce mode, so the audit cannot be used to introduce a new
dangerous capability.

The preflight emits machine-readable findings and an exact safe remediation for
every rule. Rootless cutover starts only after the inventory is clean or every
exemptible violation has a local root-owned exception bound to the exact project
and policy digest. Non-exemptible violations never receive a compatibility
allowance.

## Persistent databases and volumes

Databases such as PostgreSQL may remain on the board. Their data belongs in a
project-namespaced Docker named volume, not a host bind such as
`/srv/postgres:/var/lib/postgresql/data`.

Named volumes survive deploys, restarts, agent/package upgrades, image changes,
and ordinary `rpi rm`. Ordinary removal deletes containers, source snapshots,
runtime/encrypted secrets, the local Git deploy private key, and the project
grant from scoped CI identities, but retains a minimal tombstone: stable project
ID, name reservation, volume inventory, and audit references. It does not revoke
an identity that still has grants to other projects. Only an owner may revive
the tombstone; the same name reconnects the same volumes instead of silently
creating an empty database.

Volumes and the tombstone are deleted only by the explicit destructive command
`rpi rm --volumes`, which lists affected volumes and requires the existing
confirmation contract. Garbage collection never removes named volumes or an
unexpired migration backup.

Application rollback switches source/configuration snapshots but never rewinds
a live volume. A project that needs database rollback declares an explicit
backup hook, volume, and retention policy; PostgreSQL/MySQL use native consistent
backup tools rather than copying live files. Restore is a separate confirmed
operation. A failed health check never automatically overwrites newer database
rows.

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

The agent stores encrypted secret generations outside source trees. Only after
the image build succeeds does it decrypt the selected generation into
`/run/rpi/secrets/<project-id>/<generation>/`, a private tmpfs-backed runtime
directory. Environment values are injected only into declared services by an
agent-generated `env_file`; file secrets are exposed only to declared services
as read-only Compose secrets at declared targets. The normalized snapshot stores
trusted paths and generation identity, never values.

Secret values are unavailable to Compose interpolation, Dockerfile `ARG`/`ENV`,
image build contexts, normalized snapshots, and ordinary logs. Build-time
secrets are rejected in this program and require a separate design.

Applying secrets is generation-based and atomic: stage a new encrypted/runtime
generation, deploy and health-check it, then retire the old runtime generation.
A failed health check returns to the previous usable generation.

On stop/removal, unused plaintext generations are deleted. After reboot the
encrypted canonical bundle remains; the agent rematerializes required tmpfs
generations before allowing Docker to start managed services. Docker restart
policy cannot race ahead of that ordering.

The security boundary is stated honestly: the `rpi-agent` account and its
rootless Docker daemon can access runtime secrets, and compromise of both the
encrypted store and its age identity permits decryption. Schema 2 prevents
accidental build/repository leakage; it is not a hardware secret vault.

### Schema 1 transition

Migration preflight detects all schema 1 configurations. `rpi security
migrate-config` automatically rewrites unambiguous cases and prints an exact
schema 2 snippet for ambiguous service scope or target paths. It never guesses
which services should receive a secret. Fresh installations accept only schema
2; existing cutover is blocked until audit proves no managed project depends on
legacy `.env` or secret files being written into its source tree.

## Rootless installation and migration

### Fresh installation fast path

`sudo rpi agent setup` first classifies the host. With no managed rootful RPI
installation it uses the clean path:

1. Validate Raspberry Pi OS/Debian version, systemd, cgroup v2, disk, and
   architecture.
2. Show exact missing package versions and, after confirmation, install them
   only from already configured signed APT repositories. Setup never adds a
   repository/key and never executes a downloaded shell script.
3. Create `rpi-agent`, its home `/var/lib/rpi`, access group, and private state
   directories.
4. Allocate non-overlapping `/etc/subuid` and `/etc/subgid` ranges of at least
   65,536 IDs.
5. Install and verify rootless Docker for `rpi-agent`, enable lingering, and
   enable its user service.
6. Prove the daemon is rootless and all required cgroup resource controls work.
7. Install the hardened agent system service and production Unix socket.
8. Create the agent signing identity, start the agent, and print the board's
   Ed25519 SSH host fingerprint for later client setup.

It does not create a backup, migration journal, maintenance window, or rootful
Docker dependency. Raspberry Pi OS 32-bit receives an explicit compatibility
check because upstream Docker Engine v29+ no longer publishes official armhf
packages.

Ambiguous partial RPI state fails before package or service mutation and prints
the conflicting paths/units. Unrelated rootful workloads are never migrated;
their presence requires the same explicit local
`--allow-rootful-coexistence` decision used by migration preflight.

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

Running or retained rootful resources that are not in the RPI registry block
the migration before downtime. A local interactive root operator may choose
`--allow-rootful-coexistence`; this never makes those resources managed. Even in
coexistence mode both `rpi-agent` and the SSH login user are removed from the
rootful `docker` group, and the RPI runtime can open only its rootless socket.

If preflight fails, no managed stack is stopped and no active configuration is
changed. Migration normally needs free space for both a backup copy and a
restored rootless copy while the original data remains intact; `--dry-run`
reports the measured requirement.

After successful preflight setup displays the inventory, measured space,
expected downtime, backup destination, and rollback plan, then requires one
confirmation. `--yes` supports deliberate automation; it does not bypass any
security or capacity check.

### Maintenance, backup, and restore

After successful preflight, setup opens a maintenance window and gracefully
stops all managed stacks. For every managed local named volume it uses a pinned,
digest-addressed helper image to create an encrypted archive that preserves
numeric UID/GID, modes, links, and timestamps. Each archive has a manifest with
stable project/volume identity, driver, byte counts, and cryptographic checksum.
Setup verifies the archive before continuing.

The migration backup is streamed directly into encrypted archives. A separate
root-only recovery identity at `/var/lib/rpi-migration/recovery.key` is created
with mode `0600`; the backup is not encrypted solely to the `secret.key` that it
contains. The backup includes a consistent `state.db` plus WAL state, the agent
age identity, encrypted bundles, Git deploy keys, agent/Cloudflare
configuration and credentials, relevant systemd units, manifests, and every
managed local named volume. `--backup-dir` may name an external mounted
filesystem. No intermediate plaintext archive is written.

Rootless UID mapping is handled during restore: container UID 0 maps to the host
`rpi-agent` UID, and container UID `n >= 1` maps through the assigned subordinate
range. Restore runs inside the destination user namespace rather than blindly
chowning files from the host. The restored manifest and checksums are verified
before a project is started.

Projects start from validated immutable snapshots. Health checks and a database
smoke check where configured must pass before migration is declared operational.
The original rootful state, verified archives, old unit definition, and daemon
configuration remain available for rollback at this stage. After the cutover
passes, setup removes both RPI runtime and login-user access to the rootful
socket. The running rootless agent never keeps simultaneous authority over both
daemons.

### Failure, resumption, and finalization

Every journal step is idempotent and records intent before action and completion
after verification. Re-running setup resumes from the last verified boundary.

On failure after downtime, setup stops new rootless stacks, restores the previous
agent unit and endpoint, restarts the original rootful stacks, verifies their
health, and retains backups plus diagnostics. Crucially, membership in the
rootful Docker group and old configuration are not removed until rootless
validation succeeds.

The encrypted rollback backup is retained for seven days and at least three
successful agent/project startup checks. A root-owned retention setting may
extend the window. Automatic cleanup rechecks health and manifests, removes only
old managed RPI state, and never touches foreign Docker resources. Until cleanup,
`migration status` explains retained disk usage and rollback availability.

## Runtime resource and exposure policy

Rootless Docker must use cgroup v2 with systemd delegation. A Docker cgroup
driver of `none`, ignored limit probe, or missing required controller is fatal.
Setup tests actual enforcement with a disposable container rather than trusting
configuration output alone.

Root-owned policy defines a board-wide memory budget, OS reserve, CPU/PID
ceilings, build/command concurrency, disk reserve, and per-service defaults.
Projects may request lower or different values within the remaining budget but
cannot disable limits or exceed a local maximum. The agent checks aggregate hard
memory limits before mutation; CPU may be deliberately shared but remains capped
per service. Conservative defaults are derived from measured hardware, with one
concurrent image build by default on Raspberry Pi. Audit reports the calculated
values before migration.

Control-plane concurrency is bounded: one deploy per project, a small global
build limit, one command per project, a global command limit, bounded argv and
output, command deadlines, log-tail limits, SSE/client limits, request-body
limits, and authentication/staging rate limits. Exact numeric budgets belong
in the trust-plane and runtime implementation plans and are surfaced by
`rpi doctor`.

Remote command requests name a command declared in the active immutable
snapshot; they do not supply an arbitrary shell string. Service identifiers and
argv are schema-validated, subprocess builders insert an option terminator where
supported, and project-configured timeouts are clamped to root-owned maxima.

Deploy preflight accounts for build context, expected image, snapshot, backup,
and volume growth. A high disk watermark blocks new deploys. At the critical
watermark the agent stops the fastest-growing managed project if necessary to
protect the host, records the reason, and does not delete data. Logs and deploy
history rotate under bounds; images/build cache may be garbage-collected only
when not referenced. Volumes, secrets needed for rollback, and backups are never
automatic GC targets. Filesystem project quotas are used when supported;
otherwise accounting and emergency-stop limitations are disclosed by doctor.

Ingress is agent-owned. Repository Compose can expose container ports internally
but cannot publish them. The agent allocates a collision-free port from a
root-owned range and binds it only to `127.0.0.1`; Cloudflared/Caddy connects to
that registry entry. LAN exposure requires an exact local policy permission
bound to project and snapshot digest. Database ports are never made public merely
because the repository requests them.

Project networks are internal and receive an agent-generated, digest-pinned
egress gateway. Ordinary internet egress is allowed, while host loopback,
RFC1918/private, link-local, and metadata destinations are denied by default.
Git fetch is outside this container path and follows the source-host policy.
Application access to one LAN endpoint requires a local root exception bound to
project ID, destination, port, and policy digest. A remote client cannot widen
egress.

The trusted service overlay sets the root filesystem read-only. Every writable
path must be a bounded tmpfs or an explicitly registered named-volume target.
The policy denies a project that cannot declare the paths it needs.

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

Migration preflight reports mutable Compose image tags and Dockerfile `FROM`
tags. `rpi security lock-images` resolves them and writes `rpi.lock` containing
exact registry digests tied to the source identity. Fresh/enforced deployments
require the lock or an explicit digest and verify that normalized Compose/build
inputs match it. Registry credentials, if later supported, follow the secret
boundary and never enter Dockerfiles or snapshots.

### RPI releases

Every release artifact carries a Sigstore keyless bundle. Verification pins the
expected repository/workflow certificate identity and GitHub Actions OIDC issuer,
checks artifact digest, certificate validity, transparency evidence, platform,
and exact version, and fails closed.

npm distribution uses provenance-bearing platform-specific packages containing
the binary and its bundle. The meta package selects by OS/architecture; install
does not download a GitHub asset, install rustup, or build from source. There is
no fallback from signature/platform failure to an unsigned or locally compiled
binary.

`rpi upgrade` first verifies release metadata and the artifact on the client,
stages them through the authenticated stdio proxy, then invokes a separate fixed
SSH/TTY `sudo rpi agent update --apply-staged <opaque-id>` command. The root step
opens only the private staging directory, rejects links/unsafe ownership, and
performs the complete Sigstore verification again before atomic replacement and
the shared setup/migration state machine. A root-owned monotonic version floor
prevents remote downgrade. See
`2026-07-13-rpi-remote-agent-update-design.md` for the complete transaction.

Release CI pins third-party Actions by immutable commit SHA, pins the Rust/Node
toolchains and dependency locks, and publishes npm provenance. Production builds
do not honor a release base-URL environment override; tests inject a verifier and
fixture source through test-only code.

### cloudflared

RPI does not download a `latest` URL. Each supported cloudflared version,
architecture, digest, and upstream trust material is pinned in signed RPI
release metadata or installed from an already configured signed package source.
Setup verifies trust material before atomic installation. The migration backup
helper and egress gateway image follow the same exact-digest/signature rule.
There is no `curl | sh`, permissive checksum, or silent unsigned path.

## Errors and remediation

Security failures use stable machine-readable codes. Initial codes include:

```text
AUTH_SIGNATURE_INVALID
AUTH_REPLAY_DETECTED
AUTH_AGENT_IDENTITY_CHANGED
AUTH_SCOPE_DENIED
PROTOCOL_NO_OVERLAP
COMPOSE_BIND_MOUNT_DENIED
COMPOSE_DOCKER_SOCKET_DENIED
COMPOSE_DIGEST_CHANGED
CONFIG_IDENTIFIER_INVALID
ROOTLESS_CGROUP_UNAVAILABLE
MIGRATION_BACKUP_VERIFY_FAILED
SOURCE_HOST_UNTRUSTED
SOURCE_URL_DENIED
SOURCE_ARCHIVE_INVALID
IMAGE_DIGEST_REQUIRED
UPDATE_SIGNATURE_INVALID
UPDATE_DOWNGRADE_DENIED
```

The human message states what was blocked, why it is dangerous, the safe repair,
the local root-only permission command when an exception class is supported, and
the request ID. It must not suggest bypassing signature, policy, or ownership
checks.

## Verification strategy

### Unit and property testing

- canonical request bytes, Ed25519 verification, session binding, clock windows,
  nonce races, revocation/scopes, protocol negotiation, and agent challenge
  verification;
- Compose raw/effective policy rules, merge/profile/interpolation cases,
  snapshot canonicalization, digest rechecks, and local exception matching;
- path canonicalization and symlink/hard-link escape handling on supported
  filesystems;
- volume namespace stability, UID/GID mapping, resource limit clamping, disk
  accounting, and stable error codes;
- schema 1 migration and schema 2 service-scoped secret generation;
- property/fuzz tests for signed requests, Compose path forms, source URL
  parsing, two-phase upload state, and archive extraction primitives.

### Malicious fixture corpus

Fixtures cover privileged containers, `/` mounts in all Compose syntaxes,
Docker/rootless sockets, host namespaces, devices, unconfined security options,
external/global volumes, port publication, Compose include/extends escapes,
Dockerfile/context escapes, symlink swaps, override replacement, secret
interpolation, option-like service/command identifiers, oversized inputs, and
digest changes between validation and run. Each fixture asserts a specific
error code and no Docker mutation.

### Integration and migration testing

- real rootless Docker on supported Debian/Raspberry Pi OS environments with
  cgroup enforcement probes;
- clean install with no rootful Docker present and with unrelated rootful Docker
  present;
- authenticated Unix-socket-over-SSH-stdio requests, fake proxy/agent identity,
  concurrent nonces, scoped revocation, strict SSH/Git host-key failure, and
  clock skew;
- PostgreSQL and representative file-volume migrations with row/data checks
  before backup, after rootless restore, and after rollback;
- fault injection before and after every migration journal boundary, including
  power-loss-style interruption and repeated setup;
- schema 2 rotation, failed health rollback, checkout/build-context absence, and
  log redaction;
- signed RPI/platform-package and pinned cloudflared/helper verification,
  including wrong digest, wrong workflow identity/issuer, missing bundle,
  downgrade, and offline bundle verification;
- disk high/critical watermark behavior and proof that GC never deletes volumes
  or backups.

Each of the five subprojects receives a dedicated security review. The breaking
release requires a full adversarial end-to-end run and a documented recovery
drill on representative board hardware.

## Delivery decomposition

### 1. Secure control plane and setup

Implement SSH stdio transport, agent challenge, per-client identities and
scopes, canonical signed requests, replay defense, protocol negotiation,
request IDs/audit, strict board host-key pinning, privileged public-key
registration, and the revised client-side `rpi setup`.

### 2. Immutable source and Compose enforcement

Implement `PreparedSource`, fresh Git staging and source-host policy, immutable
snapshots, explicit Compose file selection, raw/effective normalization,
policy decisions, exceptions, image locks, and malicious source/Compose
fixtures. Define the inert two-phase archive upload state machine without
shipping the archive user command.

### 3. Runtime data and rootless isolation

Implement schema 2 tmpfs secret generations, tombstoned volume registry,
read-only service roots, resource/disk/concurrency budgets, agent-owned ingress,
per-project egress isolation, rootless Docker setup/probes, and systemd
hardening. Fresh installs enforce this path immediately.

### 4. Resumable migration and recovery

Implement existing-install inventory, unmanaged-workload blockers, clean
preflight/confirmation, the independent recovery key, complete encrypted
backup/restore, volume UID mapping, fsynced journal, failpoint recovery,
rollback, retention, and automatic cleanup.

### 5. Signed distribution, remote update, and adversarial release gate

Implement provenance-bearing platform npm packages, Sigstore verification,
anti-downgrade, two-step staged `rpi upgrade`, pinned cloudflared/helper
artifacts, immutable CI/toolchain references, real rootless board E2E, migration
power-loss drills, documentation, and the final release checklist.

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
- no client command opens a forwarded local TCP port, and a fake stdio proxy
  cannot pass the pinned agent challenge;
- a private repository key grants read access to only its associated project and
  can be independently rotated;
- deployed secrets are absent from checkout, build context, immutable snapshot,
  image history, and ordinary logs;
- a PostgreSQL database in a named volume survives deploy, package update,
  rootless migration, restart, and rollback with verified data;
- clean setup performs no backup or rootful migration steps;
- interrupted setup resumes or rolls back without deleting the original managed
  volumes, and retention cleanup never touches foreign Docker resources;
- rootless/cgroup, signature, host-key, digest, and policy failures all fail
  closed with stable remediation errors;
- a two-phase archive upload cannot extract before signed commit verification
  and can implement a new source provider without duplicating the
  post-acquisition security pipeline;
- `rpi upgrade` rejects a wrong Sigstore identity/issuer, unsigned artifact,
  unsafe staged path, platform mismatch, and remote downgrade before replacing
  the installed binary.

## Explicit non-decisions for later designs

This spec deliberately does not ship the archive command/UX, remote audit
backend, hardware secret store, full-disk encryption procedure, multi-tenant
boundary, VM isolation layer, or build-time secret protocol. The archive
staging/commit security contract is decided; its eventual CLI surface still
requires a focused design. Adding the other items changes the threat model or
public behavior and requires its own brainstorming/design.
