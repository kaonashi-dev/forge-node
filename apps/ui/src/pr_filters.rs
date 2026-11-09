//! Which of the last read's pull requests the panel shows.
//!
//! The daemon already asked the host the three questions that matter —
//! assigned, review requested, authored — and ships the answers as
//! `PullRequest.relations`. Filtering is therefore a read over data already in
//! hand, not a second round trip: a scope the user picks must not cost a `gh`
//! subprocess.

use domain::{PullRequest, PullRequestFailure, PullRequestRelations, ReviewDecision};

/// `app_state` key holding the last scope, so the panel opens where it was.
pub const PR_SCOPE_KEY: &str = "ui.pr_scope";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Scope {
    #[default]
    All,
    Assigned,
    ReviewRequested,
    Authored,
}

impl Scope {
    pub const ALL: [Scope; 4] = [
        Scope::All,
        Scope::Assigned,
        Scope::ReviewRequested,
        Scope::Authored,
    ];

    /// The stored form, shared with the original client.
    #[must_use]
    pub fn value(self) -> &'static str {
        match self {
            Scope::All => "all",
            Scope::Assigned => "assigned",
            Scope::ReviewRequested => "review_requested",
            Scope::Authored => "authored",
        }
    }

    /// Anything unknown degrades to "all": `app_state` is untyped text by
    /// design, and a value written by a newer build must not break this one.
    #[must_use]
    pub fn parse(value: Option<&str>) -> Scope {
        match value.map(str::trim) {
            Some("assigned") => Scope::Assigned,
            Some("review_requested") => Scope::ReviewRequested,
            Some("authored") => Scope::Authored,
            _ => Scope::All,
        }
    }

    /// What the scope row says. Short: four share one line.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Scope::All => "All",
            Scope::Assigned => "Assigned",
            Scope::ReviewRequested => "To review",
            Scope::Authored => "Mine",
        }
    }

    /// The empty state, which has to say *why* the list is empty.
    #[must_use]
    pub fn empty_copy(self) -> &'static str {
        match self {
            Scope::All => "No open pull requests in the last read.",
            Scope::Assigned => "No open pull request is assigned to you.",
            Scope::ReviewRequested => "Nobody has asked you for a review.",
            Scope::Authored => "You have no open pull requests.",
        }
    }

    #[must_use]
    pub fn contains(self, relations: &PullRequestRelations) -> bool {
        match self {
            Scope::All => true,
            Scope::Assigned => relations.assigned,
            Scope::ReviewRequested => relations.review_requested,
            Scope::Authored => relations.authored,
        }
    }
}

#[cfg(test)]
pub struct Indexed<'a> {
    pub pr: &'a PullRequest,
    haystack: String,
}

/// The searchable form of a pull request: fields lowercased once, so a filter
/// keystroke only filters.
fn haystack(pr: &PullRequest) -> String {
    let mut haystack = format!(
        "{} {} {} {} {} #{}",
        pr.title, pr.repository, pr.author, pr.head_ref, pr.base_ref, pr.number
    );
    for label in &pr.labels {
        haystack.push(' ');
        haystack.push_str(&label.name);
    }
    haystack.to_lowercase()
}

/// The fields `Index` keys a rebuild on: the count and the state's own refresh
/// stamp, so a snapshot arriving at 60 Hz is not re-indexed every frame.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Signature {
    len: usize,
    refreshed: Option<domain::Timestamp>,
    first: Option<u32>,
    last: Option<u32>,
}

/// The last read's searchable rows, rebuilt only when the state changed.
#[derive(Default)]
pub struct Index {
    signature: Option<Signature>,
    haystacks: Vec<String>,
}

impl Index {
    pub fn sync(&mut self, state: &domain::PullRequestState) {
        let signature = Signature {
            len: state.pull_requests.len(),
            refreshed: state.refreshed_at,
            first: state.pull_requests.first().map(|pr| pr.number),
            last: state.pull_requests.last().map(|pr| pr.number),
        };
        if self.signature == Some(signature) {
            return;
        }
        self.haystacks = state.pull_requests.iter().map(haystack).collect();
        self.signature = Some(signature);
    }

