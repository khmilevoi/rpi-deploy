# Access gate for published projects (idea)

Status: potential feature; not an implementation specification. This document
records an exploration, including the options that were rejected and why.

## Goal

Make it possible to publish a project that was never designed to be public.
Turning on a flag in `rpi.toml` should put an authentication gate in front of
the deployed service, without the project having to implement authentication
itself.

The design target is ease of use: a one-time setup on the board, then a single
per-project switch.

## The distinction that shapes everything

Two problems are easy to conflate:

- **Gate** — who is allowed to reach the application at all. The application
  is unaware of it and does not change. This is what publishing a
  not-public-ready project actually needs.
- **Identity** — who exactly signed in, and what they may do. This lives
  inside the application by definition (roles, per-user records, revoking a
  specific device).

`rpi` can offer a unified gate. It cannot own identity without becoming an
identity provider, with a user database, account recovery, and a permanent
CVE-maintenance obligation.

A project already deployed by `rpi` (`myboard`) implements the identity half
by hand: nginx `auth_request` in front of everything except the activation
app and the auth API, sessions in Valkey, WebAuthn passkeys, device invites,
and revocation through `[commands]`. Its `[healthcheck]` deliberately expects
`401`, so the deploy is healthy only while the door is locked. That project is
the prior art this idea generalises — and it is also the example of a project
that would keep owning its own identity and switch the gate off.

## Options considered

### A. Cloudflare Access at the edge

`rpi` creates an Access application and policy for the hostname through the
Cloudflare API it already talks to.

- Nothing is added to the request path on the board: no CPU, no state, no
  backups. Free for a small number of users, email OTP out of the box,
  optional Google/GitHub identity providers. Unauthenticated traffic never
  reaches the home network at all.
- Costs: hard coupling to Cloudflare; the agent's API token needs wider
  scope (Access apps and policies), enlarging the blast radius if the board
  is compromised; a one-time manual Zero Trust org setup; non-browser
  clients (webhooks, mobile apps) need bypass paths or service tokens; the
  gate is bypassed entirely when the host port is reachable on the LAN; the
  login page is Cloudflare's, not the project's.

### B. A gate stack managed by `rpi` on the board

A reverse proxy plus a forward-auth service in front of the project's
container.

- Provider-agnostic: works for LAN exposure, and survives replacing
  `cloudflared` with something else. Full control of the login experience.
  Identity can be handed to the application. Recovery happens over SSH
  rather than through someone else's dashboard.
- Costs: `rpi` becomes responsible for a live authentication service —
  sessions, credential storage, brute-force protection, upgrades. It adds
  state that must be backed up, and a component that is a single point of
  failure for every project behind it.

### B-lite. A minimal first-party gate

One shared password or magic link, stateless HMAC cookie, a tiny image `rpi`
runs itself.

- Genuinely "turn it on with a flag", no external dependency, no user
  database.
- A shared password means no per-person revocation, and it is still
  first-party code in the request path that has to be maintained.

### C. Private network / VPN

Tailscale, WireGuard, or a Cloudflare Tunnel private network.

- The strongest option: there is no public surface to attack, it covers
  non-HTTP services too, and the application needs no changes.
- It does not solve the stated goal. A link cannot simply be shared with
  someone; every client must install a VPN client and be enrolled. Webhooks,
  link previews, and invite flows are impossible. **Rejected for this
  reason**, while remaining a reasonable separate `expose` mode.

### D. No core feature — publish a documented pattern

- Zero obligation for `rpi`, and the best possible integration for each
  application.
- Leaves the original problem unsolved: every project reimplements the gate,
  and a project that is not public-ready still cannot be published quickly.
  This stays available as the escape hatch for applications that need real
  in-app identity.

### E. Edge primitives without an identity provider

WAF rules, IP allow-lists, mTLS client certificates, basic auth in a Worker.

- Ten minutes of work.
- Basic auth is weak and unpleasant, IP lists break on mobile networks, and
  mTLS on phones is painful.

## Rejected: a plugin system

An earlier framing was to ship this as a plugin, which would require a plugin
system. Rejected, for reasons specific to this codebase:

- `crates/domain/src/contracts.rs` already provides compile-time seams —
  nineteen ports, with `Ingress` already having a real backend and a disabled
  one. An `AuthGate` port beside it costs approximately nothing and needs no
  new machinery.
- `docs/cli-philosophy.md` §6 treats CLI/agent version skew as a first-class
  case with dedicated compatibility handling. Plugins add a third axis to
  that matrix, on a device that updates itself unattended.
- The agent holds Docker access, every project's secrets, and the Cloudflare
  token. Runtime plugin code inherits all of it. (A project's own Compose
  file can already execute code on the board, so *declarative*,
  project-scoped extension adds little risk — but agent-scoped code adds a
  lot.)
- A plugin system pays off when third parties write plugins. With one or two
  authors it is an expensive way to write a `match`.

Comparable tools split along the same line: Coolify and CapRover ship
declarative service templates and no plugin system, Caddy's plugins are
compile-time modules requiring a rebuilt binary, Traefik extends through
declarative middleware, and only Terraform-scale ecosystems justify an
out-of-process plugin protocol. If three similar consumers ever appear
(auth, backup, metrics), a declarative "addon" mechanism — a validated Compose
fragment plus a schema fragment, with no code executed by the agent — is the
shape to revisit. Building it for a single consumer would almost certainly
get the abstraction wrong.

