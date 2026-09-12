//! Adoption and forgetting of worktrees (§14.4).
//!
//! The rescan sees every entry `git worktree list` reports and every row the
//! model holds; what to do with them is this module's only concern. [`plan`] is
//! a **pure** function of git's listing, the model rows, and the project's
//! ignore rules, so the interesting behaviour is unit-testable without git,
//! a database or a lock. `core.rs` supplies the inputs — stats and
//! canonicalizes paths *outside* the core lock — and applies the result in one
//! critical section, the `idle.rs`/`shares::plan` split.
//!
//! Two kinds of rule share one storage: an `Exact` tombstone is state written
//! by `RemoveWorktree` and collected by the GC once the worktree is really
//! gone; a `Subtree` rule is policy written by the user and never collected.

use domain::{IgnoreScope, WorkspaceId};
use std::path::{Path, PathBuf};

/// One ignore rule as the plan needs it: the domain shape plus the one
/// filesystem fact only the caller can observe, stat'ed before the lock.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IgnoreRule {
    pub scope: IgnoreScope,
    /// Canonical, like every other path the plan compares.
    pub path: PathBuf,
    /// Whether the rule's path exists on disk right now.
    pub on_disk: bool,
}

/// One non-bare, non-main entry from `git worktree list`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Listed {
    pub path: PathBuf,
    pub branch: Option<String>,
    /// A `prunable` entry or one whose directory is gone: still listed by git,
    /// but not a live checkout.
    pub stale: bool,
}

/// One `GitWorktree` row of the project.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Known {
    pub id: WorkspaceId,
    pub path: PathBuf,
    pub on_disk: bool,
    /// Sessions reference their workspace with `ON DELETE RESTRICT`, so a row
    /// that still has one is kept and reported instead of dropped.
    pub has_sessions: bool,
}

/// What one reconcile pass does.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Plan {
    pub adopt: Vec<Listed>,
    pub drop: Vec<WorkspaceId>,
    pub kept_with_sessions: Vec<WorkspaceId>,
    /// `Exact` tombstones whose path is neither listed by git nor on disk: the
    /// worktree is really gone and a future one at that path must be adoptable.
    pub stale_rules: Vec<PathBuf>,
}

/// Whether any rule covers `path`. Both sides are canonical.
#[must_use]
pub fn ignores(rules: &[IgnoreRule], path: &Path) -> bool {
    rules.iter().any(|rule| match rule.scope {
        IgnoreScope::Exact => rule.path == path,
        IgnoreScope::Subtree => path.starts_with(&rule.path),
        // `IgnoreScope` is `#[non_exhaustive]`: an unknown scope ignores
        // nothing rather than guessing at a wider one.
        _ => false,
    })
}

