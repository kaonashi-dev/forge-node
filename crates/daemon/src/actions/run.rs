//! `RunAction` and the restart of a `Task` session.
//!
//! Resolving the directory and choosing a port happen off the core lock; the
//! single-instance check and the session's creation share one critical
//! section, which is what makes a double click or a second window harmless.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use domain::{
    resolve_command, ActionKind, ActionReadiness, ActionReady, ActionRun, AgentActivity,
    ProjectAction, ProjectActionId, Session, SessionId, SessionKind, SessionRole, SessionState,
    SessionTitle, Timestamp, WorkspaceId, PORT_PLACEHOLDER,
};
use protocol::{DaemonEvent, ErrorCode, ProtocolError, Response};

use super::ports::{self, PortChoice};
use super::ready::OutputWatch;
use crate::core::{Daemon, Inner};

/// Everything a spawn needs that was resolved before the lock.
pub(crate) struct TaskLaunch {
    pub(crate) command: String,
    pub(crate) cwd: PathBuf,
    pub(crate) run: ActionRun,
    pub(crate) env: Vec<(String, String)>,
}

/// The live session of `action_id` in `workspace_id`, if any.
pub(crate) fn live_session(
    inner: &Inner,
    workspace_id: WorkspaceId,
    action_id: ProjectActionId,
) -> Option<SessionId> {
    inner
        .sessions
        .values()
        .find(|session| {
            session.workspace_id == workspace_id
                && session.state.is_active()
                && session
                    .action
                    .as_ref()
                    .is_some_and(|run| run.action_id == action_id)
        })
        .map(|session| session.id)
}

/// Ports live action sessions hold, so two servers are not handed the same one
/// before either has bound it.
fn reserved_ports(inner: &Inner) -> HashSet<u16> {
    inner
        .sessions
        .values()
        .filter(|session| session.state.is_active())
        .filter_map(|session| session.action.as_ref()?.port)
        .collect()
}

/// Bounds re-choosing a port another launch claimed while this one probed.
const PORT_CLAIM_ATTEMPTS: u32 = 4;

/// Whether a session created since `launch` was prepared now holds its port.
/// A port reported in use was knowingly shared, so it never counts.
fn port_claimed(inner: &Inner, launch: &TaskLaunch) -> bool {
    launch.run.port.is_some_and(|port| {
        !matches!(launch.run.readiness, ActionReadiness::PortInUse { .. })
            && reserved_ports(inner).contains(&port)
    })
}

fn port_contended() -> ProtocolError {
    ProtocolError::conflict("another action kept taking the chosen port; try again")
}

/// The project's single action whose command template is the untagged task's
/// `launch_command`, which stores the template for exactly this. Two actions
/// with one command are ambiguous, so neither is adopted.
fn adopt(inner: &Inner, session: &Session) -> Option<ProjectAction> {
    let command = session.launch_command.as_deref()?;
    let project_id = inner.workspaces.get(&session.workspace_id)?.project_id;
    let mut matching = inner
        .project_actions
        .get(&project_id)?
        .iter()
        .filter(|action| action.command == command);
    let action = matching.next()?;
    matching.next().is_none().then(|| action.clone())
}

fn find_action(
    inner: &Inner,
    workspace_id: WorkspaceId,
    action_id: ProjectActionId,
) -> Result<(ProjectAction, PathBuf), ProtocolError> {
    let ws = inner
        .workspaces
        .get(&workspace_id)
        .ok_or_else(|| ProtocolError::not_found("workspace"))?;
    let action = inner
        .project_actions
        .get(&ws.project_id)
        .and_then(|actions| actions.iter().find(|action| action.id == action_id))
        .cloned()
        .ok_or_else(|| ProtocolError::not_found("action"))?;
    Ok((action, ws.path.clone()))
}

