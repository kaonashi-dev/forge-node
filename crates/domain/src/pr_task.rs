//! Agent prompts for preparing a pull request.
//!
//! A task is a *template plus a mode*. The template is the wording; the mode is
//! what the answer is for, and it is what decides who opens the pull request:
//!
//! - [`TaskMode::Draft`] — the agent only writes the text and hands it back
//!   with `forgectl pr draft`. Forge Node shows it for editing, then commits
//!   everything once (on a new branch when the checkout is on its default),
//!   pushes and calls `gh pr create` itself. The agent never touches the
//!   working tree.
//! - [`TaskMode::Implement`] — the agent does the work, commits, pushes and
//!   opens the pull request. Forge Node launches it and then only watches.
//!
//! Making the mode a field of the task rather than a separate switch is the
//! whole design: "summarise these changes" and "write the tests these changes
//! are missing" want different wording *and* different endings, and a user who
//! picked the wording would otherwise still have to remember which ending goes
//! with it.
//!
//! Built-in tasks live in code, like the agent descriptors do. The GUI may
//! persist a replacement body in its opaque app state; the task itself remains
//! runtime-only and travels with the launch rather than becoming daemon state.

use serde::{Deserialize, Serialize};

/// What the agent's answer is for, and therefore who opens the pull request.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum TaskMode {
    /// The agent writes the pull request's text; Forge Node opens it.
    Draft,
    /// The agent does the work and opens the pull request itself.
    Implement,
}

impl TaskMode {
    /// What the button that starts it should say.
    #[must_use]
    pub fn action(self) -> &'static str {
        match self {
            Self::Draft => "Draft with agent",
            Self::Implement => "Run agent",
        }
    }

    /// Whether Forge is the one that will push and open the pull request.
    #[must_use]
    pub fn forge_opens_the_pr(self) -> bool {
        matches!(self, Self::Draft)
    }
}

/// One thing an agent can be asked to do about a set of changes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrTask {
    /// Stable key, for remembering the last choice.
    pub id: String,
    /// What the chip says.
    pub name: String,
    /// One line under the name: what picking it will do.
    pub detail: String,
    /// Draft or implement.
    pub mode: TaskMode,
    /// The wording handed to the agent, before the change summary and before
    /// anything the user adds.
    pub body: String,
}

const DESCRIBE_INTRO: &str = "Write the pull request for the uncommitted changes in \
    this checkout, summarised below with their diff. Say what changed and why; do \
    not list the files, the diff already does.\n\n";

const REVIEW_INTRO: &str = "Read the diff below as a reviewer would before writing \
    anything. If you find something that would block a merge — a bug, a missing \
    test, a leak — say so plainly at the top of the body, under a `## Concerns` \
    heading. Then write the pull request for these uncommitted changes.\n\n";

/// Shared tail of every [`TaskMode::Draft`] prompt: house style, and the one
/// command that hands the text to Forge instead of printing it into a terminal
/// nothing reads.
const DRAFT_HANDOVER: &str = "Do not edit files, commit, push or open the pull \
    request: Forge Node does that once the user has read your text.\n\n\
    Write it the way this team does. Read what the repository says — CONTRIBUTING, \
    AGENTS.md or CLAUDE.md, any commit or PR guide under docs/, \
    `.github/pull_request_template.md` — and its recent history: `git log -n 20` \
    and `gh pr list --state merged --limit 5 --json title,body`. If this repository \
    has too little history to show a convention, read recent merged pull requests \
    in other repositories of the same owner. A convention you already remember for \
    this repository or team counts too; the repository's written rules win.\n\n\
    All the changes go into one commit, so the commit message describes them all, \
    in this repository's commit style. The branch is used only when this checkout \
    is on its default branch; name it the way this repository names branches.\n\n\
    Hand the result over with this command, quoting each value for the shell, and \
    stop:\n\n\
    ```sh\n\
    forgectl pr draft --title '<title>' --branch '<new-branch>' --commit '<commit subject>' [--commit-body '<commit body>'] <<'EOF'\n\
    <pull request body in Markdown>\n\
    EOF\n\
    ```";

