//! Forge Review: a structured review of one workspace's changes.
//!
//! A run and its findings are metadata the daemon persists; the diff, the pack
//! and the reviewer's transcript never are.

use serde::{Deserialize, Serialize};

use crate::ids::{ReviewId, SessionId, Timestamp, WorkspaceId};

/// The built-in preset with no wording of its own: a run of it must bring instructions.
pub const CUSTOM_PRESET: &str = "custom";

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
    /// A pull request's head, fetched from `origin` into a private ref and
    /// reviewed against its merge base with `base`, the pull request's base
    /// branch (or the default branch when `None`). The checkout is not moved.
    PullRequest {
        workspace_id: WorkspaceId,
        number: u32,
        base: Option<String>,
    },
}

impl ReviewTarget {
    #[must_use]
    pub fn workspace_id(&self) -> WorkspaceId {
        match self {
            Self::Workspace { workspace_id, .. } | Self::PullRequest { workspace_id, .. } => {
                *workspace_id
            }
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
    /// Fingerprints the reviewer reported that validation left out, so a
    /// later run's read does not count them as resolved. At most 200.
    #[serde(default)]
    pub withheld: Vec<String>,
    /// Findings dropped because their path or evidence could not be checked
    /// against the diff.
    #[serde(default)]
    pub unverified: u32,
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
    /// The workspace's branch when the run started; `None` when detached.
    #[serde(default)]
    pub branch: Option<String>,
    /// A workspace review's working tree, written as a tree object when it was
    /// reviewed: what `GetReviewDiff` diffs against `base_oid`. `None` for a pull
    /// request, or when the snapshot could not be written.
    #[serde(default)]
    pub tree_oid: Option<String>,
    /// At most two, generated from the reviewed diff and the codebase graph.
    #[serde(default)]
    pub diagrams: Vec<Diagram>,
    /// `[review.auto_approve]` would let it through; never acted on by Forge.
    #[serde(default)]
    pub auto_approvable: bool,
    /// Reviewer agents launched; `1` is the single v1 reviewer.
    #[serde(default = "one_agent")]
    pub swarm_size: u8,
    #[serde(default)]
    pub schema_version: SchemaVersion,
    #[serde(default)]
    pub sandbox_run_id: Option<String>,
}

fn one_agent() -> u8 {
    1
}

/// The answer format the run asked its reviewers for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SchemaVersion {
    #[default]
    V1,
    V2,
}

const PROMPT_HEAD: &str = "Forge Review found these issues in your changes. Fix each one, or say \
                           why it is not a problem:\n\n";

/// Whole findings, in order, within `budget` bytes, and how many did not fit.
/// Empty when none fits or none is given; `empty_prompt_reason` tells which.
#[must_use]
pub fn findings_prompt(findings: &[&Finding], budget: usize) -> (String, usize) {
    let mut prompt = PROMPT_HEAD.to_string();
    let mut left_out = 0;
    for finding in findings {
        let mut block = format!(
            "- [{:?}] {}{} — {}\n  {}\n",
            finding.severity,
            finding.path,
            finding
                .line
                .map(|line| format!(":{line}"))
                .unwrap_or_default(),
            finding.title,
            finding.body.replace('\n', "\n  "),
        );
        if let Some(suggestion) = &finding.suggestion {
            block.push_str(&format!(
                "  Suggestion:\n  {}\n",
                suggestion.replace('\n', "\n  ")
            ));
        }
        block.push('\n');
        if prompt.len() + block.len() <= budget {
            prompt.push_str(&block);
        } else {
            left_out += 1;
        }
    }
    if left_out == findings.len() {
        return (String::new(), left_out);
    }
    (prompt, left_out)
}

/// How long completed runs of one tier took, wall clock.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TierLatency {
    pub tier: Tier,
    pub runs: u32,
    pub p50_secs: u64,
    pub p95_secs: u64,
}

/// Forge Review's own numbers. Each rate is a percentage, `None` with nothing to measure.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReviewStats {
    pub runs: u32,
    pub latency: Vec<TierLatency>,
    /// Graph queries answered from a stored graph against rebuilds, since the daemon started.
    pub graph_hit_rate: Option<f32>,
    /// Hypotheses with at least one kept finding, over every swarm run's agents.
    pub swarm_coverage: Option<f32>,
    /// Completed runs with a finding that cites the knowledge base.
    pub kb_hit_rate: Option<f32>,
    /// Sandbox-linked runs with a failed target.
    pub trex_failure_rate: Option<f32>,
    pub kb_bytes: u64,
}

