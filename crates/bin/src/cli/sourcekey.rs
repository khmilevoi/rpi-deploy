//! Deploy-key preflight (spec 2026-07-10): before creating a deployment the
//! CLI verifies the agent can read the SSH repo; on denial it registers the
//! key via local `gh` or shows it with instructions and polls until access
//! works. Pure helpers live at the top, orchestration below.

use console::Emoji;

use crate::cli::api::ApiClient;
use crate::output;
use crate::output::{console_style, Sem};

static CHECK: Emoji<'_, '_> = Emoji("✓", "ok");
static MARKER: Emoji<'_, '_> = Emoji("▸", ">");
static ARROW: Emoji<'_, '_> = Emoji("→", "->");

/// `git@github.com:owner/repo(.git)` or `ssh://git@github.com/owner/repo(.git)`
/// -> `(owner, repo)`. Anything else (incl. GHES hosts) -> None: manual path.
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

const POLL_INTERVAL_SECS: u64 = 5;
const POLL_TIMEOUT_SECS: u64 = 600;

/// Deploy-key preflight (spec 2026-07-10): verify the agent can read the
/// repo before creating a deployment. `Ok(())` — proceed with the deploy;
/// `Err` — abort, the explanation is already on screen.
pub async fn preflight(
    gh: &dyn Gh,
    api: &ApiClient,
    project: &str,
    repo: &str,
    no_gh_key: bool,
) -> anyhow::Result<()> {
    if !pi_infrastructure::git::is_ssh_repo(repo) {
        return Ok(());
    }
    let started = std::time::Instant::now();
    let interactive = console::Term::stdout().features().is_attended();
    let Some(first) = api.source_check(project, repo).await? else {
        return Ok(()); // old agent: no route; the fetch stage still hints
    };
    if first.ok {
        println!(
            "{}",
            done_line("source access", started.elapsed(), interactive)
        );
        return Ok(());
    }
    let Some(pubkey) = first.pubkey else {
        anyhow::bail!(
            "agent can't read {repo} and returned no deploy key: {}",
            first.error.as_deref().unwrap_or("unknown error")
        );
    };
    let error = first.error.unwrap_or_else(|| "access denied".to_string());

    if !no_gh_key && try_gh_register(gh, api, project, repo, &pubkey).await? {
        println!(
            "{}",
            done_line(
                "deploy key registered via gh",
                started.elapsed(),
                interactive
            )
        );
        return Ok(());
    }

    // Manual path: full-width copyable key above the pane (LogPane truncates
    // to terminal width), instructions inside it.
    println!("{pubkey}");
    let mut pane = output::LogPane::new("deploy key needed", 12);
    for line in key_box_lines(repo, &error) {
        pane.push_line(&line);
    }
    if !interactive {
        anyhow::bail!("deploy key not registered; add it to the repository and re-run rpi deploy");
    }
    // While waiting for the key to be added by hand, also watch for a
    // background `gh auth login` (github repos, unless --no-gh-key): logging in
    // mid-wait auto-registers the key without a re-run. `gh_prev` is seeded with
    // the current state so only a later transition into "logged in" fires.
    let gh_watch = (!no_gh_key).then(|| parse_github_repo(repo)).flatten();
    let title = format!("pi-deploy-{project}");
    let mut gh_prev = match &gh_watch {
        Some(_) => gh.logged_in().await,
        None => None,
    };
    pane.push_line(&if gh_watch.is_some() {
        format!("waiting for access… (checking every {POLL_INTERVAL_SECS}s — also watching for gh login, Ctrl+C to abort)")
    } else {
        format!("waiting for access… (checking every {POLL_INTERVAL_SECS}s, Ctrl+C to abort)")
    });
    let deadline = started + std::time::Duration::from_secs(POLL_TIMEOUT_SECS);
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                pane.clear();
                anyhow::bail!("aborted; add the deploy key and re-run rpi deploy");
            }
            _ = tokio::time::sleep(std::time::Duration::from_secs(POLL_INTERVAL_SECS)) => {}
        }
        if std::time::Instant::now() >= deadline {
            pane.clear();
            anyhow::bail!(
                "deploy key was not added within 10 minutes; add it and re-run rpi deploy"
            );
        }
        if let Some((owner, name)) = &gh_watch {
            let (next, line) = gh_watch_step(gh, gh_prev, owner, name, &title, &pubkey).await;
            gh_prev = next;
            if let Some(line) = line {
                pane.push_line(&line);
            }
        }
        // Transient check failures (tunnel hiccup) keep polling to the deadline.
        if let Ok(Some(resp)) = api.source_check(project, repo).await {
            if resp.ok {
                pane.clear();
                println!(
                    "{}",
                    done_line("deploy key added", started.elapsed(), interactive)
                );
                return Ok(());
            }
        }
    }
}

