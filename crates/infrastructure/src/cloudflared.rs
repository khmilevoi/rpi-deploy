use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use pi_domain::contracts::{Ingress, LogSink};
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

/// `cloudflared tunnel route dns` fails when the record exists — tolerated.
pub(crate) fn is_already_exists(stderr: &str) -> bool {
    let s = stderr.to_lowercase();
    s.contains("already exists") || s.contains("already configured")
}

/// Locally-managed cloudflared (§11): edits config.yml, creates the DNS
/// route, restarts the unit without sudo — and only when the config changed.
pub struct CloudflaredIngress {
    config_path: PathBuf,
    tunnel: String,
    restart: Vec<String>,
}

impl CloudflaredIngress {
    pub fn new(
        config_path: PathBuf,
        tunnel: String,
        restart: Vec<String>,
    ) -> Arc<CloudflaredIngress> {
        Arc::new(CloudflaredIngress {
            config_path,
            tunnel,
            restart,
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
    ) -> Result<(), DomainError> {
        let text = tokio::fs::read_to_string(&self.config_path)
            .await
            .map_err(|e| {
                ingress_err(format!(
                    "cannot read {}: {e}; bootstrap the tunnel first (docs/install-agent-v0.1.md)",
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
            return Ok(());
        }
        let updated = serde_yaml::to_string(&doc).map_err(ingress_err)?;
        tokio::fs::write(&self.config_path, updated)
            .await
            .map_err(ingress_err)?;
        log.line(&format!("ingress: routing {hostname} -> {service}"));

        let mut dns = Command::new("cloudflared");
        dns.args(["tunnel", "route", "dns", &self.tunnel, hostname]);
        match run_capture(dns).await {
            Ok(_) => log.line(&format!("ingress: DNS record created for {hostname}")),
            Err(err) if is_already_exists(&err) => {
                log.line(&format!(
                    "ingress: DNS for {hostname} already exists; leaving as is"
                ));
            }
            Err(err) => return Err(ingress_err(format!("route dns: {err}"))),
        }

        let (program, args) = self
            .restart
            .split_first()
            .ok_or_else(|| ingress_err("empty cloudflared restart command"))?;
        let mut restart_cmd = Command::new(program);
        restart_cmd.args(args);
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
    ) -> Result<(), DomainError> {
        log.line(&format!(
            "ingress: [cloudflared] is not configured in agent.toml; \
             route {hostname} -> http://127.0.0.1:{host_port} manually"
        ));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pi_domain::entities::DeploymentStatus;
    use std::sync::Mutex;

    struct VecSink(Mutex<Vec<String>>);
    impl VecSink {
        fn new() -> Arc<VecSink> {
            Arc::new(VecSink(Mutex::new(vec![])))
        }
    }
    impl LogSink for VecSink {
        fn line(&self, line: &str) {
            self.0.lock().unwrap().push(line.to_string());
        }
        fn finished(&self, _status: DeploymentStatus) {}
    }

    fn doc(yaml: &str) -> serde_yaml::Value {
        serde_yaml::from_str(yaml).unwrap()
    }

    const BASE: &str = "tunnel: home\ncredentials-file: /var/lib/pi/cloudflared/home.json\ningress:\n  - hostname: old.example.com\n    service: http://127.0.0.1:8001\n  - service: http_status:404\n";

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

    #[test]
    fn already_exists_detection() {
        assert!(is_already_exists(
            "... record with that host already exists ..."
        ));
        assert!(is_already_exists(
            "Already configured CNAME for this hostname"
        ));
        assert!(!is_already_exists("connection refused"));
    }

    #[tokio::test]
    async fn missing_config_file_gives_actionable_error() {
        let dir = tempfile::tempdir().unwrap();
        let ingress = CloudflaredIngress::new(
            dir.path().join("config.yml"),
            "home".into(),
            vec!["whatever".into()],
        );
        let err = ingress
            .upsert("a.example.com", 8000, VecSink::new())
            .await
            .unwrap_err();
        assert!(
            matches!(&err, DomainError::Ingress(m) if m.contains("config.yml")),
            "got: {err}"
        );
    }

    #[tokio::test]
    async fn no_diff_skips_dns_and_restart_entirely() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.yml");
        std::fs::write(&path, BASE).unwrap();
        // commands would fail if executed: binaries do not exist
        let ingress = CloudflaredIngress::new(
            path.clone(),
            "home".into(),
            vec!["pi-test-no-such-binary".into()],
        );
        ingress
            .upsert("old.example.com", 8001, VecSink::new())
            .await
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            BASE,
            "file untouched"
        );
    }

    #[tokio::test]
    async fn diff_writes_config_before_running_commands() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.yml");
        std::fs::write(&path, BASE).unwrap();
        let ingress = CloudflaredIngress::new(
            path.clone(),
            "home".into(),
            vec!["pi-test-no-such-binary".into()],
        );
        // `cloudflared` is not on PATH in tests -> route dns fails -> Err,
        // but the config must already be updated (§11 order: config, dns, restart).
        let err = ingress
            .upsert("new.example.com", 8002, VecSink::new())
            .await
            .unwrap_err();
        assert!(matches!(err, DomainError::Ingress(_)));
        assert!(std::fs::read_to_string(&path)
            .unwrap()
            .contains("new.example.com"));
    }
}
