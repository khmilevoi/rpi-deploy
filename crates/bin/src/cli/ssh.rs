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
}