/// The action's directory, canonicalized and required to stay in the checkout:
/// a `cwd` that became a symlink out of it since it was saved does not escape.
fn resolve_cwd(root: &Path, cwd: Option<&str>) -> Result<PathBuf, ProtocolError> {
    let Some(relative) = cwd else {
        return Ok(root.to_path_buf());
    };
    let canonical_root = root.canonicalize().map_err(|e| {
        ProtocolError::new(
            ErrorCode::IoError,
            format!("the workspace is unreadable: {e}"),
        )
    })?;
    let dir = root.join(relative).canonicalize().map_err(|_| {
        ProtocolError::precondition_failed(format!("{relative} does not exist in this workspace"))
    })?;
    if !dir.starts_with(&canonical_root) || !dir.is_dir() {
        return Err(ProtocolError::precondition_failed(format!(
            "{relative} is not a directory inside this workspace"
        )));
    }
    Ok(dir)
}

impl Daemon {
    pub(crate) fn run_action(
        self: &Arc<Self>,
        workspace_id: WorkspaceId,
        action_id: ProjectActionId,
    ) -> Result<Response, ProtocolError> {
        self.ensure_env();
        let _span = tracing::info_span!("action.run", %workspace_id, %action_id).entered();
        let (action, root, reserved) = {
            let inner = self.lock();
            let (action, root) = find_action(&inner, workspace_id, action_id)?;
            if action.single_instance {
                if let Some(session_id) = live_session(&inner, workspace_id, action_id) {
                    return Ok(Response::ActionAlreadyRunning { session_id });
                }
            }
            (action, root, reserved_ports(&inner))
        };
        if !action.depends_on.is_empty() {
            return self.queue_action(workspace_id, &action);
        }
        let mut launch = prepare(&action, &root, &reserved, None)?;

        let session_id = SessionId::new();
        let mut attempts = 0;
        let (spec, session) = loop {
            let mut inner = self.lock();
            // The authoritative check: the one above only spared the port probe.
            if action.single_instance {
                if let Some(session_id) = live_session(&inner, workspace_id, action_id) {
                    return Ok(Response::ActionAlreadyRunning { session_id });
                }
            }
            if port_claimed(&inner, &launch) {
                attempts += 1;
                if attempts >= PORT_CLAIM_ATTEMPTS {
                    return Err(port_contended());
                }
                let reserved = reserved_ports(&inner);
                drop(inner);
                launch = prepare(&action, &root, &reserved, None)?;
                continue;
            }
            let now = Timestamp::now();
            let session = Session {
                id: session_id,
                workspace_id,
                kind: SessionKind::Task,
                role: SessionRole::Generic,
                parent_session_id: None,
                root_session_id: session_id,
                terminal_id: None,
                editor: None,
                agent_provider_id: None,
                agent_profile_id: None,
                title: SessionTitle::default(),
                state: SessionState::Starting,
                created_at: now,
                launch_command: Some(action.command.clone()),
                last_activity_at: now,
                ended_at: None,
                base_commit: None,
                activity: AgentActivity::unknown(),
                action: Some(Box::new(launch.run.clone())),
            };
            let spec = self.task_spawn_spec(&mut inner, &root, session_id, &launch)?;
            inner
                .db
                .sessions()
                .upsert(&session)
                .map_err(crate::core::db_err)?;
            inner.sessions.insert(session_id, session.clone());
            break (spec, session);
        };

        self.registry
            .broadcast_domain(DaemonEvent::SessionCreated(session));
        let watch = output_watch(session_id, &action, &launch.run);
        match self.spawn_terminal_watching(session_id, spec, true, watch) {
            Ok(terminal_id) => {
                self.start_readiness(
                    session_id,
                    &action.ready,
                    launch.run.port,
                    &launch.run.readiness,
                );
                Ok(Response::SessionCreated {
                    session_id,
                    terminal_id,
                })
            }
            Err(reason) => {
                self.mark_session_failed(session_id, reason.clone());
                Err(ProtocolError::new(ErrorCode::SpawnError, reason))
            }
        }
    }

