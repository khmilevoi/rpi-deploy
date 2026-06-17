use std::collections::BTreeMap;
use std::path::PathBuf;

/// Project secrets: key -> value (§4). Values never leave the agent unmasked.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct EnvBundle {
    pub vars: BTreeMap<String, String>,
}

impl EnvBundle {
    pub fn is_empty(&self) -> bool {
        self.vars.is_empty()
    }

    /// Key names only (sorted, BTreeMap order) — what `pi env ls` shows (§10).
    pub fn keys(&self) -> Vec<String> {
        self.vars.keys().cloned().collect()
    }
}

impl std::fmt::Debug for EnvBundle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EnvBundle")
            .field("len", &self.vars.len())
            .field("keys", &self.keys())
            .finish()
    }
}

/// Deploy gate settings from [healthcheck] in pi.toml (§8, §12).
/// Per-deploy input: travels with ProjectConfig, not persisted in the registry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HealthcheckConfig {
    /// HTTP probe path; None => plain TCP connect (when no docker healthcheck).
    pub path: Option<String>,
    /// Expected HTTP status: "2xx" | "3xx" | exact like "204". None => 2xx/3xx.
    pub expect: Option<String>,
    /// Total gate budget in seconds.
    pub timeout_secs: u64,
}

impl Default for HealthcheckConfig {
    fn default() -> HealthcheckConfig {
        HealthcheckConfig {
            path: None,
            expect: None,
            timeout_secs: 60,
        }
    }
}

/// Per-stage deploy timeouts (§8.1). Agent-wide defaults live in agent.toml.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StageTimeouts {
    pub fetch_secs: u64,
    pub build_secs: u64,
    pub up_secs: u64,
}

impl Default for StageTimeouts {
    fn default() -> StageTimeouts {
        StageTimeouts {
            fetch_secs: 120,
            build_secs: 1800,
            up_secs: 300,
        }
    }
}

impl StageTimeouts {
    /// Project overrides from [timeouts] in pi.toml win over agent defaults (§12).
    pub fn with_overrides(&self, overrides: &StageTimeoutOverrides) -> StageTimeouts {
        StageTimeouts {
            fetch_secs: overrides.fetch_secs.unwrap_or(self.fetch_secs),
            build_secs: overrides.build_secs.unwrap_or(self.build_secs),
            up_secs: overrides.up_secs.unwrap_or(self.up_secs),
        }
    }
}

/// Optional per-project overrides ([timeouts] in pi.toml, §12).
/// Travels with ProjectConfig like HealthcheckConfig; not persisted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct StageTimeoutOverrides {
    pub fetch_secs: Option<u64>,
    pub build_secs: Option<u64>,
    pub up_secs: Option<u64>,
}

/// How the host port is bound. `Lan` binds `0.0.0.0` (all interfaces).
///
/// NOTE: on a host with a public IPv4, `Lan` exposes the service to the
/// public internet, not just the LAN. Docker also bypasses host firewalls
/// (UFW/iptables) for published ports via its own `DOCKER` iptables chain,
/// so an operator's firewall rules will not block the port. Use `Lan` only
/// on trusted networks or behind an external firewall/router.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ExposeMode {
    #[default]
    Private,
    Lan,
}

impl ExposeMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            ExposeMode::Private => "private",
            ExposeMode::Lan => "lan",
        }
    }

    pub fn bind_addr(&self) -> &'static str {
        match self {
            ExposeMode::Private => "127.0.0.1",
            ExposeMode::Lan => "0.0.0.0",
        }
    }

    pub fn parse(s: &str) -> Option<ExposeMode> {
        match s {
            "private" => Some(ExposeMode::Private),
            "lan" => Some(ExposeMode::Lan),
            _ => None,
        }
    }
}

