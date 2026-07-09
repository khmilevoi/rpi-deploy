use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use pi_domain::contracts::{CloudflareApi, Ingress, IngressOutcome, LogSink};
use pi_domain::error::DomainError;
use tokio::process::Command;

use crate::process::run_capture;

fn ingress_err(msg: impl std::fmt::Display) -> DomainError {
    DomainError::Ingress(msg.to_string())
}

/// Upserts `hostname -> service` into the cloudflared config document.
/// Returns true when the document changed (diff-restart, §11). Keeps the
/// catch-all rule (no `hostname`) last, appending one if missing.
pub(crate) fn upsert_ingress_rule(
    doc: &mut serde_yaml::Value,
    hostname: &str,
    service: &str,
) -> Result<bool, String> {
    let map = doc
        .as_mapping_mut()
        .ok_or("config.yml: top level must be a mapping")?;
    let key = serde_yaml::Value::from("ingress");
    if !map.contains_key(&key) {
        map.insert(key.clone(), serde_yaml::Value::Sequence(vec![]));
    }
    let rules = map
        .get_mut(&key)
        .and_then(|v| v.as_sequence_mut())
        .ok_or("config.yml: `ingress` must be a list")?;

    let mut desired = serde_yaml::Mapping::new();
    desired.insert("hostname".into(), hostname.into());
    desired.insert("service".into(), service.into());
    let desired = serde_yaml::Value::Mapping(desired);

    if let Some(existing) = rules
        .iter_mut()
        .find(|r| r.get("hostname").and_then(|h| h.as_str()) == Some(hostname))
    {
        if existing.get("service").and_then(|s| s.as_str()) == Some(service) {
            return Ok(false);
        }
        *existing = desired;
        return Ok(true);
    }

    match rules.iter().position(|r| r.get("hostname").is_none()) {
        Some(catch_all) => rules.insert(catch_all, desired),
        None => {
            rules.push(desired);
            let mut catch_all = serde_yaml::Mapping::new();
            catch_all.insert("service".into(), "http_status:404".into());
            rules.push(serde_yaml::Value::Mapping(catch_all));
        }
    }
    Ok(true)
}

pub(crate) fn remove_ingress_rule(
    doc: &mut serde_yaml::Value,
    hostname: &str,
) -> Result<bool, String> {
    let map = doc
        .as_mapping_mut()
        .ok_or("config.yml: top level must be a mapping")?;
    let key = serde_yaml::Value::from("ingress");
    let Some(rules) = map.get_mut(&key).and_then(|v| v.as_sequence_mut()) else {
        return Ok(false);
    };
    let Some(pos) = rules
        .iter()
        .position(|r| r.get("hostname").and_then(|h| h.as_str()) == Some(hostname))
    else {
        return Ok(false);
    };
    rules.remove(pos);
    Ok(true)
}

/// `systemctl --user` needs XDG_RUNTIME_DIR to reach the user manager; the
/// rpi-agent unit does not set it. Compute the variable to add to the restart
/// command when the agent's own environment lacks it.
fn restart_extra_env(current: Option<&str>, uid: u32) -> Option<(&'static str, String)> {
    match current {
        Some(_) => None,
        None => Some(("XDG_RUNTIME_DIR", format!("/run/user/{uid}"))),
    }
}

#[cfg(unix)]
fn current_uid() -> u32 {
    // SAFETY: getuid has no preconditions and cannot fail.
    unsafe { libc::getuid() }
}

#[cfg(not(unix))]
fn current_uid() -> u32 {
    0
}

fn apply_restart_env(cmd: &mut Command) {
    let current = std::env::var("XDG_RUNTIME_DIR").ok();
    if let Some((k, v)) = restart_extra_env(current.as_deref(), current_uid()) {
        cmd.env(k, v);
    }
}

/// Locally-managed cloudflared (§11): edits config.yml, creates the DNS
/// route, restarts the unit without sudo — and only when the config changed.
pub struct CloudflaredIngress {
    config_path: PathBuf,
    tunnel_id: String,
    zone: String,
    restart: Vec<String>,
    cf: Arc<dyn CloudflareApi>,
}