    #[must_use]
    pub fn filter<'a>(
        &'a self,
        pulls: &'a [PullRequest],
        scope: Scope,
        query: &str,
        project: Option<domain::ProjectId>,
    ) -> (Vec<&'a PullRequest>, Vec<&'a PullRequest>) {
        let terms = terms(query);
        let kept: Vec<&PullRequest> = pulls
            .iter()
            .enumerate()
            .filter(|(index, pr)| {
                keeps(
                    pr,
                    scope,
                    &terms,
                    self.haystacks.get(*index).map(String::as_str),
                )
            })
            .map(|(_, pr)| pr)
            .collect();
        let mine = kept
            .iter()
            .copied()
            .filter(|pr| project.is_some() && pr.project_id == project)
            .collect();
        let others = kept
            .into_iter()
            .filter(|pr| project.is_none() || pr.project_id != project)
            .collect();
        (mine, others)
    }
}

/// Lowercase each pull request's searchable fields, for the pure filter tests.
#[cfg(test)]
#[must_use]
pub fn index(pull_requests: &[PullRequest]) -> Vec<Indexed<'_>> {
    pull_requests
        .iter()
        .map(|pr| Indexed {
            pr,
            haystack: haystack(pr),
        })
        .collect()
}

/// A typed filter split into lowercase words; a row must contain each of them,
/// in any order, so `login fix` finds "Fix the login bug".
#[must_use]
pub fn terms(query: &str) -> Vec<String> {
    query
        .to_lowercase()
        .split_whitespace()
        .map(str::to_string)
        .collect()
}

fn keeps(pr: &PullRequest, scope: Scope, terms: &[String], haystack: Option<&str>) -> bool {
    scope.contains(&pr.relations)
        && haystack.is_some_and(|haystack| terms.iter().all(|term| haystack.contains(term)))
}

/// The rows to draw: this project's first, then everything else.
///
/// A list that mixes repositories is one you have to read before you can click
/// in it, and the checkout on screen is the repository the reader is thinking
/// about. Within each half the host's own order is kept — it is by recency, and
/// re-sorting it here would be a second opinion with less information.
#[cfg(test)]
#[must_use]
pub fn filter<'a>(
    indexed: &'a [Indexed<'a>],
    scope: Scope,
    query: &str,
    project: Option<domain::ProjectId>,
) -> (Vec<&'a PullRequest>, Vec<&'a PullRequest>) {
    let terms = terms(query);
    let kept: Vec<&PullRequest> = indexed
        .iter()
        .filter(|row| keeps(row.pr, scope, &terms, Some(&row.haystack)))
        .map(|row| row.pr)
        .collect();
    let mine = kept
        .iter()
        .copied()
        .filter(|pr| project.is_some() && pr.project_id == project)
        .collect();
    let others = kept
        .into_iter()
        .filter(|pr| project.is_none() || pr.project_id != project)
        .collect();
    (mine, others)
}

/// The sentence a person reads for one failed host.
///
/// The daemon ships a classified `kind` precisely so this is written here: its
/// own `message` is the CLI's words — an install path, a config key, a shell
/// command — none of which the user asked to learn. That text stays available
/// as a tooltip for whoever is debugging.
#[must_use]
pub fn failure_copy(failure: &PullRequestFailure) -> String {
    let host = failure.host.trim();
    let repository = failure.repository.as_deref().unwrap_or("").trim();
    match failure.kind {
        domain::PullRequestFailureKind::CliMissing => {
            "Forge could not find the GitHub CLI, so pull requests cannot be read.".to_string()
        }
        domain::PullRequestFailureKind::NotSignedIn => {
            format!("Forge is not signed in to {}.", host_or(host, "the host"))
        }
        domain::PullRequestFailureKind::TimedOut => {
            if host.is_empty() {
                "The host did not answer in time.".to_string()
            } else {
                format!("{host} did not answer in time.")
            }
        }
        domain::PullRequestFailureKind::HostRefused => {
            if host.is_empty() {
                "The host could not be read.".to_string()
            } else {
                format!("{host} could not be read.")
            }
        }
        domain::PullRequestFailureKind::RepositoryRefused => {
            if !repository.is_empty() {
                format!("{repository} could not be read.")
            } else if host.is_empty() {
                "A repository could not be read.".to_string()
            } else {
                format!("A repository on {host} could not be read.")
            }
        }
        _ => failure.message.clone(),
    }
}

