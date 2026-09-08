//! Create and list GitHub pull requests via the `gh` CLI.
//!
//! `gh` reuses the user's existing GitHub auth, including GitHub Enterprise,
//! and keeps tokens out of the daemon config. Every invocation is non-interactive,
//! captured, bounded by a caller-controlled timeout, and killed and reaped when
//! that budget expires.

use crate::command::GitError;
use serde::Deserialize;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

/// Default budget for a `gh` invocation.
pub const GH_TIMEOUT: Duration = Duration::from_secs(60);

/// Maximum number of UTF-8 bytes retained from a pull request body.
pub const MAX_PR_BODY: usize = 4_000;

const ASSIGNED_QUERY: &str = "is:open is:pr assignee:@me archived:false";
const REVIEW_REQUESTED_QUERY: &str = "is:open is:pr review-requested:@me archived:false";
const AUTHORED_QUERY: &str = "is:open is:pr author:@me archived:false";

/// Configuration shared by `gh` operations.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitHubCli {
    /// Executable to launch. A path allows callers and tests to avoid `PATH`.
    pub executable: PathBuf,
    /// Wall-clock budget for one invocation.
    pub timeout: Duration,
    /// Directories to search when `executable` is a bare name, and the `PATH`
    /// handed to the child.
    ///
    /// A GUI launched from Finder inherits `/usr/bin:/bin:/usr/sbin:/sbin`, so
    /// spawning a bare `gh` fails with `NotFound` on a machine where `gh` is
    /// installed and works in every terminal. The daemon resolves the
    /// login-shell `PATH` for exactly this reason and passes it here; empty
    /// leaves the inherited environment alone.
    pub path_entries: Vec<PathBuf>,
}

impl Default for GitHubCli {
    fn default() -> Self {
        Self {
            executable: PathBuf::from("gh"),
            timeout: GH_TIMEOUT,
            path_entries: Vec::new(),
        }
    }
}

impl GitHubCli {
    /// The program actually launched: the configured executable when it names a
    /// path, else the first match along [`Self::path_entries`], else the bare
    /// name so the inherited `PATH` still gets its turn.
    #[must_use]
    pub fn program(&self) -> PathBuf {
        if self.executable.components().count() > 1 {
            return self.executable.clone();
        }
        self.path_entries
            .iter()
            .map(|dir| dir.join(&self.executable))
            .find(|candidate| is_executable_file(candidate))
            .unwrap_or_else(|| self.executable.clone())
    }
}

/// Whether `path` is a regular file the current user may execute.
fn is_executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::metadata(path)
        .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

/// Repository coordinates parsed from a Git remote URL.
///
/// This type deliberately does not classify hosts. A syntactically valid
/// `owner/name` remote on any host is returned; the daemon decides which forge
/// providers it supports.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct GitHubRepo {
    pub host: String,
    pub owner: String,
    pub name: String,
}

impl GitHubRepo {
    /// Stable repository identity used by GitHub's API and the daemon cache.
    #[must_use]
    pub fn name_with_owner(&self) -> String {
        format!("{}/{}", self.owner, self.name)
    }
}

/// Result of a successful `gh pr create`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PullRequest {
    /// URL printed by `gh` (the only thing the GUI needs to show).
    pub url: String,
}

/// A label attached to a listed pull request.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PullRequestLabel {
    pub name: String,
    pub color: String,
}

/// GitHub's aggregate review state for a pull request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PullRequestReviewDecision {
    Approved,
    ChangesRequested,
    ReviewRequired,
    /// A value introduced by GitHub after this crate was released.
    Unknown,
}

/// A pull request returned by the listing query.
///
/// These are transport results owned by `git-service`; the daemon maps them to
/// domain values and derives the viewer's relationship to each pull request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitHubPullRequest {
    pub host: String,
    pub repository: String,
    pub number: u32,
    pub title: String,
    pub body: String,
    pub body_truncated: bool,
    pub url: String,
    pub author: String,
    pub base_ref: String,
    pub head_ref: String,
    pub is_draft: bool,
    pub review_decision: Option<PullRequestReviewDecision>,
    pub labels: Vec<PullRequestLabel>,
    pub assignees: Vec<String>,
    pub review_requests: Vec<String>,
    pub additions: u32,
    pub deletions: u32,
    pub changed_files: u32,
    pub comment_count: u32,
    pub created_at: String,
    pub updated_at: String,
}

/// Complete result of one host-scoped GraphQL query.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PullRequestPage {
    pub pull_requests: Vec<GitHubPullRequest>,
    pub viewer: Option<String>,
    /// Repository identity and GraphQL error message for partial failures.
    pub failures: Vec<(String, String)>,
}

/// Create a pull request from the current branch in `repo`.
///
/// # Errors
/// [`GitError::CommandFailed`] when `gh` is missing, unauthenticated, or the
/// remote rejects the PR; [`GitError::Timeout`] when it hangs.
pub fn create_pull_request(
    repo: &Path,
    title: &str,
    body: &str,
    base: Option<&str>,
) -> Result<PullRequest, GitError> {
    create_pull_request_with_cli(&GitHubCli::default(), repo, title, body, base)
}

