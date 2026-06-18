/// Resolved field set for `pi init`, ready to render into pi.toml (§7).
pub struct InitFields {
    pub name: String,
    pub repo: String,
    pub branch: String,
    pub compose: String,
    pub service: String,
    pub port: u16,
    pub hostname: Option<String>,
    pub expose: Option<String>,
    pub env_file: Option<String>,
}

use std::fmt::Write as _;

/// TOML-экранированная basic-строка (с кавычками) — корректно эскейпит `"` и `\`.
fn toml_str(s: &str) -> String {
    toml::Value::String(s.to_string()).to_string()
}

/// Render canonical pi.toml text (schema 1, §12) from resolved fields.
pub fn render_pi_toml(f: &InitFields) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "schema = 1\n");
    let _ = writeln!(s, "[project]");
    let _ = writeln!(s, "name = {}\n", toml_str(&f.name));
    let _ = writeln!(s, "[source]");
    let _ = writeln!(s, "repo = {}", toml_str(&f.repo));
    let _ = writeln!(s, "branch = {}\n", toml_str(&f.branch));
    let _ = writeln!(s, "[build]");
    let _ = writeln!(s, "compose = {}\n", toml_str(&f.compose));
    let _ = writeln!(s, "[ingress]");
    if let Some(h) = &f.hostname {
        let _ = writeln!(s, "hostname = {}", toml_str(h));
    }
    let _ = writeln!(s, "service = {}", toml_str(&f.service));
    let _ = writeln!(s, "port = {}", f.port);
    if let Some(e) = &f.expose {
        let _ = writeln!(s, "expose = {}", toml_str(e));
    }
    if let Some(env) = &f.env_file {
        let _ = writeln!(s, "\n[env]");
        let _ = writeln!(s, "file = {}", toml_str(env));
    }
    s
}

use std::path::Path;
use crate::cli::prompt::Prompter;

#[derive(Default)]
pub struct InitFlags {
    pub name: Option<String>,
    pub repo: Option<String>,
    pub branch: Option<String>,
    pub compose: Option<String>,
    pub service: Option<String>,
    pub port: Option<u16>,
    pub hostname: Option<String>,
    pub expose: Option<String>,
    pub env_file: Option<String>,
    pub yes: bool,
}

pub struct DetectedDefaults {
    pub name: String,
    pub repo: Option<String>,
    pub branch: Option<String>,
    pub compose: Option<String>,
    pub env_file: Option<String>,
}

/// Best-effort auto-detection of pi.toml defaults from the project dir (§7).
pub fn detect_defaults(cwd: &Path) -> DetectedDefaults {
    let name = cwd
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("app")
        .to_string();
    let git = |args: &[&str]| -> Option<String> {
        let out = std::process::Command::new("git").args(args).current_dir(cwd).output().ok()?;
        out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
    };
    let repo = git(&["remote", "get-url", "origin"]).filter(|s| !s.is_empty());
    let branch = git(&["rev-parse", "--abbrev-ref", "HEAD"]).filter(|s| !s.is_empty() && s != "HEAD");
    let compose = ["docker-compose.yml", "compose.yaml", "compose.yml", "docker-compose.yaml"]
        .into_iter()
        .find(|f| cwd.join(f).exists())
        .map(String::from);
    let env_file = cwd.join(".env").exists().then(|| ".env".to_string());
    DetectedDefaults { name, repo, branch, compose, env_file }
}

fn ask_text(flag: &Option<String>, det: Option<&str>, label: &str, yes: bool, p: &mut dyn Prompter) -> anyhow::Result<String> {
    if let Some(v) = flag {
        return Ok(v.clone());
    }
    if yes {
        return det
            .map(str::to_string)
            .ok_or_else(|| anyhow::anyhow!("--yes: missing value for {label}; pass it as a flag"));
    }
    p.text(label, det)
}

/// Resolve final fields: flags win, then detected defaults, then prompt (§7).
pub fn resolve_init_fields(
    flags: &InitFlags,
    det: &DetectedDefaults,
    p: &mut dyn Prompter,
) -> anyhow::Result<InitFields> {
    let name = ask_text(&flags.name, Some(&det.name), "project name", flags.yes, p)?;
    let repo = ask_text(&flags.repo, det.repo.as_deref(), "git repo url", flags.yes, p)?;
    let branch = ask_text(&flags.branch, det.branch.as_deref().or(Some("main")), "branch", flags.yes, p)?;
    let compose = ask_text(&flags.compose, det.compose.as_deref().or(Some("docker-compose.yml")), "compose file", flags.yes, p)?;
    let service = ask_text(&flags.service, None, "ingress service (compose service name)", flags.yes, p)?;
    let port = match flags.port {
        Some(p) => p,
        None if flags.yes => anyhow::bail!("--yes: missing --port"),
        None => p.text("container port", Some("3000"))?.trim().parse()?,
    };
    let hostname = match &flags.hostname {
        Some(h) if !h.is_empty() => Some(h.clone()),
        Some(_) => None,
        None if flags.yes => None,
        None => {
            let v = p.text("public hostname (empty = no public ingress)", None)?;
            (!v.trim().is_empty()).then(|| v.trim().to_string())
        }
    };
    let expose = match &flags.expose {
        Some(e) => Some(e.clone()),
        None if flags.yes => None,
        None => {
            let choice = p.select("expose mode", &["private".into(), "lan".into()], 0)?;
            (choice == "lan").then(|| "lan".to_string())
        }
    };
    let env_file = flags.env_file.clone().or_else(|| det.env_file.clone());
    Ok(InitFields { name, repo, branch, compose, service, port, hostname, expose, env_file })
}

