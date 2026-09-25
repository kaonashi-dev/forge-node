//! Ledger, rails, and effects for `forgectl`.
//!
//! Decisions are [`domain::orchestration`] functions. Git, worktrees, result
//! files, and PTY writes happen outside the core lock.

mod deliver;

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use domain::orchestration::{
    accepted_dependency_summaries, allow_agent_integrate, allow_controller, allow_nested_run,
    allow_new_task, allow_start, apply_activity, attention, board_is_full, board_value,
    compare_and_set, compose_controller_prompt, compose_worker_prompt, dependencies_met,
    dependency_cycle, promote_ready, reconcile_restart, settle_report, task_status_after_loss,
    task_status_after_report, valid_board_key, ActivityTrigger, AttemptFact, AttemptPhase,
    AttemptPlacement, BoardEntry, CasOutcome, ControllerPromptInput, ControllerSpec,
    DeliveryTarget, DependencySummary, IntegrationPlacement, LiveAttempt, LostReason,
    OrchestrationLimits, QuestionFact, RailRefusal, ReportOutcome, ReportRefuse, RunStatus,
    TaskDecision, TaskLine, TaskStatus, WorkMode, WorkerPromptInput, MAX_ACCEPTANCE_BYTES,
    MAX_BRIEF_BYTES, MAX_MESSAGE_BYTES, MAX_OBJECTIVE_BYTES, MAX_RESULT_FILE_BYTES, MAX_SPEC_BYTES,
    MAX_SUMMARY_BYTES, MAX_TITLE_BYTES, MAX_VERIFICATION_BYTES,
};
use domain::orchestration::{Attempt, Run, Task};
use domain::{
    ActivityState, AgentProviderId, AttemptId, ContextEnvelope, ContextId, ContextKind, ProjectId,
    RunId, SessionId, SessionKind, SessionRole, SessionState, TaskId, Timestamp, WorkspaceId,
};
use protocol::{
    ErrorCode, IntegrateHow, OrchestrationLimitsView, ProtocolError, Request, Response,
};
use serde::{Deserialize, Serialize};

use crate::core::{Daemon, Inner};

const RECEIPT_TTL: Duration = Duration::from_secs(24 * 60 * 60);

thread_local! {
    static SPAWN_ENV: RefCell<Vec<(String, String)>> = const { RefCell::new(Vec::new()) };
}

/// Env applied to the next `create_session` on this thread, then cleared.
pub fn set_spawn_env(env: Vec<(String, String)>) {
    SPAWN_ENV.with(|slot| *slot.borrow_mut() = env);
}

pub fn take_spawn_env() -> Vec<(String, String)> {
    SPAWN_ENV.with(|slot| std::mem::take(&mut *slot.borrow_mut()))
}