/// Configured form of [`create_pull_request`].
///
/// This preserves the existing public API while allowing daemon configuration
/// and tests to select a concrete `gh` executable and timeout.
pub fn create_pull_request_with_cli(
    cli: &GitHubCli,
    repo: &Path,
    title: &str,
    body: &str,
    base: Option<&str>,
) -> Result<PullRequest, GitError> {
    let title = title.trim();
    if title.is_empty() {
        return Err(command_failed(
            cli,
            &["pr".into(), "create".into()],
            -1,
            "pull request title is empty".into(),
        ));
    }

    let mut args = vec![
        "pr".to_string(),
        "create".to_string(),
        "--title".to_string(),
        title.to_string(),
        "--body".to_string(),
        body.to_string(),
    ];
    if let Some(base) = base.filter(|base| !base.is_empty()) {
        args.push("--base".to_string());
        args.push(base.to_string());
    }

    let output = run_gh(cli, Some(repo), &args)?;
    if !output.success() {
        return Err(output.into_error(cli, &args));
    }

    let url = output
        .stdout
        .lines()
        .rev()
        .find(|line| line.starts_with("http://") || line.starts_with("https://"))
        .unwrap_or(output.stdout.trim())
        .trim()
        .to_string();
    if url.is_empty() {
        return Err(command_failed(
            cli,
            &args,
            0,
            "gh pr create produced no URL".into(),
        ));
    }
    tracing::debug!(target: "git", %url, "github.pr_create");
    Ok(PullRequest { url })
}

/// Parse a Git remote URL into host and `owner/name` coordinates.
///
/// Supported forms include scp-style SSH (`git@host:owner/repo.git`), HTTPS,
/// and `ssh://`. The host is not classified, so valid enterprise and non-GitHub
/// hosts are returned as well.
#[must_use]
pub fn parse_remote_url(url: &str) -> Option<GitHubRepo> {
    let url = url.trim();
    if url.is_empty() || url.chars().any(char::is_whitespace) {
        return None;
    }

    let (host, path) = if let Some((scheme, rest)) = url.split_once("://") {
        if !matches!(
            scheme.to_ascii_lowercase().as_str(),
            "http" | "https" | "ssh" | "git"
        ) {
            return None;
        }
        let (authority, path) = rest.split_once('/')?;
        let host = authority.rsplit('@').next()?;
        if host.is_empty() {
            return None;
        }
        (host, path)
    } else {
        let (authority, path) = url.split_once(':')?;
        if authority.contains('/') || authority.is_empty() {
            return None;
        }
        let host = authority.rsplit('@').next()?;
        if host.is_empty() {
            return None;
        }
        (host, path)
    };

    let path = path.trim_matches('/');
    let mut parts = path.split('/');
    let owner = parts.next()?;
    let name = parts.next()?;
    if parts.next().is_some() || owner.is_empty() || name.is_empty() {
        return None;
    }
    let name = name.strip_suffix(".git").unwrap_or(name);
    if name.is_empty() {
        return None;
    }

    Some(GitHubRepo {
        host: host.to_string(),
        owner: owner.to_string(),
        name: name.to_string(),
    })
}

/// List open pull requests for repositories on one host.
///
/// When `include_all_repos` is true, the same request also searches all
/// repositories visible to the active `gh` account for PRs assigned to,
/// review-requested from, or authored by the viewer. Search nodes include their
/// repository identity and are de-duplicated against the explicit repositories.
///
/// # Errors
/// - [`GitError::CommandFailed`] when repositories span hosts, `gh` returns no
///   parseable GraphQL data, or the response is structurally empty.
/// - [`GitError::Io`] when the configured executable cannot be launched.
/// - [`GitError::Timeout`] when the command exceeds `timeout`; it is killed and
///   reaped before this function returns.
pub fn list_pull_requests(
    repos: &[GitHubRepo],
    include_all_repos: bool,
    timeout: Duration,
) -> Result<PullRequestPage, GitError> {
    let cli = GitHubCli {
        timeout,
        ..GitHubCli::default()
    };
    list_pull_requests_with_cli(&cli, repos, include_all_repos)
}

/// Configured form of [`list_pull_requests`].
pub fn list_pull_requests_with_cli(
    cli: &GitHubCli,
    repos: &[GitHubRepo],
    include_all_repos: bool,
) -> Result<PullRequestPage, GitError> {
    if repos.is_empty() {
        return Ok(PullRequestPage::default());
    }

    let host = repos[0].host.as_str();
    if host.is_empty()
        || repos
            .iter()
            .any(|repo| !repo.host.eq_ignore_ascii_case(host))
    {
        return Err(command_failed(
            cli,
            &["api".into(), "graphql".into()],
            -1,
            "list_pull_requests requires non-empty repositories from one host".into(),
        ));
    }

    let repos = unique_repositories(repos);
    let (query, mut fields) = build_query(&repos, include_all_repos);
    let mut args = vec![
        "api".to_string(),
        "graphql".to_string(),
        "--hostname".to_string(),
        host.to_string(),
        "-f".to_string(),
        format!("query={query}"),
    ];
    args.append(&mut fields);

    let output = run_gh(cli, None, &args)?;
    parse_graphql_response(&output, cli, &args, host, &repos, include_all_repos)
}

fn unique_repositories(repos: &[GitHubRepo]) -> Vec<&GitHubRepo> {
    let mut seen = HashSet::new();
    repos
        .iter()
        .filter(|repo| {
            seen.insert(format!(
                "{}/{}",
                repo.owner.to_ascii_lowercase(),
                repo.name.to_ascii_lowercase()
            ))
        })
        .collect()
}

