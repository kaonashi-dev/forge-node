//! Forge Review: a structured review of one workspace's changes.
//!
//! A run and its findings are metadata the daemon persists; the diff, the pack
//! and the reviewer's transcript never are.

use serde::{Deserialize, Serialize};

use crate::ids::{ReviewId, SessionId, Timestamp, WorkspaceId};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ReviewTarget {
    /// The working tree, uncommitted changes included, against the merge base
    /// with `base` (or the default branch when `None`).
    Workspace {
        workspace_id: WorkspaceId,
        base: Option<String>,
    },
}

impl ReviewTarget {
    #[must_use]
    pub fn workspace_id(&self) -> WorkspaceId {
        match self {
            Self::Workspace { workspace_id, .. } => *workspace_id,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum ReviewStatus {
    Queued,
    Running,
    Completed,
    Failed,
    Cancelled,
    /// The daemon stopped while the run was in flight.
    Interrupted,
}

impl ReviewStatus {
    #[must_use]
    pub fn is_finished(self) -> bool {
        !matches!(self, Self::Queued | Self::Running)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum ReviewStage {
    Diff,
    Pack,
    Model,
    Validate,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Severity {
    P0,
    P1,
    P2,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Verdict {
    Block,
    Review,
    Ok,
}

/// Sets the pack's caps; every tier runs one pass.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tier {
    #[default]
    Base,
    Plus,
    Apex,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Coverage {
    pub included: Vec<String>,
    pub excluded: Vec<String>,
    /// Left out of the pack by the tier's caps.
    pub truncated: Vec<String>,
    pub redactions: u32,
    pub notes: Vec<String>,
    /// The ids of the project rules that entered the pack.
    #[serde(default)]
    pub rules: Vec<String>,
}

/// Compact on purpose: it is what `ReviewUpdated` carries, so it holds no findings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReviewRun {
    pub id: ReviewId,
    pub workspace_id: WorkspaceId,
    pub target: ReviewTarget,
    pub head_oid: String,
    pub base_oid: Option<String>,
    pub tier: Tier,
    /// The resolved preset id.
    pub preset: String,
    pub provider: String,
    pub status: ReviewStatus,
    pub stage: Option<ReviewStage>,
    pub summary: String,
    /// `0.0..=5.0` in steps of `0.5`.
    pub score: Option<f32>,
    pub verdict: Option<Verdict>,
    pub error: Option<String>,
    /// The reviewer agent session, once launched.
    pub session_id: Option<SessionId>,
    pub coverage: Coverage,
    pub created_at: Timestamp,
    pub finished_at: Option<Timestamp>,
}

// `score` is a clamped multiple of 0.5, never NaN, so equality is total.
impl Eq for ReviewRun {}

/// Anchored on the new side of the diff.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Finding {
    /// Position within its run.
    pub id: u32,
    /// Stable across line moves: path, category, rule and evidence only.
    pub fingerprint: String,
    pub path: String,
    /// 1-based; `None` is a finding about the file as a whole.
    pub line: Option<u32>,
    pub end_line: Option<u32>,
    pub severity: Severity,
    pub category: String,
    pub rule_id: Option<String>,
    /// `0.0..=1.0`.
    pub confidence: f32,
    pub title: String,
    pub body: String,
    /// Copied literally from the new code; what the anchor was checked against.
    pub evidence: String,
    pub suggestion: Option<String>,
}

// `confidence` is clamped to `0..=1`, never NaN, so equality is total.
impl Eq for Finding {}

/// `.forge/review/` of one workspace's working tree, answering `GetReviewConfig`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewSettings {
    /// Resolved, so always one of `presets`.
    pub default_preset: String,
    /// The built-ins, then the repository's; one that replaces a built-in is listed once.
    pub presets: Vec<ReviewPreset>,
    pub rules: Vec<ReviewRule>,
    pub errors: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewPreset {
    pub id: String,
    pub name: String,
    /// One line for the picker; empty when the preset file gives none.
    #[serde(default)]
    pub detail: String,
    pub tier: Option<Tier>,
}

/// A rule without its body.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewRule {
    pub id: String,
    pub title: String,
    /// The most severe level a finding under this rule may carry.
    pub severity: Severity,
    pub category: String,
    pub scope: Vec<String>,
    pub enabled: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_run_and_its_findings_round_trip_through_json() {
        let workspace_id = WorkspaceId::new();
        let run = ReviewRun {
            id: ReviewId::new(),
            workspace_id,
            target: ReviewTarget::Workspace {
                workspace_id,
                base: Some("main".into()),
            },
            head_oid: "abc".into(),
            base_oid: None,
            tier: Tier::Plus,
            preset: "security".into(),
            provider: "claude".into(),
            status: ReviewStatus::Running,
            stage: Some(ReviewStage::Model),
            summary: String::new(),
            score: Some(3.5),
            verdict: Some(Verdict::Review),
            error: None,
            session_id: Some(SessionId::new()),
            coverage: Coverage {
                included: vec!["a.rs".into()],
                redactions: 2,
                ..Coverage::default()
            },
            created_at: Timestamp::now(),
            finished_at: None,
        };
        let json = serde_json::to_value(&run).unwrap();
        assert_eq!(json["tier"], "plus");
        assert_eq!(json["target"]["kind"], "workspace");
        assert_eq!(json["verdict"], "Review");
        assert_eq!(serde_json::from_value::<ReviewRun>(json).unwrap(), run);

        let finding = Finding {
            id: 1,
            fingerprint: "0123456789abcdef".into(),
            path: "src/lib.rs".into(),
            line: Some(4),
            end_line: None,
            severity: Severity::P1,
            category: "bug".into(),
            rule_id: None,
            confidence: 0.75,
            title: "t".into(),
            body: "b".into(),
            evidence: "let x = 1;".into(),
            suggestion: Some("let x = 2;".into()),
        };
        let json = serde_json::to_value(&finding).unwrap();
        assert_eq!(json["severity"], "P1");
        assert_eq!(serde_json::from_value::<Finding>(json).unwrap(), finding);
    }
}