#[derive(Clone, Debug, Default)]
pub struct PointerSlot {
    pub queued: bool,
    pub typed_this_idle: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Ledger {
    pub runs: HashMap<RunId, Run>,
    pub tasks: HashMap<TaskId, Task>,
    pub attempts: HashMap<AttemptId, Attempt>,
    pub board: HashMap<(RunId, String), BoardEntry>,
    pub pointer: HashMap<SessionId, PointerSlot>,
    pub conflict: Vec<RunId>,
}

pub fn load_ledger(db: &persistence::Db) -> Result<Ledger, persistence::DbError> {
    let mut ledger = Ledger::default();
    for run in db.orchestration().list_runs()? {
        ledger.runs.insert(run.id, run);
    }
    for task in db.orchestration().list_tasks()? {
        ledger.tasks.insert(task.id, task);
    }
    for attempt in db.orchestration().list_attempts()? {
        ledger.attempts.insert(attempt.id, attempt);
    }
    for run_id in ledger.runs.keys().copied().collect::<Vec<_>>() {
        for entry in db.orchestration().list_board(run_id)? {
            ledger.board.insert((run_id, entry.key.clone()), entry);
        }
    }
    Ok(ledger)
}

pub fn reconcile_startup(
    db: &persistence::Db,
    limits: &OrchestrationLimits,
) -> Result<(), persistence::DbError> {
    let runs = db.orchestration().list_runs()?;
    let tasks = db.orchestration().list_tasks()?;
    let attempts = db.orchestration().list_attempts()?;
    if runs.is_empty() && attempts.is_empty() {
        return Ok(());
    }
    let reconciled = reconcile_restart(runs, tasks, attempts, limits, Timestamp::now());
    for run in &reconciled.runs {
        db.orchestration().update_run(run)?;
    }
    for task in &reconciled.tasks {
        db.orchestration().update_task(task)?;
    }
    for attempt in &reconciled.attempts {
        db.orchestration().update_attempt(attempt)?;
    }
    Ok(())
}

pub(crate) fn identity_for(
    inner: &Inner,
    session_id: SessionId,
) -> Option<(String, Option<String>, Option<String>)> {
    if let Some(attempt) = inner
        .ledger
        .attempts
        .values()
        .find(|attempt| attempt.session_id == Some(session_id))
    {
        let task = inner.ledger.tasks.get(&attempt.task_id)?;
        return Some((
            task.run_id.to_string(),
            Some(task.id.to_string()),
            Some(attempt.id.to_string()),
        ));
    }
    let run = inner
        .ledger
        .runs
        .values()
        .find(|run| run.controller_session_id == Some(session_id))?;
    Some((run.id.to_string(), None, None))
}

pub fn handles(request: &Request) -> bool {
    matches!(
        request,
        Request::OrchestrationStatus
            | Request::CreateRun { .. }
            | Request::ListRuns { .. }
            | Request::GetRun { .. }
            | Request::UpdateRunBrief { .. }
            | Request::CloseRun { .. }
            | Request::ResumeRun { .. }
            | Request::CreateTask { .. }
            | Request::StartAttempt { .. }
            | Request::RunTask { .. }
            | Request::ReportAttempt { .. }
            | Request::DecideTask { .. }
            | Request::IntegrateTask { .. }
            | Request::CleanupTask { .. }
            | Request::ReviewTask { .. }
            | Request::PostMessage { .. }
            | Request::ReadInbox { .. }
            | Request::WatchInbox { .. }
            | Request::ListRunState { .. }
            | Request::GetRunState { .. }
            | Request::SetRunState { .. }
            | Request::DeleteRunState { .. }
            | Request::ReportAgentActivity { .. }
    )
}

pub fn handle(daemon: &Arc<Daemon>, request: Request) -> Result<Response, ProtocolError> {
    if let Some((request_id, caller)) = receipt_key(&request) {
        if let Some(stored) = load_receipt(daemon, &request_id, caller)? {
            return Ok(stored);
        }
        let response = dispatch(daemon, request)?;
        store_receipt(daemon, &request_id, caller, &response)?;
        return Ok(response);
    }
    dispatch(daemon, request)
}

fn dispatch(daemon: &Arc<Daemon>, request: Request) -> Result<Response, ProtocolError> {
    match request {
        Request::OrchestrationStatus => status(daemon),
        Request::ReportAgentActivity {
            session_id,
            state,
            source: _,
        } => report_activity(daemon, session_id, state),
        Request::WatchInbox { .. } => Ok(Response::Ack),
        _other if !daemon_enabled(daemon) => Err(policy(RailRefusal {
            rail: "disabled",
            message: "orchestration is disabled in config".into(),
        })),
        Request::CreateRun {
            project_id,
            objective,
            controller,
            integration,
            base,
            parent_attempt_id,
            caller_session_id,
            ..
        } => create_run(
            daemon,
            caller_session_id,
            project_id,
            objective,
            controller,
            integration,
            base,
            parent_attempt_id,
        ),
        Request::ListRuns {
            project_id,
            active_only,
        } => list_runs(daemon, project_id, active_only),
        Request::GetRun { run_id } => Ok(Response::RunView(Box::new(run_view(daemon, run_id)?))),
        Request::UpdateRunBrief {
            caller_session_id,
            run_id,
            brief,
            expected_version,
            ..
        } => update_brief(daemon, caller_session_id, run_id, brief, expected_version),
        Request::CloseRun {
            caller_session_id,
            run_id,
            cancel,
            cleanup,
            pull_request,
            ..
        } => close_run(
            daemon,
            caller_session_id,
            run_id,
            cancel,
            cleanup,
            pull_request,
        ),
        Request::ResumeRun {
            caller_session_id,
            run_id,
            provider_id,
            profile_id,
            ..
        } => resume_run(daemon, caller_session_id, run_id, provider_id, profile_id),
        Request::CreateTask {
            caller_session_id,
            run_id,
            title,
            spec,
            acceptance,
            after,
            mode,
            allow_subruns,
            ..
        } => create_task(
            daemon,
            caller_session_id,
            run_id,
            title,
            spec,
            acceptance,
            after,
            mode,
            allow_subruns,
        ),
        Request::StartAttempt {
            caller_session_id,
            task_id,
            provider_id,
            profile_id,
            placement,
            branch,
            ..
        } => start_attempt(
            daemon,
            caller_session_id,
            task_id,
            provider_id,
            profile_id,
            placement,
            branch,
        ),
        Request::RunTask {
            caller_session_id,
            run_id,
            title,
            spec,
            acceptance,
            after,
            mode,
            allow_subruns,
            provider_id,
            profile_id,
            placement,
            branch,
            ..
        } => {
            let created = create_task(
                daemon,
                caller_session_id,
                run_id,
                title,
                spec,
                acceptance,
                after,
                mode,
                allow_subruns,
            )?;
            let Response::TaskCreated { task_id } = created else {
                return Err(internal("create task did not return an id"));
            };
            start_attempt(
                daemon,
                caller_session_id,
                task_id,
                provider_id,
                profile_id,
                placement,
                branch,
            )
        }
        Request::ReportAttempt {
            caller_session_id,
            attempt_id,
            outcome,
            summary,
            verification,
            result_file,
            ..
        } => report_attempt(
            daemon,
            caller_session_id,
            attempt_id,
            outcome,
            summary,
            verification,
            result_file,
        ),
        Request::DecideTask {
            caller_session_id,
            task_id,
            expected_revision,
            decision,
            retry_provider_id,
            retry_profile_id,
            ..
        } => decide_task(
            daemon,
            caller_session_id,
            task_id,
            expected_revision,
            decision,
            retry_provider_id,
            retry_profile_id,
        ),
        Request::IntegrateTask {
            caller_session_id,
            task_id,
            how,
            ..
        } => integrate_task(daemon, caller_session_id, task_id, how),
        Request::CleanupTask {
            caller_session_id,
            task_id,
            keep_worktree,
            ..
        } => cleanup_task(daemon, caller_session_id, task_id, keep_worktree),
        Request::ReviewTask { task_id, patch } => review_task(daemon, task_id, patch),
        Request::PostMessage {
            caller_session_id,
            run_id,
            address,
            kind,
            body,
            in_reply_to,
            ..
        } => post_message(
            daemon,
            caller_session_id,
            run_id,
            &address,
            kind,
            body,
            in_reply_to,
        ),
        Request::ReadInbox {
            session_id,
            run_id,
            unread_only,
            limit,
            ack,
        } => read_inbox(daemon, session_id, run_id, unread_only, limit, ack),
        Request::ListRunState { run_id, prefix } => list_state(daemon, run_id, prefix.as_deref()),
        Request::GetRunState { run_id, key } => get_state(daemon, run_id, &key),
        Request::SetRunState {
            caller_session_id,
            run_id,
            key,
            value_json,
            expected_version,
            ..
        } => set_state(
            daemon,
            caller_session_id,
            run_id,
            &key,
            value_json,
            expected_version,
            false,
        ),
        Request::DeleteRunState {
            caller_session_id,
            run_id,
            key,
            expected_version,
            ..
        } => set_state(
            daemon,
            caller_session_id,
            run_id,
            &key,
            String::new(),
            expected_version,
            true,
        ),
        _ => Err(ProtocolError::invalid_request(
            "not an orchestration request",
        )),
    }
}

fn daemon_enabled(daemon: &Daemon) -> bool {
    daemon.config_orchestration().enabled
}

fn status(daemon: &Daemon) -> Result<Response, ProtocolError> {
    let cfg = daemon.config_orchestration();
    Ok(Response::OrchestrationStatus(OrchestrationLimitsView {
        enabled: cfg.enabled,
        max_active_attempts_per_run: cfg.max_active_attempts_per_run,
        max_tasks_per_run: cfg.max_tasks_per_run,
        max_run_depth: cfg.max_run_depth,
        max_attempts_per_task: cfg.max_attempts_per_task,
        stall_after_secs: cfg.stall_after_secs,
        agent_may_integrate: cfg.agent_may_integrate.clone(),
    }))
}

fn report_activity(
    daemon: &Daemon,
    session_id: SessionId,
    state: ActivityState,
) -> Result<Response, ProtocolError> {
    let mut inner = daemon.lock();
    let Some(current) = inner
        .sessions
        .get(&session_id)
        .map(|session| session.activity.clone())
    else {
        return Ok(Response::Ack);
    };
    let next = apply_activity(&current, ActivityTrigger::Hook(state), Timestamp::now());
    let became_idle = next.state == ActivityState::Idle && current.state != ActivityState::Idle;
    let became_working =
        next.state == ActivityState::Working && current.state != ActivityState::Working;
    if let Some(session) = inner.sessions.get_mut(&session_id) {
        session.activity = next;
    }
    if became_working {
        inner
            .ledger
            .pointer
            .entry(session_id)
            .or_default()
            .typed_this_idle = false;
    }
    let snapshot = inner.sessions.get(&session_id).cloned();
    let queued = inner
        .ledger
        .pointer
        .get(&session_id)
        .is_some_and(|slot| slot.queued);
    drop(inner);
    if let Some(snapshot) = snapshot {
        daemon
            .registry
            .broadcast_domain(protocol::DaemonEvent::SessionUpdated(snapshot));
    }
    if became_idle && queued {
        deliver::deliver_pointer(daemon, session_id);
    }
    Ok(Response::Ack)
}

/// Keystrokes after Idle count as Working. Silence does not come through here.
pub fn note_user_input(daemon: &Daemon, session_id: SessionId) {
    let mut inner = daemon.lock();
    let Some(current) = inner
        .sessions
        .get(&session_id)
        .map(|session| session.activity.clone())
    else {
        return;
    };
    let next = apply_activity(&current, ActivityTrigger::Input, Timestamp::now());
    if next.state == current.state {
        return;
    }
    if let Some(session) = inner.sessions.get_mut(&session_id) {
        session.activity = next;
    }
    if let Some(slot) = inner.ledger.pointer.get_mut(&session_id) {
        slot.typed_this_idle = false;
    }
    let snapshot = inner.sessions.get(&session_id).cloned();
    drop(inner);
    if let Some(snapshot) = snapshot {
        daemon
            .registry
            .broadcast_domain(protocol::DaemonEvent::SessionUpdated(snapshot));
    }
}

pub fn note_session_exit(daemon: &Daemon, session_id: SessionId) {
    let limits = daemon.config_orchestration().limits();
    let now = Timestamp::now();
    let mut inner = daemon.lock();
    let Some(attempt_id) = inner
        .ledger
        .attempts
        .values()
        .find(|attempt| attempt.session_id == Some(session_id) && attempt.is_live())
        .map(|attempt| attempt.id)
    else {
        return;
    };
    let Some(attempt) = inner.ledger.attempts.get_mut(&attempt_id) else {
        return;
    };
    attempt.phase = AttemptPhase::Lost;
    attempt.lost_reason = Some(LostReason::ExitedWithoutReport);
    attempt.settled_at = Some(now);
    let attempt = attempt.clone();
    let task_id = attempt.task_id;
    if let Some(task) = inner.ledger.tasks.get_mut(&task_id) {
        if matches!(task.status, TaskStatus::Active | TaskStatus::Blocked) {
            task.status = task_status_after_loss(task.attempts_used, limits.max_attempts_per_task);
            task.updated_at = now;
            task.revision = task.revision.saturating_add(1);
        }
    }
    let task = inner.ledger.tasks.get(&task_id).cloned();
    if let Err(error) = persist_attempt(&inner, &attempt) {
        tracing::warn!(%error, "could not persist a lost attempt");
    }
    if let Some(task) = task.clone() {
        if let Err(error) = persist_task(&inner, &task) {
            tracing::warn!(%error, "could not persist a task after exit");
        }
    }
    drop(inner);
    daemon
        .registry
        .broadcast_domain(protocol::DaemonEvent::AttemptUpdated(attempt));
    if let Some(task) = task {
        daemon
            .registry
            .broadcast_domain(protocol::DaemonEvent::TaskUpdated(task));
    }
}

// Fields of `CreateRun`, kept flat so the handler matches the request.
#[allow(clippy::too_many_arguments)]
fn create_run(
    daemon: &Arc<Daemon>,
    caller: Option<SessionId>,
    project_id: ProjectId,
    objective: String,
    controller: ControllerSpec,
    integration: IntegrationPlacement,
    base: Option<String>,
    parent_attempt_id: Option<AttemptId>,
) -> Result<Response, ProtocolError> {
    let (objective, _) = domain::orchestration::clamp_text(&objective, MAX_OBJECTIVE_BYTES);
    if objective.trim().is_empty() {
        return Err(ProtocolError::invalid_request("objective is empty"));
    }
    let limits = daemon.config_orchestration().limits();
    let parent = {
        let inner = daemon.lock();
        if !inner.projects.contains_key(&project_id) {
            return Err(ProtocolError::not_found("project"));
        }
        parent_attempt_id.map(|id| -> Result<_, ProtocolError> {
            let attempt = inner
                .ledger
                .attempts
                .get(&id)
                .ok_or_else(|| ProtocolError::not_found("parent attempt"))?;
            let task = inner
                .ledger
                .tasks
                .get(&attempt.task_id)
                .ok_or_else(|| ProtocolError::not_found("parent task"))?;
            let parent_run = inner
                .ledger
                .runs
                .get(&task.run_id)
                .ok_or_else(|| ProtocolError::not_found("parent run"))?;
            allow_nested_run(run_depth(&inner, parent_run), task.allow_subruns, &limits)
                .map_err(policy)?;
            Ok(attempt.session_id)
        })
    };
    let parent_session = match parent {
        Some(result) => Some(result?),
        None => None,
    };
    let now = Timestamp::now();
    let run_id = RunId::new();
    let (workspace_id, branch_base) = place_integration(
        daemon,
        project_id,
        &objective,
        &integration,
        base.as_deref(),
        caller,
    )?;
    let mut controller_session = match &controller {
        ControllerSpec::None => None,
        ControllerSpec::SelfSession => caller,
        ControllerSpec::Agent { .. } => None,
        _ => return Err(ProtocolError::invalid_request("controller spec")),
    };
    if matches!(controller, ControllerSpec::Agent { .. }) {
        let ControllerSpec::Agent {
            provider_id,
            profile_id,
        } = controller.clone()
        else {
            unreachable!();
        };
        let workspace = workspace_id.ok_or_else(|| {
            ProtocolError::precondition_failed("an agent controller needs an integration workspace")
        })?;
        let prompt = controller_prompt(daemon, run_id, &objective, branch_base.as_deref(), false);
        set_spawn_env(vec![
            ("FORGE_RUN_ID".into(), run_id.to_string()),
            ("FORGECTL_JSON".into(), "1".into()),
        ]);
        let created = daemon.create_session(
            workspace,
            SessionKind::Agent,
            Some(provider_id),
            profile_id,
            parent_session.flatten(),
            SessionRole::Orchestrator,
            None,
            Some(prompt),
            false,
        )?;
        controller_session = Some(session_of(&created)?);
    }
    let run = Run {
        id: run_id,
        project_id,
        objective,
        brief: String::new(),
        brief_version: 0,
        controller_session_id: controller_session,
        integration_workspace_id: workspace_id,
        base: branch_base,
        parent_attempt_id,
        status: RunStatus::Active,
        revision: 1,
        created_at: now,
        updated_at: now,
        closed_at: None,
    };
    {
        let mut inner = daemon.lock();
        inner.db.orchestration().insert_run(&run).map_err(db_err)?;
        inner.ledger.runs.insert(run.id, run.clone());
    }
    daemon
        .registry
        .broadcast_domain(protocol::DaemonEvent::RunUpdated(run.clone()));
    Ok(Response::RunCreated {
        run_id: run.id,
        controller_session_id: run.controller_session_id,
        integration_workspace_id: run.integration_workspace_id,
    })
}

fn list_runs(
    daemon: &Daemon,
    project_id: Option<ProjectId>,
    active_only: bool,
) -> Result<Response, ProtocolError> {
    let limits = daemon.config_orchestration().limits();
    let inner = daemon.lock();
    let mut views = Vec::new();
    for run in inner.ledger.runs.values() {
        if project_id.is_some_and(|id| run.project_id != id) {
            continue;
        }
        if active_only && !run.status.is_open() {
            continue;
        }
        views.push(view_locked(&inner, run.id, &limits)?);
    }
    Ok(Response::Runs(views))
}

fn run_view(daemon: &Daemon, run_id: RunId) -> Result<RunViewOwned, ProtocolError> {
    let limits = daemon.config_orchestration().limits();
    let inner = daemon.lock();
    view_locked(&inner, run_id, &limits)
}

type RunViewOwned = domain::orchestration::RunView;

fn view_locked(
    inner: &Inner,
    run_id: RunId,
    limits: &OrchestrationLimits,
) -> Result<RunViewOwned, ProtocolError> {
    let run = inner
        .ledger
        .runs
        .get(&run_id)
        .cloned()
        .ok_or_else(|| ProtocolError::not_found("run"))?;
    let mut tasks: Vec<Task> = inner
        .ledger
        .tasks
        .values()
        .filter(|task| task.run_id == run_id)
        .cloned()
        .collect();
    tasks.sort_by_key(|task| task.created_at);
    let attempts: Vec<Attempt> = inner
        .ledger
        .attempts
        .values()
        .filter(|attempt| tasks.iter().any(|task| task.id == attempt.task_id))
        .cloned()
        .collect();
    let questions = questions_for(inner, run_id)?;
    let facts = attempt_facts(inner, &attempts);
    let items = attention(
        &questions,
        &facts,
        inner.ledger.conflict.contains(&run_id),
        Timestamp::now(),
        limits,
    );
    let mut board_keys: Vec<String> = inner
        .ledger
        .board
        .keys()
        .filter(|(id, _)| *id == run_id)
        .map(|(_, key)| key.clone())
        .collect();
    board_keys.sort();
    Ok(domain::orchestration::RunView {
        run,
        tasks,
        attempts,
        attention: items,
        board_keys,
    })
}

fn update_brief(
    daemon: &Daemon,
    caller: Option<SessionId>,
    run_id: RunId,
    brief: String,
    expected_version: u64,
) -> Result<Response, ProtocolError> {
    let (brief, _) = domain::orchestration::clamp_text(&brief, MAX_BRIEF_BYTES);
    let mut inner = daemon.lock();
    let run = inner
        .ledger
        .runs
        .get_mut(&run_id)
        .ok_or_else(|| ProtocolError::not_found("run"))?;
    allow_controller(caller, run.controller_session_id).map_err(policy)?;
    match compare_and_set(
        Some(run.brief_version).filter(|v| *v > 0).or(Some(0)),
        expected_version,
    ) {
        CasOutcome::Mismatch { current } if !(expected_version == 0 && run.brief_version == 0) => {
            // Version 0 creates the first brief. A later 0 is a mismatch.
            if expected_version != run.brief_version {
                return Err(version_conflict(if run.brief_version == 0 {
                    0
                } else {
                    current.max(run.brief_version)
                }));
            }
        }
        _ => {}
    }
    if expected_version != run.brief_version && !(expected_version == 0 && run.brief_version == 0) {
        return Err(version_conflict(run.brief_version));
    }
    run.brief = brief;
    run.brief_version = run.brief_version.saturating_add(1u64);
    run.revision = run.revision.saturating_add(1);
    run.updated_at = Timestamp::now();
    let run = run.clone();
    inner.db.orchestration().update_run(&run).map_err(db_err)?;
    drop(inner);
    daemon
        .registry
        .broadcast_domain(protocol::DaemonEvent::RunUpdated(run));
    Ok(Response::Ack)
}

// Fields of `CreateTask`, kept flat so the handler matches the request.
#[allow(clippy::too_many_arguments)]
fn create_task(
    daemon: &Daemon,
    caller: Option<SessionId>,
    run_id: RunId,
    title: String,
    spec: String,
    acceptance: String,
    after: Vec<TaskId>,
    mode: WorkMode,
    allow_subruns: bool,
) -> Result<Response, ProtocolError> {
    let (title, _) = domain::orchestration::clamp_text(&title, MAX_TITLE_BYTES);
    let (spec, _) = domain::orchestration::clamp_text(&spec, MAX_SPEC_BYTES);
    let (acceptance, _) = domain::orchestration::clamp_text(&acceptance, MAX_ACCEPTANCE_BYTES);
    if title.trim().is_empty() {
        return Err(ProtocolError::invalid_request("title is empty"));
    }
    let limits = daemon.config_orchestration().limits();
    let mut inner = daemon.lock();
    let run = inner
        .ledger
        .runs
        .get(&run_id)
        .ok_or_else(|| ProtocolError::not_found("run"))?
        .clone();
    allow_controller(caller, run.controller_session_id).map_err(policy)?;
    if !run.status.is_open() {
        return Err(ProtocolError::precondition_failed("run is not open"));
    }
    let existing = inner
        .ledger
        .tasks
        .values()
        .filter(|task| task.run_id == run_id)
        .count();
    allow_new_task(existing, &limits).map_err(policy)?;
    for dep in &after {
        let other = inner
            .ledger
            .tasks
            .get(dep)
            .ok_or_else(|| ProtocolError::not_found("dependency"))?;
        if other.run_id != run_id {
            return Err(ProtocolError::invalid_request(
                "dependencies must be tasks in the same run",
            ));
        }
    }
    let id = TaskId::new();
    let edges: Vec<(TaskId, TaskId)> = inner
        .ledger
        .tasks
        .values()
        .filter(|task| task.run_id == run_id)
        .flat_map(|task| task.after.iter().map(|after| (task.id, *after)))
        .collect();
    if dependency_cycle(&edges, id, &after) {
        return Err(policy(RailRefusal {
            rail: "cycle",
            message: "task dependencies would cycle".into(),
        }));
    }
    let now = Timestamp::now();
    let mut task = Task {
        id,
        run_id,
        title,
        spec,
        acceptance,
        after,
        mode,
        allow_subruns,
        status: TaskStatus::Pending,
        attempts_used: 0,
        revision: 1,
        created_at: now,
        updated_at: now,
        feedback: None,
    };
    let mut tasks: Vec<Task> = inner
        .ledger
        .tasks
        .values()
        .filter(|task| task.run_id == run_id)
        .cloned()
        .chain(std::iter::once(task.clone()))
        .collect();
    promote_ready(&mut tasks);
    if let Some(updated) = tasks.into_iter().find(|item| item.id == id) {
        task = updated;
    }
    inner
        .db
        .orchestration()
        .insert_task(&task)
        .map_err(db_err)?;
    inner.ledger.tasks.insert(task.id, task.clone());
    drop(inner);
    daemon
        .registry
        .broadcast_domain(protocol::DaemonEvent::TaskUpdated(task.clone()));
    Ok(Response::TaskCreated { task_id: task.id })
}

/// `n` for a start that is still allowed. The row is the one under the lock,
/// not the snapshot taken before the worktree and the PTY existed.
fn refuse_stale_start(
    inner: &Inner,
    run_id: RunId,
    task_id: TaskId,
    workspace: WorkspaceId,
    limits: &OrchestrationLimits,
) -> Result<u32, ProtocolError> {
    let (status, attempts_used, after, mode) = {
        let task = inner
            .ledger
            .tasks
            .get(&task_id)
            .ok_or_else(|| internal("task vanished"))?;
        (
            task.status,
            task.attempts_used,
            task.after.clone(),
            task.mode,
        )
    };
    if !matches!(status, TaskStatus::Ready | TaskStatus::Pending) {
        return Err(ProtocolError::precondition_failed(format!(
            "task is {status:?}, not ready"
        )));
    }
    if !dependencies_met(&after, |id| {
        inner.ledger.tasks.get(&id).map(|task| task.status)
    }) {
        return Err(ProtocolError::precondition_failed(
            "dependencies are not accepted or integrated",
        ));
    }
    let active = inner
        .ledger
        .attempts
        .values()
        .filter(|attempt| {
            attempt.is_live()
                && inner
                    .ledger
                    .tasks
                    .get(&attempt.task_id)
                    .is_some_and(|task| task.run_id == run_id)
        })
        .count();
    let writer = inner.ledger.attempts.values().any(|attempt| {
        attempt.is_live() && !attempt.read_only && attempt.workspace_id == Some(workspace)
    });
    allow_start(active, attempts_used, mode, writer, limits).map_err(policy)?;
    Ok(attempts_used.saturating_add(1))
}

fn abandon_uncommitted_start(
    daemon: &Arc<Daemon>,
    session_id: SessionId,
    created_workspace: Option<WorkspaceId>,
) {
    if let Some(workspace_id) = created_workspace {
        // The session is still live, so a non-force remove would refuse.
        // This checkout never became an attempt.
        let _ = daemon.remove_worktree(workspace_id, true);
        return;
    }
    let _ = daemon.kill_session(session_id);
    let _ = daemon.close_session(session_id);
}

fn start_attempt(
    daemon: &Arc<Daemon>,
    caller: Option<SessionId>,
    task_id: TaskId,
    provider_id: AgentProviderId,
    profile_id: Option<domain::AgentProfileId>,
    placement: AttemptPlacement,
    branch: Option<String>,
) -> Result<Response, ProtocolError> {
    let limits = daemon.config_orchestration().limits();
    let plan = {
        let inner = daemon.lock();
        let task = inner
            .ledger
            .tasks
            .get(&task_id)
            .cloned()
            .ok_or_else(|| ProtocolError::not_found("task"))?;
        let run = inner
            .ledger
            .runs
            .get(&task.run_id)
            .cloned()
            .ok_or_else(|| ProtocolError::not_found("run"))?;
        allow_controller(caller, run.controller_session_id).map_err(policy)?;
        if !matches!(task.status, TaskStatus::Ready | TaskStatus::Pending) {
            return Err(ProtocolError::precondition_failed(format!(
                "task is {:?}, not ready",
                task.status
            )));
        }
        if !dependencies_met(&task.after, |id| {
            inner.ledger.tasks.get(&id).map(|task| task.status)
        }) {
            return Err(ProtocolError::precondition_failed(
                "dependencies are not accepted or integrated",
            ));
        }
        let active = inner
            .ledger
            .attempts
            .values()
            .filter(|attempt| {
                attempt.is_live()
                    && inner
                        .ledger
                        .tasks
                        .get(&attempt.task_id)
                        .is_some_and(|task| task.run_id == run.id)
            })
            .count();
        let workspace = resolve_workspace(&inner, &run, &placement, caller)?;
        let writer = inner.ledger.attempts.values().any(|attempt| {
            attempt.is_live() && !attempt.read_only && attempt.workspace_id == Some(workspace)
        });
        allow_start(active, task.attempts_used, task.mode, writer, &limits).map_err(policy)?;
        (task, run, workspace)
    };
    let (task, run, workspace_hint) = plan;
    // A later attempt cannot check out the branch the previous worktree still
    // holds, so the default name includes the attempt number.
    let n = task.attempts_used.saturating_add(1);
    let generated = (branch.is_none()
        && matches!(placement, AttemptPlacement::Worktree)
        && task.mode != WorkMode::ReadOnly)
        .then(|| {
            let slug = git_service::slugify(&task.title);
            format!(
                "forge/task-{}-{n}",
                if slug.is_empty() {
                    "task"
                } else {
                    slug.as_str()
                }
            )
        });
    let (workspace, branch_name, base_commit) = materialize_placement(
        daemon,
        &run,
        &task,
        workspace_hint,
        &placement,
        branch.as_deref().or(generated.as_deref()),
    )?;
    let attempt_id = AttemptId::new();
    let dep_owned: Vec<(TaskId, String, String)> = {
        let inner = daemon.lock();
        let tasks: Vec<Task> = inner.ledger.tasks.values().cloned().collect();
        let attempts: Vec<Attempt> = inner.ledger.attempts.values().cloned().collect();
        accepted_dependency_summaries(&task.after, &tasks, &attempts)
            .into_iter()
            .map(|dep| (dep.task_id, dep.title.to_owned(), dep.summary.to_owned()))
            .collect()
    };
    let dep_refs: Vec<DependencySummary<'_>> = dep_owned
        .iter()
        .map(|(id, title, summary)| DependencySummary {
            task_id: *id,
            title,
            summary,
        })
        .collect();
    let path = daemon.workspace_path(workspace)?;
    let prompt = compose_worker_prompt(&WorkerPromptInput {
        run_id: run.id,
        task_id: task.id,
        attempt_n: n,
        title: &task.title,
        objective: &run.objective,
        spec: &task.spec,
        acceptance: &task.acceptance,
        brief: &run.brief,
        brief_version: run.brief_version,
        workspace_path: &path.to_string_lossy(),
        branch: branch_name.as_deref().unwrap_or(""),
        base_short: base_commit.as_deref().unwrap_or(""),
        dependencies: &dep_refs,
        feedback: task.feedback.as_deref(),
        previous_report: previous_summary(daemon, task.id).as_deref(),
    });
    let read_only = task.mode == WorkMode::ReadOnly;
    set_spawn_env(vec![
        ("FORGE_RUN_ID".into(), run.id.to_string()),
        ("FORGE_TASK_ID".into(), task.id.to_string()),
        ("FORGE_ATTEMPT_ID".into(), attempt_id.to_string()),
        ("FORGECTL_JSON".into(), "1".into()),
    ]);
    let parent = run.controller_session_id;
    let created = daemon.create_session(
        workspace,
        SessionKind::Agent,
        Some(provider_id.clone()),
        profile_id,
        parent,
        if read_only {
            SessionRole::Reviewer
        } else {
            SessionRole::Executor
        },
        None,
        Some(prompt),
        read_only,
    )?;
    let (session_id, terminal_id) = session_and_terminal(&created)?;
    let now = Timestamp::now();
    // A worktree this call added is not the integration checkout. The refusal
    // path removes that checkout; the integration workspace stays.
    let created_workspace = (workspace != workspace_hint).then_some(workspace);
    let mut attempt = Attempt {
        id: attempt_id,
        task_id: task.id,
        n,
        session_id: Some(session_id),
        workspace_id: Some(workspace),
        branch: branch_name.clone(),
        base_commit: base_commit.clone(),
        provider_id,
        profile_id,
        read_only,
        phase: AttemptPhase::Running,
        outcome: None,
        lost_reason: None,
        report: None,
        integrated_commit: None,
        created_at: now,
        settled_at: None,
    };
    {
        let mut inner = daemon.lock();
        // PTY and worktree work ran after the snapshot. Another start may have
        // committed; only a task that is still ready may proceed, and `n`
        // comes from that row rather than the snapshot.
        let n = match refuse_stale_start(&inner, run.id, task.id, workspace, &limits) {
            Ok(n) => n,
            Err(error) => {
                drop(inner);
                abandon_uncommitted_start(daemon, session_id, created_workspace);
                return Err(error);
            }
        };
        attempt.n = n;
        if let Some(session) = inner.sessions.get_mut(&session_id) {
            if session.base_commit.is_none() {
                session.base_commit = base_commit.clone();
            }
        }
        let Some(task_mut) = inner.ledger.tasks.get_mut(&task.id) else {
            drop(inner);
            abandon_uncommitted_start(daemon, session_id, created_workspace);
            return Err(internal("task vanished"));
        };
        task_mut.status = TaskStatus::Active;
        task_mut.attempts_used = n;
        task_mut.revision = task_mut.revision.saturating_add(1);
        task_mut.updated_at = now;
        let task_saved = task_mut.clone();
        inner
            .db
            .orchestration()
            .insert_attempt(&attempt)
            .map_err(db_err)?;
        inner
            .db
            .orchestration()
            .update_task(&task_saved)
            .map_err(db_err)?;
        inner.ledger.attempts.insert(attempt.id, attempt.clone());
        drop(task_saved);
    }
    let task_saved = {
        let inner = daemon.lock();
        inner.ledger.tasks.get(&task.id).cloned()
    };
    daemon
        .registry
        .broadcast_domain(protocol::DaemonEvent::AttemptUpdated(attempt.clone()));
    if let Some(task_saved) = task_saved {
        daemon
            .registry
            .broadcast_domain(protocol::DaemonEvent::TaskUpdated(task_saved));
    }
    Ok(Response::AttemptStarted {
        attempt_id: attempt.id,
        task_id: task.id,
        n: attempt.n,
        session_id,
        terminal_id,
        workspace_id: workspace,
        branch: branch_name,
        base_commit,
    })
}

fn previous_summary(daemon: &Daemon, task_id: TaskId) -> Option<String> {
    let inner = daemon.lock();
    inner
        .ledger
        .attempts
        .values()
        .filter(|attempt| attempt.task_id == task_id)
        .max_by_key(|attempt| attempt.n)
        .and_then(|attempt| attempt.report.as_ref().map(|report| report.summary.clone()))
}

fn report_attempt(
    daemon: &Arc<Daemon>,
    caller: Option<SessionId>,
    attempt_id: AttemptId,
    outcome: ReportOutcome,
    summary: String,
    verification: Option<String>,
    result_file: Option<String>,
) -> Result<Response, ProtocolError> {
    let (summary, _) = domain::orchestration::clamp_text(&summary, MAX_SUMMARY_BYTES);
    let verification =
        verification.map(|text| domain::orchestration::clamp_text(&text, MAX_VERIFICATION_BYTES).0);
    if summary.trim().is_empty() {
        return Err(ProtocolError::invalid_request("summary is empty"));
    }
    let (path, base, attempt_session, task_id, attempts_used) = {
        let inner = daemon.lock();
        let attempt = inner
            .ledger
            .attempts
            .get(&attempt_id)
            .ok_or_else(|| ProtocolError::not_found("attempt"))?;
        let newer = inner
            .ledger
            .attempts
            .values()
            .any(|other| other.task_id == attempt.task_id && other.n > attempt.n);
        settle_report(attempt.phase, attempt.session_id, caller, newer).map_err(report_err)?;
        let task = inner
            .ledger
            .tasks
            .get(&attempt.task_id)
            .ok_or_else(|| ProtocolError::not_found("task"))?;
        let path = attempt
            .workspace_id
            .and_then(|id| inner.workspaces.get(&id).map(|ws| ws.path.clone()));
        (
            path,
            attempt.base_commit.clone(),
            attempt.session_id,
            task.id,
            task.attempts_used,
        )
    };
    let _ = attempt_session;
    let copied = if let Some(file) = result_file.as_deref() {
        Some(copy_result(daemon, attempt_id, file)?)
    } else {
        None
    };
    let (head, dirty, files) = git_facts(path.as_deref(), base.as_deref());
    let limits = daemon.config_orchestration().limits();
    let now = Timestamp::now();
    let mut inner = daemon.lock();
    let (phase, attempt_session, task_of_attempt, n_of_attempt) = {
        let attempt = inner
            .ledger
            .attempts
            .get(&attempt_id)
            .ok_or_else(|| ProtocolError::not_found("attempt"))?;
        (
            attempt.phase,
            attempt.session_id,
            attempt.task_id,
            attempt.n,
        )
    };
    let newer = inner
        .ledger
        .attempts
        .values()
        .any(|other| other.task_id == task_of_attempt && other.n > n_of_attempt);
    settle_report(phase, attempt_session, caller, newer).map_err(report_err)?;
    let attempt = inner
        .ledger
        .attempts
        .get_mut(&attempt_id)
        .ok_or_else(|| ProtocolError::not_found("attempt"))?;
    attempt.phase = AttemptPhase::Reported;
    attempt.outcome = Some(outcome);
    attempt.settled_at = Some(now);
    attempt.report = Some(domain::orchestration::AttemptReport {
        outcome,
        summary,
        verification,
        result_path: copied,
        reported_head: head,
        dirty_at_report: dirty,
        files_changed: files,
    });
    let attempt = attempt.clone();
    let task = inner
        .ledger
        .tasks
        .get_mut(&task_id)
        .ok_or_else(|| ProtocolError::not_found("task"))?;
    task.status = task_status_after_report(outcome, attempts_used, limits.max_attempts_per_task);
    task.updated_at = now;
    task.revision = task.revision.saturating_add(1);
    let task = task.clone();
    persist_attempt(&inner, &attempt)?;
    persist_task(&inner, &task)?;
    drop(inner);
    daemon
        .registry
        .broadcast_domain(protocol::DaemonEvent::AttemptUpdated(attempt));
    daemon
        .registry
        .broadcast_domain(protocol::DaemonEvent::TaskUpdated(task));
    Ok(Response::Ack)
}

fn decide_task(
    daemon: &Arc<Daemon>,
    caller: Option<SessionId>,
    task_id: TaskId,
    expected_revision: u64,
    decision: TaskDecision,
    retry_provider: Option<AgentProviderId>,
    retry_profile: Option<domain::AgentProfileId>,
) -> Result<Response, ProtocolError> {
    let limits = daemon.config_orchestration().limits();
    let now = Timestamp::now();
    let prepared = {
        let inner = daemon.lock();
        let task = inner
            .ledger
            .tasks
            .get(&task_id)
            .ok_or_else(|| ProtocolError::not_found("task"))?;
        let run = inner
            .ledger
            .runs
            .get(&task.run_id)
            .ok_or_else(|| ProtocolError::not_found("run"))?;
        allow_controller(caller, run.controller_session_id).map_err(policy)?;
        if task.revision != expected_revision {
            return Err(version_conflict(task.revision));
        }
        match &decision {
            TaskDecision::Accept { .. } | TaskDecision::Reject { .. } => {
                if task.status != TaskStatus::Review && task.status != TaskStatus::Blocked {
                    return Err(ProtocolError::precondition_failed(
                        "task is not awaiting a decision",
                    ));
                }
            }
            TaskDecision::Cancel { .. } => {
                if task.status.is_final() {
                    return Err(ProtocolError::precondition_failed("task is already final"));
                }
            }
            _ => return Err(ProtocolError::invalid_request("decision")),
        }
        Ok::<_, ProtocolError>(())
    };
    prepared?;
    match decision {
        TaskDecision::Accept { note: _ } => {
            let mut inner = daemon.lock();
            let task = inner.ledger.tasks.get_mut(&task_id).unwrap();
            task.status = TaskStatus::Accepted;
            task.updated_at = now;
            task.revision = task.revision.saturating_add(1);
            let run_id = task.run_id;
            let mut tasks: Vec<Task> = inner
                .ledger
                .tasks
                .values()
                .filter(|task| task.run_id == run_id)
                .cloned()
                .collect();
            promote_ready(&mut tasks);
            for task in &tasks {
                inner.db.orchestration().update_task(task).map_err(db_err)?;
                inner.ledger.tasks.insert(task.id, task.clone());
            }
            let tasks = tasks;
            drop(inner);
            for task in tasks {
                daemon
                    .registry
                    .broadcast_domain(protocol::DaemonEvent::TaskUpdated(task));
            }
            Ok(Response::Ack)
        }
        TaskDecision::Reject { feedback, retry } => {
            let (feedback, _) = domain::orchestration::clamp_text(&feedback, MAX_MESSAGE_BYTES);
            if !retry {
                let mut inner = daemon.lock();
                let task = inner.ledger.tasks.get_mut(&task_id).unwrap();
                task.feedback = Some(feedback.clone());
                task.status = TaskStatus::Active;
                task.updated_at = now;
                task.revision = task.revision.saturating_add(1);
                let task = task.clone();
                let attempt = inner
                    .ledger
                    .attempts
                    .values_mut()
                    .filter(|attempt| attempt.task_id == task_id)
                    .max_by_key(|attempt| attempt.n)
                    .map(|attempt| {
                        attempt.phase = AttemptPhase::Running;
                        attempt.outcome = None;
                        attempt.settled_at = None;
                        attempt.clone()
                    });
                persist_task(&inner, &task)?;
                if let Some(attempt) = attempt.clone() {
                    persist_attempt(&inner, &attempt)?;
                }
                let session = attempt.as_ref().and_then(|attempt| attempt.session_id);
                let run_id = task.run_id;
                drop(inner);
                if let Some(session) = session {
                    let _ = post_message(
                        daemon,
                        caller,
                        run_id,
                        &format!("session:{session}"),
                        ContextKind::Feedback,
                        feedback,
                        None,
                    );
                }
                daemon
                    .registry
                    .broadcast_domain(protocol::DaemonEvent::TaskUpdated(task));
                if let Some(attempt) = attempt {
                    daemon
                        .registry
                        .broadcast_domain(protocol::DaemonEvent::AttemptUpdated(attempt));
                }
                return Ok(Response::Ack);
            }
            let (provider, profile, placement) = {
                let mut inner = daemon.lock();
                let task = inner.ledger.tasks.get_mut(&task_id).unwrap();
                if task.attempts_used >= limits.max_attempts_per_task {
                    task.status = TaskStatus::Failed;
                    task.updated_at = now;
                    let task = task.clone();
                    persist_task(&inner, &task)?;
                    drop(inner);
                    daemon
                        .registry
                        .broadcast_domain(protocol::DaemonEvent::TaskUpdated(task));
                    return Err(policy(RailRefusal {
                        rail: "max_attempts",
                        message: "attempt budget is spent".into(),
                    }));
                }
                task.feedback = Some(feedback);
                task.status = TaskStatus::Ready;
                task.updated_at = now;
                task.revision = task.revision.saturating_add(1);
                let task = task.clone();
                let previous = inner
                    .ledger
                    .attempts
                    .values_mut()
                    .filter(|attempt| attempt.task_id == task_id)
                    .max_by_key(|attempt| attempt.n)
                    .map(|attempt| {
                        attempt.phase = AttemptPhase::Rejected;
                        attempt.settled_at = Some(now);
                        attempt.clone()
                    });
                persist_task(&inner, &task)?;
                if let Some(previous) = previous.clone() {
                    persist_attempt(&inner, &previous)?;
                }
                let provider = retry_provider
                    .or_else(|| previous.as_ref().map(|attempt| attempt.provider_id.clone()))
                    .ok_or_else(|| ProtocolError::invalid_request("retry needs a provider"))?;
                let profile = retry_profile
                    .or_else(|| previous.as_ref().and_then(|attempt| attempt.profile_id));
                let read_only = task.mode == WorkMode::ReadOnly;
                drop(previous);
                (
                    provider,
                    profile,
                    if read_only {
                        AttemptPlacement::Integration
                    } else {
                        AttemptPlacement::Worktree
                    },
                )
            };
            start_attempt(daemon, caller, task_id, provider, profile, placement, None)
        }
        TaskDecision::Cancel { kill } => {
            let mut inner = daemon.lock();
            let task = inner.ledger.tasks.get_mut(&task_id).unwrap();
            task.status = TaskStatus::Cancelled;
            task.updated_at = now;
            task.revision = task.revision.saturating_add(1);
            let task = task.clone();
            let mut attempts = Vec::new();
            for attempt in inner.ledger.attempts.values_mut() {
                if attempt.task_id == task_id && attempt.is_live() {
                    attempt.phase = AttemptPhase::Lost;
                    attempt.lost_reason = Some(LostReason::Cancelled);
                    attempt.settled_at = Some(now);
                    attempts.push(attempt.clone());
                }
            }
            persist_task(&inner, &task)?;
            for attempt in &attempts {
                persist_attempt(&inner, attempt)?;
            }
            let sessions: Vec<SessionId> = attempts.iter().filter_map(|a| a.session_id).collect();
            drop(inner);
            if kill {
                for session in sessions {
                    let _ = daemon.kill_session(session);
                }
            }
            daemon
                .registry
                .broadcast_domain(protocol::DaemonEvent::TaskUpdated(task));
            for attempt in attempts {
                daemon
                    .registry
                    .broadcast_domain(protocol::DaemonEvent::AttemptUpdated(attempt));
            }
            Ok(Response::Ack)
        }
        _ => Err(ProtocolError::invalid_request("decision")),
    }
}

fn integrate_task(
    daemon: &Arc<Daemon>,
    caller: Option<SessionId>,
    task_id: TaskId,
    how: IntegrateHow,
) -> Result<Response, ProtocolError> {
    let (run, branch, integration) = {
        let inner = daemon.lock();
        let task = inner
            .ledger
            .tasks
            .get(&task_id)
            .ok_or_else(|| ProtocolError::not_found("task"))?;
        let run = inner
            .ledger
            .runs
            .get(&task.run_id)
            .ok_or_else(|| ProtocolError::not_found("run"))?;
        allow_controller(caller, run.controller_session_id).map_err(policy)?;
        if caller.is_some() {
            allow_agent_integrate(
                false,
                daemon.config_orchestration().limits().agent_may_integrate,
            )
            .map_err(policy)?;
        }
        let integration = run.integration_workspace_id.ok_or_else(|| {
            ProtocolError::precondition_failed("run has no integration workspace")
        })?;
        let branch = inner
            .ledger
            .attempts
            .values()
            .filter(|attempt| attempt.task_id == task_id)
            .max_by_key(|attempt| attempt.n)
            .and_then(|attempt| attempt.branch.clone());
        (run.clone(), branch, integration)
    };
    let path = daemon.workspace_path(integration)?;
    match how {
        IntegrateHow::Continue => {
            let continued = git_service::continue_sequencer(&path).map_err(git_err)?;
            let finished = matches!(continued, git_service::Continued::Finished(_));
            if finished {
                let head = git_service::head_commit(&path);
                mark_integrated(daemon, task_id, head.clone())?;
                clear_conflict(daemon, run.id);
                return Ok(Response::IntegrationResult {
                    state: "integrated".into(),
                    integrated_commit: head,
                    conflicts: vec![],
                });
            }
            let conflicts = conflict_paths(&path);
            set_conflict(daemon, run.id);
            Ok(Response::IntegrationResult {
                state: "conflict".into(),
                integrated_commit: None,
                conflicts,
            })
        }
        IntegrateHow::Abort => {
            git_service::abort_sequencer(&path).map_err(git_err)?;
            clear_conflict(daemon, run.id);
            Ok(Response::IntegrationResult {
                state: "aborted".into(),
                integrated_commit: None,
                conflicts: vec![],
            })
        }
        IntegrateHow::Merge { squash } => {
            let task_status = daemon
                .lock()
                .ledger
                .tasks
                .get(&task_id)
                .map(|task| task.status);
            if task_status != Some(TaskStatus::Accepted) {
                return Err(ProtocolError::precondition_failed(
                    "only an accepted task can be integrated",
                ));
            }
            let branch = branch.ok_or_else(|| {
                ProtocolError::precondition_failed("attempt has no branch to merge")
            })?;
            let args: Vec<String> = if squash {
                vec![
                    "merge".into(),
                    "--squash".into(),
                    "--".into(),
                    branch.clone(),
                ]
            } else {
                vec![
                    "merge".into(),
                    "--no-ff".into(),
                    "--no-edit".into(),
                    "--".into(),
                    branch.clone(),
                ]
            };
            let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
            let merged = git_service::run_git(Some(&path), &arg_refs).map_err(git_err)?;
            if !merged.success() {
                let conflicts = conflict_paths(&path);
                set_conflict(daemon, run.id);
                return Ok(Response::IntegrationResult {
                    state: "conflict".into(),
                    integrated_commit: None,
                    conflicts,
                });
            }
            let head = if squash {
                Some(git_service::commit(&path, &format!("Integrate {branch}")).map_err(git_err)?)
            } else {
                git_service::head_commit(&path)
            };
            mark_integrated(daemon, task_id, head.clone())?;
            clear_conflict(daemon, run.id);
            Ok(Response::IntegrationResult {
                state: "integrated".into(),
                integrated_commit: head,
                conflicts: vec![],
            })
        }
        _ => Err(ProtocolError::invalid_request("integrate how")),
    }
}

fn cleanup_task(
    daemon: &Arc<Daemon>,
    caller: Option<SessionId>,
    task_id: TaskId,
    keep_worktree: bool,
) -> Result<Response, ProtocolError> {
    let (session, workspace, status, project) = {
        let inner = daemon.lock();
        let task = inner
            .ledger
            .tasks
            .get(&task_id)
            .ok_or_else(|| ProtocolError::not_found("task"))?;
        let run = inner
            .ledger
            .runs
            .get(&task.run_id)
            .ok_or_else(|| ProtocolError::not_found("run"))?;
        allow_controller(caller, run.controller_session_id).map_err(policy)?;
        let attempt = inner
            .ledger
            .attempts
            .values()
            .filter(|attempt| attempt.task_id == task_id)
            .max_by_key(|attempt| attempt.n)
            .cloned();
        (
            attempt.as_ref().and_then(|attempt| attempt.session_id),
            attempt.and_then(|attempt| attempt.workspace_id),
            task.status,
            run.project_id,
        )
    };
    let mut residual = Vec::new();
    if let Some(session) = session {
        let state = daemon
            .lock()
            .sessions
            .get(&session)
            .map(|session| session.state.clone());
        if matches!(state, Some(SessionState::Running | SessionState::Starting))
            && daemon.kill_session(session).is_err()
        {
            residual.push(format!("session {session} could not be stopped"));
        }
        if daemon.close_session(session).is_err() {
            residual.push(format!("session {session} could not be closed"));
        }
    }
    if !keep_worktree {
        if let Some(workspace) = workspace {
            let (path, managed, git_root) = {
                let inner = daemon.lock();
                let ws = inner.workspaces.get(&workspace).cloned();
                let root = inner
                    .projects
                    .get(&project)
                    .and_then(|project| project.git_root.clone());
                (
                    ws.as_ref().map(|ws| ws.path.clone()),
                    ws.as_ref().map(|ws| ws.managed_by_app).unwrap_or(false),
                    root,
                )
            };
            let removable = matches!(status, TaskStatus::Integrated | TaskStatus::Cancelled);
            if let (Some(path), Some(root)) = (path, git_root) {
                if !managed {
                    residual.push(format!("{} is not a managed worktree", path.display()));
                } else if !removable {
                    residual.push(format!("{} is not integrated or cancelled", path.display()));
                } else {
                    match git_service::precheck_remove(&root, &path) {
                        Ok(check) if check.dirty || check.merge_or_rebase_in_progress => {
                            residual.push(format!(
                                "{} is dirty or mid-merge and was kept",
                                path.display()
                            ));
                        }
                        Ok(_) => {
                            if let Err(error) = git_service::remove(&root, &path, false) {
                                residual.push(format!(
                                    "{} could not be removed: {error}",
                                    path.display()
                                ));
                            }
                        }
                        Err(error) => residual
                            .push(format!("{} could not be checked: {error}", path.display())),
                    }
                }
            }
        }
    }
    Ok(Response::CleanupResult { residual })
}

fn close_run(
    daemon: &Arc<Daemon>,
    caller: Option<SessionId>,
    run_id: RunId,
    cancel: bool,
    cleanup: bool,
    pull_request: Option<protocol::CloseRunPullRequest>,
) -> Result<Response, ProtocolError> {
    let (tasks, integration, open_pr) = {
        let inner = daemon.lock();
        let run = inner
            .ledger
            .runs
            .get(&run_id)
            .ok_or_else(|| ProtocolError::not_found("run"))?;
        allow_controller(caller, run.controller_session_id).map_err(policy)?;
        if caller.is_some() && pull_request.is_some() {
            allow_agent_integrate(
                true,
                daemon.config_orchestration().limits().agent_may_integrate,
            )
            .map_err(policy)?;
        }
        let tasks: Vec<TaskId> = inner
            .ledger
            .tasks
            .values()
            .filter(|task| task.run_id == run_id)
            .map(|task| task.id)
            .collect();
        (tasks, run.integration_workspace_id, pull_request)
    };
    if cancel {
        for task_id in &tasks {
            let revision = daemon
                .lock()
                .ledger
                .tasks
                .get(task_id)
                .map(|task| task.revision);
            if let Some(revision) = revision {
                let _ = decide_task(
                    daemon,
                    caller,
                    *task_id,
                    revision,
                    TaskDecision::Cancel { kill: true },
                    None,
                    None,
                );
            }
        }
    }
    let mut residual = Vec::new();
    if cleanup {
        for task_id in &tasks {
            match cleanup_task(daemon, caller, *task_id, false) {
                Ok(Response::CleanupResult { residual: more }) => residual.extend(more),
                Ok(_) => {}
                Err(error) => residual.push(error.message),
            }
        }
    }
    if let Some(pr) = open_pr {
        let workspace = integration.ok_or_else(|| {
            ProtocolError::precondition_failed("run has no integration workspace to open a PR from")
        })?;
        daemon.create_pull_request(workspace, pr.title, pr.body, pr.base, pr.draft)?;
    }
    let now = Timestamp::now();
    let mut inner = daemon.lock();
    let run = inner
        .ledger
        .runs
        .get_mut(&run_id)
        .ok_or_else(|| ProtocolError::not_found("run"))?;
    run.status = if cancel {
        RunStatus::Cancelled
    } else {
        RunStatus::Completed
    };
    run.closed_at = Some(now);
    run.updated_at = now;
    run.revision = run.revision.saturating_add(1);
    let run = run.clone();
    inner.db.orchestration().update_run(&run).map_err(db_err)?;
    drop(inner);
    daemon
        .registry
        .broadcast_domain(protocol::DaemonEvent::RunUpdated(run));
    Ok(Response::RunClosed { residual })
}

fn resume_run(
    daemon: &Arc<Daemon>,
    _caller: Option<SessionId>,
    run_id: RunId,
    provider_id: AgentProviderId,
    profile_id: Option<domain::AgentProfileId>,
) -> Result<Response, ProtocolError> {
    let (objective, base, workspace) = {
        let mut inner = daemon.lock();
        let run = inner
            .ledger
            .runs
            .get_mut(&run_id)
            .ok_or_else(|| ProtocolError::not_found("run"))?;
        if run.status != RunStatus::Interrupted && run.status != RunStatus::Active {
            return Err(ProtocolError::precondition_failed(
                "only an interrupted run can be resumed",
            ));
        }
        run.status = RunStatus::Active;
        run.updated_at = Timestamp::now();
        run.revision = run.revision.saturating_add(1);
        let run = run.clone();
        inner.db.orchestration().update_run(&run).map_err(db_err)?;
        (
            run.objective.clone(),
            run.base.clone(),
            run.integration_workspace_id,
        )
    };
    let workspace = workspace.ok_or_else(|| {
        ProtocolError::precondition_failed("resume needs the integration workspace")
    })?;
    let prompt = controller_prompt(daemon, run_id, &objective, base.as_deref(), true);
    set_spawn_env(vec![
        ("FORGE_RUN_ID".into(), run_id.to_string()),
        ("FORGECTL_JSON".into(), "1".into()),
    ]);
    let created = daemon.create_session(
        workspace,
        SessionKind::Agent,
        Some(provider_id),
        profile_id,
        None,
        SessionRole::Orchestrator,
        None,
        Some(prompt),
        false,
    )?;
    let session_id = session_of(&created)?;
    {
        let mut inner = daemon.lock();
        if let Some(run) = inner.ledger.runs.get_mut(&run_id) {
            run.controller_session_id = Some(session_id);
            let run = run.clone();
            inner.db.orchestration().update_run(&run).map_err(db_err)?;
            daemon
                .registry
                .broadcast_domain(protocol::DaemonEvent::RunUpdated(run));
        }
    }
    Ok(Response::RunCreated {
        run_id,
        controller_session_id: Some(session_id),
        integration_workspace_id: Some(workspace),
    })
}

fn review_task(daemon: &Daemon, task_id: TaskId, patch: bool) -> Result<Response, ProtocolError> {
    let (session, reported_head, base) = {
        let inner = daemon.lock();
        let attempt = inner
            .ledger
            .attempts
            .values()
            .filter(|attempt| attempt.task_id == task_id)
            .max_by_key(|attempt| attempt.n)
            .cloned()
            .ok_or_else(|| ProtocolError::not_found("attempt"))?;
        (
            attempt.session_id,
            attempt
                .report
                .as_ref()
                .and_then(|report| report.reported_head.clone()),
            attempt.base_commit.clone(),
        )
    };
    let session = session.ok_or_else(|| ProtocolError::not_found("attempt session"))?;
    let changes = daemon.get_session_changes(session)?;
    let summary = match changes {
        Response::SessionChanges(changes) => {
            let files: Vec<String> = changes
                .summary
                .files
                .iter()
                .map(|file| file.path.clone())
                .collect();
            let workspace_path = {
                let inner = daemon.lock();
                let workspace = inner
                    .sessions
                    .get(&session)
                    .map(|session| session.workspace_id);
                workspace.and_then(|id| inner.workspaces.get(&id).map(|ws| ws.path.clone()))
            };
            let head = workspace_path
                .as_ref()
                .and_then(|path| git_service::head_commit(path));
            let review_base = base.or(changes.summary.base.clone());
            let patches = if patch {
                review_patches(workspace_path.as_deref(), review_base.as_deref())
            } else {
                Vec::new()
            };
            return Ok(Response::TaskReview {
                task_id,
                session_id: Some(session),
                base_commit: review_base,
                head,
                reported_head,
                dirty: !changes.summary.files.is_empty() || !patches.is_empty(),
                files,
                summary: format!(
                    "{} file(s) since {}",
                    changes.summary.files.len(),
                    changes.summary.base.as_deref().unwrap_or("the start")
                ),
                patches,
            });
        }
        _ => "no changes".into(),
    };
    Ok(Response::TaskReview {
        task_id,
        session_id: Some(session),
        base_commit: base,
        head: None,
        reported_head,
        dirty: false,
        files: vec![],
        summary,
        patches: vec![],
    })
}

fn review_patches(path: Option<&std::path::Path>, base: Option<&str>) -> Vec<domain::DiffFile> {
    let Some(path) = path else {
        return Vec::new();
    };
    let Ok(diff) = git_service::working_tree_diff(path, base, git_service::DIFF_CONTEXT_LINES)
    else {
        return Vec::new();
    };
    diff.files
        .into_iter()
        .map(|file| domain::DiffFile {
            path: file.path,
            status: match file.status {
                git_service::FileChange::Added => domain::DiffStatus::Added,
                git_service::FileChange::Deleted => domain::DiffStatus::Deleted,
                git_service::FileChange::Modified => domain::DiffStatus::Modified,
                git_service::FileChange::Conflicted => domain::DiffStatus::Conflicted,
            },
            additions: file.additions,
            deletions: file.deletions,
            patch: file.patch,
            binary: file.binary,
            truncated: file.truncated,
        })
        .collect()
}

fn post_message(
    daemon: &Arc<Daemon>,
    caller: Option<SessionId>,
    run_id: RunId,
    address: &str,
    kind: ContextKind,
    body: String,
    in_reply_to: Option<ContextId>,
) -> Result<Response, ProtocolError> {
    let (body, _) = domain::orchestration::clamp_text(&body, MAX_MESSAGE_BYTES);
    if body.trim().is_empty() {
        return Err(ProtocolError::invalid_request("message is empty"));
    }
    let parsed = domain::orchestration::parse_address(address)
        .map_err(|_| ProtocolError::invalid_request("address"))?;
    let resolved = {
        let inner = daemon.lock();
        if !inner.ledger.runs.contains_key(&run_id) {
            return Err(ProtocolError::not_found("run"));
        }
        let address = match parsed {
            domain::orchestration::ParsedAddress::Controller => {
                domain::orchestration::Address::Controller
            }
            domain::orchestration::ParsedAddress::Task(prefix) => {
                let ids: Vec<TaskId> = inner.ledger.tasks.keys().copied().collect();
                let id = *domain::orchestration::resolve_prefix(ids.iter(), &prefix)
                    .map_err(|_| ProtocolError::not_found("task"))?;
                domain::orchestration::Address::Task(id)
            }
            domain::orchestration::ParsedAddress::Session(prefix) => {
                let ids: Vec<SessionId> = inner.sessions.keys().copied().collect();
                let id = *domain::orchestration::resolve_prefix(ids.iter(), &prefix)
                    .map_err(|_| ProtocolError::not_found("session"))?;
                domain::orchestration::Address::Session(id)
            }
            domain::orchestration::ParsedAddress::Run(prefix) => {
                let ids: Vec<RunId> = inner.ledger.runs.keys().copied().collect();
                let id = *domain::orchestration::resolve_prefix(ids.iter(), &prefix)
                    .map_err(|_| ProtocolError::not_found("run"))?;
                domain::orchestration::Address::Run(id)
            }
        };
        let controller = inner
            .ledger
            .runs
            .get(&run_id)
            .and_then(|run| run.controller_session_id);
        let attempts: Vec<LiveAttempt> = inner
            .ledger
            .attempts
            .values()
            .filter(|attempt| {
                inner
                    .ledger
                    .tasks
                    .get(&attempt.task_id)
                    .is_some_and(|task| task.run_id == run_id)
            })
            .map(|attempt| LiveAttempt {
                task_id: attempt.task_id,
                session_id: attempt.session_id,
                n: attempt.n,
                live: attempt.is_live(),
            })
            .collect();
        domain::orchestration::resolve_address(&address, controller, &attempts)
            .map_err(|_| ProtocolError::not_found("address"))?
    };
    let now = Timestamp::now();
    let mut ids = Vec::new();
    let mut sessions = Vec::new();
    let resumed = {
        let mut inner = daemon.lock();
        if resolved.is_empty() {
            return Err(ProtocolError::not_found("no live attempt to message"));
        }
        let targets: Vec<SessionId> = resolved
            .iter()
            .filter_map(|target| match target {
                DeliveryTarget::Session(session) => Some(*session),
                DeliveryTarget::HumanController => None,
            })
            .collect();
        let reply_to = in_reply_to.or_else(|| {
            if kind != ContextKind::Answer {
                return None;
            }
            link_answer(&inner, run_id, &targets)
        });
        let caller_task = task_of_session(&inner, caller);
        for target in &resolved {
            let target_session = match target {
                DeliveryTarget::Session(session) => Some(*session),
                DeliveryTarget::HumanController => None,
            };
            let task_id = caller_task.or_else(|| task_of_session(&inner, target_session));
            let id = ContextId::new();
            let envelope = ContextEnvelope {
                id,
                source_session_id: caller,
                target_session_id: target_session,
                summary: None,
                instructions: Some(body.clone()),
                artifacts: vec![],
                git_context: None,
                created_at: now,
                run_id: Some(run_id),
                task_id,
                kind: Some(kind),
                in_reply_to: reply_to,
                acked_at: None,
            };
            inner.db.context().insert(&envelope).map_err(db_err)?;
            ids.push(id);
            if let Some(session) = target_session {
                sessions.push(session);
            }
        }
        resume_blocked(&mut inner, &targets)?
    };
    for (task, attempt) in resumed {
        daemon
            .registry
            .broadcast_domain(protocol::DaemonEvent::TaskUpdated(task));
        daemon
            .registry
            .broadcast_domain(protocol::DaemonEvent::AttemptUpdated(attempt));
    }
    for session in sessions {
        let unread = unread_count(daemon, session);
        daemon
            .registry
            .broadcast_domain(protocol::DaemonEvent::MailboxChanged {
                session_id: Some(session),
                run_id: Some(run_id),
                unread,
            });
        deliver::consider_pointer(daemon, session);
    }
    daemon
        .registry
        .broadcast_domain(protocol::DaemonEvent::MailboxChanged {
            session_id: None,
            run_id: Some(run_id),
            unread: 0,
        });
    Ok(Response::MessagesPosted { ids })
}

fn read_inbox(
    daemon: &Daemon,
    session_id: Option<SessionId>,
    run_id: Option<RunId>,
    unread_only: bool,
    limit: u32,
    ack: Vec<ContextId>,
) -> Result<Response, ProtocolError> {
    let limit = limit.clamp(1, domain::orchestration::INBOX_PAGE as u32) as usize;
    if !ack.is_empty() {
        daemon
            .lock()
            .db
            .context()
            .ack(&ack, Timestamp::now())
            .map_err(db_err)?;
    }
    let inner = daemon.lock();
    let mut messages = if let Some(session) = session_id {
        inner
            .db
            .context()
            .list_for_session(session)
            .map_err(db_err)?
    } else if let Some(run) = run_id {
        inner.db.context().list_for_run(run).map_err(db_err)?
    } else {
        return Err(ProtocolError::invalid_request(
            "inbox needs a session or a run",
        ));
    };
    messages.retain(|message| message.kind.is_some());
    if unread_only {
        messages.retain(|message| message.acked_at.is_none());
    }
    if let Some(run) = run_id {
        if session_id.is_none() {
            messages.retain(|message| {
                message.run_id == Some(run) && message.target_session_id.is_none()
            });
        }
    }
    let more = messages.len() > limit;
    messages.truncate(limit);
    Ok(Response::Inbox { messages, more })
}

fn list_state(
    daemon: &Daemon,
    run_id: RunId,
    prefix: Option<&str>,
) -> Result<Response, ProtocolError> {
    let inner = daemon.lock();
    if !inner.ledger.runs.contains_key(&run_id) {
        return Err(ProtocolError::not_found("run"));
    }
    let mut entries: Vec<BoardEntry> = inner
        .ledger
        .board
        .values()
        .filter(|entry| entry.run_id == run_id)
        .filter(|entry| prefix.is_none_or(|prefix| entry.key.starts_with(prefix)))
        .cloned()
        .collect();
    entries.sort_by(|a, b| a.key.cmp(&b.key));
    Ok(Response::RunStateList(entries))
}

fn get_state(daemon: &Daemon, run_id: RunId, key: &str) -> Result<Response, ProtocolError> {
    let inner = daemon.lock();
    let entry = inner
        .ledger
        .board
        .get(&(run_id, key.to_owned()))
        .cloned()
        .ok_or_else(|| ProtocolError::not_found("board key"))?;
    Ok(Response::RunStateValue(entry))
}

fn set_state(
    daemon: &Daemon,
    caller: Option<SessionId>,
    run_id: RunId,
    key: &str,
    value_json: String,
    expected_version: u64,
    delete: bool,
) -> Result<Response, ProtocolError> {
    if !valid_board_key(key) {
        return Err(ProtocolError::invalid_request(
            "board key must match [a-z0-9._/-]{1,128}",
        ));
    }
    if !delete {
        board_value(&value_json).map_err(|error| match error {
            domain::orchestration::BoardValueError::Oversize { len } => {
                ProtocolError::invalid_request(format!("board value is {len} bytes"))
            }
            domain::orchestration::BoardValueError::NotJson => {
                ProtocolError::invalid_request("board value is not json")
            }
        })?;
    }
    let mut inner = daemon.lock();
    if !inner.ledger.runs.contains_key(&run_id) {
        return Err(ProtocolError::not_found("run"));
    }
    let current = inner
        .ledger
        .board
        .get(&(run_id, key.to_owned()))
        .map(|e| e.version);
    if board_is_full(
        inner
            .ledger
            .board
            .keys()
            .filter(|(id, _)| *id == run_id)
            .count(),
        current.is_none() && !delete,
    ) {
        return Err(policy(RailRefusal {
            rail: "max_board_keys",
            message: "run board is full".into(),
        }));
    }
    match compare_and_set(current, expected_version) {
        CasOutcome::Mismatch { current } => return Err(version_conflict(current)),
        CasOutcome::Created if delete => return Err(version_conflict(0)),
        CasOutcome::Created | CasOutcome::Updated { .. } => {}
    }
    let _ = caller;
    if delete {
        inner.ledger.board.remove(&(run_id, key.to_owned()));
        inner
            .db
            .orchestration()
            .delete_board(run_id, key)
            .map_err(db_err)?;
        drop(inner);
        daemon
            .registry
            .broadcast_domain(protocol::DaemonEvent::RunStateChanged {
                run_id,
                key: key.to_owned(),
                version: 0,
            });
        return Ok(Response::Ack);
    }
    let next = current.unwrap_or(0u64).saturating_add(1);
    let entry = BoardEntry {
        run_id,
        key: key.to_owned(),
        value_json,
        version: next,
        updated_by: caller,
        updated_at: Timestamp::now(),
    };
    inner
        .db
        .orchestration()
        .upsert_board(&entry)
        .map_err(db_err)?;
    inner
        .ledger
        .board
        .insert((run_id, key.to_owned()), entry.clone());
    drop(inner);
    daemon
        .registry
        .broadcast_domain(protocol::DaemonEvent::RunStateChanged {
            run_id,
            key: key.to_owned(),
            version: next,
        });
    Ok(Response::RunStateValue(entry))
}

fn place_integration(
    daemon: &Arc<Daemon>,
    project_id: ProjectId,
    objective: &str,
    placement: &IntegrationPlacement,
    base: Option<&str>,
    caller: Option<SessionId>,
) -> Result<(Option<WorkspaceId>, Option<String>), ProtocolError> {
    match placement {
        IntegrationPlacement::New => {
            let slug = git_service::slugify(objective);
            let slug = if slug.is_empty() { "run".into() } else { slug };
            let branch = format!("forge/run-{slug}");
            let workspace =
                daemon.create_managed_worktree(project_id, &branch, base, Some(&slug))?;
            Ok((Some(workspace.id), base.map(str::to_owned).or(Some(branch))))
        }
        IntegrationPlacement::Current => {
            let inner = daemon.lock();
            let workspace = caller
                .and_then(|session| inner.sessions.get(&session).map(|s| s.workspace_id))
                .or_else(|| {
                    inner
                        .workspaces
                        .values()
                        .find(|ws| ws.project_id == project_id && !ws.managed_by_app)
                        .map(|ws| ws.id)
                });
            Ok((workspace, base.map(str::to_owned)))
        }
        IntegrationPlacement::Workspace(id) => {
            let inner = daemon.lock();
            if !inner.workspaces.contains_key(id) {
                return Err(ProtocolError::not_found("workspace"));
            }
            Ok((Some(*id), base.map(str::to_owned)))
        }
        _ => Err(ProtocolError::invalid_request("integration placement")),
    }
}

fn resolve_workspace(
    inner: &Inner,
    run: &Run,
    placement: &AttemptPlacement,
    caller: Option<SessionId>,
) -> Result<WorkspaceId, ProtocolError> {
    match placement {
        AttemptPlacement::Worktree | AttemptPlacement::Integration => run
            .integration_workspace_id
            .ok_or_else(|| ProtocolError::precondition_failed("run has no integration workspace")),
        AttemptPlacement::Same => caller
            .and_then(|session| inner.sessions.get(&session).map(|s| s.workspace_id))
            .or(run.integration_workspace_id)
            .ok_or_else(|| ProtocolError::precondition_failed("no workspace for this attempt")),
        AttemptPlacement::Workspace(id) => {
            if inner.workspaces.contains_key(id) {
                Ok(*id)
            } else {
                Err(ProtocolError::not_found("workspace"))
            }
        }
        _ => Err(ProtocolError::invalid_request("placement")),
    }
}

fn materialize_placement(
    daemon: &Arc<Daemon>,
    run: &Run,
    task: &Task,
    hinted: WorkspaceId,
    placement: &AttemptPlacement,
    branch_override: Option<&str>,
) -> Result<(WorkspaceId, Option<String>, Option<String>), ProtocolError> {
    if !matches!(placement, AttemptPlacement::Worktree) || task.mode == WorkMode::ReadOnly {
        let path = daemon.workspace_path(hinted)?;
        let branch = git_service::current_branch(&path).ok().flatten();
        let head = git_service::head_commit(&path);
        return Ok((hinted, branch.or(branch_override.map(str::to_owned)), head));
    }
    let integration_path = daemon.workspace_path(hinted)?;
    let head = git_service::head_commit(&integration_path);
    let branch = branch_override.map(str::to_owned).unwrap_or_else(|| {
        let slug = git_service::slugify(&task.title);
        format!(
            "forge/task-{}",
            if slug.is_empty() { "task" } else { &slug }
        )
    });
    let workspace = daemon.create_managed_worktree(
        run.project_id,
        &branch,
        head.as_deref(),
        Some(&git_service::slugify(&branch)),
    )?;
    let base = git_service::head_commit(&workspace.path).or(head);
    Ok((workspace.id, Some(branch), base))
}

fn controller_prompt(
    daemon: &Daemon,
    run_id: RunId,
    objective: &str,
    base: Option<&str>,
    resume: bool,
) -> String {
    let inner = daemon.lock();
    let run = inner.ledger.runs.get(&run_id);
    let brief = run.map(|run| run.brief.clone()).unwrap_or_default();
    let brief_version = run.map(|run| run.brief_version).unwrap_or(0);
    let tasks: Vec<Task> = inner
        .ledger
        .tasks
        .values()
        .filter(|task| task.run_id == run_id)
        .cloned()
        .collect();
    let lines: Vec<TaskLine<'_>> = tasks
        .iter()
        .map(|task| TaskLine {
            task_id: task.id,
            title: &task.title,
            status: status_name(task.status),
            spec: &task.spec,
            acceptance: &task.acceptance,
        })
        .collect();
    let attempts: Vec<Attempt> = inner.ledger.attempts.values().cloned().collect();
    let mut report_lines = Vec::new();
    for task in &tasks {
        if let Some(summary) = attempts.iter().rev().find_map(|attempt| {
            (attempt.task_id == task.id)
                .then(|| attempt.report.as_ref().map(|report| report.summary.clone()))
                .flatten()
        }) {
            report_lines.push((task.id, task.title.clone(), summary));
        }
    }
    let reports: Vec<domain::orchestration::ReportLine<'_>> = report_lines
        .iter()
        .map(|(id, title, summary)| domain::orchestration::ReportLine {
            task_id: *id,
            title,
            outcome: "reported",
            summary,
        })
        .collect();
    let questions = questions_for(&inner, run_id).unwrap_or_default();
    let question_text: Vec<(String, String)> = questions
        .iter()
        .map(|question| {
            (
                question
                    .task_id
                    .map(|id| id.to_string())
                    .unwrap_or_else(|| "run".into()),
                question.text.clone(),
            )
        })
        .collect();
    let question_refs: Vec<(&str, &str)> = question_text
        .iter()
        .map(|(task, text)| (task.as_str(), text.as_str()))
        .collect();
    let mut keys: Vec<String> = inner
        .ledger
        .board
        .keys()
        .filter(|(id, _)| *id == run_id)
        .map(|(_, key)| key.clone())
        .collect();
    keys.sort();
    let key_refs: Vec<&str> = keys.iter().map(String::as_str).collect();
    compose_controller_prompt(&ControllerPromptInput {
        run_id,
        objective,
        brief: &brief,
        brief_version,
        base: base.unwrap_or("HEAD"),
        tasks: &lines,
        reports: &reports,
        questions: &question_refs,
        board_keys: &key_refs,
        resume,
    })
}

fn task_of_session(inner: &Inner, session: Option<SessionId>) -> Option<TaskId> {
    let session = session?;
    inner
        .ledger
        .attempts
        .values()
        .filter(|attempt| attempt.session_id == Some(session))
        .max_by_key(|attempt| attempt.n)
        .map(|attempt| attempt.task_id)
}

/// The newest unanswered question whose author is one of `targets`.
fn link_answer(inner: &Inner, run_id: RunId, targets: &[SessionId]) -> Option<ContextId> {
    let envelopes = inner.db.context().list_for_run(run_id).ok()?;
    envelopes.iter().rev().find_map(|envelope| {
        if envelope.kind != Some(ContextKind::Question) {
            return None;
        }
        let from_target = envelope
            .source_session_id
            .is_some_and(|source| targets.contains(&source));
        if !from_target {
            return None;
        }
        let answered = envelopes.iter().any(|other| {
            other.kind == Some(ContextKind::Answer) && other.in_reply_to == Some(envelope.id)
        });
        (!answered).then_some(envelope.id)
    })
}

/// `Blocked ──answer/send──► Active` on the same attempt, so the next report
/// is not a conflict with the blocked one.
fn resume_blocked(
    inner: &mut Inner,
    sessions: &[SessionId],
) -> Result<Vec<(Task, Attempt)>, ProtocolError> {
    let now = Timestamp::now();
    let pairs: Vec<(AttemptId, TaskId)> = inner
        .ledger
        .attempts
        .values()
        .filter(|attempt| {
            attempt.session_id.is_some_and(|id| sessions.contains(&id))
                && attempt.phase == AttemptPhase::Reported
                && attempt.outcome == Some(ReportOutcome::Blocked)
        })
        .filter(|attempt| {
            inner
                .ledger
                .tasks
                .get(&attempt.task_id)
                .is_some_and(|task| task.status == TaskStatus::Blocked)
        })
        .map(|attempt| (attempt.id, attempt.task_id))
        .collect();
    let mut resumed = Vec::new();
    for (attempt_id, task_id) in pairs {
        let Some(attempt) = inner.ledger.attempts.get_mut(&attempt_id) else {
            continue;
        };
        attempt.phase = AttemptPhase::Running;
        attempt.settled_at = None;
        let attempt = attempt.clone();
        let Some(task) = inner.ledger.tasks.get_mut(&task_id) else {
            continue;
        };
        task.status = TaskStatus::Active;
        task.updated_at = now;
        task.revision = task.revision.saturating_add(1);
        let task = task.clone();
        persist_attempt(inner, &attempt)?;
        persist_task(inner, &task)?;
        resumed.push((task, attempt));
    }
    Ok(resumed)
}

fn questions_for(inner: &Inner, run_id: RunId) -> Result<Vec<QuestionFact>, ProtocolError> {
    let envelopes = inner.db.context().list_for_run(run_id).map_err(db_err)?;
    let mut facts = Vec::new();
    for envelope in &envelopes {
        if envelope.kind != Some(ContextKind::Question) {
            continue;
        }
        let answered = envelopes.iter().any(|other| {
            other.kind == Some(ContextKind::Answer) && other.in_reply_to == Some(envelope.id)
        }) || envelope.acked_at.is_some();
        facts.push(QuestionFact {
            id: envelope.id,
            task_id: envelope.task_id,
            text: envelope.instructions.clone().unwrap_or_default(),
            answered,
        });
    }
    Ok(facts)
}

fn attempt_facts(inner: &Inner, attempts: &[Attempt]) -> Vec<AttemptFact> {
    attempts
        .iter()
        .map(|attempt| {
            let session = attempt.session_id.and_then(|id| inner.sessions.get(&id));
            AttemptFact {
                attempt: attempt.clone(),
                activity: session
                    .map(|session| session.activity.state)
                    .unwrap_or(ActivityState::Unknown),
                activity_since: session
                    .map(|session| session.activity.since)
                    .unwrap_or(attempt.created_at),
                session_exited: session
                    .map(|session| session.state.is_terminal())
                    .unwrap_or(true),
            }
        })
        .collect()
}

fn run_depth(inner: &Inner, run: &Run) -> u32 {
    let mut depth: u32 = 1;
    let mut parent = run.parent_attempt_id;
    while let Some(attempt_id) = parent {
        depth = depth.saturating_add(1u32);
        parent = inner
            .ledger
            .attempts
            .get(&attempt_id)
            .and_then(|attempt| inner.ledger.tasks.get(&attempt.task_id))
            .and_then(|task| inner.ledger.runs.get(&task.run_id))
            .and_then(|run| run.parent_attempt_id);
        if depth > 8 {
            break;
        }
    }
    depth
}

fn mark_integrated(
    daemon: &Daemon,
    task_id: TaskId,
    commit: Option<String>,
) -> Result<(), ProtocolError> {
    let now = Timestamp::now();
    let mut inner = daemon.lock();
    let task = inner
        .ledger
        .tasks
        .get_mut(&task_id)
        .ok_or_else(|| ProtocolError::not_found("task"))?;
    let run_id = task.run_id;
    task.status = TaskStatus::Integrated;
    task.updated_at = now;
    task.revision = task.revision.saturating_add(1);
    if let Some(attempt) = inner
        .ledger
        .attempts
        .values_mut()
        .filter(|attempt| attempt.task_id == task_id)
        .max_by_key(|attempt| attempt.n)
    {
        attempt.integrated_commit = commit;
    }
    let mut tasks: Vec<Task> = inner
        .ledger
        .tasks
        .values()
        .filter(|task| task.run_id == run_id)
        .cloned()
        .collect();
    promote_ready(&mut tasks);
    let attempts: Vec<Attempt> = inner
        .ledger
        .attempts
        .values()
        .filter(|attempt| attempt.task_id == task_id)
        .cloned()
        .collect();
    for task in &tasks {
        persist_task(&inner, task)?;
        inner.ledger.tasks.insert(task.id, task.clone());
    }
    for attempt in &attempts {
        persist_attempt(&inner, attempt)?;
    }
    drop(inner);
    for task in tasks {
        daemon
            .registry
            .broadcast_domain(protocol::DaemonEvent::TaskUpdated(task));
    }
    Ok(())
}

fn set_conflict(daemon: &Daemon, run_id: RunId) {
    daemon.lock().ledger.conflict.push(run_id);
}

fn clear_conflict(daemon: &Daemon, run_id: RunId) {
    daemon.lock().ledger.conflict.retain(|id| *id != run_id);
}

fn conflict_paths(path: &std::path::Path) -> Vec<String> {
    git_service::sequencer_state(path)
        .map(|state| state.conflicts.into_iter().map(|c| c.path).collect())
        .unwrap_or_default()
}

fn git_facts(
    path: Option<&std::path::Path>,
    base: Option<&str>,
) -> (Option<String>, bool, Vec<String>) {
    let Some(path) = path else {
        return (None, false, vec![]);
    };
    let head = git_service::head_commit(path);
    let dirty = git_service::status(path)
        .map(|st| st.dirty)
        .unwrap_or(false);
    let mut args = vec!["diff".to_owned(), "--name-only".to_owned()];
    if let Some(base) = base {
        args.push(format!("{base}...HEAD"));
    }
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let files = git_service::run_git(Some(path), &refs)
        .ok()
        .map(|out| {
            out.stdout
                .lines()
                .filter(|line| !line.trim().is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    (head, dirty, files)
}

fn copy_result(
    daemon: &Daemon,
    attempt_id: AttemptId,
    file: &str,
) -> Result<String, ProtocolError> {
    let bytes = client::read_capped(std::path::Path::new(file), MAX_RESULT_FILE_BYTES)
        .map_err(|error| ProtocolError::new(ErrorCode::IoError, error.to_string()))?
        .bytes;
    let run_id = {
        let inner = daemon.lock();
        inner
            .ledger
            .attempts
            .get(&attempt_id)
            .and_then(|attempt| inner.ledger.tasks.get(&attempt.task_id))
            .map(|task| task.run_id)
            .ok_or_else(|| ProtocolError::not_found("attempt"))?
    };
    let dir = daemon
        .worktrees_root()
        .parent()
        .unwrap_or(daemon.worktrees_root())
        .join("runs")
        .join(run_id.to_string());
    std::fs::create_dir_all(&dir)
        .map_err(|error| ProtocolError::new(ErrorCode::IoError, error.to_string()))?;
    let dest = dir.join(format!("{attempt_id}.md"));
    std::fs::write(&dest, bytes)
        .map_err(|error| ProtocolError::new(ErrorCode::IoError, error.to_string()))?;
    Ok(dest.to_string_lossy().into_owned())
}

fn unread_count(daemon: &Daemon, session: SessionId) -> u32 {
    daemon
        .lock()
        .db
        .context()
        .list_for_session(session)
        .unwrap_or_default()
        .into_iter()
        .filter(|message| message.kind.is_some() && message.acked_at.is_none())
        .count() as u32
}

fn persist_task(inner: &Inner, task: &Task) -> Result<(), ProtocolError> {
    inner.db.orchestration().update_task(task).map_err(db_err)
}

fn persist_attempt(inner: &Inner, attempt: &Attempt) -> Result<(), ProtocolError> {
    inner
        .db
        .orchestration()
        .update_attempt(attempt)
        .map_err(db_err)
}

fn session_of(response: &Response) -> Result<SessionId, ProtocolError> {
    match response {
        Response::SessionCreated { session_id, .. } => Ok(*session_id),
        _ => Err(internal("session was not created")),
    }
}

fn session_and_terminal(
    response: &Response,
) -> Result<(SessionId, domain::TerminalId), ProtocolError> {
    match response {
        Response::SessionCreated {
            session_id,
            terminal_id,
        } => Ok((*session_id, *terminal_id)),
        _ => Err(internal("session was not created")),
    }
}

fn status_name(status: TaskStatus) -> &'static str {
    match status {
        TaskStatus::Pending => "pending",
        TaskStatus::Ready => "ready",
        TaskStatus::Active => "active",
        TaskStatus::Review => "review",
        TaskStatus::Accepted => "accepted",
        TaskStatus::Integrated => "integrated",
        TaskStatus::Blocked => "blocked",
        TaskStatus::Cancelled => "cancelled",
        TaskStatus::Failed => "failed",
        TaskStatus::Unrecognized => "unrecognized",
        _ => "unrecognized",
    }
}

fn receipt_key(request: &Request) -> Option<(String, Option<SessionId>)> {
    let pair = match request {
        Request::CreateRun {
            request_id,
            caller_session_id,
            ..
        }
        | Request::UpdateRunBrief {
            request_id,
            caller_session_id,
            ..
        }
        | Request::CloseRun {
            request_id,
            caller_session_id,
            ..
        }
        | Request::ResumeRun {
            request_id,
            caller_session_id,
            ..
        }
        | Request::CreateTask {
            request_id,
            caller_session_id,
            ..
        }
        | Request::StartAttempt {
            request_id,
            caller_session_id,
            ..
        }
        | Request::RunTask {
            request_id,
            caller_session_id,
            ..
        }
        | Request::ReportAttempt {
            request_id,
            caller_session_id,
            ..
        }
        | Request::DecideTask {
            request_id,
            caller_session_id,
            ..
        }
        | Request::IntegrateTask {
            request_id,
            caller_session_id,
            ..
        }
        | Request::CleanupTask {
            request_id,
            caller_session_id,
            ..
        }
        | Request::PostMessage {
            request_id,
            caller_session_id,
            ..
        }
        | Request::SetRunState {
            request_id,
            caller_session_id,
            ..
        }
        | Request::DeleteRunState {
            request_id,
            caller_session_id,
            ..
        } => (request_id.clone(), *caller_session_id),
        _ => return None,
    };
    pair.0.map(|id| (id, pair.1))
}

fn caller_key(caller: Option<SessionId>) -> String {
    caller
        .map(|id| id.to_string())
        .unwrap_or_else(|| "human".into())
}

fn load_receipt(
    daemon: &Daemon,
    request_id: &str,
    caller: Option<SessionId>,
) -> Result<Option<Response>, ProtocolError> {
    let found = daemon
        .lock()
        .db
        .orchestration()
        .get_receipt(request_id, &caller_key(caller))
        .map_err(db_err)?;
    let Some((bytes, at)) = found else {
        return Ok(None);
    };
    let age = Timestamp::now().as_offset() - at.as_offset();
    if age > time::Duration::seconds(RECEIPT_TTL.as_secs() as i64) {
        return Ok(None);
    }
    let response: Response = rmp_serde::from_slice(&bytes)
        .map_err(|error| ProtocolError::internal(error.to_string()))?;
    Ok(Some(response))
}

fn store_receipt(
    daemon: &Daemon,
    request_id: &str,
    caller: Option<SessionId>,
    response: &Response,
) -> Result<(), ProtocolError> {
    let bytes = rmp_serde::to_vec_named(response)
        .map_err(|error| ProtocolError::internal(error.to_string()))?;
    daemon
        .lock()
        .db
        .orchestration()
        .put_receipt(request_id, &caller_key(caller), &bytes, Timestamp::now())
        .map_err(db_err)?;
    Ok(())
}

fn policy(refusal: RailRefusal) -> ProtocolError {
    ProtocolError::with_details(
        ErrorCode::PreconditionFailed,
        refusal.message,
        serde_json::json!({ "reason": format!("policy:{}", refusal.rail) }).to_string(),
    )
}

fn version_conflict(current: u64) -> ProtocolError {
    ProtocolError::with_details(
        ErrorCode::Conflict,
        format!("current version is {current}"),
        serde_json::json!({ "current_version": current }).to_string(),
    )
}

fn report_err(error: ReportRefuse) -> ProtocolError {
    match error {
        ReportRefuse::Foreign => ProtocolError::conflict("report is from another session"),
        ReportRefuse::Superseded => {
            ProtocolError::conflict("attempt is superseded or already settled")
        }
    }
}

fn db_err(error: persistence::DbError) -> ProtocolError {
    ProtocolError::internal(error.to_string())
}

fn git_err(error: git_service::GitError) -> ProtocolError {
    ProtocolError::new(ErrorCode::GitError, error.to_string())
}

fn internal(message: &str) -> ProtocolError {
    ProtocolError::internal(message)
}

pub fn active_run_views(daemon: &Daemon) -> Vec<domain::orchestration::RunView> {
    let limits = daemon.config_orchestration().limits();
    let inner = daemon.lock();
    inner
        .ledger
        .runs
        .values()
        .filter(|run| run.status.is_open())
        .filter_map(|run| view_locked(&inner, run.id, &limits).ok())
        .collect()
}

#[derive(Serialize, Deserialize)]
struct StoredResponse(Response);

impl Daemon {
    fn config_orchestration(&self) -> crate::config::OrchestrationConfig {
        self.orchestration_config()
    }

    pub(crate) fn worktrees_root(&self) -> &std::path::Path {
        self.worktrees_root_path()
    }
}