    /// Restart a finished `Task` with the action's *current* definition, so an
    /// edit made while it ran applies now. An untagged one (its tag is
    /// runtime-only, so every task after a daemon restart) re-adopts the one
    /// action with its command; failing that it re-runs that command, untagged.
    pub(crate) fn restart_task(
        self: &Arc<Self>,
        session_id: SessionId,
    ) -> Result<Response, ProtocolError> {
        self.ensure_env();
        let (session, current, root, reserved) = {
            let inner = self.lock();
            let session = inner
                .sessions
                .get(&session_id)
                .cloned()
                .ok_or_else(|| ProtocolError::not_found("session"))?;
            if !session.state.is_terminal() {
                return Err(ProtocolError::precondition_failed(
                    "session is not in a terminal state",
                ));
            }
            let current = match &session.action {
                Some(run) => find_action(&inner, session.workspace_id, run.action_id)
                    .ok()
                    .map(|(action, _)| action),
                None => adopt(&inner, &session),
            };
            let root = inner
                .workspaces
                .get(&session.workspace_id)
                .map(|ws| ws.path.clone())
                .ok_or_else(|| ProtocolError::not_found("workspace"))?;
            (session, current, root, reserved_ports(&inner))
        };
        if let Some(action) = current.as_ref().filter(|a| !a.depends_on.is_empty()) {
            return self.requeue_task(session_id, session.workspace_id, action);
        }
        let prior_port = session.action.as_ref().and_then(|run| run.port);
        let mut launch = match &current {
            Some(action) => Some(prepare(action, &root, &reserved, prior_port)?),
            None => None,
        };

        let mut attempts = 0;
        let (spec, updated) = loop {
            let mut inner = self.lock();
            if current.as_ref().is_some_and(|action| {
                action.single_instance
                    && live_session(&inner, session.workspace_id, action.id).is_some()
            }) {
                return Err(ProtocolError::conflict(
                    "this action is already running in this workspace",
                ));
            }
            if let (Some(action), Some(claimed)) = (&current, &launch) {
                if port_claimed(&inner, claimed) {
                    attempts += 1;
                    if attempts >= PORT_CLAIM_ATTEMPTS {
                        return Err(port_contended());
                    }
                    let reserved = reserved_ports(&inner);
                    drop(inner);
                    launch = Some(prepare(action, &root, &reserved, None)?);
                    continue;
                }
            }
            let spec = match &launch {
                Some(launch) => self.task_spawn_spec(&mut inner, &root, session_id, launch)?,
                None => {
                    let command = session.launch_command.clone().unwrap_or_default();
                    if command.contains(PORT_PLACEHOLDER) {
                        return Err(ProtocolError::precondition_failed(
                            "its action is gone and the command needs a port; run it from the actions instead",
                        ));
                    }
                    self.plain_task_spec(&mut inner, &root, session_id, &command)?
                }
            };
            let mut updated =
                Self::set_session_state(&mut inner, session_id, SessionState::Starting, None)
                    .ok_or_else(|| {
                        ProtocolError::precondition_failed(
                            "session cannot restart from its current state",
                        )
                    })?;
            if let (Some(action), Some(launch)) = (&current, &launch) {
                updated.action = Some(Box::new(launch.run.clone()));
                updated.launch_command = Some(action.command.clone());
                if let Some(stored) = inner.sessions.get_mut(&session_id) {
                    stored.action = updated.action.clone();
                    stored.launch_command = updated.launch_command.clone();
                }
                inner
                    .db
                    .sessions()
                    .upsert(&updated)
                    .map_err(crate::core::db_err)?;
            }
            break (spec, updated);
        };

        self.registry
            .broadcast_domain(DaemonEvent::SessionUpdated(updated.clone()));
        let watch = current
            .as_ref()
            .zip(updated.action.as_deref())
            .and_then(|(action, run)| output_watch(session_id, action, run));
        match self.spawn_terminal_watching(session_id, spec, true, watch) {
            Ok(_) => {
                if let (Some(action), Some(run)) = (&current, &updated.action) {
                    self.start_readiness(session_id, &action.ready, run.port, &run.readiness);
                }
                Ok(Response::Ack)
            }
            Err(reason) => {
                self.mark_session_failed(session_id, reason.clone());
                Err(ProtocolError::new(ErrorCode::SpawnError, reason))
            }
        }
    }