/// GitHub auto-registration. `true` — key registered AND access confirmed.
/// `false` — fall back to the manual box: not a github.com repo, `gh`
/// missing (silent) or logged out / failed (hint printed via output::note).
async fn try_gh_register(
    gh: &dyn Gh,
    api: &ApiClient,
    project: &str,
    repo: &str,
    pubkey: &str,
) -> anyhow::Result<bool> {
    let Some((owner, name)) = parse_github_repo(repo) else {
        return Ok(false);
    };
    match gh.logged_in().await {
        None => return Ok(false), // gh not installed
        Some(false) => {
            output::note("gh is not logged in (run: gh auth login) — add the key manually below");
            return Ok(false);
        }
        Some(true) => {}
    }
    output::status(format!(
        "registering read-only deploy key via gh ({owner}/{name})…"
    ));
    let title = format!("pi-deploy-{project}");
    if let Err(e) = gh.register_key(&owner, &name, &title, pubkey).await {
        output::note(format!(
            "gh couldn't register the key ({e}) — add it manually below"
        ));
        return Ok(false);
    }
    if let Some(resp) = api.source_check(project, repo).await? {
        if resp.ok {
            return Ok(true);
        }
    }
    output::note("key registered but access not confirmed yet — waiting below");
    Ok(false)
}

/// Whether a change in `gh` login state should (re)trigger auto-registration:
/// only a fresh transition into "logged in" counts, so a key that failed to
/// register (missing repo scope, say) isn't retried on every poll tick.
fn gh_login_became_available(prev: Option<bool>, now: Option<bool>) -> bool {
    now == Some(true) && prev != Some(true)
}

/// One `gh`-login watch step for the wait loop. Re-checks login through `gh`
/// and, on a fresh logout→login transition, registers the deploy key. Returns
/// the login state to carry into the next tick plus an optional pane line.
async fn gh_watch_step(
    gh: &dyn Gh,
    prev: Option<bool>,
    owner: &str,
    name: &str,
    title: &str,
    pubkey: &str,
) -> (Option<bool>, Option<String>) {
    let now = gh.logged_in().await;
    if !gh_login_became_available(prev, now) {
        return (now, None);
    }
    let line = match gh.register_key(owner, name, title, pubkey).await {
        Ok(()) => "gh login detected — deploy key registered; confirming access…".to_string(),
        Err(e) => {
            format!("gh login detected but couldn't register the key ({e}); add it manually above")
        }
    };
    (now, Some(line))
}

/// Local `gh` CLI operations behind a trait so the preflight is testable
/// without shelling out (`ScriptedGh` in tests).
#[async_trait::async_trait]
pub trait Gh: Sync {
    /// `gh auth token`: cheap, no network. `None` — gh missing; `Some(logged_in)`.
    async fn logged_in(&self) -> Option<bool>;
    /// POST a read-only deploy key via `gh api`; `Err` carries gh's first
    /// stderr line.
    async fn register_key(
        &self,
        owner: &str,
        repo: &str,
        title: &str,
        pubkey: &str,
    ) -> Result<(), String>;
}

/// Boundary to the real `gh` binary.
pub struct GhCli;

#[async_trait::async_trait]
impl Gh for GhCli {
    async fn logged_in(&self) -> Option<bool> {
        let out = tokio::process::Command::new("gh")
            .args(["auth", "token"])
            .stdin(std::process::Stdio::null())
            .output()
            .await;
        match out {
            Ok(o) => Some(o.status.success()),
            Err(_) => None,
        }
    }

