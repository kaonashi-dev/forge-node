//! Talking to the remote: fetch. It opens a socket, so the daemon acks when
//! the work starts and reports through `RemoteRefsUpdated`; nothing here waits
//! on the network. Pull requests are opened by `open_pr`.

use std::collections::HashMap;

use domain::{ProjectId, PullRequest};

#[derive(Default)]
pub struct Sync {
    /// The last fetch's outcome per project, as git said it.
    pub fetched: HashMap<ProjectId, String>,
    pub fetching: Vec<ProjectId>,
}

impl Sync {
    pub fn fetch_started(&mut self, project: ProjectId) {
        if !self.fetching.contains(&project) {
            self.fetching.push(project);
        }
    }

    pub fn fetch_done(
        &mut self,
        project: ProjectId,
        remote: &str,
        updated: u32,
        error: Option<&str>,
    ) {
        self.fetching.retain(|id| *id != project);
        let note = match error {
            Some(error) => format!("fetch from {remote} failed: {}", error.trim()),
            None if updated == 0 => format!("{remote} is current"),
            None => format!("fetched {remote}: {updated} refs updated"),
        };
        self.fetched.insert(project, note);
    }
}

/// The open pull request whose head is `branch` in `project`, if one is known.
#[must_use]
pub fn pull_request_for<'a>(
    pulls: &'a [PullRequest],
    project: ProjectId,
    branch: Option<&str>,
) -> Option<&'a PullRequest> {
    let branch = branch?;
    pulls
        .iter()
        .find(|pr| pr.project_id == Some(project) && pr.head_ref == branch)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fetch_reports_git_in_its_own_words() {
        let mut sync = Sync::default();
        let project = ProjectId::new();
        sync.fetch_started(project);
        sync.fetch_done(project, "origin", 0, None);
        assert_eq!(sync.fetched[&project], "origin is current");
        sync.fetch_done(
            project,
            "origin",
            0,
            Some("Permission denied (publickey).\n"),
        );
        assert_eq!(
            sync.fetched[&project],
            "fetch from origin failed: Permission denied (publickey)."
        );
        assert!(sync.fetching.is_empty());
    }

    #[test]
    fn a_pull_request_belongs_to_a_branch_of_its_own_project() {
        let project = ProjectId::new();
        let mut pr: PullRequest = serde_json::from_value(serde_json::json!({
            "project_id": null, "repository": "o/r", "host": "github.com", "number": 7,
            "title": "T", "body": "", "body_truncated": false, "url": "https://x/7",
            "author": "a", "base_ref": "main", "head_ref": "topic", "is_draft": false,
            "review_decision": null, "labels": [], "assignees": [], "review_requests": [],
            "additions": 1, "deletions": 0, "changed_files": 1, "comment_count": 0,
            "created_at": "2026-11-09T00:00:00Z", "updated_at": "2026-11-09T00:00:00Z",
            "relations": { "assigned": false, "review_requested": false, "authored": true }
        }))
        .expect("a pull request");
        pr.project_id = Some(project);
        let pulls = vec![pr];
        assert!(pull_request_for(&pulls, project, Some("topic")).is_some());
        assert!(
            pull_request_for(&pulls, ProjectId::new(), Some("topic")).is_none(),
            "the same branch name in another project is not this pull request"
        );
        assert!(pull_request_for(&pulls, project, Some("other")).is_none());
        assert!(
            pull_request_for(&pulls, project, None).is_none(),
            "a detached checkout"
        );
    }
}
