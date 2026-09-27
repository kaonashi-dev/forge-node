//! Pure transitions, rails, attention, addresses, and clamps.
//!
//! The daemon is the only caller that persists a result. These functions do
//! not pick a next task.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use serde::Deserialize;

use crate::ids::{ContextId, RunId, SessionId, TaskId};
use crate::session::{ActivityEvidence, ActivityState, AgentActivity};
use crate::Timestamp;

use super::{
    Attempt, AttemptPhase, AttentionItem, AttentionKind, BoardEntry, IntegratePermission,
    LostReason, OrchestrationLimits, RailRefusal, ReportOutcome, Run, RunStatus, Task, TaskStatus,
    WorkMode, MAX_BOARD_KEYS, MAX_BOARD_KEY_BYTES, MAX_BOARD_VALUE_BYTES, MIN_ID_PREFIX,
};

/// What moved activity. `Silence` is a trigger so a test can show it is a no-op.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActivityTrigger {
    /// Provider hook. Only `Working`, `Waiting`, and `Idle` are applied.
    Hook(ActivityState),
    /// Bytes written to the PTY while the session was idle.
    Input,
    /// Time passed with no hook and no input.
    Silence,
    /// The process exited. Does not settle an attempt.
    ProcessExit,
}

/// Activity after `trigger`. Silence returns `current` unchanged.
#[must_use]
pub fn apply_activity(
    current: &AgentActivity,
    trigger: ActivityTrigger,
    now: Timestamp,
) -> AgentActivity {
    let next_state = match trigger {
        ActivityTrigger::Silence => return current.clone(),
        ActivityTrigger::ProcessExit => return current.clone(),
        ActivityTrigger::Input => {
            if current.state != ActivityState::Idle {
                return current.clone();
            }
            ActivityState::Working
        }
        ActivityTrigger::Hook(state) => match state {
            ActivityState::Working | ActivityState::Waiting | ActivityState::Idle => state,
            _ => return current.clone(),
        },
    };
    if next_state == current.state {
        return current.clone();
    }
    let evidence = match trigger {
        ActivityTrigger::Input => Some(ActivityEvidence::Input),
        ActivityTrigger::Hook(_) => Some(ActivityEvidence::Hook),
        ActivityTrigger::ProcessExit => Some(ActivityEvidence::Exit),
        ActivityTrigger::Silence => None,
    };
    AgentActivity {
        state: next_state,
        since: now,
        evidence,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointerDecision {
    /// Type one line, then a separate carriage return.
    Type,
    /// Remember it until the next Idle that has not already typed.
    Queue,
    /// Do not type. The message stays in the inbox.
    Suppress,
}

/// Whether a stored message may be hinted at on the PTY.
///
/// A watched inbox wins over Idle. Waiting, Starting, and Unknown never type.
#[must_use]
pub fn pointer_decision(
    state: ActivityState,
    inbox_watched: bool,
    typed_this_idle: bool,
) -> PointerDecision {
    if inbox_watched {
        return PointerDecision::Suppress;
    }
    match state {
        ActivityState::Idle if !typed_this_idle => PointerDecision::Type,
        ActivityState::Working => PointerDecision::Queue,
        ActivityState::Idle
        | ActivityState::Waiting
        | ActivityState::Starting
        | ActivityState::Unknown
        | ActivityState::Unrecognized => PointerDecision::Suppress,
    }
}

/// One line, no newline. The caller writes a carriage return afterwards.
#[must_use]
pub fn pointer_line(unread: usize) -> String {
    let n = unread.max(1);
    let noun = if n == 1 { "message" } else { "messages" };
    format!("Forge: {n} new {noun} for this task — run: forgectl inbox")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReportRefuse {
    /// The caller is not the attempt's session.
    Foreign,
    /// The attempt is not the live one, or it already settled.
    Superseded,
}

/// Only an explicit report of a live attempt, from its own session, settles it.
pub fn settle_report(
    phase: AttemptPhase,
    attempt_session: Option<SessionId>,
    caller: Option<SessionId>,
    newer_attempt_exists: bool,
) -> Result<(), ReportRefuse> {
    if newer_attempt_exists || !matches!(phase, AttemptPhase::Starting | AttemptPhase::Running) {
        return Err(ReportRefuse::Superseded);
    }
    if caller.is_none() || caller != attempt_session {
        return Err(ReportRefuse::Foreign);
    }
    Ok(())
}

/// Task status after a report the policy accepted. Does not look at a timer.
#[must_use]
pub fn task_status_after_report(
    outcome: ReportOutcome,
    attempts_used: u32,
    max_attempts: u32,
) -> TaskStatus {
    match outcome {
        ReportOutcome::Blocked => TaskStatus::Blocked,
        ReportOutcome::Done | ReportOutcome::Unrecognized => TaskStatus::Review,
        ReportOutcome::Failed if attempts_used >= max_attempts => TaskStatus::Failed,
        ReportOutcome::Failed => TaskStatus::Review,
    }
}

/// Where a task goes when its live attempt is lost and nobody reported.
#[must_use]
pub fn task_status_after_loss(attempts_used: u32, max_attempts: u32) -> TaskStatus {
    if attempts_used >= max_attempts {
        TaskStatus::Failed
    } else {
        TaskStatus::Ready
    }
}

pub fn allow_new_task(existing: usize, limits: &OrchestrationLimits) -> Result<(), RailRefusal> {
    if existing >= limits.max_tasks_per_run as usize {
        return Err(RailRefusal::new(
            "max_tasks",
            format!(
                "run already has {existing} tasks (limit {})",
                limits.max_tasks_per_run
            ),
        ));
    }
    Ok(())
}

pub fn allow_start(
    active_in_run: usize,
    attempts_used: u32,
    mode: WorkMode,
    workspace_has_writer: bool,
    limits: &OrchestrationLimits,
) -> Result<(), RailRefusal> {
    if active_in_run >= limits.max_active_attempts_per_run as usize {
        return Err(RailRefusal::new(
            "max_active_attempts",
            format!(
                "run already has {active_in_run} live attempts (limit {})",
                limits.max_active_attempts_per_run
            ),
        ));
    }
    if attempts_used >= limits.max_attempts_per_task {
        return Err(RailRefusal::new(
            "max_attempts",
            format!(
                "task already used {attempts_used} attempts (limit {})",
                limits.max_attempts_per_task
            ),
        ));
    }
    if mode == WorkMode::Write && workspace_has_writer {
        return Err(RailRefusal::new(
            "one_writer",
            "this workspace already has a live writing attempt",
        ));
    }
    Ok(())
}

pub fn allow_nested_run(
    parent_depth: u32,
    parent_allows_subruns: bool,
    limits: &OrchestrationLimits,
) -> Result<(), RailRefusal> {
    if !parent_allows_subruns {
        return Err(RailRefusal::new(
            "subruns",
            "the parent task does not allow a nested run",
        ));
    }
    let depth = parent_depth.saturating_add(1);
    if depth > limits.max_run_depth {
        return Err(RailRefusal::new(
            "max_run_depth",
            format!(
                "nested run would be depth {depth} (limit {})",
                limits.max_run_depth
            ),
        ));
    }
    Ok(())
}

/// Accept, reject, integrate, and cancel. Humans (no session) always pass.
pub fn allow_controller(
    caller: Option<SessionId>,
    controller: Option<SessionId>,
) -> Result<(), RailRefusal> {
    if caller.is_none() {
        return Ok(());
    }
    if caller == controller {
        return Ok(());
    }
    Err(RailRefusal::new(
        "controller_only",
        "only the run's controller or a human may decide",
    ))
}

/// An agent controller opening a PR or merging. Humans are not checked here.
pub fn allow_agent_integrate(
    open_pull_request: bool,
    permission: IntegratePermission,
) -> Result<(), RailRefusal> {
    match (open_pull_request, permission) {
        (false, IntegratePermission::None) => Err(RailRefusal::new(
            "agent_may_integrate",
            "an agent controller may not merge into the integration branch",
        )),
        (true, IntegratePermission::None | IntegratePermission::Merge) => Err(RailRefusal::new(
            "agent_may_integrate",
            "an agent controller may not open a pull request",
        )),
        (true, IntegratePermission::Pr)
        | (false, IntegratePermission::Merge | IntegratePermission::Pr) => Ok(()),
        (_, IntegratePermission::Unrecognized) => Err(RailRefusal::new(
            "agent_may_integrate",
            "integration permission is not recognized",
        )),
    }
}

/// `edges` are `(task, after)`. Adding `after` onto `task` must not cycle.
#[must_use]
pub fn dependency_cycle(edges: &[(TaskId, TaskId)], task: TaskId, after: &[TaskId]) -> bool {
    let mut graph: HashMap<TaskId, Vec<TaskId>> = HashMap::new();
    for (from, to) in edges {
        graph.entry(*from).or_default().push(*to);
    }
    graph.entry(task).or_default().extend(after.iter().copied());
    let mut seen = HashSet::new();
    let mut stack = HashSet::new();
    fn walk(
        node: TaskId,
        graph: &HashMap<TaskId, Vec<TaskId>>,
        seen: &mut HashSet<TaskId>,
        stack: &mut HashSet<TaskId>,
    ) -> bool {
        if !stack.insert(node) {
            return true;
        }
        if !seen.insert(node) {
            stack.remove(&node);
            return false;
        }
        if let Some(next) = graph.get(&node) {
            for dep in next {
                if walk(*dep, graph, seen, stack) {
                    return true;
                }
            }
        }
        stack.remove(&node);
        false
    }
    // A cycle might not include `task` only if the existing graph already
    // cycled. Walk every node so a pre-existing cycle is still refused.
    let mut nodes: Vec<TaskId> = graph.keys().copied().collect();
    nodes.push(task);
    nodes.extend(after.iter().copied());
    for node in nodes {
        if walk(node, &graph, &mut seen, &mut stack) {
            return true;
        }
    }
    false
}

/// Ready only when every dependency is accepted or integrated.
#[must_use]
pub fn dependencies_met(after: &[TaskId], status: impl Fn(TaskId) -> Option<TaskStatus>) -> bool {
    after
        .iter()
        .all(|id| status(*id).is_some_and(TaskStatus::satisfies_dependency))
}

/// Tasks that are `Pending` and whose dependencies are now met become `Ready`.
/// Does not start them.
pub fn promote_ready(tasks: &mut [Task]) {
    let statuses: HashMap<TaskId, TaskStatus> =
        tasks.iter().map(|task| (task.id, task.status)).collect();
    for task in tasks.iter_mut() {
        if task.status == TaskStatus::Pending
            && dependencies_met(&task.after, |id| statuses.get(&id).copied())
        {
            task.status = TaskStatus::Ready;
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RestartReconcile {
    pub runs: Vec<Run>,
    pub tasks: Vec<Task>,
    pub attempts: Vec<Attempt>,
}

/// Live attempts become `Lost { DaemonRestarted }`. Active runs become
/// `Interrupted`. Tasks that were in progress go back to `Ready` while the
/// attempt budget remains. Nothing is resumed.
#[must_use]
pub fn reconcile_restart(
    mut runs: Vec<Run>,
    mut tasks: Vec<Task>,
    mut attempts: Vec<Attempt>,
    limits: &OrchestrationLimits,
    now: Timestamp,
) -> RestartReconcile {
    let mut lost_tasks = HashSet::new();
    for attempt in &mut attempts {
        if attempt.is_live() {
            attempt.phase = AttemptPhase::Lost;
            attempt.lost_reason = Some(LostReason::DaemonRestarted);
            attempt.outcome = None;
            attempt.settled_at = Some(now);
            lost_tasks.insert(attempt.task_id);
        }
    }
    for task in &mut tasks {
        if lost_tasks.contains(&task.id)
            && matches!(task.status, TaskStatus::Active | TaskStatus::Blocked)
        {
            task.status = task_status_after_loss(task.attempts_used, limits.max_attempts_per_task);
            task.updated_at = now;
            task.revision = task.revision.saturating_add(1);
        }
    }
    for run in &mut runs {
        if run.status == RunStatus::Active {
            run.status = RunStatus::Interrupted;
            run.updated_at = now;
            run.revision = run.revision.saturating_add(1);
        }
    }
    RestartReconcile {
        runs,
        tasks,
        attempts,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuestionFact {
    pub id: ContextId,
    pub task_id: Option<TaskId>,
    pub text: String,
    pub answered: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttemptFact {
    pub attempt: Attempt,
    pub activity: ActivityState,
    pub activity_since: Timestamp,
    pub session_exited: bool,
}

/// Attention for `run wait --for attention`. Derived; never stored.
#[must_use]
pub fn attention(
    questions: &[QuestionFact],
    attempts: &[AttemptFact],
    integration_conflict: bool,
    now: Timestamp,
    limits: &OrchestrationLimits,
) -> Vec<AttentionItem> {
    let mut items = Vec::new();
    for question in questions {
        if question.answered {
            continue;
        }
        items.push(AttentionItem {
            kind: AttentionKind::Question,
            task_id: question.task_id,
            attempt_id: None,
            message_id: Some(question.id),
            text: Some(question.text.clone()),
            outcome: None,
        });
    }
    for fact in attempts {
        let attempt = &fact.attempt;
        if attempt.phase == AttemptPhase::Reported {
            if let Some(report) = &attempt.report {
                items.push(AttentionItem {
                    kind: AttentionKind::Reported,
                    task_id: Some(attempt.task_id),
                    attempt_id: Some(attempt.id),
                    message_id: None,
                    text: Some(report.summary.clone()),
                    outcome: Some(report.outcome),
                });
            }
        }
        if attempt.lost_reason == Some(LostReason::ExitedWithoutReport) {
            items.push(AttentionItem {
                kind: AttentionKind::ExitedWithoutReport,
                task_id: Some(attempt.task_id),
                attempt_id: Some(attempt.id),
                message_id: None,
                text: None,
                outcome: None,
            });
        }
        if !attempt.is_live() {
            continue;
        }
        let elapsed = elapsed_since(fact.activity_since, now);
        match fact.activity {
            ActivityState::Waiting if elapsed >= limits.waiting_attention_after => {
                items.push(AttentionItem {
                    kind: AttentionKind::Waiting,
                    task_id: Some(attempt.task_id),
                    attempt_id: Some(attempt.id),
                    message_id: None,
                    text: None,
                    outcome: None,
                });
            }
            ActivityState::Idle if elapsed >= limits.stall_after => {
                items.push(AttentionItem {
                    kind: AttentionKind::Stalled,
                    task_id: Some(attempt.task_id),
                    attempt_id: Some(attempt.id),
                    message_id: None,
                    text: None,
                    outcome: None,
                });
            }
            ActivityState::Starting if elapsed >= limits.starting_attention_after => {
                items.push(AttentionItem {
                    kind: AttentionKind::Starting,
                    task_id: Some(attempt.task_id),
                    attempt_id: Some(attempt.id),
                    message_id: None,
                    text: None,
                    outcome: None,
                });
            }
            _ => {}
        }
    }
    if integration_conflict {
        items.push(AttentionItem {
            kind: AttentionKind::IntegrationConflict,
            task_id: None,
            attempt_id: None,
            message_id: None,
            text: None,
            outcome: None,
        });
    }
    items
}

fn elapsed_since(since: Timestamp, now: Timestamp) -> Duration {
    let elapsed = now.as_offset() - since.as_offset();
    Duration::try_from(elapsed).unwrap_or(Duration::ZERO)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Address {
    Controller,
    Task(TaskId),
    Session(SessionId),
    Run(RunId),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AddressError {
    Empty,
    BadPrefix,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeliveryTarget {
    Session(SessionId),
    /// A human controller reads this from `inbox --run`.
    HumanController,
}

#[derive(Clone, Copy, Debug)]
pub struct LiveAttempt {
    pub task_id: TaskId,
    pub session_id: Option<SessionId>,
    pub n: u32,
    pub live: bool,
}

/// `controller` is `None` for a human-driven run.
pub fn resolve_address(
    address: &Address,
    controller: Option<SessionId>,
    attempts: &[LiveAttempt],
) -> Result<Vec<DeliveryTarget>, AddressError> {
    match address {
        Address::Controller => Ok(vec![match controller {
            Some(session) => DeliveryTarget::Session(session),
            None => DeliveryTarget::HumanController,
        }]),
        Address::Session(session) => Ok(vec![DeliveryTarget::Session(*session)]),
        Address::Task(task) => {
            let current = attempts
                .iter()
                .filter(|attempt| attempt.task_id == *task && attempt.session_id.is_some())
                .max_by_key(|attempt| (attempt.live, attempt.n));
            match current.and_then(|attempt| attempt.session_id) {
                Some(session) => Ok(vec![DeliveryTarget::Session(session)]),
                None => Err(AddressError::Unknown),
            }
        }
        Address::Run(_) => {
            let mut targets = Vec::new();
            for attempt in attempts {
                if attempt.live {
                    if let Some(session) = attempt.session_id {
                        targets.push(DeliveryTarget::Session(session));
                    }
                }
            }
            Ok(targets)
        }
    }
}

/// Parse `controller`, `task:<id>`, `session:<id>`, or `run:<id>`.
///
/// Ids may be a unique prefix of at least 8 characters. `resolve_prefix`
/// finishes that against the ledger.
pub fn parse_address(raw: &str) -> Result<ParsedAddress, AddressError> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(AddressError::Empty);
    }
    if raw == "controller" {
        return Ok(ParsedAddress::Controller);
    }
    let Some((kind, id)) = raw.split_once(':') else {
        return Err(AddressError::BadPrefix);
    };
    if id.len() < MIN_ID_PREFIX {
        return Err(AddressError::BadPrefix);
    }
    match kind {
        "task" => Ok(ParsedAddress::Task(id.to_owned())),
        "session" => Ok(ParsedAddress::Session(id.to_owned())),
        "run" => Ok(ParsedAddress::Run(id.to_owned())),
        _ => Err(AddressError::BadPrefix),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParsedAddress {
    Controller,
    Task(String),
    Session(String),
    Run(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PrefixError {
    TooShort,
    None,
    Ambiguous,
}

/// Git-style unique prefix, minimum [`MIN_ID_PREFIX`] characters.
pub fn resolve_prefix<'a, T, I>(candidates: I, raw: &str) -> Result<&'a T, PrefixError>
where
    T: std::fmt::Display,
    I: IntoIterator<Item = &'a T>,
{
    let raw = raw.trim();
    if raw.len() < MIN_ID_PREFIX {
        return Err(PrefixError::TooShort);
    }
    let mut found = None;
    for candidate in candidates {
        let text = candidate.to_string();
        if text == raw {
            return Ok(candidate);
        }
        if text.starts_with(raw) {
            if found.is_some() {
                return Err(PrefixError::Ambiguous);
            }
            found = Some(candidate);
        }
    }
    found.ok_or(PrefixError::None)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CasOutcome {
    /// Version 0 against an absent key. The stored version is 1.
    Created,
    /// `if_version` matched. The stored version is `current + 1`.
    Updated { next: u64 },
    /// Includes the version that is actually stored. `0` means the key is absent.
    Mismatch { current: u64 },
}

/// Compare-and-set. Version 0 is create-only.
#[must_use]
pub fn compare_and_set(current: Option<u64>, if_version: u64) -> CasOutcome {
    match (current, if_version) {
        (None, 0) => CasOutcome::Created,
        (None, _) => CasOutcome::Mismatch { current: 0 },
        (Some(version), expected) if version == expected && expected != 0 => CasOutcome::Updated {
            next: version.saturating_add(1),
        },
        (Some(version), _) => CasOutcome::Mismatch { current: version },
    }
}

#[must_use]
pub fn board_is_full(existing_keys: usize, key_is_new: bool) -> bool {
    key_is_new && existing_keys >= MAX_BOARD_KEYS
}

#[must_use]
pub fn valid_board_key(key: &str) -> bool {
    let bytes = key.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= MAX_BOARD_KEY_BYTES
        && bytes.iter().all(|byte| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(byte, b'.' | b'_' | b'/' | b'-')
        })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BoardValueError {
    Oversize { len: usize },
    NotJson,
}

/// Refuse an oversize value before parsing it. A short value must be JSON.
pub fn board_value(raw: &str) -> Result<&str, BoardValueError> {
    if raw.len() > MAX_BOARD_VALUE_BYTES {
        return Err(BoardValueError::Oversize { len: raw.len() });
    }
    let _: serde_json::Value = serde_json::from_str(raw).map_err(|_| BoardValueError::NotJson)?;
    Ok(raw)
}

/// Clamp to `max_bytes`, keeping the marker inside the budget.
///
/// The returned string is at most `max_bytes` long. Only the prefix is copied.
#[must_use]
pub fn clamp_text(input: &str, max_bytes: usize) -> (String, bool) {
    if input.len() <= max_bytes {
        return (input.to_owned(), false);
    }
    const MARK: &str = "\n[truncated]";
    let budget = max_bytes.saturating_sub(MARK.len());
    let mut end = budget.min(input.len());
    while end > 0 && !input.is_char_boundary(end) {
        end -= 1;
    }
    let mut out = String::with_capacity(end + MARK.len());
    out.push_str(&input[..end]);
    if max_bytes >= MARK.len() {
        out.push_str(MARK);
    }
    if out.len() > max_bytes {
        out.truncate(max_bytes);
    }
    (out, true)
}

/// Kinds `run wait --for` understands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaitKind {
    Attention,
    Reported,
    Question,
    Settled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WaitParseError;

pub fn parse_wait_kinds(raw: &str) -> Result<Vec<WaitKind>, WaitParseError> {
    let mut kinds = Vec::new();
    for part in raw.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        kinds.push(match part {
            "attention" => WaitKind::Attention,
            "reported" => WaitKind::Reported,
            "question" => WaitKind::Question,
            "settled" => WaitKind::Settled,
            _ => return Err(WaitParseError),
        });
    }
    if kinds.is_empty() {
        Err(WaitParseError)
    } else {
        Ok(kinds)
    }
}

/// Items that already satisfy `kinds`. Level-triggered: a non-empty result
/// means the caller returns immediately.
#[must_use]
pub fn matching_wake(
    kinds: &[WaitKind],
    items: &[AttentionItem],
    task_filter: &[TaskId],
    run_settled: bool,
) -> Vec<AttentionItem> {
    let mut matched = Vec::new();
    for item in items {
        if !task_filter.is_empty() && item.task_id.is_some_and(|id| !task_filter.contains(&id)) {
            continue;
        }
        if task_filter_misses(task_filter, item) {
            continue;
        }
        let hit = kinds.iter().any(|kind| match kind {
            WaitKind::Attention => true,
            WaitKind::Reported => item.kind == AttentionKind::Reported,
            WaitKind::Question => item.kind == AttentionKind::Question,
            WaitKind::Settled => false,
        });
        if hit {
            matched.push(item.clone());
        }
    }
    if kinds.contains(&WaitKind::Settled) && run_settled && matched.is_empty() {
        matched.push(AttentionItem {
            kind: AttentionKind::Unrecognized,
            task_id: None,
            attempt_id: None,
            message_id: None,
            text: Some("settled".to_owned()),
            outcome: None,
        });
    }
    matched
}

fn task_filter_misses(task_filter: &[TaskId], item: &AttentionItem) -> bool {
    if task_filter.is_empty() {
        return false;
    }
    match item.task_id {
        Some(id) => !task_filter.contains(&id),
        None => true,
    }
}

/// Startup and loss share this shape so a test can see a run was interrupted
/// without a worker being revived.
#[must_use]
pub fn run_was_interrupted(runs: &[Run]) -> bool {
    runs.iter().any(|run| run.status == RunStatus::Interrupted)
}

/// Board entries the digest may inline. Large values stay keys-only.
#[must_use]
pub fn board_inline(entries: &[BoardEntry], max_value_bytes: usize) -> Vec<(&str, &str)> {
    entries
        .iter()
        .filter(|entry| entry.value_json.len() <= max_value_bytes)
        .map(|entry| (entry.key.as_str(), entry.value_json.as_str()))
        .collect()
}

/// Silence the unused-import lint when a caller only needs the limits type
/// through a function. Kept so tests can name the same struct the daemon uses.
#[must_use]
pub fn limits_or_default(limits: Option<OrchestrationLimits>) -> OrchestrationLimits {
    limits.unwrap_or_default()
}

/// Deserialize helper used by tests that round-trip a refusal reason.
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct PolicyDetails {
    reason: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{AgentProviderId, AttemptId, ProjectId, RunId, WorkspaceId};
    use crate::orchestration::{AttemptReport, WorkMode};

    fn now() -> Timestamp {
        Timestamp::now()
    }

    fn earlier(secs: i64) -> Timestamp {
        Timestamp::from_offset(now().as_offset() - time::Duration::seconds(secs))
    }

    fn task(status: TaskStatus, after: Vec<TaskId>) -> Task {
        let id = TaskId::new();
        Task {
            id,
            run_id: RunId::new(),
            title: "t".into(),
            spec: "s".into(),
            acceptance: "a".into(),
            after,
            mode: WorkMode::Write,
            allow_subruns: false,
            status,
            attempts_used: 0,
            revision: 1,
            created_at: now(),
            updated_at: now(),
            feedback: None,
        }
    }

    fn attempt(phase: AttemptPhase, session: Option<SessionId>) -> Attempt {
        Attempt {
            id: AttemptId::new(),
            task_id: TaskId::new(),
            n: 1,
            session_id: session,
            workspace_id: None,
            branch: None,
            base_commit: None,
            provider_id: AgentProviderId::new("claude"),
            profile_id: None,
            read_only: false,
            phase,
            outcome: None,
            lost_reason: None,
            report: None,
            integrated_commit: None,
            created_at: now(),
            settled_at: None,
        }
    }

    #[test]
    fn only_an_explicit_report_from_the_attempt_session_settles() {
        let session = SessionId::new();
        assert!(settle_report(AttemptPhase::Running, Some(session), Some(session), false).is_ok());
        assert_eq!(
            settle_report(
                AttemptPhase::Running,
                Some(session),
                Some(SessionId::new()),
                false
            ),
            Err(ReportRefuse::Foreign)
        );
        assert_eq!(
            settle_report(AttemptPhase::Running, Some(session), None, false),
            Err(ReportRefuse::Foreign)
        );
        assert_eq!(
            settle_report(AttemptPhase::Reported, Some(session), Some(session), false),
            Err(ReportRefuse::Superseded)
        );
        assert_eq!(
            settle_report(AttemptPhase::Running, Some(session), Some(session), true),
            Err(ReportRefuse::Superseded)
        );
        assert_eq!(
            settle_report(AttemptPhase::Lost, Some(session), Some(session), false),
            Err(ReportRefuse::Superseded)
        );
        assert_eq!(
            task_status_after_report(ReportOutcome::Done, 1, 3),
            TaskStatus::Review
        );
        assert_eq!(
            task_status_after_report(ReportOutcome::Blocked, 1, 3),
            TaskStatus::Blocked
        );
        // Idle, exit, and a timeout are not a report: the phase stays Running.
        assert!(matches!(
            settle_report(AttemptPhase::Running, Some(session), Some(session), false),
            Ok(())
        ));
    }

    #[test]
    fn caps_and_the_one_writer_rule_refuse() {
        let limits = OrchestrationLimits::default();
        assert!(allow_new_task(31, &limits).is_ok());
        assert_eq!(allow_new_task(32, &limits).unwrap_err().rail, "max_tasks");
        assert_eq!(
            allow_start(4, 0, WorkMode::Write, false, &limits)
                .unwrap_err()
                .rail,
            "max_active_attempts"
        );
        assert_eq!(
            allow_start(0, 3, WorkMode::Write, false, &limits)
                .unwrap_err()
                .rail,
            "max_attempts"
        );
        assert_eq!(
            allow_start(0, 0, WorkMode::Write, true, &limits)
                .unwrap_err()
                .rail,
            "one_writer"
        );
        assert!(allow_start(0, 0, WorkMode::ReadOnly, true, &limits).is_ok());
        assert_eq!(
            allow_nested_run(2, true, &limits).unwrap_err().rail,
            "max_run_depth"
        );
        assert_eq!(
            allow_nested_run(1, false, &limits).unwrap_err().rail,
            "subruns"
        );
        let human = None;
        let controller = Some(SessionId::new());
        assert!(allow_controller(human, controller).is_ok());
        assert!(allow_controller(controller, controller).is_ok());
        assert_eq!(
            allow_controller(Some(SessionId::new()), controller)
                .unwrap_err()
                .rail,
            "controller_only"
        );
    }

    #[test]
    fn a_dependency_cycle_is_refused_and_ready_waits_for_accept_or_integrate() {
        let a = TaskId::new();
        let b = TaskId::new();
        assert!(dependency_cycle(&[(a, b)], b, &[a]));
        assert!(!dependency_cycle(&[], a, &[b]));
        assert!(!dependencies_met(&[a], |_| Some(TaskStatus::Ready)));
        assert!(!dependencies_met(&[a], |_| Some(TaskStatus::Review)));
        assert!(dependencies_met(&[a], |_| Some(TaskStatus::Accepted)));
        assert!(dependencies_met(&[a], |_| Some(TaskStatus::Integrated)));
        assert!(dependencies_met(&[], |_| None));

        let mut pending = task(TaskStatus::Pending, vec![]);
        let dep = task(TaskStatus::Review, vec![]);
        pending.after = vec![dep.id];
        let mut tasks = vec![dep, pending];
        promote_ready(&mut tasks);
        assert_eq!(tasks[1].status, TaskStatus::Pending);
        tasks[0].status = TaskStatus::Accepted;
        promote_ready(&mut tasks);
        assert_eq!(tasks[1].status, TaskStatus::Ready);
    }

    #[test]
    fn silence_does_not_change_activity_and_input_only_leaves_idle() {
        let start = AgentActivity::starting(now());
        let later = now();
        let quiet = apply_activity(&start, ActivityTrigger::Silence, later);
        assert_eq!(quiet, start);
        let exited = apply_activity(&start, ActivityTrigger::ProcessExit, later);
        assert_eq!(exited.state, ActivityState::Starting);

        let idle = AgentActivity {
            state: ActivityState::Idle,
            since: earlier(10),
            evidence: Some(ActivityEvidence::Hook),
        };
        let typed = apply_activity(&idle, ActivityTrigger::Input, later);
        assert_eq!(typed.state, ActivityState::Working);
        assert_eq!(typed.evidence, Some(ActivityEvidence::Input));
        let still = apply_activity(&start, ActivityTrigger::Input, later);
        assert_eq!(still.state, ActivityState::Starting);

        let working = apply_activity(&start, ActivityTrigger::Hook(ActivityState::Working), later);
        assert_eq!(working.state, ActivityState::Working);
        assert_eq!(working.evidence, Some(ActivityEvidence::Hook));
        let ignored = apply_activity(
            &working,
            ActivityTrigger::Hook(ActivityState::Unknown),
            later,
        );
        assert_eq!(ignored.state, ActivityState::Working);
    }

    #[test]
    fn pointer_eligibility_matches_the_delivery_rule() {
        assert_eq!(
            pointer_decision(ActivityState::Idle, false, false),
            PointerDecision::Type
        );
        assert_eq!(
            pointer_decision(ActivityState::Idle, false, true),
            PointerDecision::Suppress
        );
        assert_eq!(
            pointer_decision(ActivityState::Working, false, false),
            PointerDecision::Queue
        );
        for state in [
            ActivityState::Waiting,
            ActivityState::Starting,
            ActivityState::Unknown,
        ] {
            assert_eq!(
                pointer_decision(state, false, false),
                PointerDecision::Suppress,
                "{state:?}"
            );
        }
        assert_eq!(
            pointer_decision(ActivityState::Idle, true, false),
            PointerDecision::Suppress
        );
        assert!(pointer_line(2).contains("2 new messages"));
        assert!(!pointer_line(1).contains('\n'));
    }

    #[test]
    fn compare_and_set_mismatch_returns_the_current_version() {
        assert_eq!(compare_and_set(None, 0), CasOutcome::Created);
        assert_eq!(
            compare_and_set(Some(4), 3),
            CasOutcome::Mismatch { current: 4 }
        );
        assert_eq!(
            compare_and_set(Some(4), 0),
            CasOutcome::Mismatch { current: 4 }
        );
        assert_eq!(
            compare_and_set(None, 2),
            CasOutcome::Mismatch { current: 0 }
        );
        assert_eq!(compare_and_set(Some(4), 4), CasOutcome::Updated { next: 5 });
        assert!(!valid_board_key(""));
        assert!(!valid_board_key("HasSpace"));
        assert!(valid_board_key("decision/export-format"));
        assert!(matches!(
            board_value(&format!("\"{}\"", "x".repeat(MAX_BOARD_VALUE_BYTES))),
            Err(BoardValueError::Oversize { .. })
        ));
        assert!(matches!(
            board_value("{not json"),
            Err(BoardValueError::NotJson)
        ));
        assert_eq!(
            board_value(r#"{"mode":"stream"}"#).unwrap(),
            r#"{"mode":"stream"}"#
        );
        assert!(board_is_full(MAX_BOARD_KEYS, true));
        assert!(!board_is_full(MAX_BOARD_KEYS, false));
    }

    #[test]
    fn oversize_text_is_clamped_without_copying_the_tail() {
        let big = "y".repeat(MAX_BOARD_VALUE_BYTES + 8_000);
        let (clamped, truncated) = clamp_text(&big, 64);
        assert!(truncated);
        assert!(clamped.len() <= 64);
        assert!(clamped.contains("[truncated]"));
        assert!(!clamped.contains(&"y".repeat(65)));
        let (same, not) = clamp_text("short", 64);
        assert!(!not);
        assert_eq!(same, "short");
    }

    #[test]
    fn restart_marks_live_attempts_lost_and_the_run_interrupted() {
        let limits = OrchestrationLimits::default();
        let session = SessionId::new();
        let mut live = attempt(AttemptPhase::Running, Some(session));
        let task_id = live.task_id;
        let mut work = task(TaskStatus::Active, vec![]);
        work.id = task_id;
        work.attempts_used = 1;
        let mut run = Run {
            id: work.run_id,
            project_id: ProjectId::new(),
            objective: "ship".into(),
            brief: String::new(),
            brief_version: 0,
            controller_session_id: None,
            integration_workspace_id: Some(WorkspaceId::new()),
            base: Some("main".into()),
            parent_attempt_id: None,
            status: RunStatus::Active,
            revision: 1,
            created_at: now(),
            updated_at: now(),
            closed_at: None,
        };
        let reconciled = reconcile_restart(
            vec![run.clone()],
            vec![work],
            vec![live.clone()],
            &limits,
            now(),
        );
        assert_eq!(reconciled.runs[0].status, RunStatus::Interrupted);
        assert_eq!(reconciled.attempts[0].phase, AttemptPhase::Lost);
        assert_eq!(
            reconciled.attempts[0].lost_reason,
            Some(LostReason::DaemonRestarted)
        );
        assert_eq!(reconciled.tasks[0].status, TaskStatus::Ready);
        live.phase = AttemptPhase::Reported;
        live.report = Some(AttemptReport {
            outcome: ReportOutcome::Done,
            summary: "done".into(),
            verification: None,
            result_path: None,
            reported_head: None,
            dirty_at_report: false,
            files_changed: vec![],
        });
        run.status = RunStatus::Completed;
        let kept = reconcile_restart(vec![run], vec![], vec![live], &limits, now());
        assert_eq!(kept.runs[0].status, RunStatus::Completed);
        assert_eq!(kept.attempts[0].phase, AttemptPhase::Reported);
    }

    #[test]
    fn a_run_broadcast_names_each_live_attempt_once() {
        let a = SessionId::new();
        let b = SessionId::new();
        let dead = SessionId::new();
        let attempts = [
            LiveAttempt {
                task_id: TaskId::new(),
                session_id: Some(a),
                n: 1,
                live: true,
            },
            LiveAttempt {
                task_id: TaskId::new(),
                session_id: Some(b),
                n: 1,
                live: true,
            },
            LiveAttempt {
                task_id: TaskId::new(),
                session_id: Some(dead),
                n: 1,
                live: false,
            },
        ];
        let targets = resolve_address(&Address::Run(RunId::new()), None, &attempts).unwrap();
        assert_eq!(
            targets,
            vec![DeliveryTarget::Session(a), DeliveryTarget::Session(b)]
        );
        assert_eq!(
            resolve_address(&Address::Controller, None, &attempts).unwrap(),
            vec![DeliveryTarget::HumanController]
        );
        assert_eq!(
            parse_address("controller").unwrap(),
            ParsedAddress::Controller
        );
        assert!(parse_address("task:abcd").is_err());
        assert!(matches!(
            parse_address("task:01234567"),
            Ok(ParsedAddress::Task(_))
        ));
    }

    #[test]
    fn attention_includes_the_documented_categories_and_not_a_fresh_idle() {
        let limits = OrchestrationLimits::default();
        let task_id = TaskId::new();
        let mut reported = attempt(AttemptPhase::Reported, Some(SessionId::new()));
        reported.task_id = task_id;
        reported.report = Some(AttemptReport {
            outcome: ReportOutcome::Done,
            summary: "shipped".into(),
            verification: None,
            result_path: None,
            reported_head: None,
            dirty_at_report: false,
            files_changed: vec![],
        });
        let stalled = attempt(AttemptPhase::Running, Some(SessionId::new()));
        let mut lost = attempt(AttemptPhase::Lost, Some(SessionId::new()));
        lost.lost_reason = Some(LostReason::ExitedWithoutReport);
        let question = QuestionFact {
            id: ContextId::new(),
            task_id: Some(task_id),
            text: "stream it?".into(),
            answered: false,
        };
        let items = attention(
            &[question],
            &[
                AttemptFact {
                    attempt: reported,
                    activity: ActivityState::Idle,
                    activity_since: earlier(5),
                    session_exited: false,
                },
                AttemptFact {
                    attempt: stalled,
                    activity: ActivityState::Idle,
                    activity_since: earlier(5),
                    session_exited: false,
                },
                AttemptFact {
                    attempt: lost,
                    activity: ActivityState::Unknown,
                    activity_since: earlier(5),
                    session_exited: true,
                },
            ],
            true,
            now(),
            &limits,
        );
        let kinds: Vec<_> = items.iter().map(|item| item.kind).collect();
        assert!(kinds.contains(&AttentionKind::Question));
        assert!(kinds.contains(&AttentionKind::Reported));
        assert!(kinds.contains(&AttentionKind::ExitedWithoutReport));
        assert!(kinds.contains(&AttentionKind::IntegrationConflict));
        assert!(!kinds.contains(&AttentionKind::Stalled));

        let mut waiting = attempt(AttemptPhase::Running, Some(SessionId::new()));
        waiting.n = 2;
        let waited = attention(
            &[],
            &[AttemptFact {
                attempt: waiting,
                activity: ActivityState::Waiting,
                activity_since: earlier(61),
                session_exited: false,
            }],
            false,
            now(),
            &limits,
        );
        assert_eq!(waited[0].kind, AttentionKind::Waiting);

        let mut starting = attempt(AttemptPhase::Starting, Some(SessionId::new()));
        starting.n = 3;
        let slow = attention(
            &[],
            &[AttemptFact {
                attempt: starting,
                activity: ActivityState::Starting,
                activity_since: earlier(91),
                session_exited: false,
            }],
            false,
            now(),
            &limits,
        );
        assert_eq!(slow[0].kind, AttentionKind::Starting);

        let mut idle_long = attempt(AttemptPhase::Running, Some(SessionId::new()));
        idle_long.n = 4;
        let stall = attention(
            &[],
            &[AttemptFact {
                attempt: idle_long,
                activity: ActivityState::Idle,
                activity_since: earlier(901),
                session_exited: false,
            }],
            false,
            now(),
            &limits,
        );
        assert_eq!(stall[0].kind, AttentionKind::Stalled);
    }

    #[test]
    fn wait_is_level_triggered_on_the_items_already_present() {
        let kinds = parse_wait_kinds("attention,reported").unwrap();
        let item = AttentionItem {
            kind: AttentionKind::Reported,
            task_id: Some(TaskId::new()),
            attempt_id: Some(AttemptId::new()),
            message_id: None,
            text: Some("done".into()),
            outcome: Some(ReportOutcome::Done),
        };
        let hit = matching_wake(&kinds, std::slice::from_ref(&item), &[], false);
        assert_eq!(hit.len(), 1);
        assert_eq!(hit[0].text.as_deref(), Some("done"));
        assert!(matching_wake(&kinds, &[], &[], false).is_empty());
    }

    #[test]
    fn an_agent_may_open_a_pr_and_may_not_merge_it_when_the_rail_says_pr() {
        assert!(allow_agent_integrate(true, IntegratePermission::Pr).is_ok());
        assert!(allow_agent_integrate(false, IntegratePermission::Pr).is_ok());
        assert!(allow_agent_integrate(true, IntegratePermission::Merge).is_err());
        assert!(allow_agent_integrate(false, IntegratePermission::None).is_err());
    }
}
