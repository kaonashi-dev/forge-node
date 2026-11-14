use std::collections::HashSet;

use domain::{Finding, FindingReaction, Lineage, ReviewRun, ReviewTarget};

/// Same workspace and preset, then the same branch for a workspace run or the
/// same pull request for a pull-request run.
#[must_use]
pub fn same_lineage(a: &ReviewRun, b: &ReviewRun) -> bool {
    if a.workspace_id != b.workspace_id || a.preset != b.preset {
        return false;
    }
    match (&a.target, &b.target) {
        (ReviewTarget::Workspace { .. }, ReviewTarget::Workspace { .. }) => a.branch == b.branch,
        (
            ReviewTarget::PullRequest { number: a, .. },
            ReviewTarget::PullRequest { number: b, .. },
        ) => a == b,
        _ => false,
    }
}

pub fn mark_lineage(current: &mut [Finding], previous: &[Finding]) {
    let before: HashSet<&str> = previous.iter().map(|f| f.fingerprint.as_str()).collect();
    for finding in current {
        finding.lineage = if before.contains(finding.fingerprint.as_str()) {
            Lineage::Persisting
        } else {
            Lineage::New
        };
    }
}

/// The previous run's findings that the current run no longer reports. A dismissed
/// one is left out, and so is one in `withheld`: the reviewer still reported it
/// and validation hid it, which is not a fix either.
#[must_use]
pub fn resolved(previous: &[Finding], current: &[Finding], withheld: &[String]) -> Vec<Finding> {
    let now: HashSet<&str> = current
        .iter()
        .map(|f| f.fingerprint.as_str())
        .chain(withheld.iter().map(String::as_str))
        .collect();
    previous
        .iter()
        .filter(|f| {
            f.reaction != Some(FindingReaction::Dismiss) && !now.contains(f.fingerprint.as_str())
        })
        .cloned()
        .collect()
}
