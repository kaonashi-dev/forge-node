//! Read-only pull-request state fetched from remote forge hosts.
//!
//! Pull requests are cached runtime data. They travel between the daemon and
//! clients but are never persisted as authoritative application state.

use serde::{Deserialize, Serialize};

use crate::ids::{ProjectId, Timestamp};

/// An open pull request as needed by Forge clients.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PullRequest {
    /// Forge project that owns the repository, or `None` for a personal result
    /// from a repository Forge does not know.
    pub project_id: Option<ProjectId>,
    /// Stable `owner/name` repository identity.
    pub repository: String,
    /// Forge host, such as `github.com` or a GitHub Enterprise hostname.
    pub host: String,
    /// Repository-local pull-request number.
    pub number: u32,
    /// Pull-request title.
    pub title: String,
    /// Markdown description, potentially truncated for the wire budget.
    pub body: String,
    /// Whether [`PullRequest::body`] was truncated.
    pub body_truncated: bool,
    /// Browser URL for the pull request.
    pub url: String,
    /// Author login.
    pub author: String,
    /// Base branch name.
    pub base_ref: String,
    /// Head branch name.
    pub head_ref: String,
    /// Whether the pull request is a draft.
    pub is_draft: bool,
    /// Current aggregate review decision, when the host reports one.
    pub review_decision: Option<ReviewDecision>,
    /// Labels attached to the pull request.
    pub labels: Vec<PullRequestLabel>,
    /// User logins assigned to the pull request.
    pub assignees: Vec<String>,
    /// User or team logins requested to review the pull request.
    pub review_requests: Vec<String>,
    /// Added lines reported by the host.
    pub additions: u32,
    /// Deleted lines reported by the host.
    pub deletions: u32,
    /// Number of changed files reported by the host.
    pub changed_files: u32,
    /// Number of comments reported by the host.
    pub comment_count: u32,
    /// When the pull request was created.
    pub created_at: Timestamp,
    /// When the pull request was last updated.
    pub updated_at: Timestamp,
    /// How the authenticated viewer relates to this pull request.
    pub relations: PullRequestRelations,
}

/// A label attached to a pull request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PullRequestLabel {
    /// Display name.
    pub name: String,
    /// Host-provided color, conventionally a hexadecimal RGB value.
    pub color: String,
}

/// Aggregate review decision reported by the forge host.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum ReviewDecision {
    /// Required approvals have been granted.
    Approved,
    /// A reviewer requested changes.
    ChangesRequested,
    /// Approval is still required.
    ReviewRequired,
    /// A decision this build does not recognize.
    #[serde(other)]
    Unknown,
}

/// Non-exclusive relationships between the authenticated viewer and a pull
/// request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PullRequestRelations {
    /// The viewer is assigned to the pull request.
    pub assigned: bool,
    /// The viewer has been requested to review the pull request.
    pub review_requested: bool,
    /// The viewer authored the pull request.
    pub authored: bool,
}

impl PullRequestRelations {
    /// Whether the pull request concerns the viewer in any supported way.
    #[must_use]
    pub const fn is_mine(&self) -> bool {
        self.assigned || self.review_requested || self.authored
    }
}

/// Authenticated viewer for one forge host.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PullRequestViewer {
    /// Forge hostname.
    pub host: String,
    /// Authenticated account login.
    pub login: String,
}

/// What went wrong, classified so a client can write its own sentence.
///
/// The daemon knows *which* failure happened; only the GUI knows how to say it
/// to a person. Shipping the raw text and letting the panel paste it is how
/// "GitHub CLI was not found at /opt/homebrew/bin/gh; install gh or set
/// github.executable" ends up on screen — a tool name, an install path, a
/// config key and a shell command, none of which the user asked to learn.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum PullRequestFailureKind {
    /// No GitHub CLI on this machine.
    CliMissing,
    /// The CLI is installed but has no usable credentials for the host.
    NotSignedIn,
    /// The host did not answer inside the budget.
    TimedOut,
    /// The host refused or failed for some other reason.
    HostRefused,
    /// One repository could not be read, while the host itself answered.
    RepositoryRefused,
    /// A kind this build does not recognize.
    #[serde(other)]
    Unknown,
}

/// Failure encountered while refreshing pull requests.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PullRequestFailure {
    /// Forge hostname.
    pub host: String,
    /// Affected `owner/name` repository, or `None` for a host-wide failure.
    pub repository: Option<String>,
    /// What kind of failure this is. The GUI renders **this**, never
    /// [`PullRequestFailure::message`].
    pub kind: PullRequestFailureKind,
    /// Diagnostic detail for logs and a details disclosure: the CLI's own
    /// words, paths and all. Never the headline a user reads.
    pub message: String,
}

/// Refresh status for one Forge project.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PullRequestSource {
    /// Project whose repository was inspected.
    pub project_id: ProjectId,
    /// Resolved forge hostname, when available.
    pub host: Option<String>,
    /// Resolved `owner/name` repository, when available.
    pub repository: Option<String>,
    /// Outcome of resolving and refreshing this source.
    pub status: PullRequestSourceStatus,
}

/// Refresh status for a project's pull-request source.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum PullRequestSourceStatus {
    /// The source was resolved and queried successfully.
    Ready,
    /// The project is not a Git repository.
    NoGitRepository,
    /// The repository has no configured remote.
    NoRemote,
    /// The remote belongs to a forge host this build does not support.
    UnsupportedHost,
    /// The configured remote could not be parsed.
    InvalidRemote,
    /// Refreshing the resolved source failed.
    Failed,
    /// A status this build does not recognize.
    #[serde(other)]
    Unknown,
}

/// Complete cached pull-request state shared with clients.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PullRequestState {
    /// All known open pull requests.
    pub pull_requests: Vec<PullRequest>,
    /// Authenticated viewers, one per successfully queried host.
    pub viewers: Vec<PullRequestViewer>,
    /// Resolution and refresh status for every project.
    pub sources: Vec<PullRequestSource>,
    /// Host- or repository-specific failures that did not invalidate all data.
    pub failures: Vec<PullRequestFailure>,
    /// Global refresh failure, if the operation could not produce a result.
    pub error: Option<String>,
    /// When this state was last refreshed.
    pub refreshed_at: Option<Timestamp>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relations_are_mine_when_any_relation_is_set() {
        let none = PullRequestRelations {
            assigned: false,
            review_requested: false,
            authored: false,
        };
        assert!(!none.is_mine());

        for mine in [
            PullRequestRelations {
                assigned: true,
                ..none.clone()
            },
            PullRequestRelations {
                review_requested: true,
                ..none.clone()
            },
            PullRequestRelations {
                authored: true,
                ..none
            },
        ] {
            assert!(mine.is_mine());
        }
    }

    #[test]
    fn unknown_remote_enum_variants_are_tolerated() {
        let decision: ReviewDecision = serde_json::from_str("\"Dismissed\"").unwrap();
        assert_eq!(decision, ReviewDecision::Unknown);

        let status: PullRequestSourceStatus = serde_json::from_str("\"RateLimited\"").unwrap();
        assert_eq!(status, PullRequestSourceStatus::Unknown);
    }
}