impl std::fmt::Display for ExposeMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Project config from pi.toml (received in deploy request, §12).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectConfig {
    pub name: String,
    pub repo: String,
    pub branch: String,
    /// Path to compose file relative to repo root.
    pub compose_path: String,
    /// Public service from compose ([ingress].service).
    pub service: String,
    /// Container port of the public service ([ingress].port).
    pub container_port: u16,
    /// FQDN ([ingress].hostname). In v0.1 only stored (ingress — v0.2).
    pub hostname: Option<String>,
    /// How the host port should be exposed. Defaults private for existing configs.
    pub expose: ExposeMode,
    /// Health gate settings ([healthcheck] from pi.toml). Not persisted in DB.
    pub healthcheck: HealthcheckConfig,
    /// Stage timeout overrides ([timeouts] from pi.toml). Not persisted in DB.
    pub timeouts: StageTimeoutOverrides,
}

/// Registered project: config + allocated host port (§4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    pub config: ProjectConfig,
    pub host_port: u16,
    pub created_at: i64,
}

/// Branch or specific commit-sha (§4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeployRef {
    Branch(String),
    Sha(String),
}

impl DeployRef {
    /// 40 hex characters is a sha, everything else is a branch.
    pub fn parse(s: &str) -> DeployRef {
        let is_sha = s.len() == 40 && s.chars().all(|c| c.is_ascii_hexdigit());
        if is_sha {
            DeployRef::Sha(s.to_string())
        } else {
            DeployRef::Branch(s.to_string())
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            DeployRef::Branch(s) | DeployRef::Sha(s) => s,
        }
    }
}

/// All deployment statuses (§18). Stored as strings in the DB.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeploymentStatus {
    Queued,
    Running,
    Success,
    Failed,
    Canceled,
    Interrupted,
    Superseded,
}

impl DeploymentStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            DeploymentStatus::Queued => "queued",
            DeploymentStatus::Running => "running",
            DeploymentStatus::Success => "success",
            DeploymentStatus::Failed => "failed",
            DeploymentStatus::Canceled => "canceled",
            DeploymentStatus::Interrupted => "interrupted",
            DeploymentStatus::Superseded => "superseded",
        }
    }

    pub fn is_terminal(&self) -> bool {
        !matches!(self, DeploymentStatus::Queued | DeploymentStatus::Running)
    }
}

impl std::str::FromStr for DeploymentStatus {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "queued" => Ok(DeploymentStatus::Queued),
            "running" => Ok(DeploymentStatus::Running),
            "success" => Ok(DeploymentStatus::Success),
            "failed" => Ok(DeploymentStatus::Failed),
            "canceled" => Ok(DeploymentStatus::Canceled),
            "interrupted" => Ok(DeploymentStatus::Interrupted),
            "superseded" => Ok(DeploymentStatus::Superseded),
            _ => Err(()),
        }
    }
}

/// One deployment action (§4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Deployment {
    pub id: String,
    pub project: String,
    pub git_ref: String,
    pub commit_sha: Option<String>,
    pub status: DeploymentStatus,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub log_tail: String,
}

/// Result of Source::fetch — where the code is located and which sha was fetched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchedSource {
    pub workdir: PathBuf,
    pub commit_sha: String,
}

/// State of one service in a compose stack (for `pi ls`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceState {
    pub service: String,
    pub state: String,
    /// Docker healthcheck state ("healthy"/"unhealthy"/"starting"), None when
    /// the service declares no healthcheck.
    pub health: Option<String>,
}

/// What to run: project + absolute paths to compose files.
/// Repository docker-compose.override.yml is discovered by the adapter (§12.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComposeStack {
    pub project_name: String,
    pub workdir: PathBuf,
    pub compose_file: PathBuf,
    pub override_file: PathBuf,
}

/// Live container metrics of one compose service (`pi stats`, v0.4 design §4).
#[derive(Debug, Clone, PartialEq)]
pub struct ServiceStats {
    pub service: String,
    pub cpu_percent: f64,
    pub mem_used_bytes: u64,
    pub mem_limit_bytes: u64,
}