    fn task_spawn_spec(
        &self,
        inner: &mut Inner,
        root: &Path,
        session_id: SessionId,
        launch: &TaskLaunch,
    ) -> Result<domain::SpawnSpec, ProtocolError> {
        // `FORGE_WORKSPACE` names the root even when the action runs below it.
        let mut spec = self.build_spawn_spec(
            inner,
            SessionKind::Task,
            None,
            None,
            Some(&launch.command),
            root,
            session_id,
            &launch.env,
        )?;
        spec.cwd.clone_from(&launch.cwd);
        Ok(spec)
    }

    fn plain_task_spec(
        &self,
        inner: &mut Inner,
        root: &Path,
        session_id: SessionId,
        command: &str,
    ) -> Result<domain::SpawnSpec, ProtocolError> {
        self.build_spawn_spec(
            inner,
            SessionKind::Task,
            None,
            None,
            Some(command),
            root,
            session_id,
            &[],
        )
    }
}

fn output_watch(
    session_id: SessionId,
    action: &ProjectAction,
    run: &ActionRun,
) -> Option<OutputWatch> {
    match &action.ready {
        ActionReady::Output {
            pattern,
            timeout_secs,
        } if run.readiness == ActionReadiness::Probing => Some(OutputWatch::new(
            session_id,
            pattern,
            *timeout_secs,
            run.port,
        )),
        _ => None,
    }
}

/// Directory, port, command and environment for one launch; filesystem and
/// bind probes, so never under the core lock.
fn prepare(
    action: &ProjectAction,
    root: &Path,
    reserved: &HashSet<u16>,
    prior_port: Option<u16>,
) -> Result<TaskLaunch, ProtocolError> {
    let cwd = resolve_cwd(root, action.cwd.as_deref())?;
    let choice = action
        .port
        .map(|spec| ports::choose(spec, reserved, prior_port));
    let port = choice.map(PortChoice::port);
    let readiness = match (choice, &action.ready) {
        (Some(PortChoice::Taken(port)), _) => ActionReadiness::PortInUse { port },
        (_, ActionReady::None) => ActionReadiness::Unprobed,
        _ => ActionReadiness::Probing,
    };
    let mut env = vec![("FORGE_ACTION_ID".to_owned(), action.id.to_string())];
    if let Some(port) = port {
        env.push(("PORT".to_owned(), port.to_string()));
        env.push(("FORGE_ACTION_PORT".to_owned(), port.to_string()));
    }
    Ok(TaskLaunch {
        command: resolve_command(&action.command, port),
        cwd,
        run: ActionRun {
            action_id: action.id,
            label: action.label.clone(),
            port,
            readiness,
            started_at: Some(Timestamp::now()),
        },
        env,
    })
}

/// Longest a task waits for its dependencies, whatever their own timeouts say.
/// A step such as a build has no timeout of its own, so it is given all of it.
const MAX_DEPENDENCY_WAIT: Duration = Duration::from_secs(15 * 60);
/// Added to the slowest dependency chain's own readiness timeouts.
const DEPENDENCY_SLACK: Duration = Duration::from_secs(60);

/// How long `action` may wait on its dependency chain: each link waits for its
/// own dependencies before it starts, so the times add along a chain.
fn dependency_budget(actions: &[ProjectAction], action: &ProjectAction, depth: usize) -> Duration {
    if depth > super::MAX_DEPENDENCY_DEPTH {
        return MAX_DEPENDENCY_WAIT;
    }
    action
        .depends_on
        .iter()
        .filter_map(|id| actions.iter().find(|candidate| candidate.id == *id))
        .map(|dep| {
            let own = match dep.ready.timeout_secs() {
                Some(secs) => Duration::from_secs(secs),
                None if dep.kind == ActionKind::Dev => Duration::ZERO,
                None => MAX_DEPENDENCY_WAIT,
            };
            own.saturating_add(dependency_budget(actions, dep, depth + 1))
        })
        .max()
        .unwrap_or(Duration::ZERO)
}