fn build_query(repos: &[&GitHubRepo], include_all_repos: bool) -> (String, Vec<String>) {
    let mut variables = Vec::new();
    let mut fields = Vec::new();
    for (index, repo) in repos.iter().enumerate() {
        variables.push(format!("$p{index}Owner: String!"));
        variables.push(format!("$p{index}Name: String!"));
        fields.extend([
            "-f".to_string(),
            format!("p{index}Owner={}", repo.owner),
            "-f".to_string(),
            format!("p{index}Name={}", repo.name),
        ]);
    }
    if include_all_repos {
        variables.extend([
            "$assignedQuery: String!".to_string(),
            "$reviewRequestedQuery: String!".to_string(),
            "$authoredQuery: String!".to_string(),
        ]);
        fields.extend([
            "-f".to_string(),
            format!("assignedQuery={ASSIGNED_QUERY}"),
            "-f".to_string(),
            format!("reviewRequestedQuery={REVIEW_REQUESTED_QUERY}"),
            "-f".to_string(),
            format!("authoredQuery={AUTHORED_QUERY}"),
        ]);
    }

    let mut query = format!("query({}) {{\n  viewer {{ login }}\n", variables.join(", "));
    for index in 0..repos.len() {
        writeln!(
            query,
            "  p{index}: repository(owner: $p{index}Owner, name: $p{index}Name) {{\n    nameWithOwner\n    pullRequests(states: OPEN, first: 50, orderBy: {{ field: UPDATED_AT, direction: DESC }}) {{\n      totalCount\n      nodes {{ ...PrFields }}\n    }}\n  }}"
        )
        .expect("writing to a String cannot fail");
    }
    if include_all_repos {
        query.push_str(
            "  assigned: search(query: $assignedQuery, type: ISSUE, first: 50) {\n    nodes { ... on PullRequest { ...PrFields } }\n  }\n  reviewRequested: search(query: $reviewRequestedQuery, type: ISSUE, first: 50) {\n    nodes { ... on PullRequest { ...PrFields } }\n  }\n  authored: search(query: $authoredQuery, type: ISSUE, first: 50) {\n    nodes { ... on PullRequest { ...PrFields } }\n  }\n",
        );
    }
    query.push_str(
        "  rateLimit { cost remaining resetAt }\n}\nfragment PrFields on PullRequest {\n  number title url body isDraft createdAt updatedAt\n  author { login }\n  repository { nameWithOwner }\n  baseRefName headRefName reviewDecision\n  additions deletions changedFiles\n  comments { totalCount }\n  labels(first: 8) { nodes { name color } }\n  assignees(first: 8) { nodes { login } }\n  reviewRequests(first: 8) {\n    nodes { requestedReviewer { ... on User { login } ... on Team { slug } } }\n  }\n}\n",
    );
    (query, fields)
}

fn parse_graphql_response(
    output: &GhOutput,
    cli: &GitHubCli,
    args: &[String],
    host: &str,
    repos: &[&GitHubRepo],
    include_all_repos: bool,
) -> Result<PullRequestPage, GitError> {
    let response: RawResponse = serde_json::from_str(&output.stdout).map_err(|error| {
        let detail = command_detail(output, format!("invalid GraphQL response: {error}"));
        command_failed(cli, args, output.status, detail)
    })?;
    let RawResponse {
        data: response_data,
        errors,
    } = response;
    let Some(response_data) = response_data else {
        return Err(command_failed(
            cli,
            args,
            output.status,
            command_detail(output, "GraphQL response contained no data"),
        ));
    };
    let Some(data) = response_data.as_object() else {
        return Err(command_failed(
            cli,
            args,
            output.status,
            command_detail(output, "GraphQL response contained no data"),
        ));
    };
    if data.is_empty() {
        return Err(command_failed(
            cli,
            args,
            output.status,
            command_detail(output, "GraphQL response data was empty"),
        ));
    }

    let viewer = data
        .get("viewer")
        .and_then(|value| serde_json::from_value::<RawActor>(value.clone()).ok())
        .and_then(|actor| actor.login);
    let alias_repositories: HashMap<String, String> = repos
        .iter()
        .enumerate()
        .map(|(index, repo)| (format!("p{index}"), repo.name_with_owner()))
        .collect();
    let mut failures = errors
        .into_iter()
        .filter_map(|error| {
            let alias = error.path.first()?.as_str()?;
            let identity = alias_repositories
                .get(alias)
                .cloned()
                .unwrap_or_else(|| search_failure_identity(alias).to_string());
            Some((identity, error.message))
        })
        .collect::<Vec<_>>();
    let mut pull_requests = Vec::new();
    let mut seen = HashSet::new();

    for (index, repo) in repos.iter().enumerate() {
        let alias = format!("p{index}");
        let Some(value) = data.get(&alias) else {
            push_failure_once(
                &mut failures,
                repo.name_with_owner(),
                "repository alias was absent from GraphQL data".into(),
            );
            continue;
        };
        if value.is_null() {
            if !failures
                .iter()
                .any(|(identity, _)| identity == &repo.name_with_owner())
            {
                push_failure_once(
                    &mut failures,
                    repo.name_with_owner(),
                    "repository returned no data".into(),
                );
            }
            continue;
        }

        let raw_repo = match serde_json::from_value::<RawRepository>(value.clone()) {
            Ok(raw_repo) => raw_repo,
            Err(error) => {
                push_failure_once(
                    &mut failures,
                    repo.name_with_owner(),
                    format!("invalid repository data: {error}"),
                );
                continue;
            }
        };
        let repository = raw_repo
            .name_with_owner
            .filter(|identity| !identity.is_empty())
            .unwrap_or_else(|| repo.name_with_owner());
        for raw in connection_nodes(raw_repo.pull_requests) {
            push_pull_request(&mut pull_requests, &mut seen, host, &repository, raw);
        }
    }

    if include_all_repos {
        for alias in ["assigned", "reviewRequested", "authored"] {
            let Some(value) = data.get(alias) else {
                push_failure_once(
                    &mut failures,
                    search_failure_identity(alias).into(),
                    "search alias was absent from GraphQL data".into(),
                );
                continue;
            };
            if value.is_null() {
                continue;
            }
            let search = match serde_json::from_value::<RawSearch>(value.clone()) {
                Ok(search) => search,
                Err(error) => {
                    push_failure_once(
                        &mut failures,
                        search_failure_identity(alias).into(),
                        format!("invalid search data: {error}"),
                    );
                    continue;
                }
            };
            for raw in connection_nodes(Some(search)) {
                let Some(repository) = raw
                    .repository
                    .as_ref()
                    .and_then(|repo| repo.name_with_owner.as_deref())
                    .filter(|identity| !identity.is_empty())
                else {
                    push_failure_once(
                        &mut failures,
                        search_failure_identity(alias).into(),
                        "search result omitted repository identity".into(),
                    );
                    continue;
                };
                push_pull_request(&mut pull_requests, &mut seen, host, repository, raw.clone());
            }
        }
    }

    Ok(PullRequestPage {
        pull_requests,
        viewer,
        failures,
    })
}

