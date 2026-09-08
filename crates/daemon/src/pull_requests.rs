//! Pull-request source resolution, caching, and GitHub result mapping.
//!
//! Remote state is never persisted. The daemon resolves local Git remotes on a
//! worker thread, then performs at most one `gh api graphql` call per eligible
//! host. The core mutex is not held during either kind of subprocess work.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use domain::{
    ProjectId, PullRequest, PullRequestFailure, PullRequestFailureKind, PullRequestLabel,
    PullRequestRelations, PullRequestSource, PullRequestSourceStatus, PullRequestState,
    PullRequestViewer, ReviewDecision, Timestamp,
};
use git_service::{GitError, GitHubPullRequest, GitHubRepo, PullRequestReviewDecision};

use crate::config::GithubConfig;

/// A remote listing is reused for one minute when it answers the same query.
const CACHE_TTL: Duration = Duration::from_secs(60);

/// A completed pull-request refresh.
#[derive(Default)]
pub struct Cache {
    fetched_at: Option<Instant>,
    fingerprint: u64,
    state: PullRequestState,
}

impl Cache {
    /// Return the cached state without resolving Git remotes or invoking `gh`.
    #[must_use]
    pub fn snapshot(&self) -> PullRequestState {
        self.state.clone()
    }

    /// Return a fresh answer when it was produced for the same query.
    #[must_use]
    pub fn fresh(&self, fingerprint: u64) -> Option<PullRequestState> {
        self.fresh_at(fingerprint, Instant::now())
    }

    /// Replace the completed refresh associated with `fingerprint`.
    pub fn update(&mut self, fingerprint: u64, state: PullRequestState) {
        self.update_at(fingerprint, state, Instant::now());
    }

    /// Force the next matching lookup to refresh while retaining the last
    /// snapshot until its replacement arrives.
    pub fn invalidate(&mut self) {
        self.fetched_at = None;
    }

    fn fresh_at(&self, fingerprint: u64, now: Instant) -> Option<PullRequestState> {
        let fresh = self.fingerprint == fingerprint
            && self
                .fetched_at
                .is_some_and(|at| now.saturating_duration_since(at) < CACHE_TTL);
        fresh.then(|| self.state.clone())
    }

    fn update_at(&mut self, fingerprint: u64, state: PullRequestState, now: Instant) {
        self.fingerprint = fingerprint;
        self.state = state;
        self.fetched_at = Some(now);
    }
}

/// The project data copied while the daemon core lock is held.
pub(crate) type ProjectSource = (ProjectId, Option<PathBuf>);

/// One eligible GitHub repository and the Forge project that supplied it.
#[derive(Clone, Debug)]
struct EligibleRepository {
    project_id: ProjectId,
    repository: GitHubRepo,
}

/// Fully resolved local inputs to a host refresh.
pub(crate) struct Query {
    sources: Vec<PullRequestSource>,
    failures: Vec<PullRequestFailure>,
    repositories: Vec<EligibleRepository>,
}

