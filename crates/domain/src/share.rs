//! Files a project shares between its workspaces (§14.2).
//!
//! A managed worktree lives under `worktrees.root`, away from the repository,
//! so it starts without the untracked files a project needs to run: `.env`, a
//! local settings file, a certificate, an installed `node_modules`. A
//! [`ShareRule`] says how one path reaches every workspace of the project, and
//! the rules are the project's own — the global `[worktrees] copy` list is only
//! the default for a project that has none.
//!
//! Only [`ShareRule`] is persisted. [`ShareCandidate`], [`ShareAction`] and
//! [`ShareStatusEntry`] are runtime state, like [`crate::WorkspaceStatus`] and
//! [`crate::WorkspaceDiff`]: read on demand, never a column.

use crate::ids::{ProjectId, ShareRuleId, Timestamp};
use serde::{Deserialize, Serialize};

/// How one path becomes available in a workspace (§14.2).
///
/// No single mechanism is right for every file, which is why this is a rule and
/// not a list: a secret wants one copy everyone sees, a dependency directory
/// wants isolation per branch, and a build cache wants neither.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ShareStrategy {
    /// Byte-for-byte copy from the source workspace. Total isolation: what the
    /// worktree does to its copy cannot reach anyone else's.
    Copy,
    /// Copy-on-write clone (APFS `clonefile`, Linux `FICLONE`), falling back to
    /// a plain copy — reported, never silent — on a filesystem without it.
    /// The only strategy that makes a `node_modules`-sized directory free.
    Clone,
    /// A symlink into the project's shared store. One file, every workspace:
    /// an edit is visible everywhere, and nothing is duplicated at rest.
    Link,
    /// Produce the path by running a command in the workspace when it is
    /// missing. The path is the goal; the command is how it is reached.
    Run {
        /// Run through `sh -c` from the workspace root.
        command: String,
        /// Wall-clock budget; the process group is killed past it.
        timeout_secs: u64,
    },
}

impl ShareStrategy {
    /// Whether this strategy reads the project's shared store rather than the
    /// source workspace.
    #[must_use]
    pub const fn uses_store(&self) -> bool {
        matches!(self, Self::Link)
    }

    /// Whether this strategy writes a file the removal path can recognize as
    /// Forge's own (§`ShareCleanup::RemoveInjected`).
    #[must_use]
    pub const fn writes_a_file(&self) -> bool {
        matches!(self, Self::Copy | Self::Clone | Self::Link)
    }

    /// A stable, human-readable name for logs and notices.
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Copy => "copy",
            Self::Clone => "clone",
            Self::Link => "link",
            Self::Run { .. } => "run",
        }
    }
}

/// One sharing rule of one project (§14.2).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShareRule {
    /// Identity, stable across edits of the path or the strategy.
    pub id: ShareRuleId,
    /// The project whose workspaces this rule applies to.
    pub project_id: ProjectId,
    /// Relative to the repository root. No `..`, never absolute, never inside
    /// `.git`. A trailing `/` is not part of the identity.
    pub path: String,
    /// How the path reaches a workspace.
    pub strategy: ShareStrategy,
    /// Off keeps the row and stops applying it, leaving what is already on
    /// disk untouched.
    pub enabled: bool,
    /// Application order. `Run` rules always sort last, whatever this says.
    pub position: u32,
    /// When the rule was first written.
    pub created_at: Timestamp,
}

/// What a detected path looks like, which drives the strategy the GUI proposes.
///
/// A classification never *decides* anything at apply time: it is a suggestion
/// made once, and the rule the user stores is what runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum ShareClass {
    /// `.env`, a private key, a certificate.
    Secret,
    /// `node_modules`, `.venv`, `vendor`: installed, per-branch, large.
    Dependencies,
    /// `target`, `dist`, `.next`: derived and rebuildable.
    BuildOutput,
    /// A tool's own cache directory.
    Cache,
    /// Editor and agent settings that must not be aliased across workspaces.
    EditorState,
    /// Nothing the classifier recognizes.
    Other,
}

impl ShareClass {
    /// The strategy proposed for a freshly detected path of this class.
    #[must_use]
    pub const fn suggested(self, is_dir: bool) -> ShareStrategy {
        match self {
            // A secret is small and wants one source of truth; a directory of
            // them is still not something to duplicate per worktree.
            Self::Secret => ShareStrategy::Link,
            // Isolation is required: two branches can disagree on a lockfile.
            Self::Dependencies | Self::BuildOutput | Self::Cache => ShareStrategy::Clone,
            Self::EditorState => ShareStrategy::Copy,
            Self::Other => {
                if is_dir {
                    ShareStrategy::Clone
                } else {
                    ShareStrategy::Copy
                }
            }
        }
    }
}