fn search_failure_identity(alias: &str) -> &'static str {
    match alias {
        "assigned" => "@assigned",
        "reviewRequested" => "@review-requested",
        "authored" => "@authored",
        _ => "@graphql",
    }
}

fn push_failure_once(failures: &mut Vec<(String, String)>, identity: String, message: String) {
    if !failures
        .iter()
        .any(|(existing_identity, existing_message)| {
            existing_identity == &identity && existing_message == &message
        })
    {
        failures.push((identity, message));
    }
}

fn connection_nodes<T>(connection: Option<RawConnection<T>>) -> Vec<T> {
    connection
        .and_then(|connection| connection.nodes)
        .unwrap_or_default()
        .into_iter()
        .flatten()
        .collect()
}

fn push_pull_request(
    pull_requests: &mut Vec<GitHubPullRequest>,
    seen: &mut HashSet<(String, String, u32)>,
    host: &str,
    repository: &str,
    raw: RawPullRequest,
) {
    let pull_request = convert_pull_request(host, repository, raw);
    let identity = (
        pull_request.host.to_ascii_lowercase(),
        pull_request.repository.to_ascii_lowercase(),
        pull_request.number,
    );
    if seen.insert(identity) {
        pull_requests.push(pull_request);
    }
}

fn convert_pull_request(host: &str, repository: &str, raw: RawPullRequest) -> GitHubPullRequest {
    let (body, body_truncated) = truncate_body(raw.body.unwrap_or_default());
    GitHubPullRequest {
        host: host.to_string(),
        repository: repository.to_string(),
        number: u32_value(raw.number),
        title: raw.title.unwrap_or_default(),
        body,
        body_truncated,
        url: raw.url.unwrap_or_default(),
        author: raw
            .author
            .and_then(|author| author.login)
            .unwrap_or_default(),
        base_ref: raw.base_ref_name.unwrap_or_default(),
        head_ref: raw.head_ref_name.unwrap_or_default(),
        is_draft: raw.is_draft.unwrap_or(false),
        review_decision: raw.review_decision.as_deref().map(review_decision),
        labels: connection_nodes(raw.labels)
            .into_iter()
            .filter_map(|label| {
                let name = label.name.unwrap_or_default();
                (!name.is_empty()).then(|| PullRequestLabel {
                    name,
                    color: label.color.unwrap_or_default(),
                })
            })
            .collect(),
        assignees: connection_nodes(raw.assignees)
            .into_iter()
            .filter_map(|actor| actor.login)
            .collect(),
        review_requests: connection_nodes(raw.review_requests)
            .into_iter()
            .filter_map(|request| request.requested_reviewer)
            .filter_map(|reviewer| reviewer.login.or(reviewer.slug))
            .collect(),
        additions: u32_value(raw.additions),
        deletions: u32_value(raw.deletions),
        changed_files: u32_value(raw.changed_files),
        comment_count: raw
            .comments
            .and_then(|comments| comments.total_count)
            .map_or(0, u32_value_from_u64),
        created_at: raw.created_at.unwrap_or_default(),
        updated_at: raw.updated_at.unwrap_or_default(),
    }
}

fn review_decision(value: &str) -> PullRequestReviewDecision {
    match value {
        "APPROVED" => PullRequestReviewDecision::Approved,
        "CHANGES_REQUESTED" => PullRequestReviewDecision::ChangesRequested,
        "REVIEW_REQUIRED" => PullRequestReviewDecision::ReviewRequired,
        _ => PullRequestReviewDecision::Unknown,
    }
}

fn u32_value(value: Option<u64>) -> u32 {
    value.map_or(0, u32_value_from_u64)
}

fn u32_value_from_u64(value: u64) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

fn truncate_body(body: String) -> (String, bool) {
    if body.len() <= MAX_PR_BODY {
        return (body, false);
    }
    let mut end = MAX_PR_BODY;
    while !body.is_char_boundary(end) {
        end -= 1;
    }
    (body[..end].to_string(), true)
}

#[derive(Debug)]
struct GhOutput {
    stdout: String,
    stderr: String,
    status: i32,
}

impl GhOutput {
    fn success(&self) -> bool {
        self.status == 0
    }