/// Resolve every project's default remote using one local Git command each.
///
/// `github.com` is always eligible. A syntactically valid remote on another
/// host is eligible only when that host is explicitly configured as GHES; URL
/// syntax alone cannot distinguish GitHub Enterprise from GitLab or Bitbucket.
pub(crate) fn resolve(projects: Vec<ProjectSource>, config: &GithubConfig) -> Query {
    let enterprise_hosts: HashSet<String> = config
        .enterprise_hosts
        .iter()
        .filter_map(|host| normalize_configured_host(host))
        .collect();
    let mut projects = projects;
    projects.sort_unstable_by_key(|(project_id, _)| *project_id);

    let mut sources = Vec::with_capacity(projects.len());
    let mut failures = Vec::new();
    let mut repositories = Vec::new();
    for (project_id, git_root) in projects {
        let Some(git_root) = git_root else {
            sources.push(source(
                project_id,
                None,
                None,
                PullRequestSourceStatus::NoGitRepository,
            ));
            continue;
        };

        let remotes = match git_service::list_remotes(&git_root) {
            Ok(remotes) => remotes,
            Err(error) => {
                sources.push(source(
                    project_id,
                    None,
                    None,
                    PullRequestSourceStatus::Failed,
                ));
                failures.push(PullRequestFailure {
                    host: String::new(),
                    repository: None,
                    kind: PullRequestFailureKind::HostRefused,
                    message: format!("could not list project remotes: {error}"),
                });
                continue;
            }
        };
        let Some((_, remote_url)) = remotes
            .iter()
            .find(|(name, _)| name == "origin")
            .or_else(|| remotes.first())
        else {
            sources.push(source(
                project_id,
                None,
                None,
                PullRequestSourceStatus::NoRemote,
            ));
            continue;
        };
        let Some(mut repository) = git_service::parse_remote_url(remote_url) else {
            sources.push(source(
                project_id,
                None,
                None,
                PullRequestSourceStatus::InvalidRemote,
            ));
            continue;
        };

        repository.host.make_ascii_lowercase();
        let identity = repository.name_with_owner();
        let host = repository.host.clone();
        if host != "github.com" && !enterprise_hosts.contains(&host) {
            sources.push(source(
                project_id,
                Some(host),
                Some(identity),
                PullRequestSourceStatus::UnsupportedHost,
            ));
            continue;
        }

        sources.push(source(
            project_id,
            Some(host),
            Some(identity),
            PullRequestSourceStatus::Ready,
        ));
        repositories.push(EligibleRepository {
            project_id,
            repository,
        });
    }

    Query {
        sources,
        failures,
        repositories,
    }
}

fn normalize_configured_host(host: &str) -> Option<String> {
    let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
    (!host.is_empty()).then_some(host)
}

fn source(
    project_id: ProjectId,
    host: Option<String>,
    repository: Option<String>,
    status: PullRequestSourceStatus,
) -> PullRequestSource {
    PullRequestSource {
        project_id,
        host,
        repository,
        status,
    }
}

/// Fingerprint the resolved question and every setting that changes its answer.
#[must_use]
pub(crate) fn fingerprint(query: &Query, config: &GithubConfig) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();

    // Include all sources as well as eligible repositories. A newly added
    // unsupported/local project still needs its own source status in the next
    // full state even though it does not add a host query.
    query.sources.len().hash(&mut hasher);
    for source in &query.sources {
        source.project_id.hash(&mut hasher);
        source
            .host
            .as_deref()
            .map(str::to_ascii_lowercase)
            .hash(&mut hasher);
        source
            .repository
            .as_deref()
            .map(str::to_ascii_lowercase)
            .hash(&mut hasher);
        source_status_tag(&source.status).hash(&mut hasher);
    }
    query.repositories.len().hash(&mut hasher);
    for eligible in &query.repositories {
        eligible.project_id.hash(&mut hasher);
        eligible
            .repository
            .host
            .to_ascii_lowercase()
            .hash(&mut hasher);
        eligible
            .repository
            .name_with_owner()
            .to_ascii_lowercase()
            .hash(&mut hasher);
    }

    config.executable.hash(&mut hasher);
    config.timeout_secs.hash(&mut hasher);
    config.include_all_repos.hash(&mut hasher);
    let mut enterprise_hosts: Vec<String> = config
        .enterprise_hosts
        .iter()
        .filter_map(|host| normalize_configured_host(host))
        .collect();
    enterprise_hosts.sort_unstable();
    enterprise_hosts.dedup();
    enterprise_hosts.hash(&mut hasher);
    hasher.finish()
}

fn source_status_tag(status: &PullRequestSourceStatus) -> u8 {
    match status {
        PullRequestSourceStatus::Ready => 0,
        PullRequestSourceStatus::NoGitRepository => 1,
        PullRequestSourceStatus::NoRemote => 2,
        PullRequestSourceStatus::UnsupportedHost => 3,
        PullRequestSourceStatus::InvalidRemote => 4,
        PullRequestSourceStatus::Failed => 5,
        PullRequestSourceStatus::Unknown => 6,
        _ => 7,
    }
}