/// How long a restart waits, past the kill grace, for the old process to go.
const RESTART_SLACK: Duration = Duration::from_secs(5);

/// Wake every task waiting on its dependencies; drop the ones that stopped.
pub(crate) fn wake_waiters(inner: &mut Inner) {
    inner.task_waiters.retain(|waiter| {
        !matches!(
            waiter.try_send(()),
            Err(flume::TrySendError::Disconnected(()))
        )
    });
}

/// A dependency's action and the session that runs it for this launch.
struct Dependency {
    action_id: ProjectActionId,
    label: String,
    session_id: SessionId,
    /// A probed or `Dev` action serves, so running (or ready) is enough;
    /// anything else is a step that must exit 0.
    serves: bool,
}

enum Verdict {
    Ready,
    Waiting(Vec<ProjectActionId>),
    Failed(String),
}

/// Whether every dependency's session lets the dependent start: a server that
/// is ready (or unprobed, or on a taken port it was warned about), or a step
/// such as a migration that exited 0.
fn verdict(inner: &Inner, deps: &[Dependency]) -> Verdict {
    let mut waiting = Vec::new();
    for Dependency {
        action_id,
        label,
        session_id,
        serves,
    } in deps
    {
        let Some(session) = inner.sessions.get(session_id) else {
            return Verdict::Failed(format!("dependency {label} was closed"));
        };
        let readiness = session.action.as_ref().map(|run| &run.readiness);
        match &session.state {
            SessionState::Exited { code: Some(0), .. } => {}
            SessionState::Running
                if *serves
                    && readiness.is_none_or(|r| {
                        r.satisfies_dependents() || matches!(r, ActionReadiness::PortInUse { .. })
                    }) => {}
            SessionState::Running if readiness == Some(&ActionReadiness::TimedOut) => {
                return Verdict::Failed(format!("dependency {label} never became ready"));
            }
            SessionState::Starting | SessionState::Running => waiting.push(*action_id),
            SessionState::Exited { code, .. } => {
                return Verdict::Failed(match code {
                    Some(code) => format!("dependency {label} exited with {code}"),
                    None => format!("dependency {label} was stopped"),
                });
            }
            _ => return Verdict::Failed(format!("dependency {label} did not run")),
        }
    }
    if waiting.is_empty() {
        Verdict::Ready
    } else {
        Verdict::Waiting(waiting)
    }
}

impl Daemon {
    /// Restart a task whether or not it is still running. A live one is
    /// stopped first and relaunched from its own thread once it has exited,
    /// so the request acks without waiting out the kill grace.
    pub(crate) fn restart_action_session(
        self: &Arc<Self>,
        session_id: SessionId,
    ) -> Result<Response, ProtocolError> {
        let wake_rx = {
            let mut inner = self.lock();
            let session = inner
                .sessions
                .get(&session_id)
                .ok_or_else(|| ProtocolError::not_found("session"))?;
            if session.state.is_terminal() {
                drop(inner);
                return self.restart_task(session_id);
            }
            let (wake_tx, wake_rx) = flume::bounded(1);
            inner.task_waiters.push(wake_tx);
            wake_rx
        };
        self.kill_session(session_id)?;
        let deadline = Instant::now() + self.kill_grace() + RESTART_SLACK;
        let daemon = Arc::clone(self);
        std::thread::Builder::new()
            .name("action-restart".into())
            .spawn(move || daemon.restart_once_exited(session_id, &wake_rx, deadline))
            .map_err(|error| {
                ProtocolError::new(ErrorCode::Internal, format!("cannot restart: {error}"))
            })?;
        Ok(Response::Ack)
    }

    fn restart_once_exited(
        self: &Arc<Self>,
        session_id: SessionId,
        wake: &flume::Receiver<()>,
        deadline: Instant,
    ) {
        loop {
            let exited = self
                .lock()
                .sessions
                .get(&session_id)
                .map(|session| session.state.is_terminal());
            match exited {
                None => return,
                Some(true) => {
                    if let Err(error) = self.restart_task(session_id) {
                        self.mark_session_failed(session_id, format!("restart failed: {error}"));
                    }
                    return;
                }
                Some(false) => {}
            }
            if wake.recv_deadline(deadline).is_err() {
                tracing::warn!(%session_id, "task did not exit in time to restart");
                return;
            }
        }
    }

