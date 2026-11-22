//! Pure checks on what agents hand a spec run: the spec, the plan, the report and their links.

use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;
use serde_json::Value;

use super::model::{CheckRecord, Delivery, GateCheck, Route, RunOutcome};
use super::{link_host, ARTIFACT_HOSTS};
use crate::ProjectAction;

pub const MAX_SPEC_BYTES: usize = 56 * 1024;
pub const MAX_RESEARCH_BYTES: usize = 60 * 1024;
pub const MAX_PLAN_BYTES: usize = 60 * 1024;
pub const MAX_REPORT_BYTES: usize = 64 * 1024;
pub const MAX_CHECK_LOG_BYTES: usize = 16 * 1024;
pub const MAX_COMMAND_BYTES: usize = 4 * 1024;
pub const MAX_ARTIFACT_URL_BYTES: usize = 2048;
pub const PLAN_SCHEMA: &str = "forge.spec_plan/v1";
pub const REPORT_SCHEMA: &str = "forge.spec_run/v1";
pub const GATE: &str = "gate";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Criterion {
    pub id: String,
    pub text: String,
    /// The first backticked command of its automatic validation.
    pub automatic: Option<String>,
    pub manual: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpecDoc {
    pub title: String,
    pub route: Route,
    pub criteria: Vec<Criterion>,
}

impl SpecDoc {
    #[must_use]
    pub fn has(&self, id: &str) -> bool {
        self.criteria.iter().any(|c| c.id == id)
    }
}

fn criterion_id(line: &str) -> Option<(String, &str)> {
    let line = line.trim_start();
    let line = line
        .strip_prefix("- ")
        .or_else(|| line.strip_prefix("* "))
        .unwrap_or(line);
    let line = line.trim_start_matches(['*', '|', ' ']);
    let rest = line.strip_prefix("AC-")?;
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 {
        return None;
    }
    let after = rest[digits..].trim_start_matches('*');
    let text = after
        .strip_prefix(':')
        .or_else(|| after.strip_prefix('.'))
        .or_else(|| after.strip_prefix(" —"))
        .or_else(|| after.strip_prefix(" -"))?;
    Some((format!("AC-{}", &rest[..digits]), text.trim()))
}

fn first_backticked(line: &str) -> Option<String> {
    let (_, rest) = line.split_once('`')?;
    let (command, _) = rest.split_once('`')?;
    let command = command.trim();
    (!command.is_empty()).then(|| command.to_owned())
}

fn route_of(line: &str) -> Option<Route> {
    let lower = line.to_lowercase();
    let (_, rest) = lower
        .split_once("route:")
        .or_else(|| lower.split_once("ruta:"))?;
    let word = rest.trim_start().split([' ', '·', '|', ',']).next()?;
    Some(match word {
        "short" | "corta" => Route::Short,
        "full" | "completa" => Route::Full,
        _ => Route::Standard,
    })
}

/// # Errors
/// Every reason the spec is refused: no criterion, a criterion without a validation, a repeated id.
pub fn parse_spec(text: &str) -> Result<SpecDoc, Vec<String>> {
    let mut errors = Vec::new();
    if text.len() > MAX_SPEC_BYTES {
        return Err(vec![format!("a spec is capped at {MAX_SPEC_BYTES} bytes")]);
    }
    let title = text
        .lines()
        .find_map(|l| l.strip_prefix("# "))
        .unwrap_or("")
        .trim()
        .to_owned();
    let route = text.lines().find_map(route_of).unwrap_or_default();
    let mut criteria: Vec<Criterion> = Vec::new();
    let mut open = false;
    for line in text.lines() {
        if let Some((id, rest)) = criterion_id(line) {
            criteria.push(Criterion {
                id,
                text: rest.to_owned(),
                automatic: None,
                manual: false,
            });
            open = true;
            continue;
        }
        if line.starts_with("# ") || line.starts_with("## ") {
            open = false;
        }
        let Some(current) = criteria.last_mut().filter(|_| open) else {
            continue;
        };
        let lower = line.to_lowercase();
        if !(lower.contains("validation") || lower.contains("validación")) {
            continue;
        }
        if lower.contains("automatic") || lower.contains("automática") {
            if current.automatic.is_none() {
                current.automatic = first_backticked(line);
            }
        } else if lower.contains("manual") {
            current.manual = true;
        }
    }
    if criteria.is_empty() {
        errors.push("the spec has no acceptance criterion with an id (AC-1, AC-2, …)".to_owned());
    }
    let mut seen = BTreeSet::new();
    for criterion in &criteria {
        if !seen.insert(criterion.id.as_str()) {
            errors.push(format!("{} appears twice", criterion.id));
        }
        if criterion.automatic.is_none() && !criterion.manual {
            errors.push(format!(
                "{} has no validation: add `Automatic validation: `<command>` → <expected>` or a manual one",
                criterion.id
            ));
        }
    }
    if errors.is_empty() {
        Ok(SpecDoc {
            title,
            route,
            criteria,
        })
    } else {
        Err(errors)
    }
}

pub const EARLY_EXITS: [&str; 4] = [
    "needs_details",
    "intended_behavior",
    "not_actionable",
    "too_big",
];

/// # Errors
/// An exit whose first line names no known exit, or that carries no evidence.
pub fn parse_exit(text: &str) -> Result<&'static str, String> {
    let first = text.lines().next().unwrap_or("").to_lowercase();
    let kind = EARLY_EXITS
        .iter()
        .find(|kind| first.contains(*kind))
        .ok_or_else(|| format!("the first line names none of {}", EARLY_EXITS.join(", ")))?;
    if text.lines().skip(1).all(|l| l.trim().is_empty()) {
        return Err("an early exit carries its evidence after the first line".to_owned());
    }
    Ok(kind)
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct CheckSpec {
    pub command: String,
    #[serde(default)]
    pub expect: Option<String>,
    #[serde(default)]
    pub timeout_secs: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct PlanStep {
    pub id: String,
    pub criteria: Vec<String>,
    #[serde(default)]
    pub files: Vec<String>,
    #[serde(default)]
    pub check: Option<CheckSpec>,
    #[serde(default)]
    pub manual: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct PlanTask {
    pub id: String,
    pub title: String,
    pub criteria: Vec<String>,
    #[serde(default)]
    pub after: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct PlanDoc {
    pub steps: Vec<PlanStep>,
    #[serde(default)]
    pub gate: Option<CheckSpec>,
    #[serde(default)]
    pub tasks: Vec<PlanTask>,
}

/// One command Forge runs after a coder report.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlannedCheck {
    pub criterion: String,
    pub command: String,
    pub timeout_secs: Option<u64>,
}

impl PlanDoc {
    /// The automatic checks of `criteria` (all when `None`), one per criterion a step serves.
    #[must_use]
    pub fn checks(&self, criteria: Option<&[String]>) -> Vec<PlannedCheck> {
        let mut out: Vec<PlannedCheck> = Vec::new();
        for step in &self.steps {
            let Some(check) = &step.check else { continue };
            for id in &step.criteria {
                if criteria.is_some_and(|only| !only.contains(id)) {
                    continue;
                }
                out.push(PlannedCheck {
                    criterion: id.clone(),
                    command: check.command.clone(),
                    timeout_secs: check.timeout_secs,
                });
            }
        }
        out
    }
}

pub(super) fn find_block(text: &str, schema: &str) -> Option<Value> {
    text.match_indices('{').find_map(|(at, _)| {
        let mut stream = serde_json::Deserializer::from_str(&text[at..]).into_iter::<Value>();
        match stream.next() {
            Some(Ok(value)) if value.get("schema").and_then(Value::as_str) == Some(schema) => {
                Some(value)
            }
            _ => None,
        }
    })
}

/// The project's gate action wins over the plan's proposal; with neither there is no gate.
///
/// # Errors
/// Neither a gate action nor a proposal.
pub fn resolve_gate(
    actions: &[ProjectAction],
    proposed: Option<&CheckSpec>,
) -> Result<GateCheck, String> {
    if let Some(action) = actions.iter().find(|a| a.gate) {
        return Ok(GateCheck {
            command: action.command.clone(),
            action_id: Some(action.id),
        });
    }
    match proposed {
        Some(check) if !check.command.trim().is_empty() => Ok(GateCheck {
            command: check.command.trim().to_owned(),
            action_id: None,
        }),
        _ => Err(
            "the plan proposes no repository gate and the project marks no action as its gate"
                .to_owned(),
        ),
    }
}

/// The `forge.spec_plan/v1` block of a plan, crossed against the approved spec.
///
/// # Errors
/// Every reason the plan is refused.
pub fn parse_plan(
    text: &str,
    spec: &SpecDoc,
    actions: &[ProjectAction],
) -> Result<(PlanDoc, GateCheck), Vec<String>> {
    if text.len() > MAX_PLAN_BYTES {
        return Err(vec![format!("a plan is capped at {MAX_PLAN_BYTES} bytes")]);
    }
    let block = find_block(text, PLAN_SCHEMA).ok_or_else(|| {
        vec![format!(
            "the plan has no JSON block with \"schema\": \"{PLAN_SCHEMA}\""
        )]
    })?;
    let plan: PlanDoc =
        serde_json::from_value(block).map_err(|e| vec![format!("{PLAN_SCHEMA}: {e}")])?;
    let mut errors = Vec::new();
    if plan.steps.is_empty() {
        errors.push("the plan has no step".to_owned());
    }
    let mut served = BTreeSet::new();
    let mut ids = BTreeSet::new();
    for step in &plan.steps {
        if !ids.insert(step.id.as_str()) {
            errors.push(format!("step {} appears twice", step.id));
        }
        if step.criteria.is_empty() {
            errors.push(format!("step {} serves no criterion", step.id));
        }
        for id in &step.criteria {
            if spec.has(id) {
                served.insert(id.as_str());
            } else {
                errors.push(format!(
                    "step {} names {id}, which the spec does not have",
                    step.id
                ));
            }
        }
        match &step.check {
            Some(check) if check.command.trim().is_empty() => {
                errors.push(format!("step {} has an empty check command", step.id));
            }
            Some(check) if check.command.len() > MAX_COMMAND_BYTES => {
                errors.push(format!(
                    "step {}'s check is over {MAX_COMMAND_BYTES} bytes",
                    step.id
                ));
            }
            None if step.manual.as_deref().is_none_or(|m| m.trim().is_empty()) => {
                errors.push(format!(
                    "step {} has neither an automatic nor a manual check",
                    step.id
                ));
            }
            _ => {}
        }
    }
    for criterion in &spec.criteria {
        if !served.contains(criterion.id.as_str()) {
            errors.push(format!("{} has no step", criterion.id));
        }
    }
    if spec.route == Route::Full {
        errors.extend(task_errors(&plan.tasks, spec));
    }
    let gate = match resolve_gate(actions, plan.gate.as_ref()) {
        Ok(gate) => Some(gate),
        Err(error) => {
            errors.push(error);
            None
        }
    };
    match (errors.is_empty(), gate) {
        (true, Some(gate)) => Ok((plan, gate)),
        _ => Err(errors),
    }
}

fn task_errors(tasks: &[PlanTask], spec: &SpecDoc) -> Vec<String> {
    let mut errors = Vec::new();
    if tasks.is_empty() {
        errors.push("the full route splits the plan into tasks; the plan has none".to_owned());
    }
    let mut earlier = BTreeSet::new();
    let mut covered = BTreeSet::new();
    for task in tasks {
        for after in &task.after {
            if !earlier.contains(after.as_str()) {
                errors.push(format!(
                    "task {} waits for {after}, which is not an earlier task",
                    task.id
                ));
            }
        }
        for id in &task.criteria {
            if !spec.has(id) {
                errors.push(format!(
                    "task {} names {id}, which the spec does not have",
                    task.id
                ));
            }
            covered.insert(id.as_str());
        }
        if !earlier.insert(task.id.as_str()) {
            errors.push(format!("task {} appears twice", task.id));
        }
    }
    for criterion in &spec.criteria {
        if !tasks.is_empty() && !covered.contains(criterion.id.as_str()) {
            errors.push(format!("{} belongs to no task", criterion.id));
        }
    }
    errors
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CriterionState {
    Met,
    NotMet,
    NotVerified,
    NeedsDecision,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct Evidence {
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub exit_code: Option<i32>,
    #[serde(default)]
    pub excerpt: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct ReportCriterion {
    pub id: String,
    pub state: CriterionState,
    #[serde(default)]
    pub evidence: Option<Evidence>,
    /// `path` or `path:line` in the reviewed diff.
    #[serde(default)]
    pub paths: Vec<String>,
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct ReportBlock {
    pub criteria: Vec<ReportCriterion>,
    #[serde(default)]
    pub gate: Option<ReportCriterion>,
    /// `path:line` lines of "What was done".
    #[serde(default)]
    pub changed: Vec<String>,
    /// Findings deferred because they predate the change.
    #[serde(default)]
    pub deferred: Vec<String>,
}

/// What a report is checked against.
#[derive(Clone, Debug)]
pub struct ReportContext<'a> {
    pub spec: &'a SpecDoc,
    /// Files of the reviewed diff, without what was dirty at launch.
    pub diff_paths: &'a BTreeSet<String>,
    /// This round's check records.
    pub checks: &'a [CheckRecord],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidReport {
    pub outcome: RunOutcome,
    pub block: ReportBlock,
}

fn strip_line(reference: &str) -> &str {
    match reference.rsplit_once(':') {
        Some((path, line))
            if !line.is_empty() && line.bytes().all(|b| b.is_ascii_digit() || b == b'-') =>
        {
            path
        }
        _ => reference,
    }
}

fn evidence_complete(criterion: &ReportCriterion) -> bool {
    let ran = criterion.evidence.as_ref().is_some_and(|e| {
        e.command.as_deref().is_some_and(|c| !c.trim().is_empty())
            && e.exit_code.is_some()
            && e.excerpt.as_deref().is_some_and(|x| !x.trim().is_empty())
    });
    ran || criterion.paths.iter().any(|p| p.contains(':'))
}

/// # Errors
/// Every reason the report is refused: a `met` without evidence or with any check of this round
/// that did not pass, a path outside the reviewed diff, an id the approved spec does not have, a
/// criterion left out.
pub fn validate_report(text: &str, ctx: &ReportContext) -> Result<ValidReport, Vec<String>> {
    if text.len() > MAX_REPORT_BYTES {
        return Err(vec![format!(
            "a report is capped at {MAX_REPORT_BYTES} bytes"
        )]);
    }
    let value = find_block(text, REPORT_SCHEMA).ok_or_else(|| {
        vec![format!(
            "the report has no JSON block with \"schema\": \"{REPORT_SCHEMA}\""
        )]
    })?;
    let block: ReportBlock =
        serde_json::from_value(value).map_err(|e| vec![format!("{REPORT_SCHEMA}: {e}")])?;
    let passed: BTreeMap<&str, bool> = ctx.checks.iter().fold(BTreeMap::new(), |mut acc, c| {
        let entry = acc.entry(c.criterion.as_str()).or_insert(true);
        *entry &= c.passed();
        acc
    });
    let mut errors = Vec::new();
    let mut seen = BTreeSet::new();
    let check_path = |reference: &str, errors: &mut Vec<String>| {
        let path = strip_line(reference);
        if !ctx.diff_paths.contains(path) {
            errors.push(format!("{path} is not in the reviewed diff"));
        }
    };
    for criterion in &block.criteria {
        if !ctx.spec.has(&criterion.id) {
            errors.push(format!("{} is not in the approved spec", criterion.id));
        }
        if !seen.insert(criterion.id.as_str()) {
            errors.push(format!("{} is reported twice", criterion.id));
        }
        for path in &criterion.paths {
            check_path(path, &mut errors);
        }
        if criterion.state == CriterionState::Met {
            if !evidence_complete(criterion) {
                errors.push(format!(
                    "{} is met without evidence: give the command, its exit code and an excerpt",
                    criterion.id
                ));
            }
            if !passed.get(criterion.id.as_str()).copied().unwrap_or(false) {
                errors.push(format!(
                    "{} is met, but not every check of this round passed for it",
                    criterion.id
                ));
            }
        }
    }
    for path in &block.changed {
        check_path(path, &mut errors);
    }
    for criterion in &ctx.spec.criteria {
        if !seen.contains(criterion.id.as_str()) {
            errors.push(format!("{} has no state in the report", criterion.id));
        }
    }
    if let Some(gate) = &block.gate {
        if gate.state == CriterionState::Met && !passed.get(GATE).copied().unwrap_or(false) {
            errors.push("the gate is met, but the gate check did not pass this round".to_owned());
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    Ok(ValidReport {
        outcome: outcome(&block),
        block,
    })
}

/// The `forge.spec_run/v1` block of a saved report, for display; validation happened when it was
/// saved.
#[must_use]
pub fn report_block(text: &str) -> Option<ReportBlock> {
    serde_json::from_value(find_block(text, REPORT_SCHEMA)?).ok()
}

/// The overall state follows from the criteria, never from what the agent wrote.
#[must_use]
pub fn outcome(block: &ReportBlock) -> RunOutcome {
    let states: Vec<CriterionState> = block.criteria.iter().map(|c| c.state).collect();
    let gate = block.gate.as_ref().map(|g| g.state);
    if states.contains(&CriterionState::NeedsDecision)
        || gate == Some(CriterionState::NeedsDecision)
    {
        RunOutcome::NeedsDecision
    } else if states.contains(&CriterionState::NotMet) || gate == Some(CriterionState::NotMet) {
        RunOutcome::Blocked
    } else if states.contains(&CriterionState::NotVerified)
        || gate != Some(CriterionState::Met)
        || !block.deferred.is_empty()
    {
        RunOutcome::DoneWithConcerns
    } else {
        RunOutcome::Done
    }
}

/// Where the coder's commits disagree with the delivery chosen at G2.
#[must_use]
pub fn delivery_deviation(
    delivery: Delivery,
    base_commit: &str,
    reported_head: Option<&str>,
    worktree_branch: Option<&str>,
    reported_branch: Option<&str>,
) -> Option<String> {
    match delivery {
        Delivery::NoCommit | Delivery::DraftPr => {
            let head = reported_head?;
            (head != base_commit).then(|| {
                format!(
                    "HEAD moved from {} to {} although the delivery is no commit",
                    short(base_commit),
                    short(head)
                )
            })
        }
        Delivery::Commit => {
            let expected = worktree_branch?;
            match reported_branch {
                Some(branch) if branch == expected => None,
                Some(branch) => Some(format!(
                    "the coder committed on {branch}, not on its worktree branch {expected}"
                )),
                None => Some(format!(
                    "the coder's HEAD is detached, not on its worktree branch {expected}"
                )),
            }
        }
    }
}

fn short(oid: &str) -> &str {
    oid.get(..10).unwrap_or(oid)
}

/// # Errors
/// Anything but an https link to `claude.ai` or `claude.site`.
pub fn validate_artifact_url(url: &str) -> Result<(), String> {
    let url = url.trim();
    if url.len() > MAX_ARTIFACT_URL_BYTES || !url.starts_with("https://") {
        return Err("artifact_url must be an https link".to_owned());
    }
    let host = link_host(url).ok_or("artifact_url has no host")?;
    let known = ARTIFACT_HOSTS
        .iter()
        .any(|known| host == *known || host.ends_with(&format!(".{known}")));
    if known {
        Ok(())
    } else {
        Err(format!(
            "artifact_url must be on claude.ai or claude.site, not {host}"
        ))
    }
}

/// The report's "Criteria" section, heading included, for a pull request body.
#[must_use]
pub fn criteria_section(report: &str) -> Option<&str> {
    let start = report
        .match_indices("\n## ")
        .map(|(at, _)| at + 1)
        .find(|&at| {
            let heading = report[at + 3..]
                .lines()
                .next()
                .unwrap_or("")
                .trim()
                .to_lowercase();
            heading == "criteria" || heading == "criterios"
        })?;
    let end = report[start + 3..]
        .find("\n## ")
        .map_or(report.len(), |at| start + 3 + at);
    Some(report[start..end].trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::action::{ActionKind, ActionReady, ActionSource};
    use crate::{ProjectActionId, ProjectId, Timestamp};

    const SPEC: &str = "# Loader cache\nRoute: standard · Source: idea\n\n## Problem\nSlow.\n\n\
## Acceptance criteria\n- AC-1: WHEN the loader runs twice THE SYSTEM SHALL read the disk once\n  \
- Automatic validation: `cargo test -p loader cache_hit -- --exact` → exit 0\n\
- AC-2: WHEN the file changes THE SYSTEM SHALL reload it\n  - Manual validation: edit the file and watch the log\n\n\
## Out of scope\nNo validation of anything here.\n";

    fn spec() -> SpecDoc {
        parse_spec(SPEC).unwrap()
    }

    fn action(gate: bool) -> ProjectAction {
        ProjectAction {
            id: ProjectActionId::new(),
            project_id: ProjectId::new(),
            label: "check".into(),
            command: "scripts/dev check".into(),
            kind: ActionKind::Test,
            cwd: None,
            single_instance: true,
            pinned: false,
            gate,
            ready: ActionReady::default(),
            port: None,
            depends_on: Vec::new(),
            source: ActionSource::Manual,
            position: 0,
            created_at: Timestamp::now(),
        }
    }

    #[test]
    fn a_spec_needs_unique_criteria_that_each_carry_a_validation() {
        let doc = spec();
        assert_eq!(doc.title, "Loader cache");
        assert_eq!(doc.route, Route::Standard);
        assert_eq!(doc.criteria.len(), 2);
        assert_eq!(
            doc.criteria[0].automatic.as_deref(),
            Some("cargo test -p loader cache_hit -- --exact")
        );
        assert!(doc.criteria[1].manual);
        assert_eq!(
            parse_spec("# T\nRuta: corta\n- AC-1: x\n  - Validación automática: `true`")
                .unwrap()
                .route,
            Route::Short
        );

        let none = parse_spec("# T\nJust prose.").unwrap_err();
        assert!(none[0].contains("no acceptance criterion"), "{none:?}");
        let bare = parse_spec("# T\n- AC-1: x\n\n## Out of scope\nvalidation: manual").unwrap_err();
        assert!(bare[0].contains("AC-1 has no validation"), "{bare:?}");
        let twice = parse_spec(&format!(
            "{SPEC}\n- AC-1: again\n  - Manual validation: look"
        ))
        .unwrap_err();
        assert!(twice.iter().any(|e| e == "AC-1 appears twice"), "{twice:?}");
    }

    #[test]
    fn an_early_exit_names_its_kind_and_carries_evidence() {
        assert_eq!(
            parse_exit("intended_behavior\nThe docs say so: docs/x.md:3"),
            Ok("intended_behavior")
        );
        assert!(parse_exit("intended_behavior").is_err());
        assert!(parse_exit("maybe later\nreasons").is_err());
    }

    fn plan(gate: &str, steps: &str) -> String {
        format!("# Plan\n\n```json\n{{\"schema\": \"forge.spec_plan/v1\", {gate} \"steps\": [{steps}]}}\n```\n")
    }

    const STEPS: &str = "{\"id\": \"S1\", \"criteria\": [\"AC-1\"], \"check\": {\"command\": \"cargo test -p loader cache_hit -- --exact\"}}, \
{\"id\": \"S2\", \"criteria\": [\"AC-2\"], \"manual\": \"edit the file\"}";

    #[test]
    fn a_plan_crosses_every_criterion_with_a_step_and_a_check() {
        let gate = "\"gate\": {\"command\": \"scripts/dev check\"},";
        let (doc, resolved) = parse_plan(&plan(gate, STEPS), &spec(), &[]).unwrap();
        assert_eq!(resolved.command, "scripts/dev check");
        assert_eq!(resolved.action_id, None);
        assert_eq!(doc.checks(None).len(), 1);

        let missing = parse_plan(
            &plan(gate, "{\"id\": \"S1\", \"criteria\": [\"AC-1\", \"AC-9\"], \"check\": {\"command\": \"x\"}}"),
            &spec(),
            &[],
        )
        .unwrap_err();
        assert!(
            missing
                .iter()
                .any(|e| e.contains("AC-9, which the spec does not have")),
            "{missing:?}"
        );
        assert!(
            missing.iter().any(|e| e == "AC-2 has no step"),
            "{missing:?}"
        );
        let unchecked = parse_plan(
            &plan(gate, "{\"id\": \"S1\", \"criteria\": [\"AC-1\", \"AC-2\"]}"),
            &spec(),
            &[],
        )
        .unwrap_err();
        assert!(
            unchecked[0].contains("neither an automatic nor a manual check"),
            "{unchecked:?}"
        );
        assert!(parse_plan("no block", &spec(), &[]).is_err());
    }

    #[test]
    fn the_gate_is_the_marked_action_or_else_the_plans_proposal() {
        let marked = action(true);
        let resolved = resolve_gate(&[action(false), marked.clone()], None).unwrap();
        assert_eq!(resolved.action_id, Some(marked.id));
        let proposal = CheckSpec {
            command: "make test".into(),
            expect: None,
            timeout_secs: None,
        };
        assert_eq!(
            resolve_gate(std::slice::from_ref(&marked), Some(&proposal))
                .unwrap()
                .command,
            "scripts/dev check"
        );
        assert_eq!(
            resolve_gate(&[action(false)], Some(&proposal))
                .unwrap()
                .command,
            "make test"
        );
        assert!(resolve_gate(&[action(false)], None).is_err());

        let without = parse_plan(&plan("", STEPS), &spec(), &[]).unwrap_err();
        assert!(
            without.iter().any(|e| e.contains("no repository gate")),
            "{without:?}"
        );
        assert!(parse_plan(&plan("", STEPS), &spec(), &[marked]).is_ok());
    }

    #[test]
    fn the_full_route_splits_into_ordered_tasks() {
        let full = spec_with_route("completa");
        let gate = "\"gate\": {\"command\": \"g\"},";
        let tasks =
            "\"tasks\": [{\"id\": \"T1\", \"title\": \"cache\", \"criteria\": [\"AC-1\"]}, \
{\"id\": \"T2\", \"title\": \"reload\", \"criteria\": [\"AC-2\"], \"after\": [\"T1\"]}],";
        assert_eq!(
            parse_plan(&plan(&format!("{gate} {tasks}"), STEPS), &full, &[])
                .unwrap()
                .0
                .tasks
                .len(),
            2
        );
        let backwards = "\"tasks\": [{\"id\": \"T1\", \"title\": \"a\", \"criteria\": [\"AC-1\", \"AC-2\"], \"after\": [\"T2\"]}],";
        let errors =
            parse_plan(&plan(&format!("{gate} {backwards}"), STEPS), &full, &[]).unwrap_err();
        assert!(errors[0].contains("not an earlier task"), "{errors:?}");
        assert!(parse_plan(&plan(gate, STEPS), &full, &[]).is_err());
    }

    fn spec_with_route(route: &str) -> SpecDoc {
        parse_spec(&SPEC.replace("Route: standard", &format!("Ruta: {route}"))).unwrap()
    }

    fn record(criterion: &str, exit: Option<i32>) -> CheckRecord {
        CheckRecord {
            criterion: criterion.into(),
            task: 0,
            round: 1,
            command: "cmd".into(),
            exit_code: exit,
            timed_out: false,
            not_verified: None,
            not_sandboxed: None,
            duration_ms: 3,
            log: format!("checks/{criterion}-r1.log"),
        }
    }

    fn report(ac1: &str) -> String {
        format!(
            "# Loader cache · done\n\n## Criteria\n| ID | … |\n\n```json\n{{\"schema\": \"forge.spec_run/v1\", \
\"criteria\": [{ac1}, {{\"id\": \"AC-2\", \"state\": \"not_verified\", \"reason\": \"manual\"}}], \
\"gate\": {{\"id\": \"gate\", \"state\": \"met\", \"evidence\": {{\"command\": \"g\", \"exit_code\": 0, \"excerpt\": \"ok\"}}}}, \
\"changed\": [\"src/loader.rs:12\"]}}\n```\n"
        )
    }

    const MET: &str = "{\"id\": \"AC-1\", \"state\": \"met\", \"paths\": [\"src/loader.rs:12\"], \
\"evidence\": {\"command\": \"cargo test -p loader cache_hit -- --exact\", \"exit_code\": 0, \"excerpt\": \"1 passed\"}}";

    #[test]
    fn a_report_is_refused_for_bare_claims_foreign_paths_and_unknown_ids() {
        let spec = spec();
        let diff: BTreeSet<String> = ["src/loader.rs".to_owned()].into();
        let checks = [record("AC-1", Some(0)), record(GATE, Some(0))];
        let ctx = ReportContext {
            spec: &spec,
            diff_paths: &diff,
            checks: &checks,
        };

        let valid = validate_report(&report(MET), &ctx).unwrap();
        assert_eq!(valid.outcome, RunOutcome::DoneWithConcerns);
        assert_eq!(report_block(&report(MET)), Some(valid.block));
        assert_eq!(report_block("no block"), None);

        let bare =
            validate_report(&report("{\"id\": \"AC-1\", \"state\": \"met\"}"), &ctx).unwrap_err();
        assert!(bare[0].contains("AC-1 is met without evidence"), "{bare:?}");
        let foreign = validate_report(
            &report(&MET.replace("src/loader.rs:12", "src/other.rs:3")),
            &ctx,
        )
        .unwrap_err();
        assert!(
            foreign
                .iter()
                .any(|e| e == "src/other.rs is not in the reviewed diff"),
            "{foreign:?}"
        );
        let unknown = validate_report(&report(&MET.replace("AC-1", "AC-7")), &ctx).unwrap_err();
        assert!(
            unknown
                .iter()
                .any(|e| e == "AC-7 is not in the approved spec"),
            "{unknown:?}"
        );
        assert!(
            unknown
                .iter()
                .any(|e| e == "AC-1 has no state in the report"),
            "{unknown:?}"
        );
    }

    #[test]
    fn met_needs_every_check_of_this_round_to_pass() {
        let spec = spec();
        let diff: BTreeSet<String> = ["src/loader.rs".to_owned()].into();
        for checks in [
            vec![record("AC-1", Some(1)), record(GATE, Some(0))],
            vec![record(GATE, Some(0))],
            vec![
                record("AC-1", Some(0)),
                record("AC-1", Some(1)),
                record(GATE, Some(0)),
            ],
        ] {
            let ctx = ReportContext {
                spec: &spec,
                diff_paths: &diff,
                checks: &checks,
            };
            let errors = validate_report(&report(MET), &ctx).unwrap_err();
            assert_eq!(
                errors,
                ["AC-1 is met, but not every check of this round passed for it"]
            );
        }
        let failed_gate = [record("AC-1", Some(0)), record(GATE, None)];
        let ctx = ReportContext {
            spec: &spec,
            diff_paths: &diff,
            checks: &failed_gate,
        };
        assert!(validate_report(&report(MET), &ctx).unwrap_err()[0].contains("gate is met"));
    }

    #[test]
    fn the_outcome_follows_from_the_criteria() {
        let block = |states: &[CriterionState], gate: CriterionState| ReportBlock {
            criteria: states
                .iter()
                .enumerate()
                .map(|(i, s)| ReportCriterion {
                    id: format!("AC-{i}"),
                    state: *s,
                    evidence: None,
                    paths: vec![],
                    reason: None,
                })
                .collect(),
            gate: Some(ReportCriterion {
                id: GATE.into(),
                state: gate,
                evidence: None,
                paths: vec![],
                reason: None,
            }),
            changed: vec![],
            deferred: vec![],
        };
        use CriterionState::*;
        assert_eq!(outcome(&block(&[Met, Met], Met)), RunOutcome::Done);
        assert_eq!(
            outcome(&block(&[Met, NotVerified], Met)),
            RunOutcome::DoneWithConcerns
        );
        assert_eq!(
            outcome(&block(&[Met], NotVerified)),
            RunOutcome::DoneWithConcerns
        );
        assert_eq!(outcome(&block(&[NotMet, Met], Met)), RunOutcome::Blocked);
        assert_eq!(
            outcome(&block(&[NotMet, NeedsDecision], Met)),
            RunOutcome::NeedsDecision
        );
    }

    #[test]
    fn a_commit_that_contradicts_the_delivery_is_a_deviation() {
        let base = "aaaaaaaaaaaaaaaaaaaa";
        assert_eq!(
            delivery_deviation(Delivery::NoCommit, base, Some(base), None, None),
            None
        );
        assert_eq!(
            delivery_deviation(
                Delivery::NoCommit,
                base,
                Some("bbbbbbbbbbbbbbb"),
                None,
                None
            )
            .as_deref(),
            Some("HEAD moved from aaaaaaaaaa to bbbbbbbbbb although the delivery is no commit")
        );
        assert!(delivery_deviation(Delivery::DraftPr, base, Some("b"), None, None).is_some());
        assert_eq!(
            delivery_deviation(
                Delivery::Commit,
                base,
                Some("b"),
                Some("spec/x"),
                Some("spec/x")
            ),
            None
        );
        assert_eq!(
            delivery_deviation(
                Delivery::Commit,
                base,
                Some("b"),
                Some("spec/x"),
                Some("main")
            )
            .as_deref(),
            Some("the coder committed on main, not on its worktree branch spec/x")
        );
    }

    #[test]
    fn an_artifact_url_is_https_on_claude_hosts_only() {
        assert!(validate_artifact_url("https://claude.ai/artifact/Dej5Jp8YBdX1HebY9njiev").is_ok());
        assert!(validate_artifact_url("https://abc.claude.site/x").is_ok());
        assert!(validate_artifact_url("https://claude.ai.evil.example/x").is_err());
        assert!(validate_artifact_url("http://claude.ai/x").is_err());
        assert!(validate_artifact_url("https://example.com/x").is_err());
    }

    #[test]
    fn the_criteria_section_is_cut_from_the_report() {
        let text = "# x · done\n\n## What was done\n- a\n\n## Criteria\n| ID | State |\n| AC-1 | met |\n\n## Tests added\nNone\n";
        assert_eq!(
            criteria_section(text),
            Some("## Criteria\n| ID | State |\n| AC-1 | met |")
        );
        assert_eq!(
            criteria_section("# x\n\n## Criterios\n| a |"),
            Some("## Criterios\n| a |")
        );
        assert_eq!(criteria_section("# x"), None);
    }
}