/// Query every eligible host and map transport rows into domain state.
///
/// `path_entries` is the resolved login-shell `PATH` (§12): without it a GUI
/// launched from Finder cannot find a `gh` that every terminal on the machine
/// finds.
#[must_use]
pub(crate) fn refresh(
    query: Query,
    config: &GithubConfig,
    path_entries: Vec<PathBuf>,
) -> PullRequestState {
    let Query {
        mut sources,
        mut failures,
        repositories,
    } = query;
    let mut by_host: BTreeMap<String, Vec<EligibleRepository>> = BTreeMap::new();
    for eligible in repositories {
        by_host
            .entry(eligible.repository.host.to_ascii_lowercase())
            .or_default()
            .push(eligible);
    }

    let cli = config.cli(path_entries);
    let queried_hosts = by_host.len();
    let mut successful_hosts = 0usize;
    let mut host_errors = Vec::new();
    let mut viewers = Vec::new();
    let mut pull_requests = Vec::new();

    for (host, eligible) in by_host {
        let repositories: Vec<GitHubRepo> = eligible
            .iter()
            .map(|source| source.repository.clone())
            .collect();
        match git_service::list_pull_requests_with_cli(
            &cli,
            &repositories,
            config.include_all_repos,
        ) {
            Ok(page) => {
                successful_hosts += 1;
                if let Some(login) = page.viewer.as_deref().filter(|login| !login.is_empty()) {
                    viewers.push(PullRequestViewer {
                        host: host.clone(),
                        login: login.to_string(),
                    });
                }
                apply_page(
                    &host,
                    page,
                    &eligible,
                    &mut sources,
                    &mut failures,
                    &mut pull_requests,
                );
            }
            Err(error) => {
                let (kind, message) = classify_host_error(&cli, &host, &error);
                mark_host_failed(&mut sources, &host);
                failures.push(PullRequestFailure {
                    host: host.clone(),
                    repository: None,
                    kind,
                    message: message.clone(),
                });
                host_errors.push((host, message));
            }
        }
    }

    dedupe_and_sort(&mut pull_requests);
    viewers.sort_unstable_by(|left, right| left.host.cmp(&right.host));
    let error = if queried_hosts > 0 && successful_hosts == 0 {
        Some(if host_errors.len() == 1 {
            host_errors.pop().expect("one host error").1
        } else {
            let details = host_errors
                .into_iter()
                .map(|(host, message)| format!("{host}: {message}"))
                .collect::<Vec<_>>()
                .join("; ");
            format!("all GitHub hosts failed: {details}")
        })
    } else {
        None
    };

    PullRequestState {
        pull_requests,
        viewers,
        sources,
        failures,
        error,
        refreshed_at: Some(Timestamp::now()),
    }
}

fn apply_page(
    host: &str,
    page: git_service::PullRequestPage,
    eligible: &[EligibleRepository],
    sources: &mut [PullRequestSource],
    failures: &mut Vec<PullRequestFailure>,
    pull_requests: &mut Vec<PullRequest>,
) {
    let viewer = page.viewer.as_deref();
    let projects: HashMap<(String, String), ProjectId> = eligible
        .iter()
        .map(|eligible| {
            (
                (
                    eligible.repository.host.to_ascii_lowercase(),
                    eligible.repository.name_with_owner().to_ascii_lowercase(),
                ),
                eligible.project_id,
            )
        })
        .collect();

    for (repository, message) in page.failures {
        let repository = (!repository.starts_with('@')).then_some(repository);
        if let Some(repository) = repository.as_deref() {
            mark_repository_failed(sources, host, repository);
        }
        push_failure_once(
            failures,
            PullRequestFailure {
                host: host.to_string(),
                repository: repository.clone(),
                kind: if repository.is_some() {
                    PullRequestFailureKind::RepositoryRefused
                } else {
                    PullRequestFailureKind::HostRefused
                },
                message,
            },
        );
    }

    for row in page.pull_requests {
        let repository = row.repository.clone();
        match map_pull_request(row, viewer, &projects) {
            Ok(pull_request) => pull_requests.push(pull_request),
            Err(message) => {
                mark_repository_failed(sources, host, &repository);
                push_failure_once(
                    failures,
                    PullRequestFailure {
                        host: host.to_string(),
                        repository: Some(repository),
                        kind: PullRequestFailureKind::RepositoryRefused,
                        message,
                    },
                );
            }
        }
    }
}