/// Per-project slice of `pi stats`. last_deploy is filled by the GetStats
/// use-case from DeploymentHistory, not by the stats provider.
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectStats {
    pub project: String,
    pub services: Vec<ServiceStats>,
    pub last_deploy: Option<Deployment>,
}

/// Host metrics (sysinfo + DiskProbe).
#[derive(Debug, Clone, PartialEq)]
pub struct HostStats {
    pub cpu_percent: f64,
    pub mem_used_bytes: u64,
    pub mem_total_bytes: u64,
    pub disk_used_percent: u8,
    pub uptime_secs: u64,
}

/// Full `pi stats` payload.
#[derive(Debug, Clone, PartialEq)]
pub struct StatsReport {
    pub host: HostStats,
    pub projects: Vec<ProjectStats>,
}

/// One PASS/FAIL check of `pi doctor` (§14).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiagnosticCheck {
    pub name: String,
    pub passed: bool,
    pub detail: String,
    /// How to fix; only meaningful on failed checks.
    pub hint: Option<String>,
}

/// `pi doctor` result (§14).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DiagnosticReport {
    pub checks: Vec<DiagnosticCheck>,
}

impl DiagnosticReport {
    pub fn all_passed(&self) -> bool {
        self.checks.iter().all(|c| c.passed)
    }
}

/// `pi start|stop|restart` (§16). Maps 1:1 to compose subcommands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleAction {
    Start,
    Stop,
    Restart,
}

impl LifecycleAction {
    pub fn as_str(&self) -> &'static str {
        match self {
            LifecycleAction::Start => "start",
            LifecycleAction::Stop => "stop",
            LifecycleAction::Restart => "restart",
        }
    }
}

impl std::str::FromStr for LifecycleAction {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "start" => Ok(LifecycleAction::Start),
            "stop" => Ok(LifecycleAction::Stop),
            "restart" => Ok(LifecycleAction::Restart),
            _ => Err(()),
        }
    }
}