    fn into_error(self, cli: &GitHubCli, args: &[String]) -> GitError {
        let detail = if self.stderr.trim().is_empty() {
            self.stdout
        } else {
            self.stderr
        };
        command_failed(cli, args, self.status, detail)
    }
}

fn run_gh(cli: &GitHubCli, cwd: Option<&Path>, args: &[String]) -> Result<GhOutput, GitError> {
    use std::os::unix::process::CommandExt as _;

    let program = cli.program();
    let mut command = Command::new(&program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("GH_PROMPT_DISABLED", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GH_NO_UPDATE_NOTIFIER", "1")
        .env("NO_COLOR", "1")
        .env("GH_PAGER", "cat")
        // A separate group lets timeout cleanup terminate helpers spawned by gh,
        // close inherited output pipes, and then reap the direct child reliably.
        .process_group(0);
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }

    // `gh` shells out to `git` for repository detection, so the child needs the
    // same widened PATH we used to find `gh` itself.
    if !cli.path_entries.is_empty() {
        if let Ok(joined) = std::env::join_paths(&cli.path_entries) {
            command.env("PATH", joined);
        }
    }

    tracing::debug!(target: "git", executable = ?program, ?cwd, ?args, "run gh");
    let child = command.spawn()?;
    let pid = child.id();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(child.wait_with_output());
    });

    match rx.recv_timeout(cli.timeout) {
        Ok(Ok(output)) => Ok(GhOutput {
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            status: output.status.code().unwrap_or(-1),
        }),
        Ok(Err(error)) => Err(GitError::Io(error)),
        Err(mpsc::RecvTimeoutError::Timeout) => {
            kill_process_group(pid);
            // Do not return while the worker still owns an unreaped child, but do
            // not block on it forever either: the SIGKILL should close the pipes
            // at once, so a bounded wait reaps the common case and gives up on a
            // descendant wedged in uninterruptible sleep instead of hanging the
            // whole shutdown path (P8).
            let _ = rx.recv_timeout(Duration::from_secs(5));
            Err(GitError::Timeout)
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => Err(GitError::Io(std::io::Error::other(
            "gh worker thread disconnected before reporting a result",
        ))),
    }
}

fn kill_process_group(pid: u32) {
    #[allow(clippy::cast_possible_wrap)]
    let raw = pid as i32;
    let _ = nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(-raw),
        nix::sys::signal::Signal::SIGKILL,
    );
    // A configured executable can move itself to another process group. Kill
    // the direct child as a fallback so the subsequent receive still reaps it.
    let _ = nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(raw),
        nix::sys::signal::Signal::SIGKILL,
    );
}

fn command_failed(cli: &GitHubCli, args: &[String], status: i32, stderr: String) -> GitError {
    let mut full_args = vec![cli.executable.to_string_lossy().into_owned()];
    full_args.extend_from_slice(args);
    GitError::CommandFailed {
        args: full_args,
        status,
        stderr: stderr.trim().to_string(),
    }
}

fn command_detail(output: &GhOutput, fallback: impl Into<String>) -> String {
    let fallback = fallback.into();
    if !output.stderr.trim().is_empty() {
        format!("{fallback}: {}", output.stderr.trim())
    } else if !output.stdout.trim().is_empty() {
        format!("{fallback}: {}", output.stdout.trim())
    } else {
        fallback
    }
}

#[derive(Deserialize)]
struct RawResponse {
    data: Option<Value>,
    #[serde(default)]
    errors: Vec<RawGraphQlError>,
}

