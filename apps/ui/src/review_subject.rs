//! What a Forge Review started from the Review tab reads: the checkout's
//! changes, or one pull request named by number or by link.

use domain::{PullRequest, Workspace, WorkspaceId};

use crate::pr_url::{self, PullRequestLink};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Subject {
    #[default]
    Changes,
    PullRequest,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reference {
    /// Read from the reviewed checkout's own remote.
    Number(u32),
    Link(PullRequestLink),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub workspace: WorkspaceId,
    pub number: u32,
    pub base: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Resolution {
    Ready(Target),
    /// The daemon has not listed it, so which repository it lives in is unknown.
    Unlisted(PullRequestLink),
    Refused(String),
}

/// `96`, `#96` or a pull-request link.
///
/// # Errors
/// A sentence for the person when the text is none of those.
pub fn parse(input: &str) -> Result<Reference, String> {
    let input = input.trim();
    let digits = input.strip_prefix('#').unwrap_or(input);
    if !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return digits
            .parse::<u32>()
            .ok()
            .filter(|number| *number > 0)
            .map(Reference::Number)
            .ok_or_else(|| "A pull request number is a whole number from 1.".into());
    }
    pr_url::parse(input)
        .map(Reference::Link)
        .map_err(|error| format!("Type a number like 96, or {}", lowercase_first(&error)))
}

fn lowercase_first(sentence: &str) -> String {
    let mut chars = sentence.chars();
    chars
        .next()
        .map(|first| first.to_lowercase().chain(chars).collect())
        .unwrap_or_default()
}

/// The first pull-request link in free text, punctuation around it ignored.
#[must_use]
pub fn find_link(text: &str) -> Option<PullRequestLink> {
    text.split_whitespace()
        .map(|word| {
            word.trim_matches(|c: char| {
                matches!(
                    c,
                    '<' | '>' | '(' | ')' | '[' | ']' | '"' | '\'' | '`' | ',' | '.' | ';'
                )
            })
        })
        .find_map(|word| pr_url::parse(word).ok())
}

/// A link is honoured only through a listed pull request: the daemon fetches
/// by number from the checkout's remote, so a link to another repository with
/// the same number would otherwise review the wrong pull request.
#[must_use]
pub fn resolve(
    reference: &Reference,
    pull_requests: &[PullRequest],
    workspaces: &[Workspace],
    current: WorkspaceId,
) -> Resolution {
    let link = match reference {
        Reference::Number(number) => {
            return Resolution::Ready(Target {
                workspace: current,
                number: *number,
                base: None,
            });
        }
        Reference::Link(link) => link,
    };
    let Some(pr) = pull_requests.iter().find(|pr| link.names(pr)) else {
        return Resolution::Unlisted(link.clone());
    };
    let workspace = crate::pr_review::review_workspace(workspaces, pr.project_id, Some(current));
    match workspace {
        Some(workspace) => Resolution::Ready(Target {
            workspace,
            number: pr.number,
            base: (!pr.base_ref.is_empty()).then(|| pr.base_ref.clone()),
        }),
        None => Resolution::Refused(format!(
            "No checkout of {} is open here, so its pull request cannot be reviewed.",
            pr.repository
        )),
    }
}

#[cfg(test)]
pub(crate) fn listed_pr(
    project: Option<domain::ProjectId>,
    repository: &str,
    number: u32,
) -> PullRequest {
    let at = domain::Timestamp::parse_rfc3339("2026-01-01T00:00:00Z").unwrap();
    PullRequest {
        project_id: project,
        repository: repository.into(),
        host: "github.com".into(),
        number,
        title: String::new(),
        body: String::new(),
        body_truncated: false,
        url: format!("https://github.com/{repository}/pull/{number}"),
        author: String::new(),
        base_ref: "main".into(),
        head_ref: "topic".into(),
        is_draft: false,
        review_decision: None,
        labels: Vec::new(),
        assignees: Vec::new(),
        review_requests: Vec::new(),
        additions: 0,
        deletions: 0,
        changed_files: 0,
        comment_count: 0,
        created_at: at,
        updated_at: at,
        relations: domain::PullRequestRelations {
            assigned: false,
            review_requested: false,
            authored: false,
        },
    }
}

#[cfg(test)]
mod tests {
    use domain::{ProjectId, Timestamp, WorkspaceKind};

    use super::*;

    fn workspace(project: ProjectId) -> Workspace {
        Workspace {
            id: WorkspaceId::new(),
            project_id: project,
            kind: WorkspaceKind::Main,
            path: "/tmp/x".into(),
            branch: None,
            display_name: None,
            managed_by_app: false,
            created_at: Timestamp::now(),
            status: domain::WorkspaceStatus::default(),
        }
    }

    fn link(repository: &str, number: u32) -> Reference {
        parse(&format!("https://github.com/{repository}/pull/{number}")).unwrap()
    }

    #[test]
    fn a_number_a_hash_number_or_a_link_names_a_pull_request() {
        assert_eq!(parse(" 96 "), Ok(Reference::Number(96)));
        assert_eq!(parse("#96"), Ok(Reference::Number(96)));
        assert!(matches!(
            parse("https://github.com/acme/widget/pull/96/files"),
            Ok(Reference::Link(PullRequestLink { number: 96, .. }))
        ));
        assert!(parse("0").is_err());
        assert!(parse("").is_err());
        assert!(parse("main").is_err());
    }

    #[test]
    fn a_link_is_found_inside_a_sentence() {
        let found = find_link("Review this pr (https://github.com/acme/widget/pull/96).");
        assert_eq!(found.map(|link| link.number), Some(96));
        assert_eq!(find_link("look at github.com/acme/widget/pull/96"), None);
        assert_eq!(find_link("nothing here"), None);
    }

    #[test]
    fn a_number_is_read_in_the_checkout_on_screen() {
        let here = workspace(ProjectId::new());
        assert_eq!(
            resolve(
                &Reference::Number(7),
                &[],
                std::slice::from_ref(&here),
                here.id
            ),
            Resolution::Ready(Target {
                workspace: here.id,
                number: 7,
                base: None,
            })
        );
    }

    #[test]
    fn a_link_reviews_in_a_checkout_of_its_own_repository() {
        let forge = ProjectId::new();
        let other = ProjectId::new();
        let here = workspace(forge);
        let there = workspace(other);
        let listed = vec![
            listed_pr(Some(forge), "acme/forge", 96),
            listed_pr(Some(other), "acme/payments", 96),
        ];
        let all = vec![here.clone(), there.clone()];
        assert_eq!(
            resolve(&link("acme/forge", 96), &listed, &all, here.id),
            Resolution::Ready(Target {
                workspace: here.id,
                number: 96,
                base: Some("main".into()),
            })
        );
        assert_eq!(
            resolve(&link("ACME/Payments", 96), &listed, &all, here.id),
            Resolution::Ready(Target {
                workspace: there.id,
                number: 96,
                base: Some("main".into()),
            }),
            "the same number in another repository must not be read from this checkout"
        );
    }

    #[test]
    fn an_unlisted_or_unowned_link_does_not_start() {
        let here = workspace(ProjectId::new());
        assert!(matches!(
            resolve(
                &link("acme/forge", 3),
                &[],
                std::slice::from_ref(&here),
                here.id
            ),
            Resolution::Unlisted(PullRequestLink { number: 3, .. })
        ));
        let stranger = vec![listed_pr(None, "someone/else", 3)];
        assert!(matches!(
            resolve(
                &link("someone/else", 3),
                &stranger,
                std::slice::from_ref(&here),
                here.id
            ),
            Resolution::Refused(_)
        ));
    }
}
