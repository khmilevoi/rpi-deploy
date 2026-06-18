use std::path::{Path, PathBuf};

use crate::cli::config::ServerProfile;
use crate::cli::tunnel::expand_home;

/// Private-key candidates in an .ssh dir: skip *.pub, known_hosts, config, authorized_keys.
pub fn detect_ssh_keys(ssh_dir: &Path) -> Vec<PathBuf> {
    let skip = ["known_hosts", "config", "authorized_keys"];
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(ssh_dir) else { return out };
    for e in entries.flatten() {
        let path = e.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else { continue };
        if name.ends_with(".pub") || skip.contains(&name) || name.starts_with('.') {
            continue;
        }
        if path.is_file() {
            out.push(path);
        }
    }
    out
}

pub fn pubkey_path(key: &Path) -> PathBuf {
    let mut s = key.as_os_str().to_os_string();
    s.push(".pub");
    PathBuf::from(s)
}

/// Generate an ed25519 keypair at `path` (no passphrase).
pub async fn generate_key(path: &Path) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let status = tokio::process::Command::new("ssh-keygen")
        .args(["-t", "ed25519", "-N", "", "-f"])
        .arg(path)
        .status()
        .await?;
    if !status.success() {
        anyhow::bail!("ssh-keygen failed");
    }
    Ok(())
}

/// Human-readable instructions for copying the pubkey by hand when ssh-copy fails
/// (e.g. password auth disabled on the Pi). PR #7 S4.
pub fn manual_copy_instructions(pubkey_text: &str, profile: &ServerProfile) -> String {
    format!(
        "Could not copy the SSH key automatically (password auth may be disabled on the Pi).\n\
         Append this public key to ~pi/.ssh/authorized_keys on {user}@{host} manually:\n\n\
         {pubkey}\n\n\
         Then re-run `pi setup` (or `pi doctor --server <alias>`).",
        pubkey = pubkey_text.trim(),
        user = profile.user,
        host = profile.host,
    )
}

/// Append our pubkey to the Pi's authorized_keys (ssh-copy-id equivalent;
/// works on Windows OpenSSH which lacks ssh-copy-id). Interactive: may prompt
/// for the Pi password once.
pub async fn push_pubkey(profile: &ServerProfile, pubkey: &Path) -> anyhow::Result<()> {
    let pubkey_text = std::fs::read_to_string(pubkey)?;
    let mut cmd = tokio::process::Command::new("ssh");
    if let Some(key) = &profile.key {
        cmd.arg("-i").arg(expand_home(key));
    }
    cmd.args([
        "-o", "StrictHostKeyChecking=accept-new",
        &format!("{}@{}", profile.user, profile.host),
        "umask 077; mkdir -p ~/.ssh && cat >> ~/.ssh/authorized_keys",
    ]);
    cmd.stdin(std::process::Stdio::piped());
    cmd.stdout(std::process::Stdio::inherit());
    cmd.stderr(std::process::Stdio::inherit());
    let mut child = cmd.spawn();
    let failed = match &mut child {
        Ok(c) => {
            use tokio::io::AsyncWriteExt;
            if c.stdin.take().unwrap().write_all(pubkey_text.as_bytes()).await.is_err() {
                true
            } else {
                match c.wait().await {
                    Ok(s) => !s.success(),
                    Err(_) => true,
                }
            }
        }
        Err(_) => true,
    };
    if failed {
        eprintln!("{}", manual_copy_instructions(&pubkey_text, profile));
        anyhow::bail!("failed to copy public key to {} — see instructions above", profile.host);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pubkey_path_appends_pub() {
        assert_eq!(pubkey_path(Path::new("/home/u/.ssh/pi")), PathBuf::from("/home/u/.ssh/pi.pub"));
    }

    #[test]
    fn detect_finds_private_keys_skips_pub_and_known_hosts() {
        let dir = tempfile::tempdir().unwrap();
        let ssh = dir.path();
        std::fs::write(ssh.join("pi"), "k").unwrap();
        std::fs::write(ssh.join("pi.pub"), "k").unwrap();
        std::fs::write(ssh.join("id_ed25519"), "k").unwrap();
        std::fs::write(ssh.join("known_hosts"), "k").unwrap();
        std::fs::write(ssh.join("config"), "k").unwrap();
        let mut found = detect_ssh_keys(ssh);
        found.sort();
        assert_eq!(found, vec![ssh.join("id_ed25519"), ssh.join("pi")]);
    }

    #[test]
    fn manual_instructions_include_pubkey_and_target() {
        let profile = ServerProfile {
            host: "pihost.local".into(),
            user: "pi".into(),
            key: Some("~/.ssh/pi".into()),
        };
        let msg = manual_copy_instructions("ssh-ed25519 AAAA…", &profile);
        assert!(msg.contains("ssh-ed25519 AAAA…"), "pubkey printed");
        assert!(msg.contains("pi@pihost.local"), "target printed");
        assert!(msg.contains("authorized_keys"), "hints at authorized_keys");
    }
}