/// `pi status` summary (v0.4 design §4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentOverview {
    pub version: String,
    pub uptime_secs: u64,
    pub disk_used_percent: u8,
    pub projects: usize,
    pub active_deployments: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_bundle_default_is_empty_and_keys_are_sorted() {
        let mut bundle = EnvBundle::default();
        assert!(bundle.is_empty());
        bundle.vars.insert("Z_KEY".into(), "1".into());
        bundle.vars.insert("A_KEY".into(), "2".into());
        assert!(!bundle.is_empty());
        assert_eq!(
            bundle.keys(),
            vec!["A_KEY".to_string(), "Z_KEY".to_string()]
        );
    }

    #[test]
    fn env_bundle_debug_shows_keys_and_count_without_values() {
        let mut bundle = EnvBundle::default();
        bundle
            .vars
            .insert("API_TOKEN".into(), "raw-token-value".into());
        bundle
            .vars
            .insert("DATABASE_URL".into(), "postgres://secret".into());

        let debug = format!("{bundle:?}");

        assert!(debug.contains("EnvBundle"));
        assert!(debug.contains("len: 2"));
        assert!(debug.contains("API_TOKEN"));
        assert!(debug.contains("DATABASE_URL"));
        assert!(!debug.contains("raw-token-value"));
        assert!(!debug.contains("postgres://secret"));
    }

    #[test]
    fn healthcheck_defaults_match_spec() {
        let hc = HealthcheckConfig::default();
        assert_eq!(hc.path, None);
        assert_eq!(hc.expect, None);
        assert_eq!(hc.timeout_secs, 60);
    }

    #[test]
    fn expose_mode_maps_strings_bind_addrs_and_default() {
        assert_eq!(ExposeMode::default(), ExposeMode::Private);
        assert_eq!(ExposeMode::Private.as_str(), "private");
        assert_eq!(ExposeMode::Lan.as_str(), "lan");
        assert_eq!(ExposeMode::Private.bind_addr(), "127.0.0.1");
        assert_eq!(ExposeMode::Lan.bind_addr(), "0.0.0.0");
        assert_eq!(ExposeMode::parse("private"), Some(ExposeMode::Private));
        assert_eq!(ExposeMode::parse("lan"), Some(ExposeMode::Lan));
        assert_eq!(ExposeMode::parse("public"), None);
    }

    #[test]
    fn parse_40_hex_chars_as_sha() {
        let r = DeployRef::parse("0123456789abcdef0123456789abcdef01234567");
        assert_eq!(
            r,
            DeployRef::Sha("0123456789abcdef0123456789abcdef01234567".into())
        );
    }

    #[test]
    fn parse_anything_else_as_branch() {
        assert_eq!(DeployRef::parse("main"), DeployRef::Branch("main".into()));
        // 40 characters but not hex — this is a branch
        assert_eq!(
            DeployRef::parse("zzzz456789abcdef0123456789abcdef01234567"),
            DeployRef::Branch("zzzz456789abcdef0123456789abcdef01234567".into())
        );
    }

    #[test]
    fn status_roundtrips_through_str() {
        for s in [
            DeploymentStatus::Queued,
            DeploymentStatus::Running,
            DeploymentStatus::Success,
            DeploymentStatus::Failed,
            DeploymentStatus::Canceled,
            DeploymentStatus::Interrupted,
            DeploymentStatus::Superseded,
        ] {
            assert_eq!(s.as_str().parse::<DeploymentStatus>(), Ok(s));
        }
        assert_eq!("bogus".parse::<DeploymentStatus>(), Err(()));
    }

    #[test]
    fn terminal_statuses() {
        assert!(!DeploymentStatus::Queued.is_terminal());
        assert!(!DeploymentStatus::Running.is_terminal());
        for s in [
            DeploymentStatus::Success,
            DeploymentStatus::Failed,
            DeploymentStatus::Canceled,
            DeploymentStatus::Interrupted,
            DeploymentStatus::Superseded,
        ] {
            assert!(s.is_terminal(), "{s:?} must be terminal");
        }
    }

    #[test]
    fn stage_timeouts_defaults_match_spec_and_overrides_win() {
        let defaults = StageTimeouts::default();
        assert_eq!(defaults.fetch_secs, 120, "fetch 2m (§8.1)");
        assert_eq!(defaults.build_secs, 1800, "build 30m (§8.1)");
        assert_eq!(defaults.up_secs, 300, "up 5m (§8.1)");

        let overrides = StageTimeoutOverrides {
            build_secs: Some(3600),
            ..StageTimeoutOverrides::default()
        };
        let effective = defaults.with_overrides(&overrides);
        assert_eq!(effective.fetch_secs, 120, "no override -> default");
        assert_eq!(effective.build_secs, 3600, "override wins");
        assert_eq!(effective.up_secs, 300);
    }

    #[test]
    fn lifecycle_action_roundtrips_through_str() {
        for a in [
            LifecycleAction::Start,
            LifecycleAction::Stop,
            LifecycleAction::Restart,
        ] {
            assert_eq!(a.as_str().parse::<LifecycleAction>(), Ok(a));
        }
        assert_eq!("bogus".parse::<LifecycleAction>(), Err(()));
    }

    #[test]
    fn diagnostic_report_all_passed() {
        let pass = DiagnosticCheck {
            name: "docker daemon".into(),
            passed: true,
            detail: "27.0".into(),
            hint: None,
        };
        let fail = DiagnosticCheck {
            name: "cloudflared unit".into(),
            passed: false,
            detail: "inactive".into(),
            hint: Some("systemctl --user start cloudflared".into()),
        };
        assert!(DiagnosticReport {
            checks: vec![pass.clone()]
        }
        .all_passed());
        assert!(!DiagnosticReport {
            checks: vec![pass, fail]
        }
        .all_passed());
        assert!(
            DiagnosticReport::default().all_passed(),
            "no checks - nothing failed"
        );
    }
}