/// Decide what one project's reconcile pass adopts, drops and collects.
///
/// Every path in every input is already canonical, and every `_on_disk` /
/// `stale` flag was observed by the caller before the core lock.
#[must_use]
pub fn plan(listed: &[Listed], known: &[Known], rules: &[IgnoreRule]) -> Plan {
    let listed_live = |path: &Path| listed.iter().any(|l| l.path == path && !l.stale);

    let adopt = listed
        .iter()
        .filter(|l| !l.stale && !ignores(rules, &l.path))
        .filter(|l| !known.iter().any(|k| k.path == l.path))
        .cloned()
        .collect();

    let mut drop = Vec::new();
    let mut kept_with_sessions = Vec::new();
    for row in known {
        // An explicit rule forgets the row even while the directory exists;
        // otherwise a row is only dropped once both git and the disk say the
        // worktree is gone, never on one of the two.
        let forget = ignores(rules, &row.path) || (!listed_live(&row.path) && !row.on_disk);
        if !forget {
            continue;
        }
        if row.has_sessions {
            kept_with_sessions.push(row.id);
        } else {
            drop.push(row.id);
        }
    }

    let stale_rules = rules
        .iter()
        .filter(|rule| rule.scope == IgnoreScope::Exact)
        .filter(|rule| !listed.iter().any(|l| l.path == rule.path) && !rule.on_disk)
        .map(|rule| rule.path.clone())
        .collect();

    Plan {
        adopt,
        drop,
        kept_with_sessions,
        stale_rules,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listed(path: &str, stale: bool) -> Listed {
        Listed {
            path: PathBuf::from(path),
            branch: Some("b".to_owned()),
            stale,
        }
    }

    fn known(id: WorkspaceId, path: &str, on_disk: bool, has_sessions: bool) -> Known {
        Known {
            id,
            path: PathBuf::from(path),
            on_disk,
            has_sessions,
        }
    }

    fn rule(scope: IgnoreScope, path: &str, on_disk: bool) -> IgnoreRule {
        IgnoreRule {
            scope,
            path: PathBuf::from(path),
            on_disk,
        }
    }

    #[test]
    fn an_exact_rule_does_not_match_a_common_prefix_sibling() {
        let rules = [rule(IgnoreScope::Exact, "/a/b", true)];
        let entries = [listed("/a/bc", false)];
        assert_eq!(plan(&entries, &[], &rules).adopt.len(), 1);

        let same = [listed("/a/b", false)];
        assert!(plan(&same, &[], &rules).adopt.is_empty());
    }

    #[test]
    fn a_subtree_rule_matches_descendants_and_itself() {
        let rules = [rule(IgnoreScope::Subtree, "/repo/.claude/worktrees", true)];
        let entries = [
            listed("/repo/.claude/worktrees", false),
            listed("/repo/.claude/worktrees/agent-1", false),
            listed("/repo/.claude/worktrees/nested/agent-2", false),
            listed("/repo/.claude/worktrees-other/agent-3", false),
        ];
        let decision = plan(&entries, &[], &rules);
        assert_eq!(decision.adopt.len(), 1, "the prefix sibling is not covered");
        assert_eq!(
            decision.adopt[0].path,
            PathBuf::from("/repo/.claude/worktrees-other/agent-3")
        );
    }

    #[test]
    fn a_stale_entry_is_never_adopted() {
        let entries = [listed("/repo/ghost", true)];
        assert!(plan(&entries, &[], &[]).adopt.is_empty());
    }

    #[test]
    fn an_ignored_row_is_dropped_even_though_the_directory_exists() {
        let id = WorkspaceId::new();
        let rows = [known(id, "/repo/.claude/worktrees/agent-1", true, false)];
        let rules = [rule(IgnoreScope::Subtree, "/repo/.claude/worktrees", true)];
        let decision = plan(&[], &rows, &rules);
        assert_eq!(decision.drop, vec![id]);
        assert!(decision.kept_with_sessions.is_empty());
    }

    #[test]
    fn an_ignored_row_with_sessions_is_kept_and_later_released() {
        let id = WorkspaceId::new();
        let rules = [rule(IgnoreScope::Subtree, "/repo/.claude/worktrees", true)];
        let with = [known(id, "/repo/.claude/worktrees/agent-1", true, true)];
        let decision = plan(&[], &with, &rules);
        assert!(decision.drop.is_empty());
        assert_eq!(decision.kept_with_sessions, vec![id]);

        let without = [known(id, "/repo/.claude/worktrees/agent-1", true, false)];
        assert_eq!(plan(&[], &without, &rules).drop, vec![id]);
    }

    #[test]
    fn a_vanished_row_is_dropped_but_a_half_vanished_one_is_kept() {
        let id = WorkspaceId::new();
        // Neither git nor the disk knows it: gone.
        assert_eq!(
            plan(&[], &[known(id, "/repo/gone", false, false)], &[]).drop,
            vec![id]
        );
        // Git still lists it (a live entry): kept.
        let listing = [listed("/repo/live", false)];
        assert!(plan(&listing, &[known(id, "/repo/live", true, false)], &[])
            .drop
            .is_empty());
        // Git marks it prunable but the directory is there: kept, because the
        // directory is the side that can still hold work.
        let stale_listing = [listed("/repo/locked", true)];
        assert!(plan(
            &stale_listing,
            &[known(id, "/repo/locked", true, false)],
            &[]
        )
        .drop
        .is_empty());
    }

    #[test]
    fn only_exact_rules_are_collected_when_their_path_is_really_gone() {
        let rules = [
            rule(IgnoreScope::Exact, "/repo/one", false),
            rule(IgnoreScope::Exact, "/repo/two", true),
            rule(IgnoreScope::Subtree, "/repo/tree", false),
        ];
        let stale = plan(&[], &[], &rules).stale_rules;
        assert_eq!(stale, vec![PathBuf::from("/repo/one")]);
    }

    #[test]
    fn a_tombstone_stays_while_git_still_lists_the_path() {
        let rules = [rule(IgnoreScope::Exact, "/repo/one", false)];
        let entries = [listed("/repo/one", true)];
        assert!(plan(&entries, &[], &rules).stale_rules.is_empty());
    }

    #[test]
    fn an_adopted_entry_does_not_replace_an_existing_row() {
        let id = WorkspaceId::new();
        let listing = [listed("/repo/one", false)];
        let rows = [known(id, "/repo/one", true, false)];
        assert!(plan(&listing, &rows, &[]).adopt.is_empty());
    }
}