#[derive(Deserialize)]
struct RawGraphQlError {
    #[serde(default)]
    path: Vec<Value>,
    #[serde(default)]
    message: String,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawPullRequest {
    #[serde(default)]
    number: Option<u64>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    is_draft: Option<bool>,
    #[serde(default)]
    created_at: Option<String>,
    #[serde(default)]
    updated_at: Option<String>,
    #[serde(default)]
    author: Option<RawActor>,
    #[serde(default)]
    repository: Option<RawRepositoryIdentity>,
    #[serde(default)]
    base_ref_name: Option<String>,
    #[serde(default)]
    head_ref_name: Option<String>,
    #[serde(default)]
    review_decision: Option<String>,
    #[serde(default)]
    additions: Option<u64>,
    #[serde(default)]
    deletions: Option<u64>,
    #[serde(default)]
    changed_files: Option<u64>,
    #[serde(default)]
    comments: Option<RawTotalCount>,
    #[serde(default)]
    labels: Option<RawConnection<RawLabel>>,
    #[serde(default)]
    assignees: Option<RawConnection<RawActor>>,
    #[serde(default)]
    review_requests: Option<RawConnection<RawReviewRequest>>,
}

#[derive(Clone, Deserialize)]
struct RawActor {
    #[serde(default)]
    login: Option<String>,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawRepositoryIdentity {
    #[serde(default)]
    name_with_owner: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawRepository {
    #[serde(default)]
    name_with_owner: Option<String>,
    #[serde(default)]
    pull_requests: Option<RawConnection<RawPullRequest>>,
}

type RawSearch = RawConnection<RawPullRequest>;

#[derive(Clone, Deserialize)]
#[serde(bound(deserialize = "T: Deserialize<'de>"))]
struct RawConnection<T> {
    #[serde(default)]
    nodes: Option<Vec<Option<T>>>,
}

#[derive(Clone, Deserialize)]
struct RawLabel {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    color: Option<String>,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawTotalCount {
    #[serde(default)]
    total_count: Option<u64>,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawReviewRequest {
    #[serde(default)]
    requested_reviewer: Option<RawReviewer>,
}

#[derive(Clone, Deserialize)]
struct RawReviewer {
    #[serde(default)]
    login: Option<String>,
    #[serde(default)]
    slug: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::time::Instant;
    use tempfile::TempDir;

    const FULL_RESPONSE: &str = r##"{
      "data": {
        "viewer": { "login": "octocat" },
        "p0": {
          "nameWithOwner": "acme/widgets",
          "pullRequests": {
            "totalCount": 1,
            "nodes": [{
              "number": 42,
              "title": "Make widgets faster",
              "url": "https://github.com/acme/widgets/pull/42",
              "body": "Details",
              "isDraft": false,
              "createdAt": "2026-08-20T10:00:00Z",
              "updatedAt": "2026-08-25T10:00:00Z",
              "author": { "login": "alice" },
              "repository": { "nameWithOwner": "acme/widgets" },
              "baseRefName": "main",
              "headRefName": "faster",
              "reviewDecision": "APPROVED",
              "additions": 12,
              "deletions": 3,
              "changedFiles": 2,
              "comments": { "totalCount": 4 },
              "labels": { "nodes": [{ "name": "performance", "color": "ff0000" }] },
              "assignees": { "nodes": [{ "login": "octocat" }] },
              "reviewRequests": { "nodes": [
                { "requestedReviewer": { "login": "bob" } },
                { "requestedReviewer": { "slug": "core-team" } }
              ] }
            }]
          }
        },
        "rateLimit": { "cost": 1, "remaining": 4999 }
      }
    }"##;

    fn repo(host: &str, owner: &str, name: &str) -> GitHubRepo {
        GitHubRepo {
            host: host.into(),
            owner: owner.into(),
            name: name.into(),
        }
    }

    fn output(status: i32, stdout: &str, stderr: &str) -> GhOutput {
        GhOutput {
            stdout: stdout.into(),
            stderr: stderr.into(),
            status,
        }
    }

    fn parse_fixture(
        fixture: &str,
        status: i32,
        repos: &[GitHubRepo],
        include_all_repos: bool,
    ) -> Result<PullRequestPage, GitError> {
        let refs = repos.iter().collect::<Vec<_>>();
        parse_graphql_response(
            &output(status, fixture, "gh: GraphQL error"),
            &GitHubCli::default(),
            &["api".into(), "graphql".into()],
            &repos[0].host,
            &refs,
            include_all_repos,
        )
    }

    #[test]
    fn parses_remote_url_forms_without_classifying_the_host() {
        let expected = repo("github.com", "owner", "widgets");
        for url in [
            "git@github.com:owner/widgets.git",
            "git@github.com:owner/widgets",
            "https://github.com/owner/widgets.git",
            "ssh://git@github.com/owner/widgets",
        ] {
            assert_eq!(parse_remote_url(url), Some(expected.clone()), "{url}");
        }

        assert_eq!(
            parse_remote_url("https://ghe.example.test/team/service.git"),
            Some(repo("ghe.example.test", "team", "service"))
        );
        assert_eq!(
            parse_remote_url("git@gitlab.example.test:team/service.git"),
            Some(repo("gitlab.example.test", "team", "service")),
            "host classification belongs to the daemon"
        );
        for invalid in [
            "",
            "/local/repository",
            "file:///owner/repo",
            "https://github.com/owner",
            "https://github.com/owner/repo/extra",
            "git@github.com:owner/.git",
        ] {
            assert_eq!(parse_remote_url(invalid), None, "{invalid}");
        }
    }

    #[test]
    fn parses_a_complete_graphql_fixture() {
        let page = parse_fixture(
            FULL_RESPONSE,
            0,
            &[repo("github.com", "acme", "widgets")],
            false,
        )
        .unwrap();
        assert_eq!(page.viewer.as_deref(), Some("octocat"));
        assert!(page.failures.is_empty());
        assert_eq!(page.pull_requests.len(), 1);

        let pr = &page.pull_requests[0];
        assert_eq!(pr.repository, "acme/widgets");
        assert_eq!(pr.number, 42);
        assert_eq!(pr.title, "Make widgets faster");
        assert_eq!(pr.body, "Details");
        assert!(!pr.body_truncated);
        assert_eq!(pr.author, "alice");
        assert_eq!(pr.base_ref, "main");
        assert_eq!(pr.head_ref, "faster");
        assert_eq!(
            pr.review_decision,
            Some(PullRequestReviewDecision::Approved)
        );
        assert_eq!(
            pr.labels,
            vec![PullRequestLabel {
                name: "performance".into(),
                color: "ff0000".into()
            }]
        );
        assert_eq!(pr.assignees, ["octocat"]);
        assert_eq!(pr.review_requests, ["bob", "core-team"]);
        assert_eq!((pr.additions, pr.deletions, pr.changed_files), (12, 3, 2));
        assert_eq!(pr.comment_count, 4);
    }

    #[test]
    fn preserves_good_repositories_from_an_exit_one_partial_response() {
        let fixture = r#"{
          "data": {
            "viewer": { "login": "octocat" },
            "p0": { "nameWithOwner": "acme/good", "pullRequests": {
              "totalCount": 1,
              "nodes": [{ "number": 7, "title": "Good", "body": null,
                "author": null, "labels": null, "assignees": { "nodes": null },
                "reviewRequests": null, "comments": null }]
            } },
            "p1": null
          },
          "errors": [{
            "type": "NOT_FOUND",
            "path": ["p1"],
            "message": "Could not resolve repository"
          }]
        }"#;
        let repos = [
            repo("github.com", "acme", "good"),
            repo("github.com", "acme", "private"),
        ];
        let page = parse_fixture(fixture, 1, &repos, false).unwrap();
        assert_eq!(page.pull_requests.len(), 1);
        assert_eq!(page.pull_requests[0].repository, "acme/good");
        assert_eq!(page.pull_requests[0].body, "");
        assert_eq!(page.pull_requests[0].author, "");
        assert_eq!(
            page.failures,
            [("acme/private".into(), "Could not resolve repository".into())]
        );
    }

    #[test]
    fn a_repository_with_no_pull_requests_is_not_an_error() {
        let fixture = r#"{
          "data": {
            "viewer": { "login": "octocat" },
            "p0": { "nameWithOwner": "acme/quiet", "pullRequests": {
              "totalCount": 0, "nodes": []
            } }
          }
        }"#;
        let page =
            parse_fixture(fixture, 0, &[repo("github.com", "acme", "quiet")], false).unwrap();
        assert!(page.pull_requests.is_empty());
        assert!(page.failures.is_empty());
    }

    #[test]
    fn all_repository_searches_keep_repository_identity_and_deduplicate() {
        let fixture = r#"{
          "data": {
            "viewer": { "login": "octocat" },
            "p0": { "nameWithOwner": "acme/widgets", "pullRequests": {
              "nodes": [{ "number": 1, "title": "Explicit" }]
            } },
            "assigned": { "nodes": [
              { "number": 1, "title": "Duplicate", "repository": { "nameWithOwner": "acme/widgets" } },
              { "number": 2, "title": "Assigned", "repository": { "nameWithOwner": "other/thing" } }
            ] },
            "reviewRequested": { "nodes": [
              { "number": 3, "title": "Review", "repository": { "nameWithOwner": "third/repo" } }
            ] },
            "authored": { "nodes": [
              { "number": 4, "title": "Authored", "repository": { "nameWithOwner": "fourth/repo" } }
            ] }
          }
        }"#;
        let page =
            parse_fixture(fixture, 0, &[repo("github.com", "acme", "widgets")], true).unwrap();
        assert_eq!(page.pull_requests.len(), 4);
        assert_eq!(page.pull_requests[0].title, "Explicit");
        assert_eq!(page.pull_requests[1].repository, "other/thing");
        assert_eq!(page.pull_requests[2].repository, "third/repo");
        assert_eq!(page.pull_requests[3].repository, "fourth/repo");
    }

