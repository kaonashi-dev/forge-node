//! Talking to the remote: fetch, and push-and-open a pull request. Both open a
//! socket, so the daemon acks when the work starts and reports through an
//! event (`RemoteRefsUpdated`, `PullRequestOpened`); nothing here waits on
//! the network. There is no bare push: the daemon pushes only to open a PR.

use std::collections::HashMap;

use domain::{ProjectId, PullRequest, WorkspaceId};
use rune::{El, TextEdit, col, row, text};
use rune_kit::{Button, ButtonVariant, Kit};

use crate::app::AppState;

#[derive(Default)]
pub struct Sync {
    /// The last fetch's outcome per project, as git said it.
    pub fetched: HashMap<ProjectId, String>,
    pub fetching: Vec<ProjectId>,
    pub pr: Option<PrDialog>,
}

pub struct PrDialog {
    pub workspace: WorkspaceId,
    pub title: TextEdit,
    pub body: TextEdit,
    /// The message for the commit of everything uncommitted; `None` on a
    /// clean tree, where the branch is pushed as it is.
    pub commit: Option<TextEdit>,
    pub opening: bool,
    /// `Ok(url)` or the daemon's error, once `PullRequestOpened` arrives.
    pub outcome: Option<Result<String, String>>,
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

    pub fn open_pr(&mut self, workspace: WorkspaceId, branch: Option<&str>, dirty: bool) {
        let mut title = TextEdit::new(branch.unwrap_or_default().replace(['-', '_', '/'], " "));
        title.focused = true;
        self.pr = Some(PrDialog {
            workspace,
            title,
            body: TextEdit::new(""),
            commit: dirty.then(|| TextEdit::new("")),
            opening: false,
            outcome: None,
        });
    }

    pub fn pr_opened(
        &mut self,
        workspace: WorkspaceId,
        url: Option<String>,
        error: Option<String>,
    ) {
        let Some(dialog) = self
            .pr
            .as_mut()
            .filter(|dialog| dialog.workspace == workspace)
        else {
            return;
        };
        dialog.opening = false;
        dialog.outcome = Some(match (url, error) {
            (Some(url), _) => Ok(url),
            (None, Some(error)) => Err(error),
            (None, None) => Err("the daemon reported neither a URL nor an error".into()),
        });
    }
}

impl PrDialog {
    /// Title, body and commit message to send, or `None` while a field that
    /// is needed is empty or a request is already in flight.
    #[must_use]
    pub fn ready(&self) -> Option<(String, String, Option<String>)> {
        let title = self.title.text.trim();
        if title.is_empty() || self.opening {
            return None;
        }
        let commit = match &self.commit {
            Some(edit) if edit.text.trim().is_empty() => return None,
            Some(edit) => Some(edit.text.trim().to_string()),
            None => None,
        };
        Some((title.to_string(), self.body.text.trim().to_string(), commit))
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

pub fn dialog(s: &AppState, k: &Kit, layer: &mut Vec<El>) {
    let Some(pr) = &s.sync.pr else {
        return;
    };
    let mut body = col().gap(crate::ds::SPACE_4);
    match &pr.outcome {
        Some(Ok(url)) => {
            let target = url.clone();
            body = body
                .child(
                    text(format!("Opened {url}"))
                        .key("pr-outcome")
                        .role("status")
                        .font(k.ty.small)
                        .color(k.c.text_base),
                )
                .child(
                    Button::new("Open in browser")
                        .variant(ButtonVariant::Contrast)
                        .on_press_tx(move |_s: &mut AppState, tx| {
                            let _ = tx.open_url(&target);
                        })
                        .build(k)
                        .key("pr-open-url"),
                );
        }
        outcome => {
            body = body
                .child(
                    rune_kit::TextInput::new(&pr.title)
                        .label("Title")
                        .on_edit(|s: &mut AppState| match s.sync.pr.as_mut() {
                            Some(pr) => &mut pr.title,
                            None => &mut s.scratch_edit,
                        })
                        .build(k)
                        .key("pr-title"),
                )
                .child(
                    rune_kit::TextArea::new(&pr.body)
                        .label("Description")
                        .on_edit(|s: &mut AppState| match s.sync.pr.as_mut() {
                            Some(pr) => &mut pr.body,
                            None => &mut s.scratch_edit,
                        })
                        .build(k)
                        .key("pr-body")
                        .h(140.0),
                );
            if let Some(commit) = &pr.commit {
                body = body.child(
                    rune_kit::TextInput::new(commit)
                        .label("Commit message")
                        .placeholder("What this change does")
                        .on_edit(|s: &mut AppState| {
                            match s.sync.pr.as_mut().and_then(|pr| pr.commit.as_mut()) {
                                Some(edit) => edit,
                                None => &mut s.scratch_edit,
                            }
                        })
                        .build(k)
                        .key("pr-commit"),
                );
            }
            let note = match outcome {
                Some(Err(error)) => format!("The daemon refused: {error}"),
                _ if pr.opening => "Pushing and opening…".to_string(),
                _ => {
                    if pr.commit.is_some() {
                        "Commits everything uncommitted, pushes the branch, then opens a pull request.".to_string()
                    } else {
                        "Pushes this branch, then opens a pull request.".to_string()
                    }
                }
            };
            body = body.child(
                row()
                    .gap(crate::ds::SPACE_5)
                    .items_center()
                    .child(
                        text(note)
                            .key("pr-note")
                            .role("status")
                            .font(k.ty.text_xsmall)
                            .color(k.c.text_faint)
                            .grow(1.0),
                    )
                    .child(
                        Button::new(if pr.commit.is_some() {
                            "Commit, push & open PR"
                        } else {
                            "Push & open PR"
                        })
                        .variant(ButtonVariant::Contrast)
                        .on_press(|s: &mut AppState| s.create_pull_request())
                        .build(k)
                        .key("pr-go"),
                    ),
            );
        }
    }
    crate::modal::Modal::new("pull-request", "Open a pull request")
        .width(crate::modal::WIDTH_STANDARD)
        .on_close(|s: &mut AppState| s.sync.pr = None)
        .child(body)
        .build(s, k, layer);
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
    fn a_pr_outcome_lands_only_in_its_own_dialog() {
        let mut sync = Sync::default();
        let workspace = WorkspaceId::new();
        sync.open_pr(workspace, Some("feature/rune-sync"), true);
        assert_eq!(
            sync.pr.as_ref().expect("dialog").ready(),
            None,
            "a dirty tree needs a message"
        );
        let dialog = sync.pr.as_ref().expect("dialog");
        assert_eq!(dialog.title.text, "feature rune sync");
        sync.pr_opened(WorkspaceId::new(), Some("https://x/1".into()), None);
        assert!(sync.pr.as_ref().expect("dialog").outcome.is_none());
        sync.pr_opened(workspace, None, Some("no remote".into()));
        assert_eq!(
            sync.pr.as_ref().expect("dialog").outcome,
            Some(Err("no remote".into()))
        );
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