    /// Start (or reuse) each dependency, then park the dependent as `Starting`
    /// with no process until they let it go.
    fn queue_action(
        self: &Arc<Self>,
        workspace_id: WorkspaceId,
        action: &ProjectAction,
    ) -> Result<Response, ProtocolError> {
        let (deps, deadline) = self.start_dependencies(workspace_id, action)?;

        let session_id = SessionId::new();
        let (wake_rx, session) = {
            let mut inner = self.lock();
            if action.single_instance {
                if let Some(session_id) = live_session(&inner, workspace_id, action.id) {
                    return Ok(Response::ActionAlreadyRunning { session_id });
                }
            }
            let now = Timestamp::now();
            let session = Session {
                id: session_id,
                workspace_id,
                kind: SessionKind::Task,
                role: SessionRole::Generic,
                parent_session_id: None,
                root_session_id: session_id,
                terminal_id: None,
                editor: None,
                agent_provider_id: None,
                agent_profile_id: None,
                title: SessionTitle::default(),
                state: SessionState::Starting,
                created_at: now,
                launch_command: Some(action.command.clone()),
                last_activity_at: now,
                ended_at: None,
                base_commit: None,
                activity: AgentActivity::unknown(),
                action: Some(Box::new(ActionRun {
                    action_id: action.id,
                    label: action.label.clone(),
                    port: None,
                    readiness: ActionReadiness::Waiting {
                        on: action.depends_on.clone(),
                    },
                    started_at: None,
                })),
            };
            inner
                .db
                .sessions()
                .upsert(&session)
                .map_err(crate::core::db_err)?;
            inner.sessions.insert(session_id, session.clone());
            let (wake_tx, wake_rx) = flume::bounded(1);
            inner.task_waiters.push(wake_tx);
            (wake_rx, session)
        };
        self.registry
            .broadcast_domain(DaemonEvent::SessionCreated(session));

        self.spawn_waiter(session_id, workspace_id, action.id, deps, wake_rx, deadline);
        Ok(Response::ActionQueued { session_id })
    }

    /// Run (or reuse) each dependency in the workspace and work out how long
    /// the dependent may wait for them.
    fn start_dependencies(
        self: &Arc<Self>,
        workspace_id: WorkspaceId,
        action: &ProjectAction,
    ) -> Result<(Vec<Dependency>, Instant), ProtocolError> {
        let mut deps = Vec::with_capacity(action.depends_on.len());
        for dep_id in &action.depends_on {
            let dep = {
                let inner = self.lock();
                find_action(&inner, workspace_id, *dep_id)?.0
            };
            let session_id = match self.run_action(workspace_id, *dep_id)? {
                Response::SessionCreated { session_id, .. }
                | Response::ActionQueued { session_id }
                | Response::ActionAlreadyRunning { session_id } => session_id,
                _ => {
                    return Err(ProtocolError::new(
                        ErrorCode::Internal,
                        "unexpected dependency answer",
                    ))
                }
            };
            deps.push(Dependency {
                serves: dep.kind == ActionKind::Dev || dep.ready != ActionReady::None,
                action_id: dep.id,
                label: dep.label,
                session_id,
            });
        }
        let wait = {
            let inner = self.lock();
            let project_actions = inner
                .workspaces
                .get(&workspace_id)
                .and_then(|ws| inner.project_actions.get(&ws.project_id))
                .map_or(&[][..], Vec::as_slice);
            dependency_budget(project_actions, action, 0)
        };
        let deadline = Instant::now()
            + wait
                .saturating_add(DEPENDENCY_SLACK)
                .min(MAX_DEPENDENCY_WAIT);
        Ok((deps, deadline))
    }