impl CloudflaredIngress {
    pub fn new(
        config_path: PathBuf,
        tunnel_id: String,
        zone: String,
        restart: Vec<String>,
        cf: Arc<dyn CloudflareApi>,
    ) -> Arc<CloudflaredIngress> {
        Arc::new(CloudflaredIngress {
            config_path,
            tunnel_id,
            zone,
            restart,
            cf,
        })
    }
}

#[async_trait]
impl Ingress for CloudflaredIngress {
    async fn upsert(
        &self,
        hostname: &str,
        host_port: u16,
        log: Arc<dyn LogSink>,
    ) -> Result<IngressOutcome, DomainError> {
        let text = tokio::fs::read_to_string(&self.config_path)
            .await
            .map_err(|e| {
                ingress_err(format!(
                    "cannot read {}: {e}; bootstrap the tunnel first (see README.md, section 'Cloudflare Tunnel')",
                    self.config_path.display()
                ))
            })?;
        let mut doc: serde_yaml::Value = serde_yaml::from_str(&text).map_err(ingress_err)?;
        let service = format!("http://127.0.0.1:{host_port}");
        let changed = upsert_ingress_rule(&mut doc, hostname, &service).map_err(ingress_err)?;
        if !changed {
            log.line(&format!(
                "ingress: {hostname} -> {service} already routed; cloudflared untouched"
            ));
            return Ok(IngressOutcome::Applied);
        }
        let updated = serde_yaml::to_string(&doc).map_err(ingress_err)?;
        tokio::fs::write(&self.config_path, updated)
            .await
            .map_err(ingress_err)?;
        log.line(&format!("ingress: routing {hostname} -> {service}"));

        // On dns/restart failure roll the config back: a persisted change
        // would make the next deploy see "no diff" and never retry (§11).
        if let Err(err) = self.route_dns_and_restart(hostname, &log).await {
            if let Err(restore) = tokio::fs::write(&self.config_path, &text).await {
                return Err(ingress_err(format!(
                    "{err}; additionally failed to restore {}: {restore}",
                    self.config_path.display()
                )));
            }
            return Err(err);
        }
        Ok(IngressOutcome::Applied)
    }

    async fn remove(&self, hostname: &str, log: Arc<dyn LogSink>) -> Result<(), DomainError> {
        let text = match tokio::fs::read_to_string(&self.config_path).await {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                log.line("ingress: no config.yml found; skipping remove");
                return Ok(());
            }
            Err(e) => {
                return Err(ingress_err(format!(
                    "cannot read {}: {e}",
                    self.config_path.display()
                )));
            }
        };
        let mut doc: serde_yaml::Value = serde_yaml::from_str(&text).map_err(ingress_err)?;
        let changed = remove_ingress_rule(&mut doc, hostname).map_err(ingress_err)?;
        if !changed {
            log.line(&format!(
                "ingress: no route for {hostname}; cloudflared untouched"
            ));
            return Ok(());
        }
        let updated = serde_yaml::to_string(&doc).map_err(ingress_err)?;
        tokio::fs::write(&self.config_path, updated)
            .await
            .map_err(ingress_err)?;
        log.line(&format!("ingress: removed route for {hostname}"));

        let (program, args) = self
            .restart
            .split_first()
            .ok_or_else(|| ingress_err("empty cloudflared restart command"))?;
        let mut restart_cmd = Command::new(program);
        restart_cmd.args(args);
        apply_restart_env(&mut restart_cmd);
        if let Err(err) = run_capture(restart_cmd).await {
            if let Err(restore) = tokio::fs::write(&self.config_path, &text).await {
                return Err(ingress_err(format!(
                    "restart cloudflared: {err}; additionally failed to restore {}: {restore}",
                    self.config_path.display()
                )));
            }
            return Err(ingress_err(format!("restart cloudflared: {err}")));
        }
        log.line("ingress: cloudflared restarted");
        Ok(())
    }
}

