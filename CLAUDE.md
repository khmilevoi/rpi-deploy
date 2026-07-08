# Project rules

## Before finishing any task

Run these before considering a change complete (matches what CI checks on Linux — a mismatch here is a guaranteed CI failure):

```bash
rtk cargo fmt --all -- --check
rtk cargo clippy --all-targets --locked -- -D warnings
rtk cargo test --locked
```

If `cargo fmt --all -- --check` reports a diff, run `rtk cargo fmt --all` and commit the result — do not hand-edit formatting.
