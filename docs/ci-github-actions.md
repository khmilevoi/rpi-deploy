# CI: Deploy From GitHub Actions (v0.3)

`pi deploy` is CI-ready (§23 v0.3): the latest-wins queue handles two
consecutive pushes without retries, staged timeouts prevent jobs from hanging,
and the `--host/--user/--key` flags avoid requiring client config on the runner.

## Repository Secrets (Settings -> Secrets -> Actions)

| Secret | Description |
|---|---|
| `PI_HOST` | Pi host/IP reachable from the runner over SSH |
| `PI_USER` | Pi login user, not the `pi-agent` service user |
| `PI_SSH_KEY` | Private key; its public key must be in this user's `authorized_keys` |

## .github/workflows/deploy.yml

```yaml
name: deploy

on:
  push:
    branches: [main]

concurrency:
  group: deploy-production
  cancel-in-progress: true # saves minutes; the agent queue is still latest-wins

jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      # ... project tests ...

  deploy:
    needs: test
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - name: Install pi CLI
        # Binary releases + install.sh arrive in v0.5; for now, install from source.
        # Speed-up: use actions/cache for ~/.cargo/bin keyed by the tool revision hash.
        run: cargo install --git https://github.com/khmilevoi/pi --locked pi

      - name: Prepare SSH
        run: |
          mkdir -p ~/.ssh
          ssh-keyscan -H "${{ secrets.PI_HOST }}" >> ~/.ssh/known_hosts
          install -m 600 /dev/null ~/.ssh/deploy_key
          printf '%s\n' "${{ secrets.PI_SSH_KEY }}" > ~/.ssh/deploy_key

      - name: Deploy
        run: |
          pi deploy \
            --ref "$GITHUB_SHA" \
            --host "${{ secrets.PI_HOST }}" \
            --user "${{ secrets.PI_USER }}" \
            --key ~/.ssh/deploy_key
```

## Why This Shape

- **`--ref "$GITHUB_SHA"`** deploys the exact tested commit, not "main at deploy time".
- **`ssh-keyscan` is required**: the SSH tunnel runs with `BatchMode=yes`, so
  without known_hosts the connection fails on host key verification.
- **Two consecutive pushes**: the first deploy can finish as `superseded`; the
  CLI exits with code 0 and the job stays green (latest wins, §8.1). Red
  statuses are `failed`, `canceled`, and `interrupted`.
- **Project secrets** are not sent from CI on every deploy: the bundle is
  already stored on the Pi (`pi env send` is run manually when values change, §10).
- **A stuck build** is killed by the agent's staged timeout (`timeout: build`,
  default 30 minutes), so the job fails with a clear reason instead of the
  runner timeout.
- **CI cancellation is unnecessary**: a new push supersedes the queued deploy by
  itself; use `pi deploy --cancel` from a workstation for manual cancellation.
