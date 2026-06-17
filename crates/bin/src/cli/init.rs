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

/// Render canonical pi.toml text (schema 1, §12) from resolved fields.
pub fn render_pi_toml(f: &InitFields) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "schema = 1\n");
    let _ = writeln!(s, "[project]");
    let _ = writeln!(s, "name = \"{}\"\n", f.name);
    let _ = writeln!(s, "[source]");
    let _ = writeln!(s, "repo = \"{}\"", f.repo);
    let _ = writeln!(s, "branch = \"{}\"\n", f.branch);
    let _ = writeln!(s, "[build]");
    let _ = writeln!(s, "compose = \"{}\"\n", f.compose);
    let _ = writeln!(s, "[ingress]");
    if let Some(h) = &f.hostname {
        let _ = writeln!(s, "hostname = \"{h}\"");
    }
    let _ = writeln!(s, "service = \"{}\"", f.service);
    let _ = writeln!(s, "port = {}", f.port);
    if let Some(e) = &f.expose {
        let _ = writeln!(s, "expose = \"{e}\"");
    }
    if let Some(env) = &f.env_file {
        let _ = writeln!(s, "\n[env]");
        let _ = writeln!(s, "file = \"{env}\"");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