/// The tasks Forge ships.
///
/// Deliberately few. This is a starting point a user edits, not a catalogue to
/// choose from: a list long enough to need scrolling would make the common case
/// — press the button — slower than typing the prompt by hand.
#[must_use]
pub fn builtin_tasks() -> Vec<PrTask> {
    vec![
        PrTask {
            id: "describe".to_string(),
            name: "Describe the change".to_string(),
            detail: "The agent writes the PR and commit text; you edit it, Forge Node commits and opens \
                     the PR."
                .to_string(),
            mode: TaskMode::Draft,
            body: format!("{DESCRIBE_INTRO}{DRAFT_HANDOVER}"),
        },
        PrTask {
            id: "review".to_string(),
            name: "Review, then describe".to_string(),
            detail: "The agent reviews the diff first, then writes the text; Forge Node commits and \
                     opens the PR."
                .to_string(),
            mode: TaskMode::Draft,
            body: format!("{REVIEW_INTRO}{DRAFT_HANDOVER}"),
        },
        PrTask {
            id: "finish".to_string(),
            name: "Finish and open the PR".to_string(),
            detail: "The agent reviews, commits in few groups, pushes and opens the PR in \
                     the repo's style."
                .to_string(),
            mode: TaskMode::Implement,
            body: "You are working in a git checkout that already has uncommitted \
                   changes, summarised below. Take them to an open pull request.\n\
                   \n\
                   1. Learn how this team writes commits and pull requests before \
                   writing either. Read what the repository says — CONTRIBUTING, \
                   AGENTS.md or CLAUDE.md, any commit or PR guide under docs/, \
                   `.github/pull_request_template.md` — then the recent history: \
                   `git log -n 20` and `gh pr list --state merged --limit 5 \
                   --json title,body`. If this repository has too little history \
                   to show a convention, read recent merged pull requests in \
                   other repositories of the same owner (`gh repo list <owner> \
                   --limit 5`, then `gh pr list -R <owner>/<repo> --state merged \
                   --limit 3 --json title,body`). A convention you already \
                   remember for this repository or team counts too. Where these \
                   disagree, the repository's own written rules win.\n\
                   2. Review the changes as a reviewer would. Fix what would \
                   block a merge — a bug, a broken build, a failing test — and \
                   nothing else: do not refactor or extend the work. Run the \
                   project's checks.\n\
                   3. If the checked-out branch is the default branch, create a \
                   branch for this work first, named the way this repository \
                   names its branches. Never commit or push to the default \
                   branch.\n\
                   4. Commit in as few commits as tell the story honestly: \
                   related changes together, one commit per independent concern, \
                   never one commit per file. Follow the commit convention from \
                   step 1.\n\
                   5. Push with `git push -u origin HEAD`, then open the pull \
                   request with `gh pr create --assignee @me` so it is assigned to \
                   whoever `gh` is signed in as. Its title and body follow the \
                   convention from step 1; fill the repository's template when it \
                   has one. Report the pull request URL when you are done.\n\
                   \n\
                   Do not force-push and do not touch any other branch. If \
                   something stops you — a check that fails for reasons outside \
                   these changes, a convention you cannot satisfy — stop and say \
                   so instead of working around it."
                .to_string(),
        },
    ]
}

/// Upper bound on any one field of a [`PullRequestDraft`], checked by the
/// daemon before the draft is broadcast to every client.
pub const MAX_DRAFT_FIELD_BYTES: usize = 64 * 1024;

/// The text a [`TaskMode::Draft`] agent hands back through `forgectl pr draft`.
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

/// The task with `id`, or the first built-in when there is no such task.
///
/// Never `None`: every surface that asks has to render *something*, and a
/// remembered id that no longer exists is a stale preference rather than an
/// error worth showing.
#[must_use]
pub fn task_or_default(id: Option<&str>) -> PrTask {
    let tasks = builtin_tasks();
    id.and_then(|id| tasks.iter().find(|task| task.id == id).cloned())
        .unwrap_or_else(|| tasks.into_iter().next().expect("there is a built-in task"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_draft_leaves_the_pull_request_to_forge() {
        assert!(TaskMode::Draft.forge_opens_the_pr());
        assert!(!TaskMode::Implement.forge_opens_the_pr());
    }

    #[test]
    fn the_builtin_tasks_have_distinct_ids_and_both_modes() {
        let tasks = builtin_tasks();
        let mut ids: Vec<&str> = tasks.iter().map(|task| task.id.as_str()).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), tasks.len(), "two tasks share an id");
        assert!(tasks.iter().any(|task| task.mode == TaskMode::Draft));
        assert!(tasks.iter().any(|task| task.mode == TaskMode::Implement));
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
    fn every_draft_task_hands_its_text_to_forgectl() {
        for task in builtin_tasks() {
            let hands_over = task.body.contains("forgectl pr draft");
            assert_eq!(hands_over, task.mode == TaskMode::Draft, "{}", task.id);
        }
    }

    /// A remembered choice that no longer exists is a stale preference, not a
    /// reason to render nothing.
    #[test]
    fn an_unknown_task_id_falls_back_to_the_first() {
        assert_eq!(task_or_default(Some("describe")).id, "describe");
        assert_eq!(task_or_default(Some("gone")).id, builtin_tasks()[0].id);
        assert_eq!(task_or_default(None).id, builtin_tasks()[0].id);
    }
}