impl CloudflaredIngress {
    async fn route_dns_and_restart(
        &self,
        hostname: &str,
        log: &Arc<dyn LogSink>,
    ) -> Result<(), DomainError> {
        match self
            .cf
            .put_tunnel_cname(&self.zone, hostname, &self.tunnel_id)
            .await
        {
            Ok(_) => log.line(&format!("ingress: DNS record ensured for {hostname}")),
            Err(err) => return Err(ingress_err(format!("route dns: {err}"))),
        }

        let (program, args) = self
            .restart
            .split_first()
            .ok_or_else(|| ingress_err("empty cloudflared restart command"))?;
        let mut restart_cmd = Command::new(program);
        restart_cmd.args(args);
        apply_restart_env(&mut restart_cmd);
        run_capture(restart_cmd)
            .await
            .map_err(|e| ingress_err(format!("restart cloudflared: {e}")))?;
        log.line("ingress: cloudflared restarted");
        Ok(())
    }
}

/// No [cloudflared] in agent.toml: deploys succeed, routing is manual.
pub struct DisabledIngress;

impl DisabledIngress {
    pub fn new() -> Arc<DisabledIngress> {
        Arc::new(DisabledIngress)
    }
}

#[async_trait]
impl Ingress for DisabledIngress {
    async fn upsert(
        &self,
        hostname: &str,
        host_port: u16,
        log: Arc<dyn LogSink>,
    ) -> Result<IngressOutcome, DomainError> {
        log.line(&format!(
            "ingress: [cloudflared] is not configured in agent.toml; \
             route {hostname} -> http://127.0.0.1:{host_port} manually"
        ));
        Ok(IngressOutcome::Skipped)
    }

    async fn remove(&self, hostname: &str, log: Arc<dyn LogSink>) -> Result<(), DomainError> {
        log.line(&format!(
            "ingress: [cloudflared] is not configured; remove DNS/routing for {hostname} manually"
        ));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_sink::CollectSink;

    fn doc(yaml: &str) -> serde_yaml::Value {
        serde_yaml::from_str(yaml).unwrap()
    }

    const BASE: &str = "tunnel: home\ncredentials-file: /var/lib/rpi/cloudflared/home.json\ningress:\n  - hostname: old.example.com\n    service: http://127.0.0.1:8001\n  - service: http_status:404\n";

    #[test]
    fn adds_new_rule_before_catch_all() {
        let mut d = doc(BASE);
        let changed =
            upsert_ingress_rule(&mut d, "new.example.com", "http://127.0.0.1:8002").unwrap();
        assert!(changed);
        let rules = d.get("ingress").unwrap().as_sequence().unwrap();
        assert_eq!(rules.len(), 3);
        assert_eq!(
            rules[1].get("hostname").unwrap().as_str(),
            Some("new.example.com")
        );
        assert!(rules[2].get("hostname").is_none(), "catch-all stays last");
    }

    #[test]
    fn same_rule_is_a_noop() {
        let mut d = doc(BASE);
        let changed =
            upsert_ingress_rule(&mut d, "old.example.com", "http://127.0.0.1:8001").unwrap();
        assert!(!changed);
        assert_eq!(d, doc(BASE), "document untouched");
    }

    #[test]
    fn changed_port_replaces_rule_in_place() {
        let mut d = doc(BASE);
        let changed =
            upsert_ingress_rule(&mut d, "old.example.com", "http://127.0.0.1:9000").unwrap();
        assert!(changed);
        let rules = d.get("ingress").unwrap().as_sequence().unwrap();
        assert_eq!(rules.len(), 2);
        assert_eq!(
            rules[0].get("service").unwrap().as_str(),
            Some("http://127.0.0.1:9000")
        );
    }

    #[test]
    fn missing_ingress_list_and_catch_all_are_created() {
        let mut d = doc("tunnel: home\n");
        let changed =
            upsert_ingress_rule(&mut d, "a.example.com", "http://127.0.0.1:8000").unwrap();
        assert!(changed);
        let rules = d.get("ingress").unwrap().as_sequence().unwrap();
        assert_eq!(rules.len(), 2);
        assert_eq!(
            rules[1].get("service").unwrap().as_str(),
            Some("http_status:404")
        );
    }

    #[test]
    fn non_mapping_document_is_an_error() {
        let mut d = doc("- just\n- a list\n");
        assert!(upsert_ingress_rule(&mut d, "a.example.com", "x").is_err());
    }

    #[tokio::test]
    async fn missing_config_file_gives_actionable_error() {
        use pi_domain::contracts::MockCloudflareApi;

        let dir = tempfile::tempdir().unwrap();
        let ingress = CloudflaredIngress::new(
            dir.path().join("config.yml"),
            "tid".into(),
            "example.com".into(),
            vec!["whatever".into()],
            std::sync::Arc::new(MockCloudflareApi::new()),
        );
        let err = ingress
            .upsert("a.example.com", 8000, CollectSink::new())
            .await
            .unwrap_err();
        assert!(
            matches!(&err, DomainError::Ingress(m) if m.contains("config.yml")),
            "got: {err}"
        );
    }

    #[tokio::test]
    async fn no_diff_skips_dns_and_restart_entirely() {
        use pi_domain::contracts::MockCloudflareApi;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.yml");
        std::fs::write(&path, BASE).unwrap();
        // commands would fail if executed: binaries do not exist; the cf
        // mock has no expectations set, so any call would panic.
        let ingress = CloudflaredIngress::new(
            path.clone(),
            "tid".into(),
            "example.com".into(),
            vec!["pi-test-no-such-binary".into()],
            std::sync::Arc::new(MockCloudflareApi::new()),
        );
        let outcome = ingress
            .upsert("old.example.com", 8001, CollectSink::new())
            .await
            .unwrap();
        assert_eq!(outcome, IngressOutcome::Applied);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            BASE,
            "file untouched"
        );
    }