// Every rate is a finite quotient rounded to a tenth, never NaN, so equality is total.
impl Eq for ReviewStats {}

/// One target of a sandbox run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxTarget {
    pub name: String,
    /// `None` when it was killed by its timeout or a signal.
    pub exit_code: Option<i32>,
    pub duration_ms: u64,
    #[serde(default)]
    pub timed_out: bool,
    /// The last 8 KiB of each, redacted.
    pub stdout: String,
    pub stderr: String,
}

impl SandboxTarget {
    #[must_use]
    pub fn passed(&self) -> bool {
        self.exit_code == Some(0)
    }
}

/// `.forge/review/sandbox.toml`'s targets run once, for a review or by hand.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxRun {
    pub id: String,
    pub workspace_id: WorkspaceId,
    #[serde(default)]
    pub review_id: Option<ReviewId>,
    pub targets: Vec<SandboxTarget>,
    /// Why nothing ran: no file, a bad file, an unapproved file, an
    /// unsupported platform; or why the run stopped early.
    #[serde(default)]
    pub error: Option<String>,
    /// The SHA-256 of a `sandbox.toml` that did not run because the project
    /// has not approved it; `ApproveSandboxConfig` with this hash approves it.
    #[serde(default)]
    pub unapproved_hash: Option<String>,
    pub started_at: Timestamp,
    #[serde(default)]
    pub finished_at: Option<Timestamp>,
}

impl SandboxRun {
    #[must_use]
    pub fn failed(&self) -> bool {
        self.error.is_some() || self.targets.iter().any(|target| !target.passed())
    }
}

/// One file of a project's review knowledge base.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KbFile {
    /// `architecture.md`, `patterns.json`, `learned-rules.md` or `false-positives.json`.
    pub file: String,
    pub content: String,
    /// `daemon`, `user`, or `derived` for the two read off feedback.
    pub source: String,
    #[serde(default)]
    pub updated_at: Option<Timestamp>,
}

/// A run's swarm: `size` agents (`None` takes `[review] swarm_default`), or
/// one per listed hypothesis.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SwarmOptions {
    #[serde(default)]
    pub size: Option<u8>,
    #[serde(default)]
    pub hypotheses: Vec<Hypothesis>,
}

/// What one swarm agent hunts for; serialized as its name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[non_exhaustive]
pub enum Hypothesis {
    RaceCondition,
    AuthBypass,
    RetryNesting,
    BrokenContract,
    DefaultMismatch,
    ResourceLeak,
    ErrorSwallow,
    NPlusOneQuery,
    MissingValidation,
    DeadCode,
    InconsistentPattern,
    CrossLayerMismatch,
    MissingTest,
    MigrationGap,
    ConfigDrift,
}

impl Hypothesis {
    pub const ALL: [Hypothesis; 15] = [
        Hypothesis::RaceCondition,
        Hypothesis::AuthBypass,
        Hypothesis::RetryNesting,
        Hypothesis::BrokenContract,
        Hypothesis::DefaultMismatch,
        Hypothesis::ResourceLeak,
        Hypothesis::ErrorSwallow,
        Hypothesis::NPlusOneQuery,
        Hypothesis::MissingValidation,
        Hypothesis::DeadCode,
        Hypothesis::InconsistentPattern,
        Hypothesis::CrossLayerMismatch,
        Hypothesis::MissingTest,
        Hypothesis::MigrationGap,
        Hypothesis::ConfigDrift,
    ];

    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::RaceCondition => "RaceCondition",
            Self::AuthBypass => "AuthBypass",
            Self::RetryNesting => "RetryNesting",
            Self::BrokenContract => "BrokenContract",
            Self::DefaultMismatch => "DefaultMismatch",
            Self::ResourceLeak => "ResourceLeak",
            Self::ErrorSwallow => "ErrorSwallow",
            Self::NPlusOneQuery => "NPlusOneQuery",
            Self::MissingValidation => "MissingValidation",
            Self::DeadCode => "DeadCode",
            Self::InconsistentPattern => "InconsistentPattern",
            Self::CrossLayerMismatch => "CrossLayerMismatch",
            Self::MissingTest => "MissingTest",
            Self::MigrationGap => "MigrationGap",
            Self::ConfigDrift => "ConfigDrift",
        }
    }

    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|h| h.name() == name)
    }
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
    #[serde(default)]
    pub lineage: Lineage,
    /// Read from the feedback table, never stored with the finding.
    #[serde(default)]
    pub reaction: Option<FindingReaction>,
    /// The swarm agent that reported it; `None` for a single reviewer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hypothesis: Option<Hypothesis>,
    /// The knowledge-base section it cites: `architecture.md#<section>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kb_ref: Option<String>,
}

