use std::process::Command;

/// `output::success` messages are decorative human-facing status ("done",
/// "saved", "removed"), never machine-readable data — so they must go to
/// stderr like error/warn/note, leaving stdout empty for scripts that do
/// `rpi <cmd> 2>/dev/null` expecting silence on success.
#[test]
fn init_success_message_goes_to_stderr_not_stdout() {
    let dir = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_rpi"))
        .args([
            "init",
            "--name",
            "demo",
            "--repo",
            "git@example.com:x.git",
            "--branch",
            "main",
            "--compose",
            "docker-compose.yml",
            "--service",
            "web",
            "--port",
            "3000",
            "--yes",
        ])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stdout.contains("wrote"),
        "success message leaked into stdout: {stdout:?}"
    );
    assert!(
        stderr.contains("wrote"),
        "success message missing from stderr: {stderr:?}"
    );
}
