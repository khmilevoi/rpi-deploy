use std::process::Stdio;
use std::sync::Arc;

use pi_domain::contracts::LogSink;
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::process::Command;

/// Like `run_streamed`, but a nonzero exit is data, not an error: returns the
/// exit code. `Err` is reserved for spawn/wait failures. Killed-by-signal
/// (no code) logs a line and maps to 1. Dropping the future kills the child.
pub async fn run_streamed_code(mut cmd: Command, log: Arc<dyn LogSink>) -> Result<i32, String> {
    let label = format!("{:?}", cmd.as_std());
    cmd.stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null());
    cmd.kill_on_drop(true);
    let mut child = cmd.spawn().map_err(|e| format!("spawn {label}: {e}"))?;

    let stdout = child.stdout.take().ok_or("child stdout not captured")?;
    let stderr = child.stderr.take().ok_or("child stderr not captured")?;
    tokio::join!(
        forward_lines(stdout, Arc::clone(&log)),
        forward_lines(stderr, Arc::clone(&log))
    );

    let status = child
        .wait()
        .await
        .map_err(|e| format!("wait {label}: {e}"))?;
    match status.code() {
        Some(code) => Ok(code),
        None => {
            log.line("process terminated by signal");
            Ok(1)
        }
    }
}

pub async fn run_streamed(cmd: Command, log: Arc<dyn LogSink>) -> Result<(), String> {
    let label = format!("{:?}", cmd.as_std());
    match run_streamed_code(cmd, log).await? {
        0 => Ok(()),
        code => Err(format!("{label} exited with code {code}")),
    }
}

/// Strips bare CR and ANSI CSI escape sequences (`ESC '[' ... final byte`)
/// from a captured subprocess line. Defense in depth: BuildKit and other
/// interactive-terminal-UI subprocess output uses these to redraw progress
/// in place; forwarding them through unchanged corrupts the cursor position
/// of whatever prints the line later. Not a full VT100 parser — CSI final
/// bytes are technically `@`-`~`, but every real-world use (colors, cursor
/// movement, line clearing) ends in an ASCII letter, so that's the cutoff.
pub fn sanitize_line(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' => out.clear(),
            '\u{1b}' if chars.peek() == Some(&'[') => {
                let mut consumed = String::from(c);
                consumed.push(chars.next().unwrap()); // consume '['
                let mut terminated = false;
                for c in chars.by_ref() {
                    consumed.push(c);
                    if c.is_ascii_alphabetic() {
                        terminated = true;
                        break;
                    }
                }
                if !terminated {
                    out.push_str(&consumed);
                }
            }
            _ => out.push(c),
        }
    }
    out
}

async fn forward_lines<R: AsyncRead + Unpin>(reader: R, log: Arc<dyn LogSink>) {
    let mut lines = BufReader::new(reader).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        log.line(&sanitize_line(&line));
    }
}

pub async fn run_capture(mut cmd: Command) -> Result<String, String> {
    let label = format!("{:?}", cmd.as_std());
    cmd.stdin(Stdio::null());
    cmd.kill_on_drop(true);
    let out = cmd
        .output()
        .await
        .map_err(|e| format!("spawn {label}: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "{label} exited with {}: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pi_domain::entities::DeploymentStatus;
    use std::sync::Mutex;

    struct VecSink(Mutex<Vec<String>>);
    impl LogSink for VecSink {
        fn line(&self, line: &str) {
            self.0.lock().unwrap().push(line.to_string());
        }
        fn finished(&self, _status: DeploymentStatus) {}
    }

    #[test]
    fn sanitize_line_keeps_only_the_segment_after_the_last_carriage_return() {
        assert_eq!(sanitize_line("hello\rworld"), "world");
    }

    #[test]
    fn sanitize_line_handles_repeated_progress_bar_overwrites() {
        assert_eq!(sanitize_line("10%\r55%\r100%\n"), "100%\n");
    }

    #[test]
    fn sanitize_line_strips_ansi_csi_sequences() {
        // BuildKit-style cursor-up + erase-line sequence embedded mid-line.
        assert_eq!(sanitize_line("\x1b[1A\x1b[2Kstep 4/9"), "step 4/9");
    }

    #[test]
    fn sanitize_line_preserves_content_when_csi_has_no_terminator() {
        // A CSI sequence truncated mid-line (e.g. the process was killed, or
        // the line was split oddly by the reader) must not swallow the rest
        // of the line — there's no terminator to swallow *up to*.
        assert_eq!(sanitize_line("\x1b[38;5;2"), "\x1b[38;5;2");
    }

    #[test]
    fn sanitize_line_leaves_plain_text_untouched() {
        let plain = "Sending build context to Docker daemon  2.048kB";
        assert_eq!(sanitize_line(plain), plain);
    }

    #[tokio::test]
    async fn run_capture_returns_trimmed_stdout() {
        let mut cmd = tokio::process::Command::new("git");
        cmd.arg("--version");
        let out = run_capture(cmd).await.unwrap();
        assert!(out.starts_with("git version"), "got: {out}");
    }

    #[tokio::test]
    async fn run_capture_reports_failure_with_stderr() {
        let mut cmd = tokio::process::Command::new("git");
        cmd.arg("definitely-not-a-git-command");
        let err = run_capture(cmd).await.unwrap_err();
        assert!(err.contains("exited with"), "got: {err}");
    }

    #[tokio::test]
    async fn run_streamed_forwards_output_lines() {
        let sink = Arc::new(VecSink(Mutex::new(vec![])));
        let mut cmd = tokio::process::Command::new("git");
        cmd.arg("--version");
        run_streamed(cmd, sink.clone()).await.unwrap();
        let lines = sink.0.lock().unwrap();
        assert!(
            lines.iter().any(|l| l.starts_with("git version")),
            "got: {lines:?}"
        );
    }

    #[tokio::test]
    async fn run_streamed_fails_on_nonzero_exit() {
        let sink = Arc::new(VecSink(Mutex::new(vec![])));
        let mut cmd = tokio::process::Command::new("git");
        cmd.arg("definitely-not-a-git-command");
        let err = run_streamed(cmd, sink).await.unwrap_err();
        assert!(err.contains("exited with"), "got: {err}");
    }

    #[tokio::test]
    async fn run_streamed_code_returns_zero_on_success() {
        let sink = Arc::new(VecSink(Mutex::new(vec![])));
        let mut cmd = tokio::process::Command::new("git");
        cmd.arg("--version");
        assert_eq!(run_streamed_code(cmd, sink).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn run_streamed_code_returns_nonzero_code_as_ok() {
        let sink = Arc::new(VecSink(Mutex::new(vec![])));
        let mut cmd = tokio::process::Command::new("git");
        cmd.arg("definitely-not-a-git-command");
        let code = run_streamed_code(cmd, sink).await.unwrap();
        assert_ne!(code, 0, "nonzero exit is data, not an error");
    }

    #[tokio::test]
    async fn dropping_run_streamed_future_kills_the_child() {
        let mut cmd;
        #[cfg(windows)]
        {
            cmd = tokio::process::Command::new("ping");
            cmd.args(["-n", "30", "127.0.0.1"]);
        }
        #[cfg(not(windows))]
        {
            cmd = tokio::process::Command::new("sleep");
            cmd.arg("30");
        }

        let sink = Arc::new(VecSink(Mutex::new(vec![])));
        let started = std::time::Instant::now();
        let result = tokio::time::timeout(
            std::time::Duration::from_millis(500),
            run_streamed(cmd, sink),
        )
        .await;
        assert!(result.is_err(), "child must outlive the timeout");
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
    }
}
