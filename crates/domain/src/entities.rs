use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectConfig {
    pub name: String,
    pub repo: String,
    pub branch: String,
    pub compose_path: String,
    pub service: String,
    pub container_port: u16,
    pub hostname: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    pub config: ProjectConfig,
    pub host_port: u16,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeployRef {
    Branch(String),
    Sha(String),
}

impl DeployRef {
    pub fn parse(s: &str) -> DeployRef {
        let is_sha = s.len() == 40 && s.chars().all(|c| c.is_ascii_hexdigit());
        if is_sha { DeployRef::Sha(s.to_string()) } else { DeployRef::Branch(s.to_string()) }
    }

    pub fn as_str(&self) -> &str {
        match self {
            DeployRef::Branch(s) | DeployRef::Sha(s) => s,
        }
    }
}

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

    pub fn from_str(s: &str) -> Option<DeploymentStatus> {
        match s {
            "running" => Some(DeploymentStatus::Running),
            "success" => Some(DeploymentStatus::Success),
            "failed" => Some(DeploymentStatus::Failed),
            _ => None,
        }
    }

    pub fn is_terminal(&self) -> bool {
        !matches!(self, DeploymentStatus::Running)
    }
}

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchedSource {
    pub workdir: PathBuf,
    pub commit_sha: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceState {
    pub service: String,
    pub state: String,
}

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
        assert_eq!(r, DeployRef::Sha("0123456789abcdef0123456789abcdef01234567".into()));
    }

    #[test]
    fn parse_anything_else_as_branch() {
        assert_eq!(DeployRef::parse("main"), DeployRef::Branch("main".into()));
        // 40 chars but not hex — it's a branch
        assert_eq!(
            DeployRef::parse("zzzz456789abcdef0123456789abcdef01234567"),
            DeployRef::Branch("zzzz456789abcdef0123456789abcdef01234567".into())
        );
    }

    #[test]
    fn status_roundtrips_through_str() {
        for s in [DeploymentStatus::Running, DeploymentStatus::Success, DeploymentStatus::Failed] {
            assert_eq!(DeploymentStatus::from_str(s.as_str()), Some(s));
        }
        assert_eq!(DeploymentStatus::from_str("bogus"), None);
    }

    #[test]
    fn terminal_statuses() {
        assert!(!DeploymentStatus::Running.is_terminal());
        assert!(DeploymentStatus::Success.is_terminal());
        assert!(DeploymentStatus::Failed.is_terminal());
    }
}