fn host_or<'a>(host: &'a str, fallback: &'a str) -> &'a str {
    if host.is_empty() { fallback } else { host }
}

/// Stable key for a pull request in the cache.
#[must_use]
pub fn key(pr: &PullRequest) -> String {
    format!("{}/{}#{}", pr.host, pr.repository, pr.number)
}

#[must_use]
pub fn decision_label(decision: Option<ReviewDecision>) -> Option<(&'static str, Tone)> {
    match decision {
        Some(ReviewDecision::Approved) => Some(("Approved", Tone::Good)),
        Some(ReviewDecision::ChangesRequested) => Some(("Changes requested", Tone::Bad)),
        Some(ReviewDecision::ReviewRequired) => Some(("Review required", Tone::Warn)),
        Some(other) => Some((unknown_decision(&other), Tone::Neutral)),
        None => None,
    }
}

fn unknown_decision(value: &ReviewDecision) -> &'static str {
    match value {
        ReviewDecision::Approved => "Approved",
        ReviewDecision::ChangesRequested => "Changes requested",
        ReviewDecision::ReviewRequired => "Review required",
        _ => "A verdict this build does not know",
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Neutral,
    Good,
    Warn,
    Bad,
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::{ProjectId, PullRequestLabel, PullRequestRelations, PullRequestState, Timestamp};

    fn relations(assigned: bool, review_requested: bool, authored: bool) -> PullRequestRelations {
        PullRequestRelations {
            assigned,
            review_requested,
            authored,
        }
    }

    fn pr(project: Option<ProjectId>, number: u32) -> PullRequest {
        PullRequest {
            project_id: project,
            repository: "acme/widget".into(),
            host: "github.com".into(),
            number,
            title: "Add the diff view".into(),
            body: "mentions kubernetes".into(),
            body_truncated: false,
            url: format!("https://example.invalid/{number}"),
            author: "rin".into(),
            base_ref: "main".into(),
            head_ref: "diff-view".into(),
            is_draft: false,
            review_decision: None,
            labels: vec![PullRequestLabel {
                name: "rune".into(),
                color: "7c3aed".into(),
            }],
            assignees: Vec::new(),
            review_requests: Vec::new(),
            additions: 0,
            deletions: 0,
            changed_files: 0,
            comment_count: 0,
            created_at: Timestamp::parse_rfc3339("2026-01-01T00:00:00Z").unwrap(),
            updated_at: Timestamp::parse_rfc3339("2026-01-01T00:00:00Z").unwrap(),
            relations: relations(false, false, false),
        }
    }

    fn numbers(rows: &[&PullRequest]) -> Vec<u32> {
        rows.iter().map(|pr| pr.number).collect()
    }

    #[test]
    fn a_scope_keeps_only_the_relation_it_names() {
        let project = ProjectId::new();
        let mut assigned = pr(Some(project), 2);
        assigned.relations = relations(true, false, false);
        let mut requested = pr(Some(project), 3);
        requested.relations = relations(false, true, false);
        let mut authored = pr(Some(project), 4);
        authored.relations = relations(false, false, true);
        let all = [pr(Some(project), 1), assigned, requested, authored];
        let indexed = index(&all);
        let pick = |scope: Scope| {
            let (mine, others) = filter(&indexed, scope, "", Some(project));
            let mut rows = numbers(&mine);
            rows.extend(numbers(&others));
            rows
        };
        assert_eq!(pick(Scope::All), [1, 2, 3, 4]);
        assert_eq!(pick(Scope::Assigned), [2]);
        assert_eq!(pick(Scope::ReviewRequested), [3]);
        assert_eq!(pick(Scope::Authored), [4]);
    }

    #[test]
    fn this_projects_rows_lead_and_the_rest_follow() {
        let project = ProjectId::new();
        let other = pr(None, 9);
        let mine = pr(Some(project), 1);
        let rows = [other, mine];
        let indexed = index(&rows);
        let (leading, rest) = filter(&indexed, Scope::All, "", Some(project));
        assert_eq!(numbers(&leading), [1]);
        assert_eq!(numbers(&rest), [9]);
    }

    #[test]
    fn with_no_checkout_nothing_leads() {
        let rows = [pr(None, 1)];
        let indexed = index(&rows);
        let (mine, others) = filter(&indexed, Scope::All, "", None);
        assert!(mine.is_empty());
        assert_eq!(numbers(&others), [1]);
    }

    #[test]
    fn the_filter_searches_title_repository_author_branches_number_and_labels() {
        let project = ProjectId::new();
        let rows = [pr(Some(project), 1)];
        let indexed = index(&rows);
        for needle in ["diff", "ACME", "rin", "diff-view", "#1", "rune"] {
            let (mine, _) = filter(&indexed, Scope::All, needle, Some(project));
            assert_eq!(mine.len(), 1, "{needle}");
        }
        let (mine, _) = filter(&indexed, Scope::All, "kubernetes", Some(project));
        assert!(mine.is_empty(), "the description is not searched");
        let (mine, _) = filter(&indexed, Scope::All, "rune diff", Some(project));
        assert_eq!(mine.len(), 1, "every term is required, in any order");
    }

    #[test]
    fn an_unknown_remembered_scope_degrades_to_all() {
        assert_eq!(Scope::parse(Some("assigned")), Scope::Assigned);
        assert_eq!(Scope::parse(Some("something-new")), Scope::All);
        assert_eq!(Scope::parse(None), Scope::All);
    }

    #[test]
    fn every_scope_has_its_own_empty_sentence() {
        let sentences: Vec<&str> = Scope::ALL.iter().map(|scope| scope.empty_copy()).collect();
        let mut unique = sentences.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), sentences.len());
    }

    #[test]
    fn the_cached_index_filters_the_same_way_the_pure_one_does() {
        let project = ProjectId::new();
        let state = PullRequestState {
            pull_requests: vec![pr(Some(project), 1)],
            refreshed_at: Some(Timestamp::parse_rfc3339("2026-08-30T12:00:00Z").unwrap()),
            ..PullRequestState::default()
        };
        let mut index = Index::default();
        index.sync(&state);
        let (mine, others) =
            index.filter(&state.pull_requests, Scope::All, "rune diff", Some(project));
        assert_eq!(numbers(&mine), [1]);
        assert!(others.is_empty());
        let (none, _) = index.filter(
            &state.pull_requests,
            Scope::All,
            "kubernetes",
            Some(project),
        );
        assert!(none.is_empty(), "the description is not searched");
    }

    fn failure(kind: domain::PullRequestFailureKind) -> PullRequestFailure {
        PullRequestFailure {
            host: "github.com".into(),
            repository: None,
            kind,
            message: "GitHub CLI (gh) was not found on PATH".into(),
        }
    }

    #[test]
    fn a_failure_writes_its_own_sentence_not_the_clis_words() {
        let copy = failure_copy(&failure(domain::PullRequestFailureKind::CliMissing));
        assert!(!copy.contains("PATH"));
        assert!(!copy.contains("gh"));
        assert!(copy.contains("GitHub CLI"));
        let mut repository = failure(domain::PullRequestFailureKind::RepositoryRefused);
        repository.repository = Some("acme/widget".into());
        assert!(failure_copy(&repository).contains("acme/widget"));
        let unknown = PullRequestFailure {
            kind: domain::PullRequestFailureKind::Unknown,
            message: "something new went wrong".into(),
            ..failure(domain::PullRequestFailureKind::HostRefused)
        };
        assert_eq!(failure_copy(&unknown), "something new went wrong");
    }

    #[test]
    fn a_state_is_not_read_as_an_empty_list_by_accident() {
        let state = PullRequestState::default();
        assert!(state.pull_requests.is_empty());
    }

    #[test]
    fn decisions_name_the_verdict_the_host_sent() {
        assert_eq!(
            decision_label(Some(ReviewDecision::Approved)),
            Some(("Approved", Tone::Good))
        );
        assert_eq!(
            decision_label(Some(ReviewDecision::ChangesRequested)),
            Some(("Changes requested", Tone::Bad))
        );
        assert_eq!(decision_label(None), None);
    }
}
