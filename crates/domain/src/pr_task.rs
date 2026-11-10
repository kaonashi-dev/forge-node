//! The prompt that asks an agent to draft a pull request, and what it hands back.
//!
//! The agent only writes text and hands it back through `pr_draft` or
//! `forgectl pr draft`; the daemon then commits, pushes and opens the pull
//! request itself. The wording before the handover is the user's
//! (`[pull_requests] system_prompt`); the handover is not, because without it
//! nothing ever reaches the daemon.

use serde::{Deserialize, Serialize};

use crate::{DiffStatus, WorkspaceDiff};

/// The wording used when `[pull_requests] system_prompt` is empty.
pub const DEFAULT_SYSTEM_PROMPT: &str = "Write the pull request for the changes in this \
    checkout, summarised below. Say what changed and why; do not list the files, the \
    diff already does.\n\n\
    Write it the way this team does. Read what the repository says — CONTRIBUTING, \
    AGENTS.md or CLAUDE.md, any commit or PR guide under docs/, \
    `.github/pull_request_template.md` — and its recent history: `git log -n 20` \
    and `gh pr list --state merged --limit 5 --json title,body`. If this repository \
    has too little history to show a convention, read recent merged pull requests \
    in other repositories of the same owner. The repository's written rules win.";

/// Appended to every prompt, whatever the user configured.
const DRAFT_HANDOVER: &str = "Do not edit files, commit, push or open the pull \
    request: Forge Node does that as soon as you hand the text over.\n\n\
    Uncommitted changes go into one commit, so the commit message describes them \
    all, in this repository's commit style. The branch is used only when this \
    checkout is on its default branch; name it the way this repository names \
    branches.\n\n\
    Hand the result over with the forge MCP tool `pr_draft` (title, branch, commit, \
    commit_body, body: the pull request body in Markdown), and stop. Without that \
    tool, run this command instead, quoting each value for the shell:\n\n\
    ```sh\n\
    forgectl pr draft --title '<title>' --branch '<new-branch>' --commit '<commit subject>' [--commit-body '<commit body>'] <<'EOF'\n\
    <pull request body in Markdown>\n\
    EOF\n\
    ```";

/// Patch bytes a prompt carries; past it files are named but not shown.
pub const MAX_PROMPT_PATCH: usize = 24 * 1024;

/// Where the checkout stands against its default branch.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BranchFacts {
    pub default_branch: Option<String>,
    /// Commits on this branch not on its upstream; `None` when unknown.
    pub ahead: Option<u32>,
}

/// `system_prompt` when it says anything, else [`DEFAULT_SYSTEM_PROMPT`].
#[must_use]
pub fn effective_system_prompt(system_prompt: &str) -> &str {
    let trimmed = system_prompt.trim();
    if trimmed.is_empty() {
        DEFAULT_SYSTEM_PROMPT
    } else {
        trimmed
    }
}

/// The whole prompt: the user's wording, the fixed handover, then the change.
#[must_use]
pub fn agent_prompt(system_prompt: &str, diff: &WorkspaceDiff, branch: &BranchFacts) -> String {
    let mut prompt = format!(
        "{}\n\n{DRAFT_HANDOVER}\n\n## The change\n\n",
        effective_system_prompt(system_prompt)
    );
    if let Some(name) = &diff.branch {
        prompt.push_str(&format!("Branch `{name}`"));
        if let Some(default) = &branch.default_branch {
            prompt.push_str(&format!(" (default branch `{default}`)"));
        }
        prompt.push_str(". ");
    }
    if let Some(ahead) = branch.ahead.filter(|ahead| *ahead > 0) {
        let base = branch
            .default_branch
            .as_deref()
            .unwrap_or("the default branch");
        prompt.push_str(&format!(
            "{ahead} commit{} not yet pushed; read them with `git log` and \
             `git diff {base}...HEAD`, they belong in the pull request too. ",
            if ahead == 1 { "" } else { "s" }
        ));
    }
    if diff.files.is_empty() {
        prompt.push_str("Nothing is uncommitted.\n");
        return prompt;
    }
    let (additions, deletions) = diff.totals();
    prompt.push_str(&format!(
        "Uncommitted: {} file{} · +{additions} −{deletions}.\n",
        diff.files.len(),
        if diff.files.len() == 1 { "" } else { "s" }
    ));
    for file in &diff.files {
        prompt.push_str(&format!(
            "\n- `{}` ({}, +{} −{})",
            file.path,
            status_word(file.status),
            file.additions,
            file.deletions
        ));
    }
    prompt.push('\n');
    let mut patch = String::new();
    let mut truncated = diff.truncated;
    for file in &diff.files {
        if patch.len() + file.patch.len() > MAX_PROMPT_PATCH {
            truncated = true;
            break;
        }
        patch.push_str(&file.patch);
    }
    if !patch.is_empty() {
        prompt.push_str("\n## The diff\n\n```diff\n");
        prompt.push_str(&patch);
        prompt.push_str("\n```\n");
    }
    if truncated {
        prompt.push_str(
            "\nThe diff above is incomplete: some files were left out for length. Read the \
             rest with `git diff` before writing.\n",
        );
    }
    prompt
}

