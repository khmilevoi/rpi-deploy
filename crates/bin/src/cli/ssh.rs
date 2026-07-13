use crate::cli::config::ServerProfile;
use crate::cli::tunnel::expand_home;

pub struct SshExec<'a> {
    pub profile: &'a ServerProfile,
}

impl SshExec<'_> {
    fn command(&self) -> tokio::process::Command {
        let mut cmd = tokio::process::Command::new("ssh");
        if let Some(key) = &self.profile.key {
            cmd.arg("-i").arg(expand_home(key));
        }
        cmd.args([
            "-o",
            "BatchMode=yes",
            &format!("{}@{}", self.profile.user, self.profile.host),
        ]);
        cmd
    }

    pub async fn check(&self) -> Result<(), String> {
        let mut cmd = self.command();
        cmd.arg("true");
        let out = cmd.output().await.map_err(|e| e.to_string())?;
        if out.status.success() {
            Ok(())
        } else {
            Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
        }
    }

    pub async fn run(&self, args: &[&str]) -> anyhow::Result<()> {
        let mut cmd = self.command();
        cmd.args(args);
        cmd.stdin(std::process::Stdio::null());
        cmd.stdout(std::process::Stdio::inherit());
        cmd.stderr(std::process::Stdio::inherit());
        let status = cmd.status().await?;
        if !status.success() {
            anyhow::bail!("ssh command exited with {status}");
        }
        Ok(())
    }

    /// The full `ssh` argv (after the program name) for an interactive
    /// TTY-exec of `remote`: `-i <key>` when a key is set, `-t` to allocate a
    /// TTY (so a remote `sudo` can prompt), `<user>@<host>`, then the remote
    /// command words. Deliberately does NOT set `BatchMode=yes` — the
    /// interactive `-t` default is what ships.
    pub(crate) fn tty_args(&self, remote: &[&str]) -> Vec<String> {
        let mut args: Vec<String> = Vec::new();
        if let Some(key) = &self.profile.key {
            args.push("-i".into());
            args.push(expand_home(key));
        }
        args.push("-t".into());
        args.push(format!("{}@{}", self.profile.user, self.profile.host));
        args.extend(remote.iter().map(|s| s.to_string()));
        args
    }

    /// Run `remote` on the board over an interactive `ssh -t` session with
    /// inherited stdio, so a remote `sudo` prompt reaches the operator's own
    /// terminal. Errors if the remote command exits nonzero.
    #[allow(dead_code)]
    pub async fn run_tty(&self, remote: &[&str]) -> anyhow::Result<()> {
        let mut cmd = tokio::process::Command::new("ssh");
        cmd.args(self.tty_args(remote));
        cmd.stdin(std::process::Stdio::inherit());
        cmd.stdout(std::process::Stdio::inherit());
        cmd.stderr(std::process::Stdio::inherit());
        let status = cmd.status().await?;
        if !status.success() {
            anyhow::bail!("remote command exited with {status}");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::config::ServerProfile;

    #[test]
    fn tty_args_include_tty_flag_and_remote_command() {
        let profile = ServerProfile {
            host: "pi.local".into(),
            user: "deploy".into(),
            key: Some("/home/u/.ssh/pi".into()),
        };
        let ssh = SshExec { profile: &profile };
        let args = ssh.tty_args(&["sudo", "rpi", "agent", "update", "--version", "0.22.0"]);
        assert_eq!(args[0], "-i");
        assert_eq!(args[1], "/home/u/.ssh/pi");
        assert!(args.contains(&"-t".to_string()));
        assert!(args.contains(&"deploy@pi.local".to_string()));
        // remote command tail is preserved in order
        let tail = &args[args.len() - 6..];
        assert_eq!(
            tail,
            &["sudo", "rpi", "agent", "update", "--version", "0.22.0"]
        );
        // BatchMode must NOT be forced (sudo may need to prompt)
        assert!(!args.iter().any(|a| a.contains("BatchMode")));
    }

    #[test]
    fn tty_args_omit_key_flag_when_no_key() {
        let profile = ServerProfile {
            host: "pi.local".into(),
            user: "deploy".into(),
            key: None,
        };
        let ssh = SshExec { profile: &profile };
        let args = ssh.tty_args(&["true"]);
        assert!(!args.contains(&"-i".to_string()));
        assert_eq!(args[0], "-t");
        assert!(args.contains(&"deploy@pi.local".to_string()));
    }
}
