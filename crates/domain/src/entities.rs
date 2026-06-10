use std::path::PathBuf;

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

/// Statuses for v0.1. Others (`queued|canceled|interrupted|superseded`) — v0.3;
/// in the DB status is stored as a string, extending enum does not require migration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeploymentStatus {
    Running,
    Success,
    Failed,
}

impl DeploymentStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            DeploymentStatus::Running => "running",
            DeploymentStatus::Success => "success",
            DeploymentStatus::Failed => "failed",
        }
    }

    pub fn is_terminal(&self) -> bool {
        !matches!(self, DeploymentStatus::Running)
    }
}

impl std::str::FromStr for DeploymentStatus {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "running" => Ok(DeploymentStatus::Running),
            "success" => Ok(DeploymentStatus::Success),
            "failed" => Ok(DeploymentStatus::Failed),
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

#[cfg(test)]
mod tests {
    use super::*;

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
            DeploymentStatus::Running,
            DeploymentStatus::Success,
            DeploymentStatus::Failed,
        ] {
            assert_eq!(s.as_str().parse::<DeploymentStatus>(), Ok(s));
        }
        assert_eq!("bogus".parse::<DeploymentStatus>(), Err(()));
    }

    #[test]
    fn terminal_statuses() {
        assert!(!DeploymentStatus::Running.is_terminal());
        assert!(DeploymentStatus::Success.is_terminal());
        assert!(DeploymentStatus::Failed.is_terminal());
    }
}