fn status_word(status: DiffStatus) -> &'static str {
    match status {
        DiffStatus::Added => "added",
        DiffStatus::Modified => "modified",
        DiffStatus::Deleted => "deleted",
        DiffStatus::Conflicted => "unresolved conflict",
    }
}

/// Upper bound on any one field of a [`PullRequestDraft`], checked by the
/// daemon before the draft is broadcast to every client.
pub const MAX_DRAFT_FIELD_BYTES: usize = 64 * 1024;

/// The text the agent hands back through `pr_draft` or `forgectl pr draft`.
///
/// Runtime-only: it is broadcast once and never stored.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PullRequestDraft {
    pub title: String,
    /// Markdown.
    pub body: String,
    /// The branch to create when the checkout is on its default branch.
    pub branch: String,
    /// Full commit message for the single commit Forge makes.
    pub commit_message: String,
}

/// How Forge turns a checkout's uncommitted changes into the branch it pushes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommitPlan {
    /// Created and switched to when the checkout is on its default branch or
    /// detached; ignored on any other branch.
    pub branch: Option<String>,
    /// Message for one commit of everything uncommitted (`git add -A`).
    pub message: String,
}

/// What to do about the branch before committing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BranchStep {
    /// Commit on the branch already checked out.
    Keep,
    /// `git switch -c` this name first.
    Create(String),
}

/// Decide the branch a [`CommitPlan`] commits on.
///
/// Never the default branch: a pull request from it to itself cannot be
/// opened, and pushing it would publish the work without review. A checkout
/// with no known default treats `main` and `master` as the default.
///
/// # Errors
/// A message for the user when the checkout is on its default branch, or
/// detached, and no branch was named.
pub fn branch_step(
    current: Option<&str>,
    default: Option<&str>,
    requested: Option<&str>,
) -> Result<BranchStep, &'static str> {
    let requested = requested.map(str::trim).filter(|name| !name.is_empty());
    let on_default = match (current, default) {
        (None, _) => true,
        (Some(current), Some(default)) => current == default,
        (Some(current), None) => matches!(current, "main" | "master"),
    };
    if !on_default {
        return Ok(BranchStep::Keep);
    }
    requested
        .map(|name| BranchStep::Create(name.to_string()))
        .ok_or("this checkout is on its default branch; name a new branch for the pull request")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DiffFile, WorkspaceId};

    fn diff(patch: &str) -> WorkspaceDiff {
        WorkspaceDiff {
            workspace_id: WorkspaceId::new(),
            branch: Some("fix-x".into()),
            files: vec![DiffFile {
                path: "src/lib.rs".into(),
                status: DiffStatus::Modified,
                additions: 1,
                deletions: 1,
                patch: patch.into(),
                binary: false,
                truncated: false,
            }],
            truncated: false,
        }
    }

    #[test]
    fn a_commit_never_lands_on_the_default_branch() {
        assert_eq!(
            branch_step(Some("main"), Some("main"), Some("fix-x")),
            Ok(BranchStep::Create("fix-x".into()))
        );
        assert!(branch_step(Some("main"), Some("main"), Some("  ")).is_err());
        assert!(branch_step(None, Some("main"), None).is_err());
        assert_eq!(
            branch_step(Some("master"), None, Some("y")),
            Ok(BranchStep::Create("y".into()))
        );
        assert_eq!(
            branch_step(Some("feature/x"), Some("main"), Some("ignored")),
            Ok(BranchStep::Keep)
        );
    }

    #[test]
    fn a_custom_prompt_replaces_the_wording_but_never_the_handover() {
        let facts = BranchFacts::default();
        let custom = agent_prompt("Write it in Spanish.", &diff("@@ x"), &facts);
        assert!(custom.starts_with("Write it in Spanish."));
        assert!(!custom.contains(DEFAULT_SYSTEM_PROMPT));
        let default = agent_prompt("   ", &diff("@@ x"), &facts);
        assert!(default.starts_with(DEFAULT_SYSTEM_PROMPT));
        for prompt in [custom, default] {
            assert!(prompt.contains("`pr_draft`"));
            assert!(prompt.contains("forgectl pr draft"));
            assert!(prompt.contains("```diff\n@@ x"));
        }
    }

    #[test]
    fn a_patch_over_budget_is_named_but_left_out() {
        let big = "x".repeat(MAX_PROMPT_PATCH + 1);
        let prompt = agent_prompt("", &diff(&big), &BranchFacts::default());
        assert!(prompt.contains("`src/lib.rs`"));
        assert!(!prompt.contains(&big));
        assert!(prompt.contains("incomplete"));
    }

    #[test]
    fn commits_ahead_send_the_agent_to_the_branch_diff() {
        let mut clean = diff("");
        clean.files.clear();
        let facts = BranchFacts {
            default_branch: Some("main".into()),
            ahead: Some(2),
        };
        let prompt = agent_prompt("", &clean, &facts);
        assert!(prompt.contains("2 commits not yet pushed"));
        assert!(prompt.contains("git diff main...HEAD"));
        assert!(prompt.contains("Nothing is uncommitted."));
    }
}
