//! What to ask an agent for when reviewing someone else's pull request (§16.9).
//!
//! The sibling of [`crate::pr_task`], and deliberately not the same thing. A
//! `PrTask` is about *your own* uncommitted work on the way to opening a pull
//! request; a [`ReviewRecipe`] is about a pull request that already exists and
//! belongs to someone else. The difference is what the agent is allowed to do:
//! a task may write, a review may not.
//!
//! Two independent guards carry that. The launch applies the provider's own
//! [`crate::ReviewStyle`] flags, and the wording below repeats the rule in
//! prose — a model that ignores one still meets the other.
//!
//! The review runs in a checkout that already exists, never in one made for
//! it: the agent reads the pull request through `gh`, which works for a fork
//! as well as for a branch of the same repository, and nothing switches the
//! branch under a working tree the user is using.
//!
//! Built-in recipes live in code, like the agent descriptors and the pull
//! request tasks do. A recipe travels with the launch; it is never daemon
//! state.

use serde::{Deserialize, Serialize};

use crate::pull_request::PullRequest;

/// One way to ask an agent to review a pull request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewRecipe {
    /// Stable key, for remembering the last choice.
    pub id: String,
    /// What the menu entry says.
    pub name: String,
    /// One line under the name: what picking it will do.
    pub detail: String,
    /// The wording handed to the agent, before the pull-request context and
    /// before anything the user adds. Empty for [`CUSTOM_RECIPE`], which is
    /// the user's own text and nothing else.
    pub body: String,
}

/// The id of the recipe that carries no wording of its own.
pub const CUSTOM_RECIPE: &str = "custom";

/// How the prompt tells the agent to read the pull request.
///
/// `gh` rather than a checkout: it is the one command that reaches a fork's
/// head as easily as a branch of the same repository, and it reads without
/// touching the working tree the user left behind.
const READ_INSTRUCTIONS: &str = "Read the pull request first, with the GitHub CLI:\n\
     \n\
     - `gh pr view {number} --repo {repo} --comments`\n\
     - `gh pr diff {number} --repo {repo}`\n\
     \n\
     You are a reader here. Do not edit, stage, commit, checkout or push \
     anything, and do not post to GitHub — no `gh pr review`, no `gh pr \
     comment`. Write your findings in this session and stop; the person \
     watching decides what reaches the pull request.";

/// The recipes Forge ships.
///
/// Deliberately few, and ordered by how often they are the right answer. This
/// is a starting point a user edits into [`CUSTOM_RECIPE`], not a catalogue:
/// a list long enough to need reading would make the common case — press the
/// button — slower than typing the prompt by hand.
#[must_use]
pub fn builtin_recipes() -> Vec<ReviewRecipe> {
    vec![
        ReviewRecipe {
            id: "standard".to_string(),
            name: "Review the change".to_string(),
            detail: "Correctness, missing tests, and anything that would block a merge."
                .to_string(),
            body: "Review this pull request the way a careful colleague would \
                   before approving it.\n\
                   \n\
                   Lead with a verdict — approve, approve with comments, or \
                   request changes — and one sentence saying why. Then list \
                   what you found, most serious first, each as `path:line` \
                   plus what breaks and when. Say plainly when a change needs \
                   a test it does not have.\n\
                   \n\
                   Do not summarise the diff back: the person reading you can \
                   already see it. Report only what they would have missed."
                .to_string(),
        },
        ReviewRecipe {
            id: "deep".to_string(),
            name: "Deep review".to_string(),
            detail: "Reads the surrounding code too — design, edge cases, cost.".to_string(),
            body: "Review this pull request against the code around it, not \
                   only against itself. Open the files it touches and their \
                   callers.\n\
                   \n\
                   Look for: a case the change does not handle, an invariant \
                   it breaks somewhere else, an error path that silently \
                   swallows a failure, work that grows with the input where \
                   the caller assumed it would not, and a public contract that \
                   changed without its callers changing.\n\
                   \n\
                   Lead with a verdict and one sentence. Then the findings, \
                   most serious first, each as `path:line` with the concrete \
                   input or state that makes it go wrong. A finding you cannot \
                   make concrete is a question, not a finding — say so."
                .to_string(),
        },
        ReviewRecipe {
            id: "security".to_string(),
            name: "Security review".to_string(),
            detail: "Input handling, secrets, authorization, injection, resource limits."
                .to_string(),
            body: "Review this pull request for security defects only.\n\
                   \n\
                   Look for: input that reaches a shell, a query or a path \
                   without being bounded or escaped; a secret, token or key in \
                   the diff or in a log line; an authorization check that is \
                   missing or applied after the effect; an allocation sized by \
                   something the caller controls; and a dependency added or \
                   bumped without a reason in the description.\n\
                   \n\
                   Report each finding as `path:line`, what an attacker \
                   controls, and what they get. Rank by what the attacker \
                   gets, not by how easy it was to spot. If you find nothing, \
                   say so — do not pad the list."
                .to_string(),
        },
        ReviewRecipe {
            id: CUSTOM_RECIPE.to_string(),
            name: "Custom".to_string(),
            detail: "Your own wording, remembered for next time.".to_string(),
            body: String::new(),
        },
    ]
}

/// The recipe with `id`, or the first built-in when there is no such recipe.
///
/// Never `None`, for the same reason [`crate::task_or_default`] never is: a
/// remembered id that no longer exists is a stale preference, not an error
/// worth putting on screen.
#[must_use]
pub fn recipe_or_default(id: Option<&str>) -> ReviewRecipe {
    let recipes = builtin_recipes();
    id.and_then(|id| recipes.iter().find(|recipe| recipe.id == id).cloned())
        .unwrap_or_else(|| {
            recipes
                .into_iter()
                .next()
                .expect("there is a built-in recipe")
        })
}