    fn spawn_waiter(
        self: &Arc<Self>,
        session_id: SessionId,
        workspace_id: WorkspaceId,
        action_id: ProjectActionId,
        deps: Vec<Dependency>,
        wake: flume::Receiver<()>,
        deadline: Instant,
    ) {
        let daemon = Arc::clone(self);
        let spawned = std::thread::Builder::new()
            .name("action-deps".into())
            .spawn(move || {
                daemon.await_dependencies(
                    session_id,
                    workspace_id,
                    action_id,
                    &deps,
                    &wake,
                    deadline,
                );
            });
        if let Err(error) = spawned {
            self.mark_session_failed(session_id, format!("cannot wait for dependencies: {error}"));
        }
    }

    /// Park a finished dependent again behind its dependencies.
    fn requeue_task(
        self: &Arc<Self>,
        session_id: SessionId,
        workspace_id: WorkspaceId,
        action: &ProjectAction,
    ) -> Result<Response, ProtocolError> {
        let (deps, deadline) = self.start_dependencies(workspace_id, action)?;
        let (wake_rx, updated) = {
            let mut inner = self.lock();
            if action.single_instance && live_session(&inner, workspace_id, action.id).is_some() {
                return Err(ProtocolError::conflict(
                    "this action is already running in this workspace",
                ));
            }
            let mut updated =
                Self::set_session_state(&mut inner, session_id, SessionState::Starting, None)
                    .ok_or_else(|| {
                        ProtocolError::precondition_failed(
                            "session cannot restart from its current state",
                        )
                    })?;
            let run = ActionRun {
                action_id: action.id,
                label: action.label.clone(),
                port: None,
                readiness: ActionReadiness::Waiting {
                    on: action.depends_on.clone(),
                },
                started_at: None,
            };
            updated.action = Some(Box::new(run));
            if let Some(stored) = inner.sessions.get_mut(&session_id) {
                stored.action.clone_from(&updated.action);
            }
            let (wake_tx, wake_rx) = flume::bounded(1);
            inner.task_waiters.push(wake_tx);
            (wake_rx, updated)
        };
        self.registry
            .broadcast_domain(DaemonEvent::SessionUpdated(updated));
        self.spawn_waiter(session_id, workspace_id, action.id, deps, wake_rx, deadline);
        Ok(Response::Ack)
    }

    fn await_dependencies(
        self: &Arc<Self>,
        session_id: SessionId,
        workspace_id: WorkspaceId,
        action_id: ProjectActionId,
        deps: &[Dependency],
        wake: &flume::Receiver<()>,
        deadline: Instant,
    ) {
        let mut announced: Option<Vec<ProjectActionId>> = None;
        loop {
            let outcome = {
                let mut inner = self.lock();
                let still_waiting = inner.sessions.get(&session_id).is_some_and(|session| {
                    session.state == SessionState::Starting
                        && session.action.as_ref().is_some_and(|run| {
                            matches!(run.readiness, ActionReadiness::Waiting { .. })
                        })
                });
                if !still_waiting {
                    return;
                }
                let outcome = verdict(&inner, deps);
                if let Verdict::Waiting(on) = &outcome {
                    if announced.as_ref() != Some(on) {
                        announced = Some(on.clone());
                        let updated = inner.sessions.get_mut(&session_id).and_then(|session| {
                            let run = session.action.as_mut()?;
                            run.readiness = ActionReadiness::Waiting { on: on.clone() };
                            Some(session.clone())
                        });
                        drop(inner);
                        if let Some(updated) = updated {
                            self.registry
                                .broadcast_domain(DaemonEvent::SessionUpdated(updated));
                        }
                    }
                }
                outcome
            };
            match outcome {
                Verdict::Ready => break,
                Verdict::Failed(reason) => {
                    self.mark_session_failed(session_id, reason);
                    return;
                }
                Verdict::Waiting(_) => {}
            }
            match wake.recv_deadline(deadline) {
                Ok(()) => {}
                Err(flume::RecvTimeoutError::Timeout) => {
                    self.mark_session_failed(
                        session_id,
                        "its dependencies did not become ready in time".to_owned(),
                    );
                    return;
                }
                Err(flume::RecvTimeoutError::Disconnected) => return,
            }
        }
        if let Err(error) = self.launch_queued(session_id, workspace_id, action_id) {
            self.mark_session_failed(session_id, error.message);
        }
    }