fn map_pull_request(
    row: GitHubPullRequest,
    viewer: Option<&str>,
    projects: &HashMap<(String, String), ProjectId>,
) -> Result<PullRequest, String> {
    let created_at = Timestamp::parse_rfc3339(&row.created_at)
        .map_err(|error| format!("invalid createdAt for #{}: {error}", row.number))?;
    let updated_at = Timestamp::parse_rfc3339(&row.updated_at)
        .map_err(|error| format!("invalid updatedAt for #{}: {error}", row.number))?;
    let project_id = projects
        .get(&(
            row.host.to_ascii_lowercase(),
            row.repository.to_ascii_lowercase(),
        ))
        .copied();
    let is_viewer =
        |candidate: &str| viewer.is_some_and(|viewer| candidate.eq_ignore_ascii_case(viewer));

    Ok(PullRequest {
        project_id,
        repository: row.repository,
        host: row.host,
        number: row.number,
        title: row.title,
        body: row.body,
        body_truncated: row.body_truncated,
        url: row.url,
        author: row.author.clone(),
        base_ref: row.base_ref,
        head_ref: row.head_ref,
        is_draft: row.is_draft,
        review_decision: row.review_decision.map(map_review_decision),
        labels: row
            .labels
            .into_iter()
            .map(|label| PullRequestLabel {
                name: label.name,
                color: label.color,
            })
            .collect(),
        relations: PullRequestRelations {
            assigned: row.assignees.iter().any(|login| is_viewer(login)),
            review_requested: row.review_requests.iter().any(|login| is_viewer(login)),
            authored: is_viewer(&row.author),
        },
        assignees: row.assignees,
        review_requests: row.review_requests,
        additions: row.additions,
        deletions: row.deletions,
        changed_files: row.changed_files,
        comment_count: row.comment_count,
        created_at,
        updated_at,
    })
}

fn map_review_decision(decision: PullRequestReviewDecision) -> ReviewDecision {
    match decision {
        PullRequestReviewDecision::Approved => ReviewDecision::Approved,
        PullRequestReviewDecision::ChangesRequested => ReviewDecision::ChangesRequested,
        PullRequestReviewDecision::ReviewRequired => ReviewDecision::ReviewRequired,
        PullRequestReviewDecision::Unknown => ReviewDecision::Unknown,
    }
}

fn mark_host_failed(sources: &mut [PullRequestSource], host: &str) {
    for source in sources.iter_mut().filter(|source| {
        source
            .host
            .as_deref()
            .is_some_and(|source_host| source_host.eq_ignore_ascii_case(host))
    }) {
        source.status = PullRequestSourceStatus::Failed;
    }
}

fn mark_repository_failed(sources: &mut [PullRequestSource], host: &str, repository: &str) {
    for source in sources.iter_mut().filter(|source| {
        source
            .host
            .as_deref()
            .is_some_and(|source_host| source_host.eq_ignore_ascii_case(host))
            && source
                .repository
                .as_deref()
                .is_some_and(|identity| identity.eq_ignore_ascii_case(repository))
    }) {
        source.status = PullRequestSourceStatus::Failed;
    }
}

fn push_failure_once(failures: &mut Vec<PullRequestFailure>, failure: PullRequestFailure) {
    if !failures.contains(&failure) {
        failures.push(failure);
    }
}

/// `gh` exit status for "you are not authenticated".
const GH_AUTH_EXIT: i32 = 4;