## Component research

Findings as of 2026-07, to be re-verified before implementation.

- **tinyauth** — forward-auth middleware for Traefik/nginx/Caddy, also an
  OpenID-certified provider in its own right. Injects `Remote-User`,
  `Remote-Name`, `Remote-Email`, `Remote-Groups`, `Remote-Sub`. Returns a
  plain `401` plus an `x-tinyauth-location` header for non-browser clients
  instead of redirecting. Supports subdomain SSO through a cookie domain
  setting. **No passkeys and no invites**: login methods are local
  username/password, TOTP, OAuth, LDAP, and Tailscale identity; users are
  `username:hash:totp` entries in an environment variable or file, created by
  its CLI. Stateful since v4 (SQLite sessions).
- **Pocket ID** — a passkey-only, OpenID-certified provider; a single Go
  binary with SQLite. Users are created by an admin, through invite links, or
  by open registration, and a one-time login code covers signing in from a
  device without the passkey. This is effectively the `myboard` model
  available off the shelf.
- Consequence: passkeys plus invites needs **two** components — Pocket ID as
  the identity source and a forward-auth gate (tinyauth or oauth2-proxy) as
  the enforcement point — plus a reverse proxy, because `cloudflared` cannot
  do forward auth. Heavier alternatives (Authelia, Authentik) trade the
  invite flow for more protocols and more configuration.

## Proposed shape, if the self-hosted gate is chosen

Ingress becomes `cloudflared -> reverse proxy -> project host port` instead of
`cloudflared -> project host port`. The gate stack is installed once at agent
level, in the same spirit as `rpi agent setup --with-cloudflared`; a project
then only carries a switch in `[ingress]`.

## Application-facing contract

Two tiers, with very different costs.

**Tier 1 — identity headers.** The gate already knows who signed in, so
passing `Remote-Email` and friends through costs almost nothing and gives an
application per-user behaviour with no authentication code.

**Tier 2 — real OIDC.** Both candidate components are certified OIDC
providers, so an application needing its own sessions, tokens, or API clients
performs an ordinary OIDC login against the same identity source. `rpi`'s only
role is injecting the issuer URL, registering the client, and delivering the
client secret through the existing secrets channel.

Tier 1 belongs in a first release because it is nearly free and settles the
question. Tier 2 should not be built, only left possible by the configuration
shape. A role model, user management commands, and an application SDK are out
of scope entirely.

## The custom-UI seam

There are three layers: verification, ceremony (sign in, register a passkey,
redeem an invite), and UI. A default UI ships, because that is the ease of use
the feature exists for. The customisation seam is **what happens on 401**, not
the ceremony: by default the user is redirected to the gate's login page; with
the UI disabled the application receives a real `401` (plus the sign-in
location in a header) and renders whatever it likes. This is exactly what
`myboard` does with `error_page 401 /activate/index.html` — the URL does not
change and the status stays `401`.

Offering "your own UI on top of our ceremony" is deliberately not promised.
With an off-the-shelf component, the ceremony API belongs to that component;
freezing it as an `rpi` contract would mean owning someone else's
compatibility. "Own your 401" is a one-line contract that does not break.

## Safety boundaries

- Header-based identity may be trusted only if the application is
  unreachable except through the gate. The gate must strip client-supplied
  `Remote-*` headers, and the application's port must not be published
  anywhere else. `rpi` can enforce this mechanically, because it owns host
  port allocation and the Compose override — enabling the gate should be
  incompatible with `expose = "lan"` rather than merely documented as unwise.
- The gate stands in front of its own administrative interface, so losing the
  only credential locks everything out. A break-glass path is mandatory, and
  `rpi command` over SSH is the right channel for it (`myboard` already does
  this with a one-time device token).
- Gate state — sessions and passkey credentials — needs a volume and a
  backup story; the `[commands]` backup pattern already covers this shape.
- A shared gate is a single point of failure for every project behind it, and
  adds images in the request path that must be kept patched.
- `[healthcheck]` with `expect = "401"` doubles as a test that the door is
  locked, and should be the documented default for gated projects.

## Non-goals

- Owning per-user application data, roles, or permissions.
- Becoming an identity provider, or shipping a first-party user database.
- Replacing an application's own authentication when it already has one.
- A general plugin or extension system.
- VPN-style private access as a way to satisfy this goal.

## Open questions

- Cloudflare Access (A) or a self-hosted gate stack (B). A trades control for
  zero runtime cost and zero state; B trades operational burden for
  provider independence, passkeys, invites, and SSH-based recovery.
- One gate for the whole board — a single auth hostname, cookie-domain SSO
  between projects, one place holding state and one place to fail — or a gate
  per project, with isolation, no cross-project SSO, and N databases. Ease of
  use argues for the former; per-project access lists argue for the latter.
- Whether an `AuthGate` port can express both A and B behind one
  application-facing contract, so the choice stays an implementation detail.
- How the gate interacts with the existing route stage and stable host port:
  what the ingress rule points at, and what happens to a gated project when
  the gate stack is absent or down.
- Which paths must bypass the gate by default, and how they are declared
  (webhooks, link previews, health endpoints).
- Whether a first release ships identity headers only, or the OIDC issuer
  wiring as well.
