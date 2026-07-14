# Runtime configuration metadata (idea)

Status: potential feature; not an implementation specification.

## Goal

Make selected, non-secret values from `rpi.toml` available to a deployed
application without repeating them in a Compose file or a secrets `.env` file.
For example, an application may read `RPI_INGRESS_HOSTNAME` to construct its
canonical public URL.

## Proposed v1 behaviour

At deploy time the agent adds the following environment variables to the
configured `[ingress].service` through its generated Compose override:

```text
RPI_PROJECT_NAME
RPI_INGRESS_SERVICE
RPI_INGRESS_PORT
RPI_INGRESS_HOSTNAME
```

`RPI_INGRESS_HOSTNAME` is not set when `[ingress].hostname` is omitted. This
makes a private/worker deployment distinguishable from a deployment whose
hostname happens to be an empty string.

The variables are normal runtime environment variables. They are available to
the application and to `rpi command` entries that run in that service. They
are not supplied while building images and do not modify the repository's
`docker-compose.yml`.

Example input:

```toml
[project]
name = "myapp"

[ingress]
hostname = "app.example.com"
service = "web"
port = 3000
```

The running `web` container receives:

```text
RPI_PROJECT_NAME=myapp
RPI_INGRESS_SERVICE=web
RPI_INGRESS_PORT=3000
RPI_INGRESS_HOSTNAME=app.example.com
```

## Safety boundaries

- This is a fixed allow-list, not a mechanism to export arbitrary `rpi.toml`
  fields.
- Values that can contain credentials or reveal agent internals are never
  exported: notably `source.repo`, secret-file paths, secret values, agent
  paths, host ports, and ingress-provider credentials.
- `RPI_*` is reserved for rpi-generated metadata. A secret bundle or user
  Compose configuration must not be able to silently override these names;
  conflicting secret keys should fail validation with a clear error.
- The generated override must serialize YAML values safely rather than splice
  untrusted strings into YAML text.
- The first release does not interpolate `$VARS` inside `rpi.toml` or provide
  them to arbitrary Compose interpolation. That would make deployment depend
  on the CLI/CI process environment and make resolution less reproducible.

`ingress.hostname` is public routing data already used by the ingress layer,
so exposing it to the workload does not create a new secrecy boundary. A
repository that controls its Compose configuration can already execute code on
the Pi; the main additional risk is accidentally exporting secrets, which the
allow-list prevents.

## Scope and future options

V1 targets only `[ingress].service`, matching the service rpi already owns and
for which it already writes an override. A future opt-in mechanism could make
the metadata available to selected additional Compose services, but it should
not default to every container in a stack.

New metadata names may be added later. Existing `RPI_*` names must retain
their documented meaning for compatibility.

## Non-goals

- General template syntax or environment-variable expansion in `rpi.toml`.
- Passing secrets through this metadata channel.
- Replacing `[secrets]` or Compose's own application configuration.
- Making agent-specific or network-topology values part of the public
  application contract.