use crate::cli::prompt::InquirePrompter;

/// Entrypoint for `pi init`: detect, resolve, write ./pi.toml (backup if present).
pub async fn run(flags: InitFlags) -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;
    let det = detect_defaults(&cwd);
    let mut prompter = InquirePrompter;
    let fields = resolve_init_fields(&flags, &det, &mut prompter)?;
    let text = render_pi_toml(&fields);

    let path = cwd.join("pi.toml");
    if path.exists() {
        let overwrite = flags.yes || prompter.confirm("pi.toml exists — overwrite? (a .bak is kept)", false)?;
        if !overwrite {
            println!("aborted: pi.toml left unchanged");
            return Ok(());
        }
        std::fs::rename(&path, cwd.join("pi.toml.bak"))?;
    }
    std::fs::write(&path, &text)?;
    println!("wrote {}", path.display());
    println!("next: `pi env send` (if you use secrets), then `pi deploy`");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::prompt::ScriptedPrompter;

    fn sample() -> InitFields {
        InitFields {
            name: "rateme".into(),
            repo: "git@github.com:isskelo/rateme.git".into(),
            branch: "main".into(),
            compose: "docker-compose.yml".into(),
            service: "web".into(),
            port: 3000,
            hostname: Some("rateme.example.com".into()),
            expose: None,
            env_file: Some(".env".into()),
        }
    }

    #[test]
    fn render_is_parseable_and_round_trips() {
        let text = render_pi_toml(&sample());
        let parsed = crate::cli::pitoml::PiToml::parse(&text).unwrap();
        let cfg = parsed.to_project_config();
        assert_eq!(cfg.name, "rateme");
        assert_eq!(cfg.service, "web");
        assert_eq!(cfg.container_port, 3000);
        assert_eq!(cfg.hostname.as_deref(), Some("rateme.example.com"));
        assert!(text.starts_with("schema = 1\n"));
    }

    #[test]
    fn lan_expose_and_no_hostname_render() {
        let mut f = sample();
        f.hostname = None;
        f.expose = Some("lan".into());
        let text = render_pi_toml(&f);
        assert!(!text.contains("hostname"));
        assert!(text.contains("expose = \"lan\""));
        let cfg = crate::cli::pitoml::PiToml::parse(&text).unwrap().to_project_config();
        assert_eq!(cfg.expose, pi_domain::entities::ExposeMode::Lan);
    }

    #[test]
    fn render_escapes_backslashes_and_quotes_and_round_trips() {
        let mut f = sample();
        f.name = "a\"b".into();
        f.env_file = Some("C:\\app\\.env".into());
        let text = render_pi_toml(&f);
        // Сгенерированный файл должен быть валидным TOML и разбор возвращать исходные значения.
        let parsed = crate::cli::pitoml::PiToml::parse(&text).unwrap();
        assert_eq!(parsed.project.name, "a\"b");
        assert_eq!(parsed.env.file, "C:\\app\\.env");
        // И должно содержать эскейпированные литералы в тексте.
        assert!(text.contains("name = 'a\"b'"));
        assert!(text.contains("file = 'C:\\app\\.env'"));
    }

    fn detected() -> DetectedDefaults {
        DetectedDefaults {
            name: "rateme".into(),
            repo: Some("git@github.com:isskelo/rateme.git".into()),
            branch: Some("main".into()),
            compose: Some("docker-compose.yml".into()),
            env_file: Some(".env".into()),
        }
    }

    #[test]
    fn yes_mode_uses_detected_without_prompting() {
        let flags = InitFlags { yes: true, ..InitFlags::default() };
        let mut p = ScriptedPrompter {
            texts: Default::default(),
            confirms: Default::default(),
            selects: Default::default(),
        };
        // service/port/hostname have no detected default -> in --yes they come from flags or fall back.
        let flags = InitFlags { service: Some("web".into()), port: Some(3000), ..flags };
        let f = resolve_init_fields(&flags, &detected(), &mut p).unwrap();
        assert_eq!(f.name, "rateme");
        assert_eq!(f.repo, "git@github.com:isskelo/rateme.git");
        assert_eq!(f.service, "web");
        assert_eq!(f.port, 3000);
    }

    #[test]
    fn flags_override_detected() {
        let flags = InitFlags {
            yes: true,
            name: Some("other".into()),
            service: Some("api".into()),
            port: Some(8080),
            expose: Some("lan".into()),
            ..InitFlags::default()
        };
        let mut p = ScriptedPrompter {
            texts: Default::default(),
            confirms: Default::default(),
            selects: Default::default(),
        };
        let f = resolve_init_fields(&flags, &detected(), &mut p).unwrap();
        assert_eq!(f.name, "other");
        assert_eq!(f.service, "api");
        assert_eq!(f.expose.as_deref(), Some("lan"));
    }

    #[test]
    fn interactive_selects_expose_mode() {
        // yes = false -> expose is chosen via select (no --expose flag)
        let flags = InitFlags { service: Some("web".into()), port: Some(3000), ..InitFlags::default() };
        let mut p = ScriptedPrompter {
            texts: Default::default(),
            confirms: Default::default(),
            selects: ["lan".to_string()].into_iter().collect(),
        };
        let f = resolve_init_fields(&flags, &detected(), &mut p).unwrap();
        assert_eq!(f.expose.as_deref(), Some("lan"), "expose chosen via select");
    }
}