/// A path Forge found ignored in the project and can propose a rule for (§14.2).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShareCandidate {
    /// Relative to the repository root.
    pub path: String,
    /// What the classifier made of it.
    pub class: ShareClass,
    /// Whether the path is a directory in the source workspace.
    pub is_dir: bool,
    /// `None` when the scan hit its budget before finishing the subtree: a
    /// missing number is honest, an incomplete one is not.
    pub size_bytes: Option<u64>,
    /// Entries walked, on the same terms as `size_bytes`.
    pub entries: Option<u64>,
    /// What the GUI preselects.
    pub suggested: ShareStrategy,
    /// A rule for this path already exists, so the row is informational.
    pub already_ruled: bool,
}

/// What one rule looks like in one workspace right now (§14.2).
///
/// Runtime-only, like [`crate::WorkspaceStatus`]: recomputed on demand, never
/// persisted, and "not measured" is not the same as "applied".
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ShareState {
    /// Present, and shaped the way the rule says.
    Applied,
    /// Nothing at that path.
    Missing,
    /// A `Link` whose symlink is now a regular file — an editor wrote through
    /// `rename()` and the share silently stopped. The failure this whole
    /// status exists to make visible.
    Severed,
    /// A copy or clone whose source has changed since it was made.
    Diverged,
    /// The rule did not apply here, and why.
    Skipped {
        /// Human-readable reason; never carries file contents.
        reason: String,
    },
    /// The last attempt failed.
    Failed {
        /// Human-readable message; never carries file contents.
        message: String,
    },
}

/// One rule's state in one workspace.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShareStatusEntry {
    /// Which rule this row is about.
    pub rule_id: ShareRuleId,
    /// The rule's path, so a client can render without joining.
    pub path: String,
    /// What the disk says.
    pub state: ShareState,
}

/// What applying a rule did, or would do (§14.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ShareVerb {
    /// A byte-for-byte copy.
    Copy,
    /// A copy-on-write clone.
    Clone,
    /// A symlink into the store.
    Link,
    /// A command run in the workspace.
    Run,
    /// Nothing to do, or nothing that may be done.
    Skip,
    /// Something that was in the way was moved aside first.
    Backup,
    /// Something Forge had written was taken away.
    Remove,
}

/// One line of a preview or of an application result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShareAction {
    /// The rule that produced this line.
    pub rule_id: ShareRuleId,
    /// Relative path in the workspace.
    pub path: String,
    /// What was, or would be, done.
    pub verb: ShareVerb,
    /// Bytes involved, where they are known and worth showing.
    pub bytes: Option<u64>,
    /// A `Clone` that degraded to a full copy on this filesystem.
    pub fallback: bool,
    /// Why a `Skip`, or anything else the user should read.
    pub note: Option<String>,
}

/// What happens to the files a rule already put in every workspace when the
/// rule is removed (§14.2).
///
/// Removal is the one edit with effects outside its own row, so it is asked
/// once, explicitly, and never guessed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ShareCleanup {
    /// Leave every workspace exactly as it is.
    #[default]
    Leave,
    /// Delete only what Forge wrote and can still recognize: a link into the
    /// store, or a copy that has not been modified since. Anything else is
    /// left and reported — an edited `.env` is somebody's work.
    RemoveInjected,
    /// `Link` only: replace each symlink with a real copy of the store file, so
    /// every workspace keeps a working, independent file.
    Materialize,
}

/// Where a workspace's shared files are read from when a rule is applied.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum ShareTrigger {
    /// A worktree Forge has just created.
    Created,
    /// A worktree created outside Forge and adopted by the rescan.
    Adopted,
    /// An explicit request from the GUI.
    Requested,
}

/// Maximum rules one project may hold. A list this long is a misconfiguration,
/// and the cap keeps one `ApplyShares` bounded.
pub const MAX_SHARE_RULES: usize = 200;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_are_linked_and_dependencies_cloned() {
        assert_eq!(ShareClass::Secret.suggested(false), ShareStrategy::Link);
        assert_eq!(
            ShareClass::Dependencies.suggested(true),
            ShareStrategy::Clone
        );
        assert_eq!(
            ShareClass::EditorState.suggested(false),
            ShareStrategy::Copy
        );
        assert_eq!(ShareClass::Other.suggested(true), ShareStrategy::Clone);
        assert_eq!(ShareClass::Other.suggested(false), ShareStrategy::Copy);
    }

    #[test]
    fn a_strategy_round_trips_through_json() {
        for strategy in [
            ShareStrategy::Copy,
            ShareStrategy::Clone,
            ShareStrategy::Link,
            ShareStrategy::Run {
                command: "pnpm install".to_owned(),
                timeout_secs: 600,
            },
        ] {
            let encoded = serde_json::to_string(&strategy).expect("encode");
            let decoded: ShareStrategy = serde_json::from_str(&encoded).expect("decode");
            assert_eq!(strategy, decoded);
        }
    }

    #[test]
    fn cleanup_defaults_to_leaving_the_disk_alone() {
        assert_eq!(ShareCleanup::default(), ShareCleanup::Leave);
    }
}
