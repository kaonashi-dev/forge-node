//! What to ask an agent for, on the way to a pull request (§16.8).
//!
//! A task is a *template plus a mode*. The template is the wording; the mode is
//! what the answer is for, and it is what decides who opens the pull request:
//!
//! - [`TaskMode::Draft`] — the agent only writes the title and body. Forge Node
//!   pushes and calls `gh pr create` itself, through the path that already
//!   exists for Juva. The agent never touches the working tree.
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
            detail: "The agent writes the title and body; Forge Node opens the PR.".to_string(),
            mode: TaskMode::Draft,
            body: "Read the diff below and write a pull request for it.\n\
                   \n\
                   Answer with the title on the first line, then a blank line, \
                   then the body in Markdown. Say what changed and why; do not \
                   list the files, the diff already does. Do not write anything \
                   else — no preamble, no closing remark."
                .to_string(),
        },
        PrTask {
            id: "review".to_string(),
            name: "Review, then describe".to_string(),
            detail: "The agent reads the diff critically first, then writes the PR.".to_string(),
            mode: TaskMode::Draft,
            body: "Read the diff below as a reviewer would. If you find something \
                   that would block a merge — a bug, a missing test, a leak — say \
                   so plainly at the top of the body, under a `## Concerns` \
                   heading.\n\
                   \n\
                   Then write the pull request: title on the first line, a blank \
                   line, then the body in Markdown."
                .to_string(),
        },
        PrTask {
            id: "finish".to_string(),
            name: "Finish and open the PR".to_string(),
            detail: "The agent works in this checkout, commits, pushes and opens the PR."
                .to_string(),
            mode: TaskMode::Implement,
            body: "You are working in a git checkout that already has \
                   uncommitted changes, summarised below.\n\
                   \n\
                   Finish the work: make it build and pass its tests, then commit \
                   it, push the branch, and open a pull request with `gh`. Report \
                   the pull request URL when you are done. Do not force-push and \
                   do not touch any branch other than the one checked out here."
                .to_string(),
        },
    ]
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

    /// A remembered choice that no longer exists is a stale preference, not a
    /// reason to render nothing.
    #[test]
    fn an_unknown_task_id_falls_back_to_the_first() {
        assert_eq!(task_or_default(Some("describe")).id, "describe");
        assert_eq!(task_or_default(Some("gone")).id, builtin_tasks()[0].id);
        assert_eq!(task_or_default(None).id, builtin_tasks()[0].id);
    }
}