    /// Spawn a task whose dependencies just let it go, with the action as it
    /// is *now*: it may have been edited while it waited.
    fn launch_queued(
        self: &Arc<Self>,
        session_id: SessionId,
        workspace_id: WorkspaceId,
        action_id: ProjectActionId,
    ) -> Result<(), ProtocolError> {
        let (action, root, reserved) = {
            let inner = self.lock();
            let (action, root) = find_action(&inner, workspace_id, action_id)
                .map_err(|_| ProtocolError::not_found("its action was removed while it waited"))?;
            (action, root, reserved_ports(&inner))
        };
        let mut launch = prepare(&action, &root, &reserved, None)?;
        let mut attempts = 0;
        let spec = loop {
            let mut inner = self.lock();
            if port_claimed(&inner, &launch) {
                attempts += 1;
                if attempts >= PORT_CLAIM_ATTEMPTS {
                    return Err(port_contended());
                }
                let reserved = reserved_ports(&inner);
                drop(inner);
                launch = prepare(&action, &root, &reserved, None)?;
                continue;
            }
            let spec = self.task_spawn_spec(&mut inner, &root, session_id, &launch)?;
            let Some(session) = inner.sessions.get_mut(&session_id) else {
                return Ok(());
            };
            if session.state != SessionState::Starting {
                return Ok(());
            }
            session.action = Some(Box::new(launch.run.clone()));
            session.launch_command = Some(action.command.clone());
            let snapshot = session.clone();
            inner
                .db
                .sessions()
                .upsert(&snapshot)
                .map_err(crate::core::db_err)?;
            break spec;
        };
        let watch = output_watch(session_id, &action, &launch.run);
        self.spawn_terminal_watching(session_id, spec, true, watch)
            .map_err(|reason| ProtocolError::new(ErrorCode::SpawnError, reason))?;
        self.start_readiness(
            session_id,
            &action.ready,
            launch.run.port,
            &launch.run.readiness,
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::{ActionSource, ProjectId};

    fn action(
        kind: ActionKind,
        ready: ActionReady,
        depends_on: &[&ProjectAction],
    ) -> ProjectAction {
        ProjectAction {
            id: ProjectActionId::new(),
            project_id: ProjectId::new(),
            label: "a".to_owned(),
            command: "true".to_owned(),
            kind,
            cwd: None,
            single_instance: true,
            pinned: false,
            ready,
            port: None,
            depends_on: depends_on.iter().map(|dep| dep.id).collect(),
            source: ActionSource::Manual,
            position: 0,
            created_at: Timestamp::now(),
        }
    }

    fn port(timeout_secs: u64) -> ActionReady {
        ActionReady::Port { timeout_secs }
    }

    #[test]
    fn a_step_dependency_gets_the_whole_cap_not_the_slack() {
        let build = action(ActionKind::Build, ActionReady::None, &[]);
        let test = action(ActionKind::Test, ActionReady::None, &[&build]);
        assert_eq!(
            dependency_budget(&[build.clone(), test.clone()], &test, 0),
            MAX_DEPENDENCY_WAIT
        );
    }

    #[test]
    fn probe_timeouts_add_along_a_chain() {
        let db = action(ActionKind::Custom, port(30), &[]);
        let api = action(ActionKind::Dev, port(90), &[&db]);
        let web = action(ActionKind::Dev, ActionReady::None, &[&api]);
        let unprobed = action(ActionKind::Dev, ActionReady::None, &[]);
        let all = [db, api, web.clone(), unprobed.clone()];
        assert_eq!(dependency_budget(&all, &web, 0), Duration::from_secs(120));
        let after_unprobed = action(ActionKind::Test, ActionReady::None, &[&unprobed]);
        assert_eq!(
            dependency_budget(&all, &after_unprobed, 0),
            Duration::ZERO,
            "an unprobed server lets its dependents go once it runs"
        );
    }
}