// `confidence` is clamped to `0..=1`, never NaN, so equality is total.
impl Eq for Finding {}

/// Against the previous run of the same lineage; a run with none marks every finding `New`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Lineage {
    #[default]
    New,
    Persisting,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingReaction {
    Up,
    Down,
    Dismiss,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SuppressionKind {
    Rule,
    Category,
    Fingerprint,
}

/// Derived from a project's dismissals; never stored.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewSuppression {
    pub kind: SuppressionKind,
    /// The rule id, the category or the fingerprint.
    pub value: String,
    /// Set for a fingerprint: only a finding at the dismissed severity is suppressed.
    pub severity: Option<Severity>,
    pub reason: String,
}

/// `.forge/review/` of one workspace's working tree, answering `GetReviewConfig`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewSettings {
    /// Resolved, so always one of `presets`.
    pub default_preset: String,
    /// The built-ins, then the repository's; one that replaces a built-in is listed once.
    pub presets: Vec<ReviewPreset>,
    pub rules: Vec<ReviewRule>,
    pub errors: Vec<String>,
    /// The workspace's project's, derived from its feedback.
    #[serde(default)]
    pub suppressions: Vec<ReviewSuppression>,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[non_exhaustive]
pub enum NodeKind {
    File,
    Function,
    Class,
    Variable,
    /// A call nothing in the project defines, one node per name.
    ExternalCall,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[non_exhaustive]
pub enum EdgeKind {
    Calls,
    Imports,
    Uses,
    Declares,
    Inherits,
    Defines,
}

/// One node of a project's codebase graph, as a query answers it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphNode {
    pub kind: NodeKind,
    /// Empty for an external call.
    pub path: String,
    pub name: String,
    /// 1-based line of the declaration; `0` for a file or an external call.
    pub line: u32,
    #[serde(default)]
    pub signature: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "arg", rename_all = "snake_case")]