    async fn register_key(
        &self,
        owner: &str,
        repo: &str,
        title: &str,
        pubkey: &str,
    ) -> Result<(), String> {
        let out = tokio::process::Command::new("gh")
            .args(gh_register_args(owner, repo, title, pubkey))
            .stdin(std::process::Stdio::null())
            .output()
            .await
            .map_err(|e| format!("gh: {e}"))?;
        if out.status.success() {
            return Ok(());
        }
        let stderr = String::from_utf8_lossy(&out.stderr);
        Err(stderr
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .unwrap_or("gh api failed")
            .to_string())
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

    use crate::cli::api::ApiClient;
    use axum::response::IntoResponse;
    use axum::routing::post;
    use axum::Router;

    /// Ephemeral local agent stand-in (same pattern as api.rs tests).
    async fn spawn_app(app: Router) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn preflight_skips_https_repos_without_calling_the_agent() {
        // port 1 would refuse any request — proves no request is made
        let api = ApiClient::new("http://127.0.0.1:1".into());
        preflight(&GhCli, &api, "demo", "https://github.com/x/y.git", true)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn preflight_skips_when_agent_lacks_the_route() {
        let app = Router::new(); // any request -> bare 404 (old agent)
        let api = ApiClient::new(spawn_app(app).await);
        preflight(&GhCli, &api, "demo", "git@github.com:x/y.git", true)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn preflight_passes_when_access_is_ok() {
        async fn ok() -> impl IntoResponse {
            axum::Json(serde_json::json!({ "ok": true }))
        }
        let app = Router::new().route("/v1/projects/demo/source/check", post(ok));
        let api = ApiClient::new(spawn_app(app).await);
        preflight(&GhCli, &api, "demo", "git@github.com:x/y.git", true)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn preflight_denied_without_tty_bails_with_rerun_hint() {
        // cargo test captures stdout -> is_attended() is false -> the manual
        // path prints the key + instructions and bails instead of polling.
        // (Under `--nocapture` on a real terminal this would poll once, get
        // the same denial... and keep polling to the 10-min cap — run
        // normally.) no_gh_key=true keeps `gh` out of the test.
        async fn denied() -> impl IntoResponse {
            axum::Json(serde_json::json!({
                "ok": false,
                "pubkey": "ssh-ed25519 AAAA pi-deploy-demo",
                "error": "Permission denied (publickey)"
            }))
        }
        let app = Router::new().route("/v1/projects/demo/source/check", post(denied));
        let api = ApiClient::new(spawn_app(app).await);
        let err = preflight(&GhCli, &api, "demo", "git@github.com:x/y.git", true)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("re-run rpi deploy"), "{err}");
    }

    /// Scripted `gh` stand-in (mirrors `ScriptedPrompter`): `logged_in` replays
    /// a queue of states, `register_key` returns a fixed result and records its
    /// calls so tests can assert what was registered.
    struct ScriptedGh {
        logins: std::sync::Mutex<std::collections::VecDeque<Option<bool>>>,
        register: Result<(), String>,
        register_calls: std::sync::Mutex<Vec<(String, String, String, String)>>,
    }

    impl ScriptedGh {
        fn new(
            logins: impl IntoIterator<Item = Option<bool>>,
            register: Result<(), String>,
        ) -> Self {
            Self {
                logins: std::sync::Mutex::new(logins.into_iter().collect()),
                register,
                register_calls: std::sync::Mutex::new(Vec::new()),
            }
        }
        fn register_calls(&self) -> usize {
            self.register_calls.lock().unwrap().len()
        }
    }

    #[async_trait::async_trait]
    impl Gh for ScriptedGh {
        async fn logged_in(&self) -> Option<bool> {
            self.logins.lock().unwrap().pop_front().unwrap_or(None)
        }
        async fn register_key(
            &self,
            owner: &str,
            repo: &str,
            title: &str,
            pubkey: &str,
        ) -> Result<(), String> {
            self.register_calls.lock().unwrap().push((
                owner.into(),
                repo.into(),
                title.into(),
                pubkey.into(),
            ));
            self.register.clone()
        }
    }

    #[test]
    fn gh_login_became_available_only_on_fresh_login() {
        assert!(gh_login_became_available(None, Some(true)));
        assert!(gh_login_became_available(Some(false), Some(true)));
        assert!(!gh_login_became_available(Some(true), Some(true)));
        assert!(!gh_login_became_available(Some(true), Some(false)));
        assert!(!gh_login_became_available(Some(false), Some(false)));
        assert!(!gh_login_became_available(None, None));
        assert!(!gh_login_became_available(None, Some(false)));
    }

    #[tokio::test]
    async fn gh_watch_step_registers_on_fresh_login() {
        let gh = ScriptedGh::new([Some(true)], Ok(()));
        let (next, line) = gh_watch_step(
            &gh,
            Some(false),
            "khmil",
            "myapp",
            "pi-deploy-demo",
            "ssh-ed25519 AAAA",
        )
        .await;
        assert_eq!(next, Some(true), "carries the new login state forward");
        assert!(line.unwrap().contains("registered"), "shows a success line");
        assert_eq!(gh.register_calls(), 1, "registers exactly once");
        assert_eq!(
            gh.register_calls.lock().unwrap()[0],
            (
                "khmil".into(),
                "myapp".into(),
                "pi-deploy-demo".into(),
                "ssh-ed25519 AAAA".into()
            ),
            "registers with the repo/title/key it was handed"
        );
    }

    #[tokio::test]
    async fn gh_watch_step_ignores_steady_logged_in_state() {
        let gh = ScriptedGh::new([Some(true)], Ok(()));
        let (next, line) = gh_watch_step(&gh, Some(true), "khmil", "myapp", "t", "k").await;
        assert_eq!(next, Some(true));
        assert!(line.is_none(), "no pane line when nothing changed");
        assert_eq!(gh.register_calls(), 0, "never re-registers a steady state");
    }

    #[tokio::test]
    async fn gh_watch_step_surfaces_registration_error() {
        let gh = ScriptedGh::new([Some(true)], Err("HTTP 403: forbidden".into()));
        let (next, line) = gh_watch_step(&gh, Some(false), "khmil", "myapp", "t", "k").await;
        assert_eq!(next, Some(true), "still advances the login state");
        let line = line.expect("a fresh login always yields a pane line");
        assert!(line.contains("couldn't register"), "{line}");
        assert!(line.contains("HTTP 403"), "carries gh's error text: {line}");
        assert_eq!(gh.register_calls(), 1);
    }

    #[tokio::test]
    async fn gh_watch_step_stays_quiet_while_logged_out() {
        let gh = ScriptedGh::new([Some(false)], Ok(()));
        let (next, line) = gh_watch_step(&gh, Some(false), "khmil", "myapp", "t", "k").await;
        assert_eq!(next, Some(false));
        assert!(line.is_none());
        assert_eq!(gh.register_calls(), 0);
    }
}
