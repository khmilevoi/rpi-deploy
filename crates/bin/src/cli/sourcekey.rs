//! Deploy-key preflight (spec 2026-07-10): before creating a deployment the
//! CLI verifies the agent can read the SSH repo; on denial it registers the
//! key via local `gh` or shows it with instructions and polls until access
//! works. Pure helpers live at the top, orchestration below.

use console::Emoji;

use crate::output::{console_style, Sem};

#[allow(dead_code)] // wired in the next commit (preflight orchestration)
static CHECK: Emoji<'_, '_> = Emoji("✓", "ok");
#[allow(dead_code)] // wired in the next commit (preflight orchestration)
static MARKER: Emoji<'_, '_> = Emoji("▸", ">");
#[allow(dead_code)] // wired in the next commit (preflight orchestration)
static ARROW: Emoji<'_, '_> = Emoji("→", "->");

/// `git@github.com:owner/repo(.git)` or `ssh://git@github.com/owner/repo(.git)`
/// -> `(owner, repo)`. Anything else (incl. GHES hosts) -> None: manual path.
#[allow(dead_code)] // wired in the next commit (preflight orchestration)
pub(crate) fn parse_github_repo(url: &str) -> Option<(String, String)> {
    let rest = url
        .strip_prefix("git@github.com:")
        .or_else(|| url.strip_prefix("ssh://git@github.com/"))?;
    let rest = rest.strip_suffix(".git").unwrap_or(rest);
    let (owner, repo) = rest.split_once('/')?;
    if owner.is_empty() || repo.is_empty() || repo.contains('/') {
        return None;
    }
    Some((owner.to_string(), repo.to_string()))
}

/// Argv for `gh api` registering a read-only deploy key. Pure for tests.
#[allow(dead_code)] // wired in the next commit (preflight orchestration)
pub(crate) fn gh_register_args(owner: &str, repo: &str, title: &str, pubkey: &str) -> Vec<String> {
    vec![
        "api".into(),
        "--method".into(),
        "POST".into(),
        format!("repos/{owner}/{repo}/keys"),
        "-f".into(),
        format!("title={title}"),
        "-f".into(),
        format!("key={pubkey}"),
        "-F".into(),
        "read_only=true".into(),
    ]
}

/// Body of the `deploy key needed` pane. The pubkey itself prints as a plain
/// full-width line above the pane — `LogPane` truncates content to the
/// terminal width and a clipped key can't be copied.
#[allow(dead_code)] // wired in the next commit (preflight orchestration)
pub(crate) fn key_box_lines(repo: &str, error: &str) -> Vec<String> {
    let mut lines = vec![
        format!("The Pi can't read {repo} yet."),
        "Add the key above to the repository as a read-only deploy key:".to_string(),
    ];
    match parse_github_repo(repo) {
        Some((owner, name)) => {
            lines.push(format!(
                "{ARROW} https://github.com/{owner}/{name}/settings/keys/new"
            ));
            lines.push("  (check nothing extra: read-only is the default)".to_string());
        }
        None => lines.push(format!(
            "{ARROW} add it as a read-only deploy key in your git hosting"
        )),
    }
    lines.push(format!("agent said: {error}"));
    lines
}

/// One-line collapsed step, mirroring the pipeline's stage summary style
/// (`✓ label (elapsed)` interactive, `▸ label ok (elapsed)` otherwise).
#[allow(dead_code)] // wired in the next commit (preflight orchestration)
pub(crate) fn done_line(label: &str, elapsed: std::time::Duration, interactive: bool) -> String {
    let elapsed = format!("({})", crate::duration::format_elapsed(elapsed));
    if interactive {
        format!(
            "{} {label} {}",
            console_style(Sem::Success).apply_to(CHECK.to_string()),
            console_style(Sem::Muted).apply_to(elapsed),
        )
    } else {
        format!("{MARKER} {label} ok {elapsed}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_github_repo_accepts_both_ssh_forms() {
        assert_eq!(
            parse_github_repo("git@github.com:khmil/myapp.git"),
            Some(("khmil".into(), "myapp".into()))
        );
        assert_eq!(
            parse_github_repo("ssh://git@github.com/khmil/myapp.git"),
            Some(("khmil".into(), "myapp".into()))
        );
        assert_eq!(
            parse_github_repo("git@github.com:khmil/myapp"),
            Some(("khmil".into(), "myapp".into())),
            ".git suffix optional"
        );
    }

    #[test]
    fn parse_github_repo_rejects_non_github_and_malformed() {
        assert_eq!(
            parse_github_repo("https://github.com/khmil/myapp.git"),
            None
        );
        assert_eq!(parse_github_repo("git@gitlab.com:khmil/myapp.git"), None);
        assert_eq!(parse_github_repo("git@github.com:justowner"), None);
        assert_eq!(parse_github_repo("git@github.com:a/b/c"), None);
        assert_eq!(parse_github_repo("git@github.com:/x.git"), None);
    }

    #[test]
    fn gh_register_args_post_a_read_only_key() {
        let args = gh_register_args("khmil", "myapp", "pi-deploy-myapp", "ssh-ed25519 AAAA");
        assert_eq!(
            args[..4],
            ["api", "--method", "POST", "repos/khmil/myapp/keys"]
        );
        assert!(args.contains(&"title=pi-deploy-myapp".to_string()));
        assert!(args.contains(&"key=ssh-ed25519 AAAA".to_string()));
        assert!(
            args.contains(&"read_only=true".to_string()),
            "never a write key"
        );
    }

    #[test]
    fn key_box_lines_github_variant_links_the_keys_page() {
        let lines = key_box_lines("git@github.com:khmil/myapp.git", "Permission denied");
        let all = lines.join("\n");
        assert!(
            all.contains("can't read git@github.com:khmil/myapp.git"),
            "{all}"
        );
        assert!(
            all.contains("https://github.com/khmil/myapp/settings/keys/new"),
            "{all}"
        );
        assert!(all.contains("agent said: Permission denied"), "{all}");
    }

    #[test]
    fn key_box_lines_non_github_gives_generic_instruction() {
        let lines = key_box_lines("git@gitlab.com:k/m.git", "denied");
        let all = lines.join("\n");
        assert!(
            all.contains("read-only deploy key in your git hosting"),
            "{all}"
        );
        assert!(!all.contains("github.com/"), "{all}");
    }

    #[test]
    fn done_line_non_interactive_is_a_boundary_line() {
        let line = done_line("source access", std::time::Duration::from_secs(1), false);
        assert!(line.contains("source access ok (1.0s)"), "{line}");
    }

    #[test]
    fn done_line_interactive_has_label_and_elapsed() {
        let line = done_line("deploy key added", std::time::Duration::from_secs(83), true);
        assert!(line.contains("deploy key added"), "{line}");
        assert!(line.contains("(1m23s)"), "{line}");
    }
}
