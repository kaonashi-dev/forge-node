//! # git-service
//!
//! Git and worktree operations for Forge (ForgeNode), implemented on top of the
//! system `git` CLI per **ADR-008** ("Git CLI first"): every command runs with
//! `-C <repo>`, `LC_ALL=C`, `GIT_TERMINAL_PROMPT=0`, a 30 s timeout, and never
//! inside a user PTY. It covers the discovery, status, create and remove flows
//! described in **§14** (Git y worktrees).
//!
//! This crate returns its own lightweight result structs ([`RepoStatus`],
//! [`WorktreeEntry`], [`RemovePrechecks`]); it does **not** construct
//! `domain::Workspace` values — the daemon's `WorkspaceService` maps these
//! results into domain objects (§9.3, §17).
//!
//! Modules mirror §17 (`src/{command,repository,remote,worktree,change,github}.rs`):
//! - [`command`]: the [`command::run_git`] subprocess helper and [`GitError`].
//! - [`repository`]: read-only queries (discovery, branch, refs, status, worktrees).
//! - [`remote`]: the only commands that open a socket ([`remote::fetch`]).
//! - [`change`]: working-tree diffs, local commits, and (explicit) push.
//! - [`diff`]: per-file working-tree patches for the GUI's diff view.
//! - [`rebase`]: the stopped sequencer — conflicts, continue, abort.
//! - [`github`]: create and list pull requests through `gh` (host API, not git objects).
//! - [`worktree`]: slug generation, validation, create/remove, pre-checks.

pub mod change;
pub mod command;
pub mod diff;
pub mod github;
pub mod rebase;
pub mod remote;
pub mod repository;
pub mod worktree;

pub use change::{change_context, commit, push, ChangeSnapshot, MAX_FILES, MAX_PATCH_BYTES};
pub use command::{
    run_git, run_git_network, GitError, GitOutput, GIT_NETWORK_TIMEOUT, GIT_TIMEOUT,
};
pub use diff::{
    change_summary, commit_count, commits_since, working_tree_diff, ChangeSummaryLine, CommitEntry,
    FileChange, FileDiff, SummaryOfChanges, WorkingTreeDiff, DIFF_CONTEXT_LINES, MAX_DIFF_BYTES,
    MAX_DIFF_FILES, MAX_FILE_PATCH_BYTES, MAX_SUMMARY_COMMITS,
};
pub use github::{
    create_pull_request, create_pull_request_with_cli, list_pull_requests,
    list_pull_requests_with_cli, parse_remote_url, GitHubCli, GitHubPullRequest, GitHubRepo,
    PullRequest, PullRequestLabel, PullRequestPage, PullRequestReviewDecision, GH_TIMEOUT,
    MAX_PR_BODY,
};
pub use rebase::{
    abort as abort_sequencer, continue_sequencer, mark_resolved, state as sequencer_state,
    Conflict, Continued, Sequencer, SequencerState, MAX_CONFLICTS,
};
pub use remote::{default_remote, fetch, has_remote, FetchOutcome};
pub use repository::{
    commit_exists, common_dir, current_branch, default_branch, discover_root, head_commit,
    ignored_paths, is_tracked, list_branches, list_refs, list_remotes, list_worktrees, merge_base,
    set_local_excludes, short_commit, status, IgnoredPath, RefEntry, RepoStatus, WorktreeEntry,
};
pub use worktree::{
    create, precheck_remove, remove, slugify, unique_slug, validate_branch_name, RemovePrechecks,
};