/// The whole prompt: the recipe, how to read the pull request, what the pull
/// request is, and whatever the user added.
///
/// The identity block is small on purpose. The agent is about to read the
/// title, the body and the diff itself through `gh`; repeating them here would
/// spend context on a second, staler copy. What it cannot derive — which
/// repository, which number, and that the head may live in a fork — is what
/// this carries.
#[must_use]
pub fn compose_review_prompt(recipe: &ReviewRecipe, pr: &PullRequest, extra: &str) -> String {
    let mut prompt = recipe.body.trim().to_owned();
    if !prompt.is_empty() {
        prompt.push_str("\n\n");
    }

    prompt.push_str(
        &READ_INSTRUCTIONS
            .replace("{number}", &pr.number.to_string())
            .replace("{repo}", &pr.repository),
    );

    prompt.push_str("\n\n## The pull request\n\n");
    prompt.push_str(&format!("- {}/{} #{}\n", pr.host, pr.repository, pr.number));
    prompt.push_str(&format!("- {}\n", pr.title));
    prompt.push_str(&format!("- opened by {}\n", pr.author));
    prompt.push_str(&format!("- {} → {}\n", pr.head_ref, pr.base_ref));
    prompt.push_str(&format!(
        "- {} file{}, +{} −{}\n",
        pr.changed_files,
        if pr.changed_files == 1 { "" } else { "s" },
        pr.additions,
        pr.deletions
    ));
    if pr.is_draft {
        prompt.push_str("- still a draft\n");
    }

    let trimmed = extra.trim();
    if !trimmed.is_empty() {
        prompt.push_str("\n## Also\n\n");
        prompt.push_str(trimmed);
        prompt.push('\n');
    }

    prompt
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::Timestamp;
    use crate::pull_request::{PullRequestRelations, ReviewDecision};

    fn pull_request() -> PullRequest {
        PullRequest {
            project_id: None,
            repository: "acme/widget".to_string(),
            host: "github.com".to_string(),
            number: 128,
            title: "Add the diff view".to_string(),
            body: "It adds a diff view.".to_string(),
            body_truncated: false,
            url: "https://github.com/acme/widget/pull/128".to_string(),
            author: "rin".to_string(),
            base_ref: "main".to_string(),
            head_ref: "diff-view".to_string(),
            is_draft: false,
            review_decision: Some(ReviewDecision::ReviewRequired),
            labels: Vec::new(),
            assignees: vec!["rin".to_string()],
            review_requests: Vec::new(),
            additions: 340,
            deletions: 87,
            changed_files: 12,
            comment_count: 3,
            created_at: Timestamp::now(),
            updated_at: Timestamp::now(),
            relations: PullRequestRelations {
                assigned: true,
                review_requested: false,
                authored: false,
            },
        }
    }

    #[test]
    fn the_builtin_recipes_have_distinct_ids_and_one_empty_body() {
        let recipes = builtin_recipes();
        let mut ids: Vec<&str> = recipes.iter().map(|r| r.id.as_str()).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), recipes.len(), "two recipes share an id");

        let empty: Vec<&str> = recipes
            .iter()
            .filter(|r| r.body.is_empty())
            .map(|r| r.id.as_str())
            .collect();
        assert_eq!(empty, [CUSTOM_RECIPE], "only Custom brings no wording");
    }

    #[test]
    fn an_unknown_recipe_id_falls_back_to_the_first() {
        assert_eq!(recipe_or_default(Some("security")).id, "security");
        assert_eq!(recipe_or_default(Some("gone")).id, builtin_recipes()[0].id);
        assert_eq!(recipe_or_default(None).id, builtin_recipes()[0].id);
    }

    /// The number and the repository are what `gh` needs and what the model
    /// cannot guess; an interpolation that silently kept `{number}` would send
    /// the agent a command that cannot run.
    #[test]
    fn the_prompt_names_the_pull_request_for_gh() {
        let prompt = compose_review_prompt(&recipe_or_default(None), &pull_request(), "");
        assert!(prompt.contains("gh pr diff 128 --repo acme/widget"));
        assert!(prompt.contains("gh pr view 128 --repo acme/widget"));
        assert!(!prompt.contains('{'), "an unreplaced placeholder survived");
        assert!(prompt.contains("github.com/acme/widget #128"));
        assert!(prompt.contains("diff-view → main"));
    }

    /// Prose is the second guard; the provider's own read-only flags are the
    /// first. Dropping this sentence would leave a review that can write when
    /// the flags are ever wrong.
    #[test]
    fn every_recipe_forbids_writing_and_posting() {
        for recipe in builtin_recipes() {
            let prompt = compose_review_prompt(&recipe, &pull_request(), "");
            assert!(prompt.contains("Do not edit"), "{}", recipe.id);
            assert!(prompt.contains("gh pr comment"), "{}", recipe.id);
        }
    }

    #[test]
    fn the_users_own_words_are_appended_never_substituted() {
        let recipe = recipe_or_default(Some("standard"));
        let prompt = compose_review_prompt(&recipe, &pull_request(), "  Focus on the SQL.  ");
        assert!(prompt.contains(recipe.body.trim()));
        assert!(prompt.ends_with("## Also\n\nFocus on the SQL.\n"));
    }

    /// Custom starts empty, so the prompt is the context and the user's words;
    /// a leading blank block would be the only thing it added.
    #[test]
    fn the_custom_recipe_starts_with_the_read_instructions() {
        let prompt =
            compose_review_prompt(&recipe_or_default(Some(CUSTOM_RECIPE)), &pull_request(), "");
        assert!(prompt.starts_with("Read the pull request first"));
    }
}
