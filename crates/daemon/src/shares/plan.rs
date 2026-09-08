//! What applying a rule *would* do — a pure function of the rules and the disk
//! (§14.2).
//!
//! Every decision this feature makes lives here, so the whole table can be
//! tested without a filesystem, a repository or a daemon. `apply.rs` reads
//! these actions and performs them; it makes no decisions of its own.

use domain::{ShareAction, ShareRule, ShareStrategy, ShareVerb};

use super::{Capabilities, EntryKind, MAX_COPY_BYTES, MAX_COPY_ENTRIES};

/// What one rule's paths look like right now.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Observation {
    /// The path in the source workspace.
    pub source: EntryKind,
    /// The path in the workspace being provisioned.
    pub target: EntryKind,
    /// The path in the project's shared store.
    pub store: EntryKind,
    /// The target is a symlink and it resolves to this rule's store path.
    pub link_is_ours: bool,
    /// Bytes the source occupies, when they were measured.
    pub bytes: Option<u64>,
    /// Entries the source holds, when they were measured.
    pub entries: Option<u64>,
}

/// Decide what to do with one rule.
///
/// `explicit` is the user asking for this rule by name: only then may something
/// that is already on disk be moved aside and rewritten. A background run —
/// creating a worktree, adopting one — never overwrites, because the file it
/// would replace may be an agent's work.
#[must_use]
pub fn plan_rule(
    rule: &ShareRule,
    seen: Observation,
    caps: Capabilities,
    explicit: bool,
) -> ShareAction {
    let mut action = ShareAction {
        rule_id: rule.id,
        path: rule.path.clone(),
        verb: ShareVerb::Skip,
        bytes: seen.bytes,
        fallback: false,
        note: None,
    };

    if !rule.enabled {
        action.note = Some("rule is off".to_owned());
        return action;
    }

    match &rule.strategy {
        ShareStrategy::Run { command, .. } => {
            if seen.target.exists() {
                action.note = Some("already present".to_owned());
            } else {
                action.verb = ShareVerb::Run;
                action.note = Some(command.clone());
            }
        }
        ShareStrategy::Link => plan_link(&mut action, seen, explicit),
        ShareStrategy::Copy => plan_write(&mut action, seen, caps, explicit, false),
        ShareStrategy::Clone => plan_write(&mut action, seen, caps, explicit, true),
        _ => action.note = Some("strategy not supported by this build".to_owned()),
    }
    action
}

fn plan_link(action: &mut ShareAction, seen: Observation, explicit: bool) {
    if !seen.store.exists() {
        // Seeding the store *moves* the user's real file, so it is its own
        // gesture with its own confirmation. Never a side effect of an apply.
        action.note = Some(if seen.source.exists() {
            "not in the shared store yet — adopt it first".to_owned()
        } else {
            "nothing to share yet".to_owned()
        });
        return;
    }
    match seen.target {
        EntryKind::Missing => action.verb = ShareVerb::Link,
        EntryKind::Symlink if seen.link_is_ours => {
            action.note = Some("already linked".to_owned());
        }
        // A `rename()` from an editor turns the link into a regular file and
        // the share stops without a word. Naming it is the whole point of the
        // status; repairing it is a gesture, not a background rewrite.
        EntryKind::File | EntryKind::Symlink if explicit => action.verb = ShareVerb::Link,
        EntryKind::File | EntryKind::Symlink => {
            action.note = Some("a real file is in the way — re-link to replace it".to_owned());
        }
        EntryKind::Dir | EntryKind::Other => {
            action.note = Some("a directory is in the way".to_owned());
        }
    }
}