    #[test]
    fn truncates_pull_request_bodies_on_a_utf8_boundary() {
        let body = format!("{}éafter", "a".repeat(MAX_PR_BODY - 1));
        let fixture = serde_json::json!({
            "data": {
                "viewer": { "login": "octocat" },
                "p0": {
                    "nameWithOwner": "acme/widgets",
                    "pullRequests": { "nodes": [{ "number": 1, "body": body }] }
                }
            }
        })
        .to_string();
        let page =
            parse_fixture(&fixture, 0, &[repo("github.com", "acme", "widgets")], false).unwrap();
        let pr = &page.pull_requests[0];
        assert!(pr.body_truncated);
        assert_eq!(pr.body.len(), MAX_PR_BODY - 1);
        assert!(pr.body.is_char_boundary(pr.body.len()));
    }

    #[test]
    fn malformed_or_absent_graphql_data_is_a_global_error() {
        let repos = [repo("github.com", "acme", "widgets")];
        assert!(parse_fixture("not json", 1, &repos, false).is_err());
        assert!(parse_fixture(r#"{"data":null}"#, 1, &repos, false).is_err());
        assert!(parse_fixture(r#"{"data":{}}"#, 1, &repos, false).is_err());
    }

    #[test]
    fn query_uses_variables_aliases_and_all_three_optional_searches() {
        let repos = [
            repo("github.com", "acme", "one"),
            repo("github.com", "acme", "two"),
        ];
        let refs = repos.iter().collect::<Vec<_>>();
        let (query, fields) = build_query(&refs, true);
        assert!(query.contains("p0: repository(owner: $p0Owner, name: $p0Name)"));
        assert!(query.contains("p1: repository(owner: $p1Owner, name: $p1Name)"));
        assert!(query.contains("assigned: search"));
        assert!(query.contains("reviewRequested: search"));
        assert!(query.contains("authored: search"));
        assert!(query.contains("repository { nameWithOwner }"));
        assert!(fields.iter().any(|field| field == "p0Owner=acme"));
        assert!(fields.iter().any(|field| field
            == "reviewRequestedQuery=is:open is:pr review-requested:@me archived:false"));
    }

    #[test]
    fn configured_list_path_parses_stdout_from_exit_one_without_network() {
        let temp = TempDir::new().unwrap();
        let fixture_path = temp.path().join("response.json");
        let args_path = temp.path().join("args");
        fs::write(&fixture_path, FULL_RESPONSE).unwrap();
        let script = write_script(
            temp.path(),
            "fake-gh",
            &format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"{}\"\n/bin/cat \"{}\"\nexit 1\n",
                args_path.display(),
                fixture_path.display()
            ),
        );
        let cli = GitHubCli {
            executable: script,
            timeout: Duration::from_secs(2),
            ..GitHubCli::default()
        };

        let page =
            list_pull_requests_with_cli(&cli, &[repo("github.com", "acme", "widgets")], false)
                .unwrap();
        assert_eq!(page.pull_requests.len(), 1);
        assert_eq!(page.pull_requests[0].repository, "acme/widgets");

        let args = fs::read_to_string(args_path).unwrap();
        assert!(args.contains("graphql"));
        assert!(args.contains("--hostname\ngithub.com"));
        assert!(args.contains("p0Owner=acme"));
        assert!(args.contains("p0Name=widgets"));
    }

    #[test]
    fn gh_runner_uses_configured_executable_cwd_environment_and_captures_exit_one_stdout() {
        let temp = TempDir::new().unwrap();
        let script = write_script(
            temp.path(),
            "fake-gh",
            r#"#!/bin/sh
printf '%s\n' "$PWD"
printf '%s|%s|%s|%s|%s\n' "$GH_PROMPT_DISABLED" "$GIT_TERMINAL_PROMPT" "$GH_NO_UPDATE_NOTIFIER" "$NO_COLOR" "$GH_PAGER"
printf '%s\n' 'partial stdout'
printf '%s\n' 'partial stderr' >&2
exit 1
"#,
        );
        let cli = GitHubCli {
            executable: script,
            timeout: Duration::from_secs(2),
            ..GitHubCli::default()
        };
        let output = run_gh(&cli, Some(temp.path()), &["api".into()]).unwrap();
        assert_eq!(output.status, 1);
        assert!(output.stdout.contains(temp.path().to_str().unwrap()));
        assert!(output.stdout.contains("1|0|1|1|cat"));
        assert!(output.stdout.contains("partial stdout"));
        assert!(output.stderr.contains("partial stderr"));
    }

    #[test]
    fn gh_runner_kills_and_reaps_a_timed_out_process() {
        let temp = TempDir::new().unwrap();
        let pid_path = temp.path().join("pid");
        let script = write_script(
            temp.path(),
            "hanging-gh",
            r#"#!/bin/sh
printf '%s' "$$" > "$1"
while :; do sleep 1; done
"#,
        );
        let cli = GitHubCli {
            executable: script,
            timeout: Duration::from_millis(500),
            ..GitHubCli::default()
        };
        let started = Instant::now();
        let error = run_gh(&cli, None, &[pid_path.to_string_lossy().into_owned()]).unwrap_err();
        assert!(matches!(error, GitError::Timeout));
        assert!(started.elapsed() < Duration::from_secs(2));

        // Under heavy parallel test load the child can be killed before its
        // first instruction. If it did record a PID, verify no zombie remains.
        if let Ok(pid) = fs::read_to_string(pid_path) {
            let pid: i32 = pid.parse().unwrap();
            assert!(nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).is_err());
        }
    }

    #[test]
    fn a_bare_executable_is_resolved_along_the_configured_path_entries() {
        // The bug this covers: a GUI launched from Finder inherits
        // /usr/bin:/bin:/usr/sbin:/sbin, so a bare `gh` installed by Homebrew
        // is NotFound even though every terminal on the machine runs it.
        let empty = TempDir::new().unwrap();
        let temp = TempDir::new().unwrap();
        let script = write_script(temp.path(), "gh", "#!/bin/sh\nexit 0\n");

        let cli = GitHubCli {
            path_entries: vec![empty.path().to_path_buf(), temp.path().to_path_buf()],
            ..GitHubCli::default()
        };
        assert_eq!(cli.program(), script, "the first executable match wins");

        // Nothing on the search path: the bare name is kept so the inherited
        // PATH still gets its turn rather than being pre-empted by a guess.
        let unresolved = GitHubCli {
            path_entries: vec![empty.path().to_path_buf()],
            ..GitHubCli::default()
        };
        assert_eq!(unresolved.program(), PathBuf::from("gh"));
    }

    #[test]
    fn a_configured_path_is_never_searched_for() {
        let temp = TempDir::new().unwrap();
        let decoy = write_script(temp.path(), "gh", "#!/bin/sh\nexit 0\n");
        let cli = GitHubCli {
            executable: PathBuf::from("/opt/homebrew/bin/gh"),
            path_entries: vec![temp.path().to_path_buf()],
            ..GitHubCli::default()
        };
        assert_ne!(cli.program(), decoy);
        assert_eq!(cli.program(), PathBuf::from("/opt/homebrew/bin/gh"));
    }

    #[test]
    fn a_non_executable_file_of_the_right_name_is_not_taken_for_the_cli() {
        let temp = TempDir::new().unwrap();
        fs::write(temp.path().join("gh"), "not a program").unwrap();
        let cli = GitHubCli {
            path_entries: vec![temp.path().to_path_buf()],
            ..GitHubCli::default()
        };
        assert_eq!(cli.program(), PathBuf::from("gh"));
    }

    #[test]
    fn the_child_receives_the_configured_path() {
        // `gh` shells out to `git`; a child left with the GUI's PATH would fail
        // for the same reason the parent lookup did.
        let temp = TempDir::new().unwrap();
        let bin = TempDir::new().unwrap();
        let script = write_script(
            temp.path(),
            "echo-path",
            "#!/bin/sh\nprintf '%s' \"$PATH\"\n",
        );
        let cli = GitHubCli {
            executable: script,
            timeout: Duration::from_secs(2),
            path_entries: vec![bin.path().to_path_buf()],
        };
        let output = run_gh(&cli, None, &[]).unwrap();
        assert_eq!(output.stdout, bin.path().to_string_lossy());
    }

    fn write_script(dir: &Path, name: &str, contents: &str) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, contents).unwrap();
        let mut permissions = fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&path, permissions).unwrap();
        path
    }
}
