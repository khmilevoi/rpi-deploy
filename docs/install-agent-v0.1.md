# Install Agent Notes

## v0.4: logs and diagnostics

Add this block to `/etc/pi/agent.toml` when you want explicit rolling-log
settings:

```toml
[logs]
dir = "/var/log/pi"        # rolling files pi-agent.log.YYYY-MM-DD
retention_days = 14        # older files are removed on agent startup
```

New CLI commands: `pi logs <project> [-f]`, `pi stats [project]`,
`pi start|stop|restart <project>`, `pi rm <project> [--volumes]`, `pi status`,
`pi doctor`, `pi agent status`, `pi agent logs [-f] [--since 2h]`.

If the agent API is unavailable, `pi agent status|logs` falls back over SSH to
`systemctl status pi-agent` / `journalctl -u pi-agent`.

The `/var/log/pi` directory must be writable by `pi-agent`:

```bash
sudo install -d -o pi-agent -g pi-agent /var/log/pi
```

## v0.5: setup automation

`pi agent setup` now creates `/var/log/pi` (owner `pi-agent`) automatically, so
the rolling agent logs activate without the manual `install -d` step above.
Re-run `sudo pi agent setup` on an existing install to repair a missing
`/var/log/pi`, then `sudo systemctl restart pi-agent`.