fn plan_write(
    action: &mut ShareAction,
    seen: Observation,
    caps: Capabilities,
    explicit: bool,
    cloning: bool,
) {
    if !seen.source.exists() {
        action.note = Some("not present in the source workspace".to_owned());
        return;
    }
    if matches!(seen.source, EntryKind::Other) {
        action.note = Some("not a file or a directory".to_owned());
        return;
    }
    if seen.target.exists() && !explicit {
        action.note = Some("already present".to_owned());
        return;
    }

    let cloned = cloning && caps.can_clone();
    action.fallback = cloning && !cloned;

    // A clone costs nothing whatever the size; a copy — including the one a
    // clone falls back to — is budgeted before the bytes are written, not
    // after (`AGENTS.md`: clamp before the allocation).
    if !cloned {
        if seen.bytes.is_some_and(|b| b > MAX_COPY_BYTES) {
            action.note = Some(format!(
                "{} is over the copy budget of {} MB",
                if action.fallback {
                    "this volume cannot clone, and the copy"
                } else {
                    "the copy"
                },
                MAX_COPY_BYTES / (1024 * 1024)
            ));
            return;
        }
        if seen.entries.is_some_and(|n| n > MAX_COPY_ENTRIES) {
            action.note = Some(format!(
                "over the copy budget of {MAX_COPY_ENTRIES} entries"
            ));
            return;
        }
        if seen.bytes.is_none() && matches!(seen.source, EntryKind::Dir) {
            action.note = Some("could not measure the directory".to_owned());
            return;
        }
    }

    action.verb = if cloned {
        ShareVerb::Clone
    } else {
        ShareVerb::Copy
    };
    if seen.target.exists() {
        action.note = Some("replacing what is there, backed up first".to_owned());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::{ProjectId, ShareRuleId, Timestamp};

    fn rule(strategy: ShareStrategy) -> ShareRule {
        ShareRule {
            id: ShareRuleId::new(),
            project_id: ProjectId::new(),
            path: ".env".to_owned(),
            strategy,
            enabled: true,
            position: 0,
            created_at: Timestamp::now(),
        }
    }

    const CLONES: Capabilities = Capabilities {
        supports_clone: true,
        same_filesystem: true,
    };
    const COPIES: Capabilities = Capabilities {
        supports_clone: false,
        same_filesystem: true,
    };

    #[test]
    fn a_missing_source_is_skipped_rather_than_failed() {
        let action = plan_rule(
            &rule(ShareStrategy::Copy),
            Observation::default(),
            COPIES,
            false,
        );
        assert_eq!(action.verb, ShareVerb::Skip);
        assert_eq!(
            action.note.as_deref(),
            Some("not present in the source workspace")
        );
    }

    #[test]
    fn a_clone_falls_back_to_a_copy_and_says_so() {
        let seen = Observation {
            source: EntryKind::Dir,
            bytes: Some(1024),
            entries: Some(10),
            ..Observation::default()
        };
        let cloned = plan_rule(&rule(ShareStrategy::Clone), seen, CLONES, false);
        assert_eq!(cloned.verb, ShareVerb::Clone);
        assert!(!cloned.fallback);

        let copied = plan_rule(&rule(ShareStrategy::Clone), seen, COPIES, false);
        assert_eq!(copied.verb, ShareVerb::Copy);
        assert!(copied.fallback, "a degraded clone is reported, not silent");
    }

    #[test]
    fn a_clone_is_free_at_any_size_but_its_fallback_is_budgeted() {
        let huge = Observation {
            source: EntryKind::Dir,
            bytes: Some(MAX_COPY_BYTES * 4),
            entries: Some(100),
            ..Observation::default()
        };
        assert_eq!(
            plan_rule(&rule(ShareStrategy::Clone), huge, CLONES, false).verb,
            ShareVerb::Clone
        );
        let refused = plan_rule(&rule(ShareStrategy::Clone), huge, COPIES, false);
        assert_eq!(refused.verb, ShareVerb::Skip);
        assert!(refused.note.unwrap().contains("copy budget"));
    }

    #[test]
    fn an_existing_target_is_left_alone_unless_the_user_asked_for_this_rule() {
        let seen = Observation {
            source: EntryKind::File,
            target: EntryKind::File,
            bytes: Some(12),
            entries: Some(1),
            ..Observation::default()
        };
        assert_eq!(
            plan_rule(&rule(ShareStrategy::Copy), seen, COPIES, false).verb,
            ShareVerb::Skip
        );
        let forced = plan_rule(&rule(ShareStrategy::Copy), seen, COPIES, true);
        assert_eq!(forced.verb, ShareVerb::Copy);
        assert!(forced.note.unwrap().contains("backed up"));
    }

    #[test]
    fn a_link_waits_for_the_store_and_names_a_severed_one() {
        let no_store = Observation {
            source: EntryKind::File,
            ..Observation::default()
        };
        let waiting = plan_rule(&rule(ShareStrategy::Link), no_store, CLONES, false);
        assert_eq!(waiting.verb, ShareVerb::Skip);
        assert!(waiting.note.unwrap().contains("adopt it first"));

        let severed = Observation {
            source: EntryKind::File,
            target: EntryKind::File,
            store: EntryKind::File,
            ..Observation::default()
        };
        let quiet = plan_rule(&rule(ShareStrategy::Link), severed, CLONES, false);
        assert_eq!(quiet.verb, ShareVerb::Skip);
        assert!(quiet.note.unwrap().contains("re-link"));
        assert_eq!(
            plan_rule(&rule(ShareStrategy::Link), severed, CLONES, true).verb,
            ShareVerb::Link
        );
    }

    #[test]
    fn a_link_that_is_already_ours_is_not_rewritten() {
        let seen = Observation {
            source: EntryKind::File,
            target: EntryKind::Symlink,
            store: EntryKind::File,
            link_is_ours: true,
            ..Observation::default()
        };
        let action = plan_rule(&rule(ShareStrategy::Link), seen, CLONES, true);
        assert_eq!(action.verb, ShareVerb::Skip);
        assert_eq!(action.note.as_deref(), Some("already linked"));
    }

    #[test]
    fn a_run_rule_only_runs_when_its_goal_is_missing() {
        let strategy = ShareStrategy::Run {
            command: "pnpm install".to_owned(),
            timeout_secs: 60,
        };
        let missing = plan_rule(
            &rule(strategy.clone()),
            Observation::default(),
            COPIES,
            false,
        );
        assert_eq!(missing.verb, ShareVerb::Run);
        assert_eq!(missing.note.as_deref(), Some("pnpm install"));

        let present = Observation {
            target: EntryKind::Dir,
            ..Observation::default()
        };
        assert_eq!(
            plan_rule(&rule(strategy), present, COPIES, false).verb,
            ShareVerb::Skip
        );
    }

    #[test]
    fn a_disabled_rule_does_nothing_at_all() {
        let mut off = rule(ShareStrategy::Copy);
        off.enabled = false;
        let seen = Observation {
            source: EntryKind::File,
            bytes: Some(1),
            entries: Some(1),
            ..Observation::default()
        };
        let action = plan_rule(&off, seen, COPIES, true);
        assert_eq!(action.verb, ShareVerb::Skip);
        assert_eq!(action.note.as_deref(), Some("rule is off"));
    }
}