    #[tokio::test]
    async fn disabled_ingress_upsert_reports_skipped() {
        let ingress = DisabledIngress::new();
        let outcome = ingress
            .upsert("a.example.com", 8000, CollectSink::new())
            .await
            .unwrap();
        assert_eq!(outcome, IngressOutcome::Skipped);
    }

    #[tokio::test]
    async fn failed_dns_restores_config_so_redeploy_retries() {
        use pi_domain::contracts::MockCloudflareApi;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.yml");
        std::fs::write(&path, BASE).unwrap();

        let mut cf = MockCloudflareApi::new();
        cf.expect_put_tunnel_cname()
            .times(2)
            .returning(|_, _, _| Err(DomainError::Ingress("boom".into())));

        let ingress = CloudflaredIngress::new(
            path.clone(),
            "tid".into(),
            "example.com".into(),
            vec!["pi-test-no-such-binary".into()],
            std::sync::Arc::new(cf),
        );
        // the cf mock fails put_tunnel_cname -> route dns fails -> Err.
        // The config write must be rolled back: otherwise the next deploy
        // sees no diff and never retries dns/restart for this hostname.
        let err = ingress
            .upsert("new.example.com", 8002, CollectSink::new())
            .await
            .unwrap_err();
        assert!(matches!(err, DomainError::Ingress(_)));
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            BASE,
            "config restored after failure"
        );
        // a retry diffs again and re-attempts dns instead of silently passing
        let err = ingress
            .upsert("new.example.com", 8002, CollectSink::new())
            .await
            .unwrap_err();
        assert!(matches!(err, DomainError::Ingress(_)), "retry re-runs dns");
    }

    #[test]
    fn restart_env_added_only_when_missing() {
        assert_eq!(
            restart_extra_env(None, 999),
            Some(("XDG_RUNTIME_DIR", "/run/user/999".into()))
        );
        assert_eq!(restart_extra_env(Some("/run/user/1000"), 999), None);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn upsert_routes_dns_via_api_not_shell() {
        use pi_domain::contracts::MockCloudflareApi;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.yml");
        std::fs::write(
            &path,
            "tunnel: home\ningress:\n  - service: http_status:404\n",
        )
        .unwrap();

        let mut cf = MockCloudflareApi::new();
        cf.expect_put_tunnel_cname()
            .withf(|zone, name, tid| {
                zone == "example.com" && name == "a.example.com" && tid == "tid"
            })
            .returning(|_, _, _| Ok(()));

        let ingress = CloudflaredIngress::new(
            path.clone(),
            "tid".into(),
            "example.com".into(),
            vec!["true".into()], // restart command that succeeds
            std::sync::Arc::new(cf),
        );
        ingress
            .upsert("a.example.com", 8002, CollectSink::new())
            .await
            .unwrap();
    }
}