/// Classify a host failure, and keep the CLI's own words as *detail*.
///
/// The kind is what the GUI renders; the string is diagnostic. Composing a
/// display sentence here is what put "install gh or set github.executable" on
/// a user's screen: the daemon knows which failure happened, only the panel
/// knows how to say it to a person (see `domain::PullRequestFailureKind`).
///
/// Not-signed-in is separated from every other refusal because it is the one
/// with a remedy the user can act on. `gh` exits 4 for it; older builds only
/// say so on stderr, so both are checked.
fn classify_host_error(
    cli: &git_service::GitHubCli,
    host: &str,
    error: &GitError,
) -> (PullRequestFailureKind, String) {
    match error {
        GitError::Io(error) if error.kind() == std::io::ErrorKind::NotFound => (
            PullRequestFailureKind::CliMissing,
            // A bare name was searched, not opened: saying "not found at gh"
            // reads as a broken path, which is what the panel showed.
            if cli.executable.components().count() > 1 {
                format!("GitHub CLI was not found at {}", cli.executable.display())
            } else {
                format!(
                    "GitHub CLI ({}) was not found on PATH",
                    cli.executable.display()
                )
            },
        ),
        GitError::Io(error) => (
            PullRequestFailureKind::HostRefused,
            format!(
                "could not start GitHub CLI {} for {host}: {error}",
                cli.program().display()
            ),
        ),
        GitError::Timeout => (
            PullRequestFailureKind::TimedOut,
            format!(
                "GitHub CLI timed out after {}s while refreshing {host}",
                cli.timeout.as_secs()
            ),
        ),
        GitError::CommandFailed { status, stderr, .. } => {
            let kind = if *status == GH_AUTH_EXIT || mentions_authentication(stderr) {
                PullRequestFailureKind::NotSignedIn
            } else {
                PullRequestFailureKind::HostRefused
            };
            let stderr = stderr.trim();
            let detail = if stderr.is_empty() {
                format!("GitHub CLI failed while refreshing {host}")
            } else {
                format!("GitHub CLI failed while refreshing {host}: {stderr}")
            };
            (kind, detail)
        }
        _ => (
            PullRequestFailureKind::HostRefused,
            format!("could not refresh pull requests from {host}: {error}"),
        ),
    }
}

/// Whether `gh` said the problem is credentials rather than the request.
fn mentions_authentication(stderr: &str) -> bool {
    let stderr = stderr.to_ascii_lowercase();
    [
        "gh auth login",
        "not logged",
        "bad credentials",
        "authentication",
        "unauthorized",
        "http 401",
    ]
    .iter()
    .any(|needle| stderr.contains(needle))
}