#[non_exhaustive]
pub enum GraphQuery {
    Callers(String),
    Callees(String),
    Similar(String),
    Path(String, String),
    Imports(String),
    ExternalCalls(String),
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphStats {
    pub nodes: u32,
    pub edges: u32,
    /// Files parsed by this build; an incremental build parses only what changed.
    pub parsed: u32,
    /// Files listed but not parsed (over a budget, unreadable, no grammar is not one).
    pub skipped: Vec<String>,
    pub git_oid: String,
    /// What the build could not do besides the graph, such as writing `cache.bin`.
    #[serde(default)]
    pub notes: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum GraphFormat {
    Dot,
    Mermaid,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum DiagramKind {
    Sequence,
    Er,
    Class,
    Flow,
}

/// Forge's own diagram, drawn by the GUI and exportable to mermaid. A
/// sequence's nodes are its participants in order and its edges its messages
/// in order; an entity's label is its name, then one field per line.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagram {
    pub kind: DiagramKind,
    pub title: String,
    pub nodes: Vec<DiagramNode>,
    pub edges: Vec<DiagramEdge>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiagramNode {
    pub id: String,
    pub label: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiagramEdge {
    pub from: String,
    pub to: String,
    #[serde(default)]
    pub label: String,
}

fn mermaid_text(text: &str) -> String {
    text.replace('"', "#quot;").replace(['\n', '\r'], " ")
}

fn mermaid_id(id: &str) -> String {
    id.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

impl Diagram {
    #[must_use]
    pub fn to_mermaid(&self) -> String {
        use std::fmt::Write as _;
        let mut out = String::new();
        match self.kind {
            DiagramKind::Sequence => {
                out.push_str("sequenceDiagram\n");
                for node in &self.nodes {
                    let _ = writeln!(
                        out,
                        "    participant {} as {}",
                        mermaid_id(&node.id),
                        mermaid_text(&node.label)
                    );
                }
                for edge in &self.edges {
                    let _ = writeln!(
                        out,
                        "    {}->>{}: {}",
                        mermaid_id(&edge.from),
                        mermaid_id(&edge.to),
                        mermaid_text(&edge.label)
                    );
                }
            }
            DiagramKind::Er => {
                out.push_str("erDiagram\n");
                for node in &self.nodes {
                    let mut lines = node.label.lines();
                    let _ = lines.next();
                    let _ = writeln!(out, "    {} {{", mermaid_id(&node.id));
                    for field in lines {
                        let _ = writeln!(out, "        field {}", mermaid_id(field.trim()));
                    }
                    out.push_str("    }\n");
                }
                for edge in &self.edges {
                    let _ = writeln!(
                        out,
                        "    {} }}o--|| {} : \"{}\"",
                        mermaid_id(&edge.from),
                        mermaid_id(&edge.to),
                        mermaid_text(&edge.label)
                    );
                }
            }
            DiagramKind::Class => {
                out.push_str("classDiagram\n");
                for node in &self.nodes {
                    let _ = writeln!(
                        out,
                        "    class {}[\"{}\"]",
                        mermaid_id(&node.id),
                        mermaid_text(&node.label)
                    );
                }
                for edge in &self.edges {
                    let _ = writeln!(
                        out,
                        "    {} <|-- {}",
                        mermaid_id(&edge.to),
                        mermaid_id(&edge.from)
                    );
                }
            }
            DiagramKind::Flow => {
                out.push_str("flowchart TD\n");
                for node in &self.nodes {
                    let _ = writeln!(
                        out,
                        "    {}[\"{}\"]",
                        mermaid_id(&node.id),
                        mermaid_text(&node.label)
                    );
                }
                for edge in &self.edges {
                    if edge.label.is_empty() {
                        let _ = writeln!(
                            out,
                            "    {} --> {}",
                            mermaid_id(&edge.from),
                            mermaid_id(&edge.to)
                        );
                    } else {
                        let _ = writeln!(
                            out,
                            "    {} -->|{}| {}",
                            mermaid_id(&edge.from),
                            mermaid_text(&edge.label),
                            mermaid_id(&edge.to)
                        );
                    }
                }
            }
        }
        out
    }
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
            branch: Some("main".into()),
            tree_oid: None,
            diagrams: Vec::new(),
            auto_approvable: false,
            swarm_size: 1,
            schema_version: SchemaVersion::V1,
            sandbox_run_id: None,
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
            lineage: Lineage::Persisting,
            reaction: Some(FindingReaction::Dismiss),
            hypothesis: Some(Hypothesis::NPlusOneQuery),
            kb_ref: None,
        };
        let json = serde_json::to_value(&finding).unwrap();
        assert_eq!(json["severity"], "P1");
        assert_eq!(serde_json::from_value::<Finding>(json).unwrap(), finding);
    }

    fn diagram(kind: DiagramKind) -> Diagram {
        let node = |id: &str, label: &str| DiagramNode {
            id: id.into(),
            label: label.into(),
        };
        Diagram {
            kind,
            title: "t".into(),
            nodes: vec![node("a.rs", "a.rs"), node("b", "Users\nid\nemail")],
            edges: vec![DiagramEdge {
                from: "a.rs".into(),
                to: "b".into(),
                label: "total \"x\"".into(),
            }],
        }
    }

    #[test]
    fn each_diagram_kind_exports_to_its_mermaid_form() {
        let sequence = diagram(DiagramKind::Sequence).to_mermaid();
        assert_eq!(
            sequence,
            "sequenceDiagram\n    participant a_rs as a.rs\n    participant b as Users id email\n    a_rs->>b: total #quot;x#quot;\n"
        );
        let er = diagram(DiagramKind::Er).to_mermaid();
        assert!(
            er.starts_with("erDiagram\n")
                && er.contains("    b {\n        field id\n        field email\n    }\n"),
            "{er}"
        );
        assert!(diagram(DiagramKind::Class)
            .to_mermaid()
            .contains("    b <|-- a_rs\n"));
        let flow = diagram(DiagramKind::Flow).to_mermaid();
        assert!(
            flow.starts_with("flowchart TD\n    a_rs[\"a.rs\"]\n"),
            "{flow}"
        );
        assert!(flow.contains("a_rs -->|total #quot;x#quot;| b"));
        let json = serde_json::to_value(diagram(DiagramKind::Er)).unwrap();
        assert_eq!(json["kind"], "er");
    }
}
