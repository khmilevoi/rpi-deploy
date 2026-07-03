use crate::cli::config::ServerProfile;
use crate::cli::prompt::Prompter;

#[derive(Default)]
pub struct SetupFlags {
    pub host: Option<String>,
    pub user: Option<String>,
    pub key: Option<String>,
    pub name: Option<String>,
    pub default: bool,
    pub yes: bool,
}

/// Resolve (alias, profile, make_default) from flags, prompting when needed.
/// `keys` are SSH key candidates (from `detect_ssh_keys`), offered via `select`.
pub fn resolve_profile(
    flags: &SetupFlags,
    keys: &[std::path::PathBuf],
    p: &mut dyn Prompter,
) -> anyhow::Result<(String, ServerProfile, bool)> {
    let name = match &flags.name {
        Some(n) => n.clone(),
        None if flags.yes => "home".to_string(),
        None => {
            let v = p.text("server alias", Some("home"))?;
            if v.trim().is_empty() { "home".to_string() } else { v.trim().to_string() }
        }
    };
    let host = match &flags.host {
        Some(h) => h.clone(),
        None if flags.yes => anyhow::bail!("--yes: missing --host"),
        None => p.text("Pi host or IP", None)?,
    };
    let user = match &flags.user {
        Some(u) => u.clone(),
        None if flags.yes => anyhow::bail!("--yes: missing --user"),
        None => p.text("SSH login user", None)?,
    };
    let key = match &flags.key {
        Some(k) => Some(k.clone()),
        None if flags.yes => None,
        None if keys.is_empty() => Some("~/.ssh/id_ed25519".to_string()),
        None => {
            let mut opts: Vec<String> = keys.iter().map(|p| p.display().to_string()).collect();
            let generate = "(generate a new key at ~/.ssh/id_ed25519)";
            opts.push(generate.into());
            let choice = p.select("SSH key", &opts, 0)?;
            if choice == generate {
                Some("~/.ssh/id_ed25519".to_string())
            } else {
                Some(choice)
            }
        }
    };
    let make_default = flags.default || flags.yes;
    Ok((name, ServerProfile { host, user, key }, make_default))
}

use crate::cli::config::{ClientConfig, ConnectOpts};
use crate::cli::prompt::InquirePrompter;
use crate::cli::ssh::SshExec;

/// Entrypoint for `rpi setup`: profile + key bootstrap + save + connectivity test.
pub async fn run(flags: SetupFlags) -> anyhow::Result<()> {
    let mut prompter = InquirePrompter;
    let ssh_dir = dirs::home_dir().map(|h| h.join(".ssh")).unwrap_or_default();
    let detected_keys = crate::cli::keys::detect_ssh_keys(&ssh_dir);
    let (name, profile, make_default) = resolve_profile(&flags, &detected_keys, &mut prompter)?;

    // Key bootstrap: adopt if SSH already works, else offer to generate+push.
    let ssh = SshExec { profile: &profile };
    if ssh.check().await.is_err() && !flags.yes {
        if let Some(key) = &profile.key {
            let key_path = std::path::PathBuf::from(crate::cli::tunnel::expand_home(key));
            let pubkey = crate::cli::keys::pubkey_path(&key_path);
            if !key_path.exists()
                && prompter.confirm(&format!("no key at {} — generate one?", key_path.display()), true)?
            {
                crate::cli::keys::generate_key(&key_path).await?;
            }
            if pubkey.exists()
                && prompter.confirm("copy public key to the Pi now? (asks Pi password once)", true)?
            {
                crate::cli::keys::push_pubkey(&profile, &pubkey).await?;
            }
        }
    }

    let path = ClientConfig::save_merged(&name, profile.clone(), make_default)?;
    println!("saved profile '{name}' to {}", path.display());

    // Connectivity test reuses the existing doctor path against the new profile.
    println!("testing connection...");
    if let Err(e) = ssh.check().await {
        println!("ssh check failed: {e}");
        println!("fix SSH access, then run `rpi doctor --server {name}`");
        return Ok(());
    }
    let connect = ConnectOpts { server: Some(name.clone()), host: None, user: None, key: None };
    crate::cli::commands::doctor(connect).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::prompt::ScriptedPrompter;

    #[test]
    fn resolve_profile_from_flags_yes() {
        let flags = SetupFlags {
            host: Some("pihost.local".into()),
            user: Some("piuser".into()),
            key: Some("~/.ssh/pi".into()),
            name: Some("home".into()),
            default: false,
            yes: true,
        };
        let mut p = ScriptedPrompter {
            texts: Default::default(), confirms: Default::default(), selects: Default::default(),
        };
        let (name, profile, make_default) = resolve_profile(&flags, &[], &mut p).unwrap();
        assert_eq!(name, "home");
        assert_eq!(profile.host, "pihost.local");
        assert_eq!(profile.key.as_deref(), Some("~/.ssh/pi"));
        assert!(make_default);
    }

    #[test]
    fn yes_without_host_errors() {
        let flags = SetupFlags { user: Some("u".into()), yes: true, ..SetupFlags::default() };
        let mut p = ScriptedPrompter {
            texts: Default::default(), confirms: Default::default(), selects: Default::default(),
        };
        assert!(resolve_profile(&flags, &[], &mut p).is_err());
    }

    #[test]
    fn interactive_selects_ssh_key() {
        use std::path::PathBuf;
        let flags = SetupFlags {
            host: Some("pihost.local".into()),
            user: Some("piuser".into()),
            ..SetupFlags::default()
        };
        let keys = vec![PathBuf::from("/home/u/.ssh/pi"), PathBuf::from("/home/u/.ssh/id_ed25519")];
        let mut p = ScriptedPrompter {
            texts: Default::default(),
            confirms: Default::default(),
            selects: ["/home/u/.ssh/pi".to_string()].into_iter().collect(),
        };
        let (_, profile, _) = resolve_profile(&flags, &keys, &mut p).unwrap();
        assert_eq!(profile.key.as_deref(), Some("/home/u/.ssh/pi"));
    }
}