fn dedupe_and_sort(pull_requests: &mut Vec<PullRequest>) {
    let mut seen = HashSet::new();
    pull_requests.retain(|pull_request| {
        seen.insert((
            pull_request.host.to_ascii_lowercase(),
            pull_request.repository.to_ascii_lowercase(),
            pull_request.number,
        ))
    });
    pull_requests.sort_unstable_by(|left, right| {
        right
            .updated_at
            .cmp(&left.updated_at)
            .then_with(|| left.host.cmp(&right.host))
            .then_with(|| left.repository.cmp(&right.repository))
            .then_with(|| left.number.cmp(&right.number))
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use git_service::PullRequestLabel as GitHubLabel;

    #[test]
    fn a_missing_bare_cli_is_reported_against_path_not_against_a_path_that_never_was() {
        let cli = GithubConfig::default().cli(vec![PathBuf::from("/usr/bin")]);
        let not_found = GitError::Io(std::io::Error::from(std::io::ErrorKind::NotFound));
        let (kind, message) = classify_host_error(&cli, "github.com", &not_found);
        assert_eq!(kind, PullRequestFailureKind::CliMissing);
        assert_eq!(message, "GitHub CLI (gh) was not found on PATH");
        assert!(
            !message.contains("not found at gh"),
            "a bare name was searched, not opened"
        );

        let configured = GithubConfig {
            executable: "/opt/homebrew/bin/gh".into(),
            ..GithubConfig::default()
        }
        .cli(Vec::new());
        let (kind, message) = classify_host_error(&configured, "github.com", &not_found);
        assert_eq!(kind, PullRequestFailureKind::CliMissing);
        assert_eq!(
            message, "GitHub CLI was not found at /opt/homebrew/bin/gh",
            "a configured path is quoted back, because that one really was opened"
        );
    }

    fn state(title: &str) -> PullRequestState {
        PullRequestState {
            error: Some(title.to_string()),
            ..PullRequestState::default()
        }
    }

    fn query(project_id: ProjectId, host: &str, repository: &str) -> Query {
        let (owner, name) = repository.split_once('/').expect("owner/name");
        Query {
            sources: vec![source(
                project_id,
                Some(host.to_string()),
                Some(repository.to_string()),
                PullRequestSourceStatus::Ready,
            )],
            failures: Vec::new(),
            repositories: vec![EligibleRepository {
                project_id,
                repository: GitHubRepo {
                    host: host.to_string(),
                    owner: owner.to_string(),
                    name: name.to_string(),
                },
            }],
        }
    }

    #[test]
    fn cache_requires_matching_fingerprint_and_unexpired_ttl() {
        let now = Instant::now();
        let mut cache = Cache::default();
        cache.update_at(7, state("cached"), now);

        assert_eq!(
            cache.fresh_at(7, now).unwrap().error.as_deref(),
            Some("cached")
        );
        assert!(cache.fresh_at(8, now).is_none());
        assert!(cache.fresh_at(7, now + CACHE_TTL).is_none());
    }

    #[test]
    fn invalidation_bypasses_reuse_but_keeps_the_snapshot() {
        let mut cache = Cache::default();
        cache.update(7, state("visible"));
        cache.invalidate();

        assert!(cache.fresh(7).is_none());
        assert_eq!(cache.snapshot().error.as_deref(), Some("visible"));
    }

    #[test]
    fn fingerprint_changes_with_query_or_relevant_config() {
        let first = ProjectId::new();
        let second = ProjectId::new();
        let config = GithubConfig::default();
        let base = fingerprint(&query(first, "github.com", "Acme/one"), &config);

        assert_ne!(
            base,
            fingerprint(&query(second, "github.com", "Acme/one"), &config)
        );
        assert_ne!(
            base,
            fingerprint(&query(first, "github.com", "Acme/two"), &config)
        );

        let mut configured = config;
        configured.include_all_repos = true;
        assert_ne!(
            base,
            fingerprint(&query(first, "github.com", "Acme/one"), &configured)
        );
    }

    #[test]
    fn mapping_parses_timestamps_and_derives_all_viewer_relations() {
        let project_id = ProjectId::new();
        let projects = HashMap::from([(
            ("github.com".to_string(), "acme/repo".to_string()),
            project_id,
        )]);
        let row = GitHubPullRequest {
            host: "GitHub.com".into(),
            repository: "Acme/Repo".into(),
            number: 42,
            title: "Ready".into(),
            body: "body".into(),
            body_truncated: false,
            url: "https://github.com/Acme/Repo/pull/42".into(),
            author: "Viewer".into(),
            base_ref: "main".into(),
            head_ref: "feature".into(),
            is_draft: false,
            review_decision: Some(PullRequestReviewDecision::Approved),
            labels: vec![GitHubLabel {
                name: "ship".into(),
                color: "00ff00".into(),
            }],
            assignees: vec!["viewer".into()],
            review_requests: vec!["VIEWER".into()],
            additions: 3,
            deletions: 1,
            changed_files: 2,
            comment_count: 4,
            created_at: "2026-08-24T10:00:00Z".into(),
            updated_at: "2026-08-25T10:00:00Z".into(),
        };

        let mapped = map_pull_request(row, Some("viewer"), &projects).expect("valid row");
        assert_eq!(mapped.project_id, Some(project_id));
        assert_eq!(mapped.review_decision, Some(ReviewDecision::Approved));
        assert_eq!(mapped.labels[0].name, "ship");
        assert!(mapped.relations.assigned);
        assert!(mapped.relations.review_requested);
        assert!(mapped.relations.authored);
        assert_eq!(mapped.updated_at.to_rfc3339(), "2026-08-25T10:00:00Z");
    }
}
