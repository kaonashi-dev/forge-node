//! In-memory domain state, request dispatch, and the terminal-runtime methods
//! the PTY loop calls.
//!
//! All state lives behind one `Mutex<Inner>`. Handlers are synchronous and run
//! off the async accept loop via `spawn_blocking` (git and PTY spawns block), so
//! the lock is never held across an `.await`. Lock order is always
//! `inner` → `registry` / `pending_notices`; neither leaf lock takes `inner`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use agents::AgentRegistry;
use domain::{
    AgentDescriptor, AgentProfile, AgentProfileId, AgentProviderId, ChildWorkspacePolicy,
    ContextArtifactRef, ContextEnvelope, ContextId, DetectionResult, DetectionStatus, EnvSource,
    LaunchAgentRequest, Project, ProjectGroup, ProjectGroupId, ProjectId, PtySize,
    ResolvedEnvironment, ScrollbackRows, Session, SessionId, SessionKind, SessionRole,
    SessionState, SessionTitle, ShareCleanup, ShareRule, ShareRuleId, ShareStrategy, ShareTrigger,
    SpawnSpec, TerminalId, Timestamp, Workspace, WorkspaceId, WorkspaceKind, WorkspaceStatus,
    DEFAULT_SCROLLBACK_TAIL, MAX_GRAPH_DEPTH, MAX_SHARE_RULES,
};
use persistence::Db;
use protocol::{
    DaemonEvent, DaemonStats, ErrorCode, NoticeLevel, ProtocolError, ProviderInfo,
    RemoveProjectPolicy, Request, Response, SendContextSpawn, SessionsByState, Signal,
};
use terminal_core::{AlacrittyEngine, DeltaBuilder, PtyBackend, TerminalEngine};

use crate::config::Config;
use crate::environment::ShellEnvironmentService;
use crate::shares::{self, apply as share_apply, ShareContext};
use crate::terminal::{pty_loop, TerminalRuntime};
use crate::terminfo::{self, TermSelection};

/// ADR-008: at most one `git status` per workspace every 2 s.
const GIT_STATUS_THROTTLE: Duration = Duration::from_secs(2);

/// Caps one `FetchScrollback` so a wire `u32` cannot materialise the grid.
const MAX_SCROLLBACK_FETCH: u32 = 4_096;

/// Caps notices queued before the first client connects.
const MAX_PENDING_NOTICES: usize = 64;

/// Lines a handoff capture reads back when the caller names no number.
const DEFAULT_TRANSCRIPT_LINES: u32 = 800;

/// Caps one capture so a wire `u32` cannot materialise the whole scrollback.
const MAX_TRANSCRIPT_LINES: u32 = 4_096;

/// Bytes a handoff capture returns when the caller names no number.
///
/// The prompt built from this becomes one trailing argv entry, and Linux caps
/// a single argument at 128 KiB (`MAX_ARG_STRLEN`). This leaves room for the
/// rest of the prompt inside that.
const DEFAULT_TRANSCRIPT_BYTES: u32 = 36_000;

/// Hard cap on one capture, well inside the smallest per-argument limit.
const MAX_TRANSCRIPT_BYTES: u32 = 96_000;

/// Sessions a review header describes. Each one costs a `rev-list --count`,
/// which is the only cost here that scales with the session list.
const MAX_REVIEW_SESSIONS: usize = 20;

pub(crate) struct Inner {
    db: Db,
    project_groups: HashMap<ProjectGroupId, ProjectGroup>,
    pub(crate) projects: HashMap<ProjectId, Project>,
    pub(crate) workspaces: HashMap<WorkspaceId, Workspace>,
    sessions: HashMap<SessionId, Session>,
    terminals: HashMap<TerminalId, TerminalRuntime>,
    pub(crate) agents: AgentRegistry,
    /// Provider then name — the order every launch menu shows.
    profiles: Vec<AgentProfile>,
    env: ShellEnvironmentService,
    /// Cached per provider so a single-provider refresh does not rerun `detect_all`.
    detections: HashMap<AgentProviderId, DetectionResult>,
    /// Already reported by the idle sweeper; one quiet spell produces one notice.
    idle_warned: HashSet<SessionId>,
    /// Provider session id this launch was resumed from. Runtime-only, like `terminal_id`.
    resumed_from: HashMap<SessionId, String>,
    /// Sessions launched in the provider's read-only mode (§16.9). Runtime-only
    /// like `resumed_from`, and remembered for the *opposite* reason the
    /// initial prompt is forgotten: a restarted review that came back able to
    /// write would be a different session wearing the same name.
    read_only: HashSet<SessionId>,
    /// Coalesces `FetchRemote` per project. The worker clears this on exit.
    fetching: HashSet<ProjectId>,
    /// Coalesces `CreatePullRequest` per workspace (push + `gh`).
    pr_opening: HashSet<WorkspaceId>,
    /// Workspaces with a Juva draft in flight, coalesced like `pr_opening`.
    drafting: HashSet<WorkspaceId>,
    /// Coalesces `RefreshPullRequests` globally — one refresh covers every host.
    pr_refreshing: bool,
    /// Coalesces `ApplyShares` per workspace (§14.2). A second request rides
    /// the first one's run rather than queueing a second copy of it.
    provisioning: HashSet<WorkspaceId>,
    /// Runtime-only: a job is a process and none survive a restart.
    pub(crate) jobs: HashMap<domain::JobId, domain::Job>,
    pub(crate) job_queue: std::collections::VecDeque<crate::jobs::PendingJob>,
    pub(crate) job_processes: HashMap<domain::JobId, crate::jobs::JobProcess>,
    pub(crate) max_concurrent_jobs: usize,
    /// Last `git status` per workspace (ADR-008 throttle).
    status_checks: HashMap<WorkspaceId, Instant>,
    /// Last usage reading per *account* — one provider can report several
    /// (§13.4); `GetSnapshot` must not re-probe CLIs. Kept in probe order so
    /// the status bar does not reshuffle between sweeps.
    usage: Vec<domain::ProviderUsage>,
    /// Login-shell fallback notice, once per daemon rather than once per spawn.
    env_fallback_noticed: bool,
    /// `TERM`/`TERMINFO` for shells. Filesystem probe; cannot change mid-session.
    term_selection: Option<TermSelection>,
}

/// Shared as `Arc<Daemon>` across the accept loop and PTY threads.
pub struct Daemon {
    inner: Mutex<Inner>,
    pub(crate) registry: crate::registry::ClientRegistry,
    config: Config,
    pty_backend: Box<dyn PtyBackend>,
    worktrees_root: PathBuf,
    /// Queued while no client is connected; replayed on the first snapshot.
    pending_notices: Mutex<Vec<DaemonEvent>>,
    /// Own lock: transcript scan is filesystem IO and must not sit on the core lock.
    external_agents: Mutex<crate::external_agents::Cache>,
    /// Own lock: `gh` must not serialize PTY work.
    pull_requests: Mutex<crate::pull_requests::Cache>,
    /// Own lock: a month of JSONL must not sit in front of a keystroke.
    usage_stats: Mutex<crate::usage_stats::Cache>,
    pub instance_id: String,
    pub version: String,
    pub started_at: Timestamp,
    shutdown: AtomicBool,
    pub(crate) resetting: AtomicBool,
    /// Absolute paths of the Forge attention assets, or `None` when install
    /// failed. Injected at launch by [`agents::inject_attention`].
    attention_assets: Option<agents::AttentionAssets>,
}

struct ResetGuard<'a>(&'a AtomicBool);

impl Drop for ResetGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

impl Daemon {
    /// Load state, reconcile paths and worktrees, then start background sweepers.
    /// Runs before the socket is bound: notices raised here queue until the first snapshot.
    pub fn start(
        db: Db,
        config: Config,
        worktrees_root: PathBuf,
        instance_id: String,
        version: String,
    ) -> Result<Arc<Daemon>, String> {
        let daemon = Self::load(db, config, worktrees_root, instance_id, version)?;

        daemon.validate_known_paths();
        daemon.rescan_worktrees();

        {
            let d = daemon.clone();
            std::thread::spawn(move || {
                let results = d.detect_agents();
                d.registry
                    .broadcast_domain(DaemonEvent::AgentDetectionChanged { results });
            });
        }

        {
            let d = daemon.clone();
            std::thread::spawn(move || d.run_usage_sweeper());
        }

        // Before anything can be started, settle what the previous daemon left
        // running: a feature whose attempt has no process is stuck otherwise.
        daemon.recover_harness_after_restart();

        {
            let d = daemon.clone();
            if let Err(error) = std::thread::Builder::new()
                .name("forge-harness-watchdog".into())
                .spawn(move || d.run_harness_watchdog())
            {
                tracing::warn!(%error, "could not start the harness watchdog");
            }
        }

        if daemon.config.idle_policy().is_enabled() {
            let d = daemon.clone();
            std::thread::spawn(move || d.run_idle_sweeper());
        }

        if daemon.config.github.refresh_secs > 0 {
            let d = daemon.clone();
            if let Err(error) = std::thread::Builder::new()
                .name("forge-pr-auto-refresh".into())
                .spawn(move || d.run_pull_request_sweeper())
            {
                tracing::warn!(%error, "could not start pull-request refresh timer");
            }
        }

        Ok(daemon)
    }

    /// Load caches without startup scans or background threads (used by tests).
    pub(crate) fn load(
        db: Db,
        config: Config,
        worktrees_root: PathBuf,
        instance_id: String,
        version: String,
    ) -> Result<Arc<Daemon>, String> {
        Self::load_with_backend(
            db,
            config,
            worktrees_root,
            instance_id,
            version,
            Box::new(terminal_core::PortablePtyBackend::new()),
        )
    }

    /// [`Daemon::load`] with an explicit [`PtyBackend`] (tests inject `FakePtyBackend`).
    pub fn load_with_backend(
        db: Db,
        config: Config,
        worktrees_root: PathBuf,
        instance_id: String,
        version: String,
        pty_backend: Box<dyn PtyBackend>,
    ) -> Result<Arc<Daemon>, String> {
        // A PTY never survives the daemon. Default: drop session rows. Opt-in: mark them Orphaned.
        if config.sessions.persist_history {
            let orphaned = db.reconcile_orphaned().map_err(|e| e.to_string())?;
            if orphaned > 0 {
                tracing::info!(orphaned, "reconciled orphaned sessions on startup");
            }
        } else {
            let dropped = db.purge_sessions().map_err(|e| e.to_string())?;
            if dropped > 0 {
                tracing::info!(dropped, "dropped the session history on startup");
            }
        }

        let project_groups = db
            .project_groups()
            .list()
            .map_err(|e| e.to_string())?
            .into_iter()
            .map(|group| (group.id, group))
            .collect::<HashMap<_, _>>();
        let projects = db
            .projects()
            .list()
            .map_err(|e| e.to_string())?
            .into_iter()
            .map(|p| (p.id, p))
            .collect::<HashMap<_, _>>();
        let mut workspaces = HashMap::new();
        for p in projects.keys() {
            for w in db
                .workspaces()
                .list_by_project(*p)
                .map_err(|e| e.to_string())?
            {
                workspaces.insert(w.id, w);
            }
        }
        let sessions = db
            .sessions()
            .list()
            .map_err(|e| e.to_string())?
            .into_iter()
            .map(|s| (s.id, s))
            .collect::<HashMap<_, _>>();

        let shell_override = if config.sessions.shell.is_empty() {
            None
        } else {
            Some(config.sessions.shell.clone())
        };

        let mut agents = AgentRegistry::new();
        for (provider_id, path) in db.provider_overrides().list().map_err(|e| e.to_string())? {
            agents.set_override(&provider_id, Some(path));
        }

        let profiles = db.agent_profiles().list().map_err(|e| e.to_string())?;

        let inner = Inner {
            db,
            project_groups,
            projects,
            workspaces,
            sessions,
            terminals: HashMap::new(),
            agents,
            profiles,
            env: ShellEnvironmentService::new(shell_override),
            detections: HashMap::new(),
            idle_warned: HashSet::new(),
            resumed_from: HashMap::new(),
            read_only: HashSet::new(),
            fetching: HashSet::new(),
            pr_opening: HashSet::new(),
            drafting: HashSet::new(),
            pr_refreshing: false,
            provisioning: HashSet::new(),
            jobs: HashMap::new(),
            job_queue: std::collections::VecDeque::new(),
            job_processes: HashMap::new(),
            max_concurrent_jobs: crate::jobs::DEFAULT_MAX_CONCURRENT_JOBS,
            status_checks: HashMap::new(),
            usage: Vec::new(),
            env_fallback_noticed: false,
            term_selection: None,
        };

        let attention_assets = install_attention_assets(&worktrees_root);

        Ok(Arc::new(Daemon {
            inner: Mutex::new(inner),
            registry: crate::registry::ClientRegistry::new(),
            config,
            pty_backend,
            worktrees_root,
            pending_notices: Mutex::new(Vec::new()),
            external_agents: Mutex::new(crate::external_agents::Cache::default()),
            pull_requests: Mutex::new(crate::pull_requests::Cache::default()),
            usage_stats: Mutex::new(crate::usage_stats::Cache::default()),
            instance_id,
            version,
            started_at: Timestamp::now(),
            shutdown: AtomicBool::new(false),
            resetting: AtomicBool::new(false),
            attention_assets,
        }))
    }

    pub fn registry(&self) -> &crate::registry::ClientRegistry {
        &self.registry
    }

    pub fn is_shutting_down(&self) -> bool {
        self.shutdown.load(Ordering::SeqCst)
    }

    /// Signal-path shutdown: the accept loop stops and unlinks its socket.
    pub fn request_shutdown(&self) {
        self.shutdown.store(true, Ordering::SeqCst);
    }

    // --- Notices ---

    /// Log and broadcast a notice; queue it if no client is connected yet.
    fn notice(&self, level: NoticeLevel, message: impl Into<String>) {
        let message = message.into();
        match level {
            NoticeLevel::Error => tracing::error!(%message, "daemon notice"),
            NoticeLevel::Info => tracing::info!(%message, "daemon notice"),
            _ => tracing::warn!(%message, "daemon notice"),
        }
        let event = DaemonEvent::DaemonNotice { level, message };
        if self.registry.client_count() > 0 {
            self.registry.broadcast_domain(event);
            return;
        }
        if let Ok(mut queued) = self.pending_notices.lock() {
            if queued.len() < MAX_PENDING_NOTICES {
                queued.push(event);
            }
        }
    }

    fn take_pending_notices(&self) -> Vec<DaemonEvent> {
        match self.pending_notices.lock() {
            Ok(mut queued) => std::mem::take(&mut *queued),
            // Poisoned notice queue is not worth taking the daemon down.
            Err(_) => Vec::new(),
        }
    }

    // --- Startup reconciliation ---

    /// Missing project/main-workspace paths are kept (unmounted volume) and noticed.
    /// Git worktrees are [`Self::rescan_worktrees`]'s job: it drops a vanished
    /// worktree, so warning here would toast rows that disappear a moment later.
    fn validate_known_paths(&self) {
        let missing: Vec<String> = {
            let inner = self.lock();
            let projects = inner
                .projects
                .values()
                .filter(|p| !p.root_path.exists())
                .map(|p| format!("project \"{}\" ({})", p.name, p.root_path.display()));
            let workspaces = inner
                .workspaces
                .values()
                .filter(|w| w.kind != WorkspaceKind::GitWorktree && !w.path.exists())
                .map(|w| format!("workspace {}", w.path.display()));
            projects.chain(workspaces).collect()
        };
        for what in missing {
            self.notice(
                NoticeLevel::Warning,
                format!("{what} no longer exists on disk; keeping it in the model"),
            );
        }
    }

    /// Re-list git worktrees and add/drop `Workspace` rows. Never deletes from disk.
    fn rescan_worktrees(self: &Arc<Self>) {
        let projects: Vec<(ProjectId, PathBuf, PathBuf)> = {
            let inner = self.lock();
            inner
                .projects
                .values()
                .filter_map(|p| p.git_root.clone().map(|gr| (p.id, gr, p.root_path.clone())))
                .collect()
        };

        for (project_id, git_root, root_path) in projects {
            self.rescan_project_worktrees(project_id, &git_root, &root_path);
        }
    }

    fn rescan_project_worktrees(
        self: &Arc<Self>,
        project_id: ProjectId,
        git_root: &Path,
        root_path: &Path,
    ) {
        if !git_root.exists() {
            return;
        }
        let entries = match git_service::list_worktrees(git_root) {
            Ok(entries) => entries,
            Err(e) => {
                tracing::warn!(git_root = %git_root.display(), error = %e, "worktree rescan failed");
                return;
            }
        };

        let main_path = canonical_or_self(root_path);
        let listed: HashMap<PathBuf, Option<String>> = entries
            .into_iter()
            .filter(|e| !e.bare)
            .map(|e| (canonical_or_self(&e.path), e.branch))
            .filter(|(path, _)| *path != main_path)
            .collect();

        let (added, removed) = self.reconcile_project_worktrees(project_id, &listed);
        for ws in added {
            let workspace_id = ws.id;
            self.registry
                .broadcast_domain(DaemonEvent::WorkspaceCreated(ws));
            // A worktree made in a terminal needs the project's files as much
            // as one Forge created; the difference was only who ran `git`.
            self.start_provisioning(workspace_id, ShareTrigger::Adopted, None);
        }
        for workspace_id in removed {
            self.registry
                .broadcast_domain(DaemonEvent::WorkspaceRemoved { workspace_id });
        }
    }

    fn reconcile_project_worktrees(
        self: &Arc<Self>,
        project_id: ProjectId,
        listed: &HashMap<PathBuf, Option<String>>,
    ) -> (Vec<Workspace>, Vec<WorkspaceId>) {
        let mut added = Vec::new();
        let mut removed = Vec::new();
        let mut keep_notices = Vec::new();
        {
            let mut inner = self.lock();
            let known: HashMap<PathBuf, WorkspaceId> = inner
                .workspaces
                .values()
                .filter(|w| w.project_id == project_id)
                .map(|w| (canonical_or_self(&w.path), w.id))
                .collect();

            for (path, branch) in listed {
                if known.contains_key(path) {
                    continue;
                }
                let ws = Workspace {
                    id: WorkspaceId::new(),
                    project_id,
                    kind: WorkspaceKind::GitWorktree,
                    path: path.clone(),
                    branch: branch.clone(),
                    display_name: inferred_display_name(path, branch.as_deref()),
                    managed_by_app: false,
                    created_at: Timestamp::now(),
                    status: WorkspaceStatus::default(),
                };
                if let Err(e) = inner.db.workspaces().upsert(&ws) {
                    tracing::warn!(path = %ws.path.display(), error = %e, "persist rescanned worktree");
                    continue;
                }
                inner.workspaces.insert(ws.id, ws.clone());
                added.push(ws);
            }

            let vanished: Vec<WorkspaceId> = inner
                .workspaces
                .values()
                .filter(|w| {
                    w.project_id == project_id
                        && w.kind == WorkspaceKind::GitWorktree
                        && !listed.contains_key(&canonical_or_self(&w.path))
                        && !w.path.exists()
                })
                .map(|w| w.id)
                .collect();
            for workspace_id in vanished {
                // Sessions still reference it; the FK would refuse the delete.
                if inner
                    .sessions
                    .values()
                    .any(|s| s.workspace_id == workspace_id)
                {
                    keep_notices.push(format!(
                        "worktree of workspace {workspace_id} is gone but still has sessions; keeping it"
                    ));
                    continue;
                }
                let managed = inner
                    .workspaces
                    .get(&workspace_id)
                    .is_some_and(|w| w.managed_by_app);
                if let Err(e) = inner.db.workspaces().delete(workspace_id) {
                    tracing::warn!(%workspace_id, error = %e, "drop vanished worktree");
                    continue;
                }
                inner.workspaces.remove(&workspace_id);
                if managed {
                    keep_notices.push(format!(
                        "worktree {workspace_id} created by Forge was removed outside the app"
                    ));
                }
                removed.push(workspace_id);
            }
        }
        for message in keep_notices {
            self.notice(NoticeLevel::Warning, message);
        }
        (added, removed)
    }

    // --- Request dispatch ---

    pub fn handle_request(self: &Arc<Self>, request: Request) -> Result<Response, ProtocolError> {
        if self.resetting.load(Ordering::SeqCst) && !matches!(&request, Request::FactoryReset) {
            return Err(ProtocolError::conflict("factory reset is in progress"));
        }
        match request {
            Request::GetSnapshot => Ok(self.snapshot()),
            Request::StopDaemon { kill_sessions } => self.stop_daemon(kill_sessions),
            Request::FactoryReset => self.factory_reset(),
            Request::GetAppState { key } => self.get_app_state(&key),
            Request::SetAppState { key, value } => self.set_app_state(&key, &value),
            Request::RefreshPullRequests => self.refresh_pull_requests(),
            Request::GetStats => Ok(Response::DaemonStats(self.collect_stats())),
            Request::ListHarnessFeatures { project_id } => self.list_harness_features(project_id),
            Request::GetHarnessFeature {
                project_id,
                feature_id,
            } => self.get_harness_feature(project_id, feature_id),
            Request::GetHarnessTimeline {
                project_id,
                feature_id,
            } => self.get_harness_timeline(project_id, feature_id),
            Request::ReadHarnessArtifact {
                project_id,
                feature_id,
                artifact,
            } => self.read_harness_artifact(project_id, feature_id, artifact),
            Request::RegisterHarnessFeature {
                project_id,
                workspace_id,
                spec_raw,
                title,
            } => self.register_harness_feature(project_id, workspace_id, spec_raw, title),
            Request::RegisterHarnessFromIssue {
                project_id,
                workspace_id,
                issue_number,
            } => self.register_harness_from_issue(project_id, workspace_id, issue_number),
            Request::HarnessAdvance {
                project_id,
                feature_id,
                revision,
                action,
            } => self.harness_advance(project_id, feature_id, revision, action),
            Request::LinkHarnessSession {
                project_id,
                feature_id,
                session_id,
            } => self.link_harness_session(project_id, feature_id, session_id),
            Request::ValidateHarness { project_id } => self.validate_harness(project_id),
            Request::RunHarnessStep {
                project_id,
                feature_id,
                step,
                force,
            } => self.run_harness_step_forced(project_id, feature_id, step, force),
            Request::AskHarness {
                project_id,
                question,
                resume_from,
            } => self.ask_harness(project_id, question, resume_from),

            Request::StartJob { request } => self.start_job(request),
            Request::CancelJob { job_id } => self.cancel_job(job_id),
            Request::ListJobs => Ok(self.list_jobs()),
            Request::ReadJobLog { job_id, from_line } => self.read_job_log(job_id, from_line),

            Request::AddProject { path } => self.add_project(&path),
            Request::AddProjectToGroup {
                path,
                project_group_id,
            } => self.add_project_to_group(&path, project_group_id),
            Request::CreateProjectGroup { name } => self.create_project_group(&name),
            Request::RenameProjectGroup {
                project_group_id,
                name,
            } => self.rename_project_group(project_group_id, &name),
            Request::RemoveProjectGroup { project_group_id } => {
                self.remove_project_group(project_group_id)
            }
            Request::MoveProject {
                project_id,
                project_group_id,
            } => self.move_project(project_id, project_group_id),
            Request::RemoveProject { project_id, policy } => {
                self.remove_project(project_id, policy)
            }
            Request::RefreshProject { project_id } => self.refresh_project(project_id),
            Request::RenameProject { project_id, name } => self.rename_project(project_id, &name),
            Request::SetProjectIcon { project_id, icon } => {
                self.set_project_icon(project_id, icon.as_deref())
            }

            Request::ListWorkspaces { project_id } => self.list_workspaces(project_id),
            Request::CreateWorktree {
                project_id,
                branch,
                base,
                name,
            } => self.create_worktree(project_id, &branch, base.as_deref(), name.as_deref()),
            Request::RemoveWorktree {
                workspace_id,
                force,
            } => self.remove_worktree(workspace_id, force),
            Request::RenameWorkspace {
                workspace_id,
                display_name,
            } => self.rename_workspace(workspace_id, display_name),
            Request::RefreshWorkspaceStatus { workspace_id } => {
                self.refresh_workspace_status(workspace_id)
            }

            Request::ListBranches { project_id } => self.list_branches(project_id),
            Request::FetchRemote { project_id, remote } => {
                self.fetch_remote(project_id, remote.as_deref())
            }
            Request::GetChangeContext { workspace_id } => self.get_change_context(workspace_id),
            Request::GetWorkspaceDiff {
                workspace_id,
                context_lines,
            } => self.get_workspace_diff(workspace_id, context_lines),
            Request::GetSessionChanges { session_id } => self.get_session_changes(session_id),
            Request::GetWorkspaceReview {
                workspace_id,
                context_lines,
            } => self.get_workspace_review(workspace_id, context_lines),
            Request::GetSessionTranscript {
                session_id,
                max_lines,
                max_bytes,
            } => self.get_session_transcript(session_id, max_lines, max_bytes),
            Request::GetExternalTranscript {
                session_id,
                provider,
                profile_id,
                max_turns,
                max_bytes,
            } => self.get_external_transcript(
                &session_id,
                &provider,
                profile_id,
                max_turns,
                max_bytes,
            ),
            Request::DeleteExternalSession {
                session_id,
                provider,
                profile_id,
            } => self.delete_external_session(&session_id, &provider, profile_id),
            Request::ListFiles { workspace_id } => self.list_files(workspace_id),
            Request::ReadFile { workspace_id, path } => self.read_file(workspace_id, &path),
            Request::WriteFile {
                workspace_id,
                path,
                text,
                expected_revision,
            } => self.write_file(workspace_id, &path, &text, &expected_revision),
            Request::CreatePath {
                workspace_id,
                path,
                directory,
            } => self.create_path(workspace_id, &path, directory),
            Request::RenamePath {
                workspace_id,
                from,
                to,
            } => self.rename_path(workspace_id, &from, &to),
            Request::DeletePath { workspace_id, path } => self.delete_path(workspace_id, &path),
            Request::SearchFiles {
                workspace_id,
                query,
                kind,
                limit,
            } => self.search_files(workspace_id, &query, kind, limit),
            Request::DraftWithJuva { workspace_id, kind } => {
                self.draft_with_juva(workspace_id, kind)
            }
            Request::GetRebaseState { workspace_id } => self.get_rebase_state(workspace_id),
            Request::ContinueRebase { workspace_id } => self.continue_rebase(workspace_id),
            Request::AbortRebase { workspace_id } => self.abort_rebase(workspace_id),
            Request::MarkConflictResolved {
                workspace_id,
                paths,
            } => self.mark_conflict_resolved(workspace_id, &paths),
            Request::CreateCommit {
                workspace_id,
                message,
            } => self.create_commit(workspace_id, &message),
            Request::CreatePullRequest {
                workspace_id,
                title,
                body,
                base,
            } => self.create_pull_request(workspace_id, title, body, base),

            Request::CreateShellSession {
                workspace_id,
                parent,
                role,
            } => self.create_session(
                workspace_id,
                SessionKind::Shell,
                None,
                None,
                parent,
                role,
                None,
                None,
                false,
            ),
            Request::CreateAgentSession {
                workspace_id,
                provider_id,
                profile_id,
                parent,
                role,
                resume,
                initial_prompt,
                read_only,
            } => self.create_session(
                workspace_id,
                SessionKind::Agent,
                Some(provider_id),
                profile_id,
                parent,
                role,
                resume,
                initial_prompt,
                read_only,
            ),
            Request::CreateChildSession {
                parent_session_id,
                kind,
                provider_id,
                profile_id,
                role,
                workspace_policy,
                initial_prompt,
            } => self.create_child_session(
                parent_session_id,
                kind,
                provider_id,
                profile_id,
                role,
                workspace_policy,
                initial_prompt,
            ),
            Request::KillSession { session_id } => self.kill_session(session_id),
            Request::CloseSession { session_id } => self.close_session(session_id),
            Request::RestartSession { session_id } => self.restart_session(session_id),
            Request::RenameSession { session_id, title } => self.rename_session(session_id, title),
            Request::SetSessionRole { session_id, role } => self.set_session_role(session_id, role),
            Request::CreateContextEnvelope { envelope } => {
                let inner = self.lock();
                if !inner.sessions.contains_key(&envelope.source_session_id) {
                    return Err(ProtocolError::not_found("source session"));
                }
                if let Some(target) = envelope.target_session_id {
                    if !inner.sessions.contains_key(&target) {
                        return Err(ProtocolError::not_found("target session"));
                    }
                }
                inner.db.context().insert(&envelope).map_err(db_err)?;
                Ok(Response::Ack)
            }
            Request::SendContext {
                source_session_id,
                target_session_id,
                spawn,
                summary,
                instructions,
                include_transcript,
                max_transcript_bytes,
            } => self.send_context(
                source_session_id,
                target_session_id,
                spawn,
                summary,
                instructions,
                include_transcript,
                max_transcript_bytes,
            ),
            Request::ListContextEnvelopes { session_id } => {
                let inner = self.lock();
                if !inner.sessions.contains_key(&session_id) {
                    return Err(ProtocolError::not_found("session"));
                }
                let envelopes = inner
                    .db
                    .context()
                    .list_for_session(session_id)
                    .map_err(db_err)?;
                Ok(Response::ContextEnvelopes(envelopes))
            }

            Request::AttachTerminal { terminal_id, size } => {
                self.attach_terminal(terminal_id, size)
            }
            Request::DetachTerminal { .. } => Ok(Response::Ack),
            Request::WriteTerminalInput { terminal_id, bytes } => {
                self.write_terminal_input(terminal_id, &bytes)
            }
            Request::ResizeTerminal { terminal_id, size } => {
                self.resize_terminal(terminal_id, size)
            }
            Request::FetchScrollback {
                terminal_id,
                from_line,
                count,
            } => self.fetch_scrollback(terminal_id, from_line, count),
            Request::SendSignal { session_id, signal } => self.send_signal(session_id, signal),

            Request::ListAgentProviders => Ok(Response::Providers(self.provider_infos())),
            Request::GetUsageAnalytics { window_days } => Ok(Response::UsageAnalytics(Box::new(
                self.usage_analytics(window_days.unwrap_or(0)),
            ))),
            Request::ListProviderUsage => {
                let usage = self.collect_usage();
                self.registry
                    .broadcast_domain(DaemonEvent::ProviderUsageChanged {
                        usage: usage.clone(),
                    });
                Ok(Response::ProviderUsage(usage))
            }
            Request::RefreshAgentDetection { provider_id } => {
                let results = match provider_id {
                    Some(id) => vec![self.detect_agent(&id)?],
                    None => self.detect_agents(),
                };
                self.registry
                    .broadcast_domain(DaemonEvent::AgentDetectionChanged { results });
                Ok(Response::Ack)
            }
            Request::SetProviderExecutable { provider_id, path } => {
                {
                    let mut inner = self.lock();
                    inner.agents.set_override(&provider_id, path.clone());
                    inner
                        .db
                        .provider_overrides()
                        .set(&provider_id, path.as_ref())
                        .map_err(db_err)?;
                }
                let results = self.detect_agents();
                self.registry
                    .broadcast_domain(DaemonEvent::AgentDetectionChanged { results });
                Ok(Response::Ack)
            }
            Request::SaveAgentProfile { profile } => self.save_agent_profile(profile),
            Request::RemoveAgentProfile { profile_id } => self.remove_agent_profile(profile_id),

            Request::DetectShareCandidates { project_id } => {
                self.detect_share_candidates(project_id)
            }
            Request::SetProjectShares { project_id, rules } => {
                self.set_project_shares(project_id, rules)
            }
            Request::RemoveShareRule {
                project_id,
                rule_id,
                cleanup,
            } => self.remove_share_rule(project_id, rule_id, cleanup),
            Request::PreviewShares { workspace_id } => self.preview_shares(workspace_id),
            Request::GetShareStatus { workspace_id } => self.share_status(workspace_id),
            Request::ApplyShares { workspace_id, only } => self.apply_shares(workspace_id, only),
            Request::AdoptIntoShareStore { project_id, path } => {
                self.adopt_into_share_store(project_id, path)
            }
            Request::MaterializeFromShareStore {
                project_id,
                path,
                workspace_id,
            } => self.materialize_from_share_store(project_id, path, workspace_id),

            _ => Err(ProtocolError::new(
                ErrorCode::InvalidRequest,
                "unsupported request in this daemon build",
            )),
        }
    }

    /// Runtime counters. Lock order `inner → registry`; neither lock across I/O.
    fn collect_stats(&self) -> DaemonStats {
        let (sessions_by_state, open_terminals) = {
            let inner = self.lock();
            let mut sessions_by_state = SessionsByState {
                starting: 0,
                running: 0,
                exited: 0,
                failed: 0,
                orphaned: 0,
            };
            for session in inner.sessions.values() {
                match &session.state {
                    SessionState::Starting => sessions_by_state.starting += 1,
                    SessionState::Running => sessions_by_state.running += 1,
                    SessionState::Exited { .. } => sessions_by_state.exited += 1,
                    SessionState::Failed { .. } => sessions_by_state.failed += 1,
                    SessionState::Orphaned => sessions_by_state.orphaned += 1,
                    _ => {}
                }
            }
            let open_terminals = inner.terminals.len() as u64;
            (sessions_by_state, open_terminals)
        };
        let connected_clients = self.registry.client_count() as u64;
        let elapsed = Timestamp::now().as_offset() - self.started_at.as_offset();
        let uptime_secs = elapsed.whole_seconds().max(0) as u64;
        DaemonStats {
            uptime_secs,
            sessions_by_state,
            open_terminals,
            connected_clients,
        }
    }

    /// Recover from poison: `Inner` is a cache over SQLite, not an invariant a
    /// panicking handler can silently corrupt. `expect` here made one bad
    /// request a permanently dead daemon.
    pub(crate) fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|poisoned| {
            tracing::error!("daemon core lock was poisoned by a panicking handler; recovering");
            poisoned.into_inner()
        })
    }

    /// Path of a checkout that belongs to `project_id`. A workspace from another
    /// project is refused, not silently resolved.
    pub(crate) fn workspace_path_in(
        &self,
        project_id: ProjectId,
        workspace_id: domain::WorkspaceId,
    ) -> Result<PathBuf, ProtocolError> {
        let inner = self.lock();
        let workspace = inner
            .workspaces
            .get(&workspace_id)
            .ok_or_else(|| ProtocolError::not_found("workspace"))?;
        if workspace.project_id != project_id {
            return Err(ProtocolError::new(
                ErrorCode::InvalidRequest,
                "workspace belongs to another project",
            ));
        }
        Ok(workspace.path.clone())
    }

    /// Where this project's `harness/` lives. See [`harness_root_of`].
    pub(crate) fn harness_root_for(&self, project_id: ProjectId) -> Result<PathBuf, ProtocolError> {
        let inner = self.lock();
        inner
            .projects
            .get(&project_id)
            .map(harness_root_of)
            .ok_or_else(|| ProtocolError::not_found("project"))
    }

    // --- Global ---

    fn snapshot(&self) -> Response {
        let (
            project_groups,
            projects,
            workspaces,
            sessions,
            providers,
            agent_profiles,
            worktree_shares,
            app_state,
            usage,
        ) = {
            let inner = self.lock();
            let providers = self.provider_infos_locked(&inner);
            let app_state = inner.db.app_state().list().unwrap_or_default();
            let worktree_shares = inner.db.shares().list_all().unwrap_or_default();
            (
                inner.project_groups.values().cloned().collect::<Vec<_>>(),
                inner.projects.values().cloned().collect::<Vec<_>>(),
                inner.workspaces.values().cloned().collect::<Vec<_>>(),
                inner.sessions.values().cloned().collect::<Vec<_>>(),
                providers,
                inner.profiles.clone(),
                worktree_shares,
                app_state,
                inner.usage.clone(),
            )
        };
        // Filesystem IO: after the core lock, through the TTL cache.
        let external_agents = self
            .external_agents
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .discover(&projects, &workspaces, &agent_profiles);
        // Clone-only: no git/`gh` on this path.
        let pull_requests = self
            .pull_requests
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .snapshot();
        let jobs = {
            let inner = self.lock();
            let mut jobs: Vec<domain::Job> = inner.jobs.values().cloned().collect();
            jobs.sort_by_key(|job| job.id.as_uuid());
            jobs
        };
        let response = Response::Snapshot {
            project_groups,
            projects,
            workspaces,
            sessions,
            providers,
            agent_profiles,
            worktree_shares,
            app_state,
            external_agents,
            pull_requests,
            jobs,
            usage,
        };
        for event in self.take_pending_notices() {
            self.registry.broadcast_domain(event);
        }
        response
    }

    fn stop_daemon(self: &Arc<Self>, kill_sessions: bool) -> Result<Response, ProtocolError> {
        let live: Vec<SessionId> = {
            let inner = self.lock();
            inner
                .sessions
                .values()
                .filter(|s| s.state.is_active())
                .map(|s| s.id)
                .collect()
        };
        if !live.is_empty() && !kill_sessions {
            return Err(ProtocolError::precondition_failed(format!(
                "{} live session(s); pass kill_sessions to stop anyway",
                live.len()
            )));
        }
        for id in live {
            let _ = self.kill_session(id);
        }
        self.shutdown.store(true, Ordering::SeqCst);
        self.registry
            .broadcast_domain(DaemonEvent::DaemonShuttingDown {
                reason: "StopDaemon requested".into(),
            });
        Ok(Response::Ack)
    }

    fn factory_reset(self: &Arc<Self>) -> Result<Response, ProtocolError> {
        if self.resetting.swap(true, Ordering::SeqCst) {
            return Err(ProtocolError::conflict(
                "factory reset is already in progress",
            ));
        }
        let _reset = ResetGuard(&self.resetting);

        let (session_ids, managed_worktrees) = {
            let inner = self.lock();
            let session_ids = inner.sessions.keys().copied().collect::<Vec<_>>();
            let managed_worktrees = inner
                .workspaces
                .values()
                .filter(|workspace| workspace.managed_by_app)
                .map(|workspace| {
                    let git_root = inner
                        .projects
                        .get(&workspace.project_id)
                        .and_then(|project| project.git_root.clone());
                    (workspace.path.clone(), git_root)
                })
                .collect::<Vec<_>>();
            (session_ids, managed_worktrees)
        };

        self.cancel_all_jobs();
        self.kill_groups_blocking(&self.kill_targets(&session_ids));

        {
            let mut inner = self.lock();
            inner.db.reset().map_err(db_err)?;
            inner.project_groups.clear();
            inner.projects.clear();
            inner.workspaces.clear();
            inner.sessions.clear();
            inner.agents = AgentRegistry::new();
            inner.profiles.clear();
            inner.detections.clear();
            inner.idle_warned.clear();
            inner.resumed_from.clear();
            inner.read_only.clear();
            inner.fetching.clear();
            inner.pr_opening.clear();
            inner.drafting.clear();
            inner.provisioning.clear();
            inner.pr_refreshing = false;
            inner.status_checks.clear();
            inner.usage.clear();
            // PTY threads remove and reap their own runtimes after the group
            // kills above reach EOF; dropping them here would leak children.
        }

        let mut failed_worktrees = Vec::new();
        for (path, git_root) in managed_worktrees {
            let result = git_root.as_deref().map_or_else(
                || Err("its repository is no longer available".to_owned()),
                |root| git_service::remove(root, &path, true).map_err(|error| error.to_string()),
            );
            if let Err(error) = result {
                tracing::warn!(worktree = %path.display(), %error, "factory reset could not remove a managed worktree");
                failed_worktrees.push(path);
            }
        }

        self.external_agents
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .invalidate();
        *self
            .pull_requests
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            crate::pull_requests::Cache::default();
        *self
            .usage_stats
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            crate::usage_stats::Cache::default();

        self.registry.broadcast_domain(DaemonEvent::FactoryReset);
        if !failed_worktrees.is_empty() {
            self.notice(
                NoticeLevel::Warning,
                format!(
                    "factory reset completed, but {} managed worktree(s) could not be removed; see the daemon log",
                    failed_worktrees.len()
                ),
            );
        }

        let daemon = Arc::clone(self);
        if let Err(error) = std::thread::Builder::new()
            .name("forge-reset-detection".into())
            .spawn(move || {
                let results = daemon.detect_agents();
                daemon
                    .registry
                    .broadcast_domain(DaemonEvent::AgentDetectionChanged { results });
            })
        {
            tracing::warn!(%error, "could not refresh agent detection after factory reset");
        }

        Ok(Response::Ack)
    }

    fn get_app_state(&self, key: &str) -> Result<Response, ProtocolError> {
        let inner = self.lock();
        let value = inner.db.app_state().get(key).map_err(db_err)?;
        Ok(Response::AppState { value })
    }

    fn set_app_state(&self, key: &str, value: &str) -> Result<Response, ProtocolError> {
        let inner = self.lock();
        inner.db.app_state().set(key, value).map_err(db_err)?;
        Ok(Response::Ack)
    }

    // --- Projects ---

    fn add_project(&self, path: &Path) -> Result<Response, ProtocolError> {
        self.add_project_in_group(path, None)
    }

    fn add_project_to_group(
        &self,
        path: &Path,
        project_group_id: ProjectGroupId,
    ) -> Result<Response, ProtocolError> {
        self.add_project_in_group(path, Some(project_group_id))
    }

    fn create_project_group(&self, name: &str) -> Result<Response, ProtocolError> {
        let name = validate_project_group_name(name)?;

        let group = ProjectGroup {
            id: ProjectGroupId::new(),
            name: name.to_owned(),
            created_at: Timestamp::now(),
        };
        let mut inner = self.lock();
        inner.db.project_groups().upsert(&group).map_err(db_err)?;
        inner.project_groups.insert(group.id, group.clone());
        drop(inner);
        self.registry
            .broadcast_domain(DaemonEvent::ProjectGroupCreated(group));
        Ok(Response::Ack)
    }

    fn rename_project_group(
        &self,
        project_group_id: ProjectGroupId,
        name: &str,
    ) -> Result<Response, ProtocolError> {
        let name = validate_project_group_name(name)?;
        let mut inner = self.lock();
        let mut group = inner
            .project_groups
            .get(&project_group_id)
            .cloned()
            .ok_or_else(|| ProtocolError::not_found("project group"))?;
        group.name = name.to_owned();
        inner.db.project_groups().upsert(&group).map_err(db_err)?;
        inner.project_groups.insert(group.id, group.clone());
        drop(inner);
        self.registry
            .broadcast_domain(DaemonEvent::ProjectGroupUpdated(group));
        Ok(Response::Ack)
    }

    fn remove_project_group(
        &self,
        project_group_id: ProjectGroupId,
    ) -> Result<Response, ProtocolError> {
        let mut inner = self.lock();
        if !inner.project_groups.contains_key(&project_group_id) {
            return Err(ProtocolError::not_found("project group"));
        }
        let removed = inner
            .db
            .project_groups()
            .delete(project_group_id)
            .map_err(db_err)?;
        if !removed {
            return Err(ProtocolError::not_found("project group"));
        }
        inner.project_groups.remove(&project_group_id);
        let mut moved_projects = Vec::new();
        for project in inner.projects.values_mut() {
            if project.project_group_id == Some(project_group_id) {
                project.project_group_id = None;
                moved_projects.push(project.clone());
            }
        }
        drop(inner);

        for project in moved_projects {
            self.registry
                .broadcast_domain(DaemonEvent::ProjectUpdated(project));
        }
        self.registry
            .broadcast_domain(DaemonEvent::ProjectGroupRemoved { project_group_id });
        Ok(Response::Ack)
    }

    fn move_project(
        &self,
        project_id: ProjectId,
        project_group_id: Option<ProjectGroupId>,
    ) -> Result<Response, ProtocolError> {
        let mut inner = self.lock();
        if project_group_id.is_some_and(|id| !inner.project_groups.contains_key(&id)) {
            return Err(ProtocolError::not_found("project group"));
        }
        let mut project = inner
            .projects
            .get(&project_id)
            .cloned()
            .ok_or_else(|| ProtocolError::not_found("project"))?;
        if project.project_group_id == project_group_id {
            return Ok(Response::Ack);
        }
        project.project_group_id = project_group_id;
        inner.db.projects().upsert(&project).map_err(db_err)?;
        inner.projects.insert(project.id, project.clone());
        drop(inner);
        self.registry
            .broadcast_domain(DaemonEvent::ProjectUpdated(project));
        Ok(Response::Ack)
    }

    fn add_project_in_group(
        &self,
        path: &Path,
        project_group_id: Option<ProjectGroupId>,
    ) -> Result<Response, ProtocolError> {
        let _span = tracing::info_span!("project.add").entered();
        let root = std::fs::canonicalize(path).map_err(|e| {
            ProtocolError::new(
                ErrorCode::IoError,
                format!("canonicalize {}: {e}", path.display()),
            )
        })?;

        let git_root = git_service::discover_root(&root).ok();
        let (branch, worktrees) = match &git_root {
            Some(gr) => (
                git_service::current_branch(gr).ok().flatten(),
                git_service::list_worktrees(gr).unwrap_or_default(),
            ),
            None => (None, Vec::new()),
        };

        let mut inner = self.lock();
        if project_group_id.is_some_and(|id| !inner.project_groups.contains_key(&id)) {
            return Err(ProtocolError::not_found("project group"));
        }
        if inner.projects.values().any(|p| p.root_path == root) {
            return Err(ProtocolError::conflict(format!(
                "project already added: {}",
                root.display()
            )));
        }

        let mut warnings: Vec<String> = inner
            .projects
            .values()
            .filter(|p| root.starts_with(&p.root_path) || p.root_path.starts_with(&root))
            .map(|p| {
                format!(
                    "project {} is nested with the already registered \"{}\" ({})",
                    root.display(),
                    p.name,
                    p.root_path.display()
                )
            })
            .collect();

        // Protocol has no confirmation field; warn for paths outside $HOME instead of refusing.
        if let Some(home) = home_dir() {
            if !root.starts_with(&home) {
                warnings.push(format!(
                    "project {} is outside {}",
                    root.display(),
                    home.display()
                ));
            }
        }

        let now = Timestamp::now();
        let project = Project {
            id: ProjectId::new(),
            project_group_id,
            name: root
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| root.display().to_string()),
            icon: None,
            root_path: root.clone(),
            git_root: git_root.clone(),
            created_at: now,
            last_opened_at: now,
        };
        inner.db.projects().upsert(&project).map_err(db_err)?;
        inner.projects.insert(project.id, project.clone());

        let mut created = Vec::new();
        let main = Workspace {
            id: WorkspaceId::new(),
            project_id: project.id,
            kind: WorkspaceKind::Main,
            path: root.clone(),
            branch,
            display_name: None,
            managed_by_app: false,
            created_at: now,
            status: WorkspaceStatus::default(),
        };
        inner.db.workspaces().upsert(&main).map_err(db_err)?;
        inner.workspaces.insert(main.id, main.clone());
        created.push(main);

        if git_root.is_some() {
            let main_path = root.clone();
            for wt in worktrees {
                if wt.path == main_path {
                    continue; // the main checkout, already added
                }
                let ws = Workspace {
                    id: WorkspaceId::new(),
                    project_id: project.id,
                    kind: WorkspaceKind::GitWorktree,
                    path: wt.path.clone(),
                    branch: wt.branch.clone(),
                    display_name: inferred_display_name(&wt.path, wt.branch.as_deref()),
                    managed_by_app: false,
                    created_at: now,
                    status: WorkspaceStatus::default(),
                };
                inner.db.workspaces().upsert(&ws).map_err(db_err)?;
                inner.workspaces.insert(ws.id, ws.clone());
                created.push(ws);
            }
        }
        drop(inner);

        self.registry
            .broadcast_domain(DaemonEvent::ProjectAdded(project));
        for ws in created {
            self.registry
                .broadcast_domain(DaemonEvent::WorkspaceCreated(ws));
        }
        for message in warnings {
            self.notice(NoticeLevel::Warning, message);
        }
        Ok(Response::Ack)
    }

    fn remove_project(
        self: &Arc<Self>,
        project_id: ProjectId,
        policy: RemoveProjectPolicy,
    ) -> Result<Response, ProtocolError> {
        let (ws_ids, session_ids, managed_worktrees, git_root) = {
            let inner = self.lock();
            if !inner.projects.contains_key(&project_id) {
                return Err(ProtocolError::not_found("project"));
            }
            let ws_ids: Vec<WorkspaceId> = inner
                .workspaces
                .values()
                .filter(|w| w.project_id == project_id)
                .map(|w| w.id)
                .collect();
            let session_ids: Vec<SessionId> = inner
                .sessions
                .values()
                .filter(|s| ws_ids.contains(&s.workspace_id))
                .map(|s| s.id)
                .collect();
            let managed_worktrees: Vec<(WorkspaceId, PathBuf)> = inner
                .workspaces
                .values()
                .filter(|w| w.project_id == project_id && w.managed_by_app)
                .map(|w| (w.id, w.path.clone()))
                .collect();
            let git_root = inner.projects[&project_id].git_root.clone();
            (ws_ids, session_ids, managed_worktrees, git_root)
        };

        let has_running = {
            let inner = self.lock();
            session_ids
                .iter()
                .any(|id| inner.sessions.get(id).is_some_and(|s| s.state.is_active()))
        };

        match policy {
            RemoveProjectPolicy::KeepEverything if has_running => {
                return Err(ProtocolError::precondition_failed(
                    "project has running sessions; kill them or choose another policy",
                ));
            }
            RemoveProjectPolicy::KeepEverything => {}
            RemoveProjectPolicy::KillSessionsKeepWorktrees
            | RemoveProjectPolicy::KillSessionsRemoveManagedWorktrees => {
                // Groups must be dead before rows and managed worktrees go (P1).
                self.kill_groups_blocking(&self.kill_targets(&session_ids));
            }
            _ if has_running => {
                return Err(ProtocolError::precondition_failed(
                    "project has running sessions",
                ));
            }
            _ => {}
        }

        if matches!(
            policy,
            RemoveProjectPolicy::KillSessionsRemoveManagedWorktrees
        ) {
            if let Some(gr) = &git_root {
                for (_id, path) in &managed_worktrees {
                    let _ = git_service::remove(gr, path, true);
                }
            }
        }

        // FK order: sessions → workspaces → project. Fail before caches diverge.
        let mut inner = self.lock();
        for id in &session_ids {
            inner.db.sessions().delete(*id).map_err(db_err)?;
        }
        for id in &ws_ids {
            inner.db.workspaces().delete(*id).map_err(db_err)?;
        }
        inner.db.projects().delete(project_id).map_err(db_err)?;

        for id in &session_ids {
            inner.sessions.remove(id);
        }
        for id in &ws_ids {
            inner.workspaces.remove(id);
        }
        inner.projects.remove(&project_id);
        drop(inner);

        // Replica drops by id: sessions before their workspace/project.
        for session_id in session_ids {
            self.registry
                .broadcast_domain(DaemonEvent::SessionRemoved { session_id });
        }
        for id in ws_ids {
            self.registry
                .broadcast_domain(DaemonEvent::WorkspaceRemoved { workspace_id: id });
        }
        self.registry
            .broadcast_domain(DaemonEvent::ProjectRemoved { project_id });
        Ok(Response::Ack)
    }

    /// Re-detect git root, branches, and worktrees. Git runs with the core lock released.
    fn refresh_project(self: &Arc<Self>, project_id: ProjectId) -> Result<Response, ProtocolError> {
        let project = {
            let inner = self.lock();
            inner
                .projects
                .get(&project_id)
                .cloned()
                .ok_or_else(|| ProtocolError::not_found("project"))?
        };

        let git_root = git_service::discover_root(&project.root_path).ok();

        let mut inner = self.lock();
        let mut updated = inner
            .projects
            .get(&project_id)
            .cloned()
            .ok_or_else(|| ProtocolError::not_found("project"))?;
        updated.git_root.clone_from(&git_root);
        inner.db.projects().upsert(&updated).map_err(db_err)?;
        inner.projects.insert(project_id, updated.clone());
        let workspace_paths: Vec<(WorkspaceId, PathBuf)> = inner
            .workspaces
            .values()
            .filter(|ws| ws.project_id == project_id)
            .map(|ws| (ws.id, ws.path.clone()))
            .collect();
        drop(inner);

        self.registry
            .broadcast_domain(DaemonEvent::ProjectUpdated(updated));

        if let Some(git_root) = git_root {
            self.rescan_project_worktrees(project_id, &git_root, &project.root_path);
            for (workspace_id, path) in workspace_paths {
                let Ok(status) = git_service::status(&path) else {
                    continue;
                };
                self.apply_workspace_status(workspace_id, &status);
            }
        }
        Ok(Response::Ack)
    }

    /// Persist `branch` (a column); [`WorkspaceStatus`] is runtime-only.
    fn apply_workspace_status(&self, workspace_id: WorkspaceId, status: &git_service::RepoStatus) {
        let updated = {
            let mut inner = self.lock();
            let Some(mut ws) = inner.workspaces.get(&workspace_id).cloned() else {
                return;
            };
            /*
             * Assigned, not merged in.
             *
             * This was `status.branch.clone().or(ws.branch)`, which reads as
             * caution and is not: `parse_status_v2` returns `None` for exactly
             * one reason — `# branch.head (detached)` — and a failed read
             * never reaches here at all, because `status()` returning `Err` is
             * handled by the callers. So the `or` could only ever fire on a
             * genuine detached HEAD, where it kept naming the branch the
             * worktree had left. `Workspace::label` already falls back to the
             * directory name when there is no branch, which is the true thing
             * to say.
             */
            ws.branch = status.branch.clone();
            ws.status = WorkspaceStatus {
                dirty: status.dirty,
                ahead: status.ahead,
                behind: status.behind,
                measured_at: Some(Timestamp::now()),
            };
            if let Err(e) = inner.db.workspaces().upsert(&ws) {
                tracing::warn!(%workspace_id, error = %e, "persist workspace branch");
                return;
            }
            inner.workspaces.insert(workspace_id, ws.clone());
            ws
        };
        self.registry
            .broadcast_domain(DaemonEvent::WorkspaceUpdated(updated));
    }

    /// `None` or whitespace clears to initials. An invalid mark is refused, not cleared.
    fn set_project_icon(
        &self,
        project_id: ProjectId,
        icon: Option<&str>,
    ) -> Result<Response, ProtocolError> {
        let icon = match icon {
            Some(raw) => Project::normalize_icon(raw).map_err(|error| {
                ProtocolError::new(ErrorCode::InvalidRequest, error.to_string())
            })?,
            None => None,
        };
        let mut inner = self.lock();
        let mut project = inner
            .projects
            .get(&project_id)
            .cloned()
            .ok_or_else(|| ProtocolError::not_found("project"))?;
        project.icon = icon;
        inner.db.projects().upsert(&project).map_err(db_err)?;
        inner.projects.insert(project_id, project.clone());
        drop(inner);
        self.registry
            .broadcast_domain(DaemonEvent::ProjectUpdated(project));
        Ok(Response::Ack)
    }

    fn rename_project(&self, project_id: ProjectId, name: &str) -> Result<Response, ProtocolError> {
        let mut inner = self.lock();
        let mut project = inner
            .projects
            .get(&project_id)
            .cloned()
            .ok_or_else(|| ProtocolError::not_found("project"))?;
        project.name = name.to_owned();
        inner.db.projects().upsert(&project).map_err(db_err)?;
        inner.projects.insert(project_id, project.clone());
        drop(inner);
        self.registry
            .broadcast_domain(DaemonEvent::ProjectUpdated(project));
        Ok(Response::Ack)
    }

    // --- Workspaces ---

    fn list_workspaces(&self, project_id: ProjectId) -> Result<Response, ProtocolError> {
        let inner = self.lock();
        let list = inner
            .workspaces
            .values()
            .filter(|w| w.project_id == project_id)
            .cloned()
            .collect();
        Ok(Response::Workspaces(list))
    }

    /// Local refs only. `FetchRemote` is the separate step that talks to the network.
    fn list_branches(&self, project_id: ProjectId) -> Result<Response, ProtocolError> {
        let (git_root, workspaces) = {
            let inner = self.lock();
            let project = inner
                .projects
                .get(&project_id)
                .ok_or_else(|| ProtocolError::not_found("project"))?;
            let git_root = project.git_root.clone().ok_or_else(|| {
                ProtocolError::new(ErrorCode::GitError, "project is not a git repo")
            })?;
            let workspaces: HashMap<String, WorkspaceId> = inner
                .workspaces
                .values()
                .filter(|ws| ws.project_id == project_id)
                .filter_map(|ws| ws.branch.clone().map(|b| (b, ws.id)))
                .collect();
            (git_root, workspaces)
        };

        let refs = git_service::list_refs(&git_root).map_err(git_err)?;
        let remotes = git_service::list_remotes(&git_root).map_err(git_err)?;
        let default_branch = git_service::default_branch(&git_root)
            .ok()
            .flatten()
            .or_else(|| git_service::current_branch(&git_root).ok().flatten());

        let branches = refs
            .into_iter()
            .map(|entry| {
                let scope = match entry.remote {
                    Some(remote) => domain::RefScope::Remote { remote },
                    None => domain::RefScope::Local,
                };
                let checked_out_in = match scope {
                    domain::RefScope::Local => workspaces.get(&entry.name).copied(),
                    _ => None,
                };
                domain::BranchRef {
                    name: entry.name,
                    scope,
                    upstream: entry.upstream,
                    committed_at: entry.committed_at.and_then(unix_to_timestamp),
                    subject: entry.subject,
                    checked_out_in,
                }
            })
            .collect();

        Ok(Response::Branches {
            branches,
            remotes: remotes
                .into_iter()
                .map(|(name, url)| domain::Remote { name, url })
                .collect(),
            default_branch,
        })
    }

    /// Ack when the fetch *starts*. A synchronous network write would freeze typing.
    fn fetch_remote(
        self: &Arc<Self>,
        project_id: ProjectId,
        remote: Option<&str>,
    ) -> Result<Response, ProtocolError> {
        let git_root = {
            let inner = self.lock();
            let project = inner
                .projects
                .get(&project_id)
                .ok_or_else(|| ProtocolError::not_found("project"))?;
            project.git_root.clone().ok_or_else(|| {
                ProtocolError::new(ErrorCode::GitError, "project is not a git repo")
            })?
        };

        let remote = match remote {
            Some(name) => name.to_string(),
            None => git_service::default_remote(&git_root)
                .map_err(git_err)?
                .ok_or_else(|| {
                    ProtocolError::new(ErrorCode::GitError, "the repository has no remote")
                })?,
        };

        {
            let mut inner = self.lock();
            if !inner.fetching.insert(project_id) {
                tracing::debug!(%project_id, remote, "fetch already in flight; coalescing");
                return Ok(Response::Ack);
            }
        }

        let daemon = Arc::clone(self);
        let timeout = self.config.git.fetch_timeout();
        std::thread::Builder::new()
            .name("forge-fetch".into())
            .spawn(move || {
                let outcome = git_service::fetch(&git_root, &remote, Some(timeout));
                {
                    let mut inner = daemon.lock();
                    inner.fetching.remove(&project_id);
                }
                let event = match outcome {
                    Ok(outcome) => DaemonEvent::RemoteRefsUpdated {
                        project_id,
                        remote,
                        updated: u32::try_from(outcome.updated.len()).unwrap_or(u32::MAX),
                        error: None,
                    },
                    Err(e) => {
                        tracing::warn!(%project_id, error = %e, "fetch failed");
                        DaemonEvent::RemoteRefsUpdated {
                            project_id,
                            remote,
                            updated: 0,
                            error: Some(e.to_string()),
                        }
                    }
                };
                daemon.registry.broadcast_domain(event);
            })
            .map_err(|e| {
                let mut inner = self.lock();
                inner.fetching.remove(&project_id);
                ProtocolError::new(ErrorCode::IoError, format!("cannot start fetch: {e}"))
            })?;

        Ok(Response::Ack)
    }

    /// Ack when the refresh *starts*. Coalesces globally: one refresh covers every host.
    fn refresh_pull_requests(self: &Arc<Self>) -> Result<Response, ProtocolError> {
        {
            let mut inner = self.lock();
            if inner.pr_refreshing {
                tracing::debug!("pull-request refresh already in flight; coalescing");
                return Ok(Response::Ack);
            }
            inner.pr_refreshing = true;
        }

        let daemon = Arc::clone(self);
        std::thread::Builder::new()
            .name("forge-pr-refresh".into())
            .spawn(move || {
                let (projects, path_entries): (Vec<crate::pull_requests::ProjectSource>, _) = {
                    let mut inner = daemon.lock();
                    let projects = inner
                        .projects
                        .values()
                        .map(|project| (project.id, project.git_root.clone()))
                        .collect();
                    // `gh` is looked up along the login-shell PATH, not the
                    // daemon's: a GUI launched from Finder has neither
                    // Homebrew nor any user bin directory on its own (§12).
                    let env = daemon.resolved_env(&mut inner);
                    (projects, env.path_entries)
                };
                let query = crate::pull_requests::resolve(projects, &daemon.config.github);
                let fingerprint = crate::pull_requests::fingerprint(&query, &daemon.config.github);
                let cached = daemon
                    .pull_requests
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .fresh(fingerprint);
                let state = cached.unwrap_or_else(|| {
                    let state = crate::pull_requests::refresh(
                        query,
                        &daemon.config.github,
                        path_entries.clone(),
                    );
                    daemon
                        .pull_requests
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .update(fingerprint, state.clone());
                    state
                });

                daemon.lock().pr_refreshing = false;
                daemon
                    .registry
                    .broadcast_domain(DaemonEvent::PullRequestsUpdated { state });
            })
            .map_err(|error| {
                self.lock().pr_refreshing = false;
                ProtocolError::new(
                    ErrorCode::IoError,
                    format!("cannot start pull-request refresh: {error}"),
                )
            })?;

        Ok(Response::Ack)
    }

    fn get_change_context(&self, workspace_id: WorkspaceId) -> Result<Response, ProtocolError> {
        let path = self.workspace_path(workspace_id)?;
        let snap = git_service::change_context(&path).map_err(git_err)?;
        let context = crate::juva::context_from_snapshot(snap);
        Ok(Response::ChangeContext(context))
    }

    /// Core lock released before git runs.
    fn get_workspace_diff(
        &self,
        workspace_id: WorkspaceId,
        context_lines: Option<u32>,
    ) -> Result<Response, ProtocolError> {
        let path = self.workspace_path(workspace_id)?;
        let context = context_lines.unwrap_or(git_service::DIFF_CONTEXT_LINES);
        let diff = git_service::working_tree_diff(&path, None, context).map_err(git_err)?;
        Ok(Response::WorkspaceDiff(domain::WorkspaceDiff {
            workspace_id,
            branch: diff.branch,
            files: diff.files.into_iter().map(diff_file).collect(),
            truncated: diff.truncated,
        }))
    }

    /// Core lock released before git runs.
    ///
    /// A recorded baseline that git no longer resolves falls back to `HEAD`
    /// and says so through [`domain::BaseOrigin`]: a silent fallback would
    /// present a much smaller diff as if it were everything the session did.
    fn get_session_changes(&self, session_id: SessionId) -> Result<Response, ProtocolError> {
        let (path, base, sharing) = {
            let inner = self.lock();
            let session = inner
                .sessions
                .get(&session_id)
                .ok_or_else(|| ProtocolError::not_found("session"))?;
            let workspace_id = session.workspace_id;
            let base = session.base_commit.clone();
            let path = inner
                .workspaces
                .get(&workspace_id)
                .map(|ws| ws.path.clone())
                .ok_or_else(|| ProtocolError::not_found("workspace"))?;
            let sharing = inner
                .sessions
                .values()
                .filter(|other| {
                    other.id != session_id
                        && other.workspace_id == workspace_id
                        && other.state.is_active()
                })
                .count();
            (path, base, u32::try_from(sharing).unwrap_or(u32::MAX))
        };

        let (base, origin) = resolve_base(&path, base.as_deref());
        let summary = git_service::change_summary(&path, base.as_deref()).map_err(git_err)?;
        Ok(Response::SessionChanges(domain::SessionChanges {
            session_id,
            origin,
            summary: change_summary(base.as_deref(), summary),
            sharing_sessions: sharing,
        }))
    }

    /// Core lock released before git runs.
    ///
    /// One diff for the checkout, based at the common ancestor of every
    /// baseline its sessions carry — `merge-base --octopus`, one subprocess
    /// whatever the number of sessions. Per-session commit counts are one
    /// `rev-list` each, which is why the session list is capped: that cost
    /// scales with sessions, and nothing else here does.
    fn get_workspace_review(
        &self,
        workspace_id: WorkspaceId,
        context_lines: Option<u32>,
    ) -> Result<Response, ProtocolError> {
        let (path, mut sessions) = {
            let inner = self.lock();
            let path = inner
                .workspaces
                .get(&workspace_id)
                .map(|ws| ws.path.clone())
                .ok_or_else(|| ProtocolError::not_found("workspace"))?;
            let sessions: Vec<Session> = inner
                .sessions
                .values()
                .filter(|session| session.workspace_id == workspace_id)
                .cloned()
                .collect();
            (path, sessions)
        };

        // Newest first: a review is read from the most recent work back.
        sessions.sort_unstable_by(|a, b| b.created_at.cmp(&a.created_at));
        sessions.truncate(MAX_REVIEW_SESSIONS);

        let recorded: Vec<String> = sessions
            .iter()
            .filter_map(|session| session.base_commit.clone())
            .filter(|base| git_service::commit_exists(&path, base))
            .collect();
        let had_baseline = sessions.iter().any(|s| s.base_commit.is_some());
        let (base, origin) = match git_service::merge_base(&path, &recorded) {
            Some(base) => (Some(base), domain::BaseOrigin::Recorded),
            None if had_baseline => (None, domain::BaseOrigin::Unreachable),
            None => (None, domain::BaseOrigin::Missing),
        };

        let context = context_lines.unwrap_or(git_service::DIFF_CONTEXT_LINES);
        let diff =
            git_service::working_tree_diff(&path, base.as_deref(), context).map_err(git_err)?;
        let (commits, _) = match base.as_deref() {
            Some(base) => git_service::commits_since(&path, base),
            None => (Vec::new(), 0),
        };

        let rows = sessions
            .into_iter()
            .map(|session| {
                let base = session
                    .base_commit
                    .as_deref()
                    .filter(|base| git_service::commit_exists(&path, base));
                domain::ReviewSession {
                    session_id: session.id,
                    title: session
                        .title
                        .resolve(session_fallback_title(&session))
                        .to_owned(),
                    provider: session
                        .agent_provider_id
                        .as_ref()
                        .map(|id| id.as_str().to_owned()),
                    active: session.state.is_active(),
                    commit_count: base.map_or(0, |base| git_service::commit_count(&path, base)),
                    base: base.map(git_service::short_commit),
                    created_at: session.created_at,
                    ended_at: session.ended_at,
                }
            })
            .collect();

        Ok(Response::WorkspaceReview(Box::new(
            domain::WorkspaceReview {
                workspace_id,
                base: base.as_deref().map(git_service::short_commit),
                origin,
                sessions: rows,
                commits: commits.into_iter().map(commit_line).collect(),
                diff: domain::WorkspaceDiff {
                    workspace_id,
                    branch: diff.branch,
                    files: diff.files.into_iter().map(diff_file).collect(),
                    truncated: diff.truncated,
                },
            },
        )))
    }

    /// The tail of a session's terminal as plain text.
    ///
    /// The rows are decoded cells, so this is a fold and not a parse: the VT
    /// engine consumed every escape sequence on the way in, and nothing here
    /// has to strip one. The lock is held for the row clone and released
    /// before the text is built.
    fn get_session_transcript(
        &self,
        session_id: SessionId,
        max_lines: Option<u32>,
        max_bytes: Option<u32>,
    ) -> Result<Response, ProtocolError> {
        // Clamped before the clone, not after: an unclamped wire `u32` would
        // otherwise ask the engine to materialise millions of rows under the
        // core lock.
        let lines = max_lines
            .unwrap_or(DEFAULT_TRANSCRIPT_LINES)
            .min(MAX_TRANSCRIPT_LINES) as usize;
        let budget = max_bytes
            .unwrap_or(DEFAULT_TRANSCRIPT_BYTES)
            .min(MAX_TRANSCRIPT_BYTES) as usize;

        let snapshot = {
            let inner = self.lock();
            let session = inner
                .sessions
                .get(&session_id)
                .ok_or_else(|| ProtocolError::not_found("session"))?;
            let terminal_id = session
                .terminal_id
                .ok_or_else(|| ProtocolError::not_found("terminal"))?;
            let rt = inner
                .terminals
                .get(&terminal_id)
                .ok_or_else(|| ProtocolError::not_found("terminal"))?;
            rt.engine.snapshot(lines)
        };

        let mut rows: Vec<String> = snapshot
            .scrollback_tail
            .iter()
            .chain(snapshot.visible.iter())
            .map(row_text)
            .collect();
        // A pane that has just been cleared is mostly blank rows; they carry
        // nothing and would spend the whole budget.
        while rows.last().is_some_and(String::is_empty) {
            rows.pop();
        }

        // Walk back from the newest line so the budget bounds what is built
        // rather than trimming what was already allocated.
        let mut kept = 0usize;
        let mut size = 0usize;
        for row in rows.iter().rev() {
            let next = size + row.len() + 1;
            if kept > 0 && next > budget {
                break;
            }
            size = next;
            kept += 1;
        }
        let truncated = kept < rows.len();
        let text = rows[rows.len() - kept..].join("\n");

        Ok(Response::SessionTranscript(domain::SessionTranscript {
            session_id,
            text,
            lines: u32::try_from(kept).unwrap_or(u32::MAX),
            truncated,
        }))
    }

    /// A discovered run's conversation, read off disk.
    ///
    /// The core lock is never taken: the run is resolved against the external
    /// scanner's own cache, and the transcript read is filesystem IO.
    fn get_external_transcript(
        &self,
        session_id: &str,
        provider: &str,
        profile_id: Option<domain::AgentProfileId>,
        max_turns: Option<u32>,
        max_bytes: Option<u32>,
    ) -> Result<Response, ProtocolError> {
        // Clamped before the read, not after: an unclamped wire `u32` would
        // otherwise fold a year of history into one allocation.
        let turns = max_turns
            .unwrap_or(crate::external_agents::DEFAULT_EXTERNAL_TURNS)
            .min(crate::external_agents::MAX_EXTERNAL_TURNS);
        let budget = max_bytes
            .unwrap_or(DEFAULT_TRANSCRIPT_BYTES)
            .min(MAX_TRANSCRIPT_BYTES) as usize;

        let session = self.find_external(session_id, provider, profile_id)?;
        Ok(Response::ExternalTranscript(
            crate::external_agents::read_transcript(&session, turns, budget),
        ))
    }

    /// Remove a discovered run's transcript from disk.
    ///
    /// No event: `external_agents` reaches a client only through
    /// `Response::Snapshot`, so the GUI re-reads the snapshot rather than
    /// waiting for a broadcast that does not exist. The cache is invalidated so
    /// that re-read does not answer from the pass taken before the delete.
    fn delete_external_session(
        &self,
        session_id: &str,
        provider: &str,
        profile_id: Option<domain::AgentProfileId>,
    ) -> Result<Response, ProtocolError> {
        let session = self.find_external(session_id, provider, profile_id)?;
        crate::external_agents::delete_transcript(&session).map_err(|error| match error {
            crate::external_agents::DeleteError::Shared => ProtocolError::invalid_request(
                "this run is recorded in a database shared with every other opencode session",
            ),
            crate::external_agents::DeleteError::Io(message) => {
                ProtocolError::internal(format!("could not remove the transcript: {message}"))
            }
        })?;

        self.external_agents
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .invalidate();
        Ok(Response::Ack)
    }

    /// Resolve an external run by identity against the last discovery pass.
    fn find_external(
        &self,
        session_id: &str,
        provider: &str,
        profile_id: Option<domain::AgentProfileId>,
    ) -> Result<domain::ExternalAgentSession, ProtocolError> {
        self.external_agents
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .find(session_id, provider, profile_id)
            .ok_or_else(|| ProtocolError::not_found("session"))
    }

    /// Core lock released before git runs.
    fn get_rebase_state(&self, workspace_id: WorkspaceId) -> Result<Response, ProtocolError> {
        let path = self.workspace_path(workspace_id)?;
        let state = git_service::sequencer_state(&path).map_err(git_err)?;
        Ok(Response::RebaseState(rebase_state(workspace_id, state)))
    }

    /// Unmerged paths come back as `RebaseState`, not a protocol error.
    fn continue_rebase(&self, workspace_id: WorkspaceId) -> Result<Response, ProtocolError> {
        let path = self.workspace_path(workspace_id)?;
        let continued = git_service::continue_sequencer(&path).map_err(git_err)?;

        match &continued {
            git_service::Continued::Finished(_) => {
                self.notice(NoticeLevel::Info, "Rebase finished");
            }
            git_service::Continued::Stopped { state, message } => {
                let detail = if message.is_empty() {
                    format!("Rebase still stopped: {} unresolved", state.conflicts.len())
                } else {
                    message.lines().next().unwrap_or(message).to_string()
                };
                self.notice(NoticeLevel::Warning, detail);
            }
        }

        if let Ok(status) = git_service::status(&path) {
            self.apply_workspace_status(workspace_id, &status);
        }
        Ok(Response::RebaseState(rebase_state(
            workspace_id,
            continued.state().clone(),
        )))
    }

    fn abort_rebase(&self, workspace_id: WorkspaceId) -> Result<Response, ProtocolError> {
        let path = self.workspace_path(workspace_id)?;
        git_service::abort_sequencer(&path).map_err(git_err)?;
        if let Ok(status) = git_service::status(&path) {
            self.apply_workspace_status(workspace_id, &status);
        }
        self.notice(NoticeLevel::Info, "Rebase aborted");
        Ok(Response::Ack)
    }

    fn mark_conflict_resolved(
        &self,
        workspace_id: WorkspaceId,
        paths: &[String],
    ) -> Result<Response, ProtocolError> {
        let path = self.workspace_path(workspace_id)?;
        git_service::mark_resolved(&path, paths).map_err(git_err)?;
        let state = git_service::sequencer_state(&path).map_err(git_err)?;
        Ok(Response::RebaseState(rebase_state(workspace_id, state)))
    }

    /// List files under a workspace (ADR-012). Lock released before IO.
    fn list_files(&self, workspace_id: WorkspaceId) -> Result<Response, ProtocolError> {
        let path = self.workspace_path(workspace_id)?;
        let tree = fs_service::list_files(&path).map_err(fs_err)?;
        Ok(Response::FileTree(domain::FileTree {
            workspace_id,
            entries: tree
                .entries
                .into_iter()
                .map(|e| domain::FileEntry {
                    path: e.path,
                    kind: match e.kind {
                        fs_service::EntryKind::File => domain::FileKind::File,
                        fs_service::EntryKind::Directory => domain::FileKind::Directory,
                    },
                    ignored: e.ignored,
                })
                .collect(),
            truncated: tree.truncated,
        }))
    }

    /// Read one file (ADR-012). Lock released before IO.
    fn read_file(
        &self,
        workspace_id: WorkspaceId,
        relative: &str,
    ) -> Result<Response, ProtocolError> {
        let path = self.workspace_path(workspace_id)?;
        let contents = fs_service::read_file(&path, relative).map_err(fs_err)?;
        Ok(Response::FileContents(domain::FileContents {
            workspace_id,
            path: contents.path,
            text: contents.text,
            revision: contents.revision,
            language: contents.language,
            binary: contents.binary,
            too_large: contents.too_large,
        }))
    }

    /// Create an empty file or a directory (ADR-012).
    ///
    /// The three mutations below all answer `Ack` and broadcast nothing. The
    /// file tree is a *read*, not a subscription — `ListFiles` is asked for and
    /// answered, and `GetWorkspaceDiff` beside it works the same way — so the
    /// client refreshes what it is showing rather than the daemon pushing at
    /// every window that happens to have a tree open.
    fn create_path(
        &self,
        workspace_id: WorkspaceId,
        relative: &str,
        directory: bool,
    ) -> Result<Response, ProtocolError> {
        let root = self.workspace_path(workspace_id)?;
        let kind = if directory {
            fs_service::PathKind::Directory
        } else {
            fs_service::PathKind::File
        };
        fs_service::create_path(&root, relative, kind).map_err(fs_err)?;
        Ok(Response::Ack)
    }

    /// Move one path to another inside the same checkout (ADR-012).
    fn rename_path(
        &self,
        workspace_id: WorkspaceId,
        from: &str,
        to: &str,
    ) -> Result<Response, ProtocolError> {
        let root = self.workspace_path(workspace_id)?;
        fs_service::rename_path(&root, from, to).map_err(fs_err)?;
        Ok(Response::Ack)
    }

    /// Delete one path, recursively for a directory (ADR-012).
    fn delete_path(
        &self,
        workspace_id: WorkspaceId,
        relative: &str,
    ) -> Result<Response, ProtocolError> {
        let root = self.workspace_path(workspace_id)?;
        fs_service::delete_path(&root, relative).map_err(fs_err)?;
        Ok(Response::Ack)
    }

    /// Write one file conditioned on a revision (ADR-012).
    fn write_file(
        &self,
        workspace_id: WorkspaceId,
        relative: &str,
        text: &str,
        expected_revision: &str,
    ) -> Result<Response, ProtocolError> {
        let path = self.workspace_path(workspace_id)?;
        match fs_service::write_file(&path, relative, text, expected_revision) {
            Ok(()) => Ok(Response::Ack),
            Err(fs_service::FsError::RevisionMismatch { .. }) => {
                Err(ProtocolError::precondition_failed(
                    "file changed on disk while you were editing it",
                ))
            }
            Err(e) => Err(fs_err(e)),
        }
    }

    /// Search files by name, content or declaration (ADR-012).
    fn search_files(
        &self,
        workspace_id: WorkspaceId,
        query: &str,
        kind: domain::SearchKind,
        limit: Option<u32>,
    ) -> Result<Response, ProtocolError> {
        let path = self.workspace_path(workspace_id)?;
        let limit = limit
            .map(|n| n as usize)
            .unwrap_or(fs_service::MAX_SEARCH_RESULTS);
        let kind = match kind {
            domain::SearchKind::Name => fs_service::SearchKind::Name,
            domain::SearchKind::Content => fs_service::SearchKind::Content,
            domain::SearchKind::Definition => fs_service::SearchKind::Definition,
            _ => fs_service::SearchKind::Name,
        };
        let results = fs_service::search_files(&path, query, kind, limit).map_err(fs_err)?;
        Ok(Response::SearchResults(domain::SearchResults {
            workspace_id,
            query: query.to_string(),
            matches: results
                .matches
                .into_iter()
                .map(|m| domain::SearchMatch {
                    path: m.path,
                    line: m.line,
                    column: m.column,
                    text: m.text,
                })
                .collect(),
            truncated: results.truncated,
        }))
    }

    /// Acks when the draft *starts*; the text arrives as `JuvaDraftReady`.
    ///
    /// The `[juva]` endpoint opens a socket, which puts this on the same path
    /// as `FetchRemote` and `CreatePullRequest`: ack early, report through an
    /// event, and coalesce per workspace rather than queue. The change context
    /// is still read synchronously first, because a clean tree has to refuse
    /// the commit-message draft with an error the caller can act on rather
    /// than with an event carrying an empty message.
    fn draft_with_juva(
        self: &Arc<Self>,
        workspace_id: WorkspaceId,
        kind: domain::JuvaKind,
    ) -> Result<Response, ProtocolError> {
        let path = self.workspace_path(workspace_id)?;
        let snap = git_service::change_context(&path).map_err(git_err)?;
        let context = crate::juva::context_from_snapshot(snap);
        if kind == domain::JuvaKind::CommitMessage && !context.dirty {
            return Err(ProtocolError::new(
                ErrorCode::InvalidRequest,
                "nothing to commit: working tree is clean",
            ));
        }

        // Nothing to coalesce and nothing to spawn when the endpoint is off:
        // the local draft is immediate, so it goes out as the event a caller
        // is already listening for rather than down a second path.
        if self.config.juva.api_key().is_none() {
            let draft = crate::juva::draft(kind, &context);
            self.registry.broadcast_domain(DaemonEvent::JuvaDraftReady {
                workspace_id,
                draft,
                fell_back: false,
            });
            return Ok(Response::Ack);
        }

        {
            let mut inner = self.lock();
            if !inner.drafting.insert(workspace_id) {
                return Ok(Response::Ack);
            }
        }

        let daemon = Arc::clone(self);
        std::thread::Builder::new()
            .name("forge-juva".into())
            .spawn(move || {
                let _guard = DraftingGuard {
                    daemon: Arc::clone(&daemon),
                    workspace_id,
                };
                let local = crate::juva::draft(kind, &context);
                let draft = crate::juva::draft_remote(
                    &crate::juva::UreqJuva,
                    &daemon.config.juva,
                    kind,
                    &context,
                );
                let fell_back = draft == local;
                daemon
                    .registry
                    .broadcast_domain(DaemonEvent::JuvaDraftReady {
                        workspace_id,
                        draft,
                        fell_back,
                    });
            })
            .map_err(|e| {
                // A worker that never started must not latch the flag: the
                // guard only runs inside the thread.
                let mut inner = self.lock();
                inner.drafting.remove(&workspace_id);
                ProtocolError::new(ErrorCode::IoError, format!("cannot start juva: {e}"))
            })?;
        Ok(Response::Ack)
    }

    /// Local commit only. Never pushes.
    fn create_commit(
        &self,
        workspace_id: WorkspaceId,
        message: &str,
    ) -> Result<Response, ProtocolError> {
        let path = self.workspace_path(workspace_id)?;
        let sha = git_service::commit(&path, message).map_err(git_err)?;
        if let Ok(status) = git_service::status(&path) {
            self.apply_workspace_status(workspace_id, &status);
        }
        self.notice(
            NoticeLevel::Info,
            format!("Committed {}", &sha[..sha.len().min(7)]),
        );
        Ok(Response::Ack)
    }

    /// Ack when the push+`gh` *starts*.
    fn create_pull_request(
        self: &Arc<Self>,
        workspace_id: WorkspaceId,
        title: String,
        body: String,
        base: Option<String>,
    ) -> Result<Response, ProtocolError> {
        let path = self.workspace_path(workspace_id)?;
        let path_entries = {
            let mut inner = self.lock();
            if !inner.pr_opening.insert(workspace_id) {
                return Ok(Response::Ack);
            }
            self.resolved_env(&mut inner).path_entries
        };

        let daemon = Arc::clone(self);
        let timeout = self.config.git.fetch_timeout();
        let github_cli = self.config.github.cli(path_entries);
        std::thread::Builder::new()
            .name("forge-pr".into())
            .spawn(move || {
                let result = (|| -> Result<String, git_service::GitError> {
                    let remote = git_service::default_remote(&path)?.ok_or_else(|| {
                        git_service::GitError::CommandFailed {
                            args: vec!["push".into()],
                            status: -1,
                            stderr: "the repository has no remote".into(),
                        }
                    })?;
                    git_service::push(&path, &remote, Some(timeout))?;
                    let pr = git_service::create_pull_request_with_cli(
                        &github_cli,
                        &path,
                        &title,
                        &body,
                        base.as_deref(),
                    )?;
                    Ok(pr.url)
                })();

                {
                    let mut inner = daemon.lock();
                    inner.pr_opening.remove(&workspace_id);
                }

                match result {
                    Ok(url) => {
                        if let Ok(status) = git_service::status(&path) {
                            daemon.apply_workspace_status(workspace_id, &status);
                        }
                        daemon
                            .registry
                            .broadcast_domain(DaemonEvent::PullRequestOpened {
                                workspace_id,
                                url: Some(url),
                                error: None,
                            });
                        daemon
                            .pull_requests
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .invalidate();
                        if let Err(error) = daemon.refresh_pull_requests() {
                            tracing::warn!(
                                error = %error.message,
                                "could not refresh pull requests after creation"
                            );
                        }
                    }
                    Err(error) => {
                        daemon
                            .registry
                            .broadcast_domain(DaemonEvent::PullRequestOpened {
                                workspace_id,
                                url: None,
                                error: Some(error.to_string()),
                            });
                    }
                }
            })
            .map_err(|e| {
                let mut inner = self.lock();
                inner.pr_opening.remove(&workspace_id);
                ProtocolError::new(
                    ErrorCode::IoError,
                    format!("cannot start pull request: {e}"),
                )
            })?;

        Ok(Response::Ack)
    }

    fn workspace_path(&self, workspace_id: WorkspaceId) -> Result<PathBuf, ProtocolError> {
        let inner = self.lock();
        inner
            .workspaces
            .get(&workspace_id)
            .map(|ws| ws.path.clone())
            .ok_or_else(|| ProtocolError::not_found("workspace"))
    }

    fn create_worktree(
        self: &Arc<Self>,
        project_id: ProjectId,
        branch: &str,
        base: Option<&str>,
        name: Option<&str>,
    ) -> Result<Response, ProtocolError> {
        self.create_managed_worktree(project_id, branch, base, name)?;
        Ok(Response::Ack)
    }

    /// Shared with `CreateChildSession` (`NewManagedWorktree`).
    fn create_managed_worktree(
        self: &Arc<Self>,
        project_id: ProjectId,
        branch: &str,
        base: Option<&str>,
        name: Option<&str>,
    ) -> Result<Workspace, ProtocolError> {
        let _span = tracing::info_span!("worktree.create", %project_id).entered();
        let (git_root, existing_slugs) = {
            let inner = self.lock();
            let project = inner
                .projects
                .get(&project_id)
                .ok_or_else(|| ProtocolError::not_found("project"))?;
            let gr = project.git_root.clone().ok_or_else(|| {
                ProtocolError::new(ErrorCode::GitError, "project is not a git repo")
            })?;
            let slugs: HashSet<String> = inner
                .workspaces
                .values()
                .filter(|w| w.project_id == project_id && w.managed_by_app)
                .filter_map(|w| w.path.file_name().map(|s| s.to_string_lossy().into_owned()))
                .collect();
            (gr, slugs)
        };

        git_service::validate_branch_name(branch).map_err(git_err)?;
        let slug = match name {
            Some(n) => git_service::unique_slug(&existing_slugs, n),
            None => git_service::unique_slug(&existing_slugs, branch),
        };
        let path = self.worktrees_root.join(project_id.to_string()).join(&slug);
        if let Some(parent) = path.parent() {
            crate::paths::ensure_private_dir(parent)
                .map_err(|e| ProtocolError::new(ErrorCode::IoError, e.to_string()))?;
        }

        git_service::create(&git_root, &path, branch, base).map_err(git_err)?;

        let display_name = name
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .map(str::to_owned)
            .or_else(|| inferred_display_name(&path, Some(branch)));
        let ws = Workspace {
            id: WorkspaceId::new(),
            project_id,
            kind: WorkspaceKind::GitWorktree,
            path,
            branch: Some(branch.to_owned()),
            display_name,
            managed_by_app: true,
            created_at: Timestamp::now(),
            status: WorkspaceStatus::default(),
        };
        {
            let mut inner = self.lock();
            inner.db.workspaces().upsert(&ws).map_err(db_err)?;
            inner.workspaces.insert(ws.id, ws.clone());
        }
        self.registry
            .broadcast_domain(DaemonEvent::WorkspaceCreated(ws.clone()));
        // After the row exists and the event is out: the worktree is visible
        // immediately in a "setting up" state instead of the request thread
        // freezing for as long as a setup script takes (§14.2).
        self.start_provisioning(ws.id, ShareTrigger::Created, None);
        Ok(ws)
    }

    // ------------------------------------------- shared files (§14.2) ---

    /// The rules that apply to a project: its own, or the global
    /// `[worktrees]` list synthesised into rules when it has none.
    ///
    /// One engine, two sources. Merging the two would make it impossible to
    /// *remove* an inherited entry, so a project with rules ignores the global
    /// list entirely and the GUI says which layer is in effect.
    fn rules_for_project(&self, project_id: ProjectId) -> Vec<ShareRule> {
        let stored = {
            let inner = self.lock();
            inner
                .db
                .shares()
                .list_for_project(project_id)
                .unwrap_or_default()
        };
        if !stored.is_empty() {
            return stored;
        }

        let mut synthesised = Vec::new();
        for (position, path) in self.config.worktrees.copy.iter().enumerate() {
            synthesised.push(ShareRule {
                id: ShareRuleId::new(),
                project_id,
                path: path.clone(),
                strategy: ShareStrategy::Copy,
                enabled: true,
                position: u32::try_from(position).unwrap_or(u32::MAX),
                created_at: Timestamp::now(),
            });
        }
        let script = self.config.worktrees.setup_script.trim();
        if !script.is_empty() {
            synthesised.push(ShareRule {
                id: ShareRuleId::new(),
                project_id,
                // The legacy script has no path of its own: it runs when the
                // worktree is new, which is what an absent `.forge-setup`
                // marker means.
                path: ".forge-setup".to_owned(),
                strategy: ShareStrategy::Run {
                    command: script.to_owned(),
                    timeout_secs: self.config.worktrees.setup_timeout().as_secs(),
                },
                enabled: true,
                position: u32::MAX,
                created_at: Timestamp::now(),
            });
        }
        synthesised
    }

    /// Where one workspace's shares come from and go to.
    ///
    /// The source is the project's `Main` workspace — the primary checkout,
    /// whatever branch it has out — and the store lives in the repository's
    /// common git dir, which every workspace of the project resolves to the
    /// same path.
    fn share_context(
        &self,
        workspace_id: WorkspaceId,
        explicit: bool,
    ) -> Result<(ShareContext, ProjectId), ProtocolError> {
        let (project_id, target, source, git_root) = {
            let inner = self.lock();
            let workspace = inner
                .workspaces
                .get(&workspace_id)
                .ok_or_else(|| ProtocolError::not_found("workspace"))?;
            let project_id = workspace.project_id;
            let target = workspace.path.clone();
            let project = inner
                .projects
                .get(&project_id)
                .ok_or_else(|| ProtocolError::not_found("project"))?;
            let source = inner
                .workspaces
                .values()
                .find(|w| w.project_id == project_id && w.kind == WorkspaceKind::Main)
                .map_or_else(|| project.root_path.clone(), |w| w.path.clone());
            (project_id, target, source, project.git_root.clone())
        };

        // Filesystem and subprocess work, deliberately after the lock is gone.
        let store_root = git_root
            .as_deref()
            .and_then(|root| git_service::common_dir(root).ok());
        let caps = share_apply::capabilities(&source, &target);
        Ok((
            ShareContext {
                store: store_root.as_ref().map(|dir| dir.join(shares::STORE_DIR)),
                backups: store_root.map(|dir| dir.join(shares::BACKUP_DIR)),
                source,
                target,
                caps,
                explicit,
            },
            project_id,
        ))
    }

    fn broadcast_shares(&self, project_id: ProjectId) {
        let rules = {
            let inner = self.lock();
            inner
                .db
                .shares()
                .list_for_project(project_id)
                .unwrap_or_default()
        };
        self.registry
            .broadcast_domain(DaemonEvent::ProjectSharesChanged { project_id, rules });
    }

    /// Keep the managed block of `info/exclude` in step with the rules.
    ///
    /// Without it every provisioned worktree reads as dirty: what Forge injects
    /// is untracked by construction, and a rail full of dirty dots teaches the
    /// user to ignore the one that means something.
    fn sync_local_excludes(&self, project_id: ProjectId, rules: &[ShareRule]) {
        let git_root = {
            let inner = self.lock();
            inner
                .projects
                .get(&project_id)
                .and_then(|p| p.git_root.clone())
        };
        let Some(git_root) = git_root else {
            return;
        };
        let patterns: Vec<String> = rules
            .iter()
            .filter_map(|rule| share_apply::validate(&rule.path).ok())
            .map(|path| format!("/{path}"))
            .collect();
        if let Err(e) = git_service::set_local_excludes(&git_root, &patterns) {
            tracing::warn!(%project_id, error = %e, "could not update info/exclude");
        }
    }

    fn detect_share_candidates(&self, project_id: ProjectId) -> Result<Response, ProtocolError> {
        let (source, git_root) = {
            let inner = self.lock();
            let project = inner
                .projects
                .get(&project_id)
                .ok_or_else(|| ProtocolError::not_found("project"))?;
            let source = inner
                .workspaces
                .values()
                .find(|w| w.project_id == project_id && w.kind == WorkspaceKind::Main)
                .map_or_else(|| project.root_path.clone(), |w| w.path.clone());
            (source, project.git_root.clone())
        };
        let Some(git_root) = git_root else {
            return Err(ProtocolError::new(
                ErrorCode::GitError,
                "project is not a git repo",
            ));
        };

        let reported = git_service::ignored_paths(&git_root).map_err(git_err)?;
        let rules = self.rules_for_project(project_id);
        let (candidates, truncated) = shares::detect::candidates(&source, &reported, &rules);
        Ok(Response::ShareCandidates {
            project_id,
            candidates,
            truncated,
        })
    }

    fn set_project_shares(
        &self,
        project_id: ProjectId,
        rules: Vec<ShareRule>,
    ) -> Result<Response, ProtocolError> {
        if rules.len() > MAX_SHARE_RULES {
            return Err(ProtocolError::new(
                ErrorCode::InvalidRequest,
                format!("at most {MAX_SHARE_RULES} rules per project"),
            ));
        }
        let mut normalized = Vec::with_capacity(rules.len());
        for mut rule in rules {
            rule.project_id = project_id;
            rule.path = share_apply::validate(&rule.path)
                .map_err(|e| ProtocolError::new(ErrorCode::InvalidRequest, e.0))?;
            normalized.push(rule);
        }

        {
            let inner = self.lock();
            if !inner.projects.contains_key(&project_id) {
                return Err(ProtocolError::not_found("project"));
            }
            inner
                .db
                .shares()
                .replace_for_project(project_id, &normalized)
                .map_err(db_err)?;
        }
        self.sync_local_excludes(project_id, &normalized);
        self.broadcast_shares(project_id);
        Ok(Response::Ack)
    }

    /// Drop one rule, saying what happens to what it already wrote.
    ///
    /// The cleanup runs over every workspace of the project before the row
    /// goes, and its result is the answer: a removal that silently left files
    /// behind, or silently deleted an agent's edited `.env`, would be equally
    /// wrong.
    fn remove_share_rule(
        &self,
        project_id: ProjectId,
        rule_id: ShareRuleId,
        cleanup: ShareCleanup,
    ) -> Result<Response, ProtocolError> {
        let rules = {
            let inner = self.lock();
            inner
                .db
                .shares()
                .list_for_project(project_id)
                .map_err(db_err)?
        };
        let rule = rules
            .iter()
            .find(|rule| rule.id == rule_id)
            .cloned()
            .ok_or_else(|| ProtocolError::not_found("share rule"))?;

        let workspaces: Vec<WorkspaceId> = {
            let inner = self.lock();
            inner
                .workspaces
                .values()
                .filter(|w| w.project_id == project_id)
                .map(|w| w.id)
                .collect()
        };
        let mut actions = Vec::new();
        if cleanup != ShareCleanup::Leave {
            for workspace_id in workspaces {
                let Ok((ctx, _)) = self.share_context(workspace_id, true) else {
                    continue;
                };
                actions.push(share_apply::clean_up(&ctx, &rule, cleanup));
            }
        }

        let kept: Vec<ShareRule> = rules.into_iter().filter(|r| r.id != rule_id).collect();
        {
            let inner = self.lock();
            inner
                .db
                .shares()
                .replace_for_project(project_id, &kept)
                .map_err(db_err)?;
        }
        self.sync_local_excludes(project_id, &kept);
        self.broadcast_shares(project_id);
        Ok(Response::SharePlan {
            workspace_id: None,
            actions,
        })
    }

    fn preview_shares(&self, workspace_id: WorkspaceId) -> Result<Response, ProtocolError> {
        let (ctx, project_id) = self.share_context(workspace_id, true)?;
        let rules = self.rules_for_project(project_id);
        Ok(Response::SharePlan {
            workspace_id: Some(workspace_id),
            actions: share_apply::preview(&ctx, &rules),
        })
    }

    fn share_status(&self, workspace_id: WorkspaceId) -> Result<Response, ProtocolError> {
        let (ctx, project_id) = self.share_context(workspace_id, false)?;
        let rules = self.rules_for_project(project_id);
        Ok(Response::ShareStatus {
            workspace_id,
            entries: share_apply::status(&ctx, &rules),
        })
    }

    fn adopt_into_share_store(
        &self,
        project_id: ProjectId,
        path: String,
    ) -> Result<Response, ProtocolError> {
        let main = {
            let inner = self.lock();
            inner
                .workspaces
                .values()
                .find(|w| w.project_id == project_id && w.kind == WorkspaceKind::Main)
                .map(|w| w.id)
                .ok_or_else(|| ProtocolError::not_found("main workspace"))?
        };
        let (ctx, _) = self.share_context(main, true)?;
        share_apply::adopt_into_store(&ctx, &path)
            .map_err(|e| ProtocolError::new(ErrorCode::IoError, e.to_string()))?;
        Ok(Response::Ack)
    }

    fn materialize_from_share_store(
        &self,
        project_id: ProjectId,
        path: String,
        workspace_id: Option<WorkspaceId>,
    ) -> Result<Response, ProtocolError> {
        let targets: Vec<WorkspaceId> = match workspace_id {
            Some(id) => vec![id],
            None => {
                let inner = self.lock();
                inner
                    .workspaces
                    .values()
                    .filter(|w| w.project_id == project_id)
                    .map(|w| w.id)
                    .collect()
            }
        };
        for target in targets {
            let (ctx, _) = self.share_context(target, true)?;
            if let Err(e) = share_apply::materialize(&ctx, &path) {
                tracing::debug!(%target, error = %e, "materialize skipped");
            }
        }
        Ok(Response::Ack)
    }

    /// Ack when the work *starts*; the outcome arrives as `SharesApplied`.
    ///
    /// A `Run` rule is a subprocess of up to an hour and the GUI drains one
    /// command channel on one thread — the same channel that carries every
    /// keystroke. Provisioning therefore never happens on the request thread,
    /// and never under the core lock.
    fn apply_shares(
        self: &Arc<Self>,
        workspace_id: WorkspaceId,
        only: Option<Vec<ShareRuleId>>,
    ) -> Result<Response, ProtocolError> {
        // Validates the workspace before anything is spawned.
        let (_, _) = self.share_context(workspace_id, true)?;
        self.start_provisioning(workspace_id, ShareTrigger::Requested, only);
        Ok(Response::Ack)
    }

    /// Provision a workspace on a worker thread, coalescing per workspace.
    pub(crate) fn start_provisioning(
        self: &Arc<Self>,
        workspace_id: WorkspaceId,
        trigger: ShareTrigger,
        only: Option<Vec<ShareRuleId>>,
    ) {
        {
            let mut inner = self.lock();
            if !inner.provisioning.insert(workspace_id) {
                tracing::debug!(%workspace_id, "provisioning already in flight; coalescing");
                return;
            }
        }

        let daemon = Arc::clone(self);
        let spawned = std::thread::Builder::new()
            .name("forge-shares".into())
            .spawn(move || {
                // The flag is released by a guard: `Daemon::lock` recovers from
                // poisoning, and a panicked worker would otherwise latch it for
                // the life of the daemon.
                let _guard = ProvisioningGuard {
                    daemon: Arc::clone(&daemon),
                    workspace_id,
                };
                let event = daemon.provision_now(workspace_id, trigger, only.as_deref());
                daemon.registry.broadcast_domain(event);
            });
        if let Err(e) = spawned {
            let mut inner = self.lock();
            inner.provisioning.remove(&workspace_id);
            drop(inner);
            self.notice(
                NoticeLevel::Warning,
                format!("could not start provisioning: {e}"),
            );
        }
    }

    fn provision_now(
        &self,
        workspace_id: WorkspaceId,
        trigger: ShareTrigger,
        only: Option<&[ShareRuleId]>,
    ) -> DaemonEvent {
        let (ctx, project_id) = match self.share_context(workspace_id, only.is_some()) {
            Ok(pair) => pair,
            Err(e) => {
                return DaemonEvent::SharesApplied {
                    workspace_id,
                    trigger,
                    actions: Vec::new(),
                    error: Some(e.message),
                }
            }
        };
        let mut rules = self.rules_for_project(project_id);
        if let Some(only) = only {
            rules.retain(|rule| only.contains(&rule.id));
        }
        let actions = share_apply::apply_rules(&ctx, &rules);

        // A run that could do nothing at all is worth a notice; one that
        // skipped a file is not — the checkout exists and is usable either
        // way, which is the rule provisioning has always followed.
        for action in &actions {
            if action.fallback {
                self.notice(
                    NoticeLevel::Info,
                    format!(
                        "{} was copied instead of cloned: this volume has no copy-on-write",
                        action.path
                    ),
                );
            }
        }
        DaemonEvent::SharesApplied {
            workspace_id,
            trigger,
            actions,
            error: None,
        }
    }

    fn remove_worktree(
        self: &Arc<Self>,
        workspace_id: WorkspaceId,
        force: bool,
    ) -> Result<Response, ProtocolError> {
        let _span = tracing::info_span!("worktree.remove", %workspace_id).entered();
        let (ws, git_root, running) = {
            let inner = self.lock();
            let ws = inner
                .workspaces
                .get(&workspace_id)
                .cloned()
                .ok_or_else(|| ProtocolError::not_found("workspace"))?;
            let git_root = ws.project_id;
            let git_root = inner
                .projects
                .get(&git_root)
                .and_then(|p| p.git_root.clone());
            let running = inner
                .sessions
                .values()
                .any(|s| s.workspace_id == workspace_id && s.state.is_active());
            (ws, git_root, running)
        };

        if ws.kind != WorkspaceKind::GitWorktree {
            return Err(ProtocolError::new(
                ErrorCode::InvalidRequest,
                "not a worktree",
            ));
        }
        let git_root = git_root
            .ok_or_else(|| ProtocolError::new(ErrorCode::GitError, "project is not a git repo"))?;

        // Ended sessions (`Exited`/`Failed`/`Orphaned`) are not a reason to refuse.
        if !force {
            let checks = git_service::precheck_remove(&git_root, &ws.path).map_err(git_err)?;
            if running || checks.dirty || checks.merge_or_rebase_in_progress {
                return Err(ProtocolError::precondition_failed(format!(
                    "cannot remove: running_sessions={running}, dirty={}, merge_or_rebase={}",
                    checks.dirty, checks.merge_or_rebase_in_progress
                )));
            }
        }

        // Kill synchronously before rows (and cwd) go: detached SIGKILL would skip (P1).
        if force {
            let session_ids: Vec<SessionId> = {
                let inner = self.lock();
                inner
                    .sessions
                    .values()
                    .filter(|s| s.workspace_id == workspace_id)
                    .map(|s| s.id)
                    .collect()
            };
            self.kill_groups_blocking(&self.kill_targets(&session_ids));
        }

        // DB first (`ON DELETE RESTRICT`). Disk-then-row left a workspace pointing at a gone path.
        let (removed_sessions, updated_sessions) = self.drop_workspace_rows(workspace_id)?;

        // Replica drops by id: sessions before their workspace.
        for session in updated_sessions {
            self.registry
                .broadcast_domain(DaemonEvent::SessionUpdated(session));
        }
        for session_id in removed_sessions {
            self.registry
                .broadcast_domain(DaemonEvent::SessionRemoved { session_id });
        }
        self.registry
            .broadcast_domain(DaemonEvent::WorkspaceRemoved { workspace_id });

        // Unmanaged: drop the model row, never someone else's directory.
        if ws.managed_by_app {
            git_service::remove(&git_root, &ws.path, force).map_err(git_err)?;
        }
        Ok(Response::Ack)
    }

    /// FK order. Returns `(removed session ids, re-parented sessions)`. No broadcast.
    fn drop_workspace_rows(
        &self,
        workspace_id: WorkspaceId,
    ) -> Result<(Vec<SessionId>, Vec<Session>), ProtocolError> {
        let mut inner = self.lock();
        let session_ids: Vec<SessionId> = inner
            .sessions
            .values()
            .filter(|s| s.workspace_id == workspace_id)
            .map(|s| s.id)
            .collect();

        // `sessions.workspace_id` is `ON DELETE RESTRICT`.
        let mut updated: Vec<Session> = Vec::new();
        for id in &session_ids {
            updated.extend(Self::delete_session_locked(&mut inner, *id)?);
        }
        inner.db.workspaces().delete(workspace_id).map_err(db_err)?;
        inner.workspaces.remove(&workspace_id);
        // One owner, one deletion path.
        inner.status_checks.remove(&workspace_id);
        // Re-parented-on-the-way-out: no update event.
        updated.retain(|s| !session_ids.contains(&s.id));
        Ok((session_ids, updated))
    }

    /// Empty/whitespace clears the label. Never touches the git branch.
    fn rename_workspace(
        &self,
        workspace_id: WorkspaceId,
        display_name: Option<String>,
    ) -> Result<Response, ProtocolError> {
        let mut inner = self.lock();
        let mut workspace = inner
            .workspaces
            .get(&workspace_id)
            .cloned()
            .ok_or_else(|| ProtocolError::not_found("workspace"))?;
        workspace.display_name = display_name
            .map(|name| name.trim().to_owned())
            .filter(|name| !name.is_empty());
        inner.db.workspaces().upsert(&workspace).map_err(db_err)?;
        inner.workspaces.insert(workspace_id, workspace.clone());
        drop(inner);
        self.registry
            .broadcast_domain(DaemonEvent::WorkspaceUpdated(workspace));
        Ok(Response::Ack)
    }

    /// Git runs with the core lock released.
    fn refresh_workspace_status(
        self: &Arc<Self>,
        workspace_id: WorkspaceId,
    ) -> Result<Response, ProtocolError> {
        let (path, project_id) = {
            let mut inner = self.lock();
            let ws = inner
                .workspaces
                .get(&workspace_id)
                .cloned()
                .ok_or_else(|| ProtocolError::not_found("workspace"))?;

            // ADR-008: at most one `git status` per workspace every 2 s.
            if !Self::status_check_due(&mut inner, workspace_id, Instant::now()) {
                return Ok(Response::Ack);
            }
            (ws.path, ws.project_id)
        };

        // Missing directory: reconcile now. Ack-and-wait left a dead row until restart.
        let Ok(status) = git_service::status(&path) else {
            if !path.exists() {
                self.rescan_project(project_id);
            }
            return Ok(Response::Ack);
        };
        self.apply_workspace_status(workspace_id, &status);
        Ok(Response::Ack)
    }

    /// Repair path: no-op if the project is gone or not a git repo.
    fn rescan_project(self: &Arc<Self>, project_id: ProjectId) {
        let project = {
            let inner = self.lock();
            inner.projects.get(&project_id).cloned()
        };
        let Some(project) = project else { return };
        let Some(git_root) = project.git_root.clone() else {
            return;
        };
        self.rescan_project_worktrees(project_id, &git_root, &project.root_path);
    }

    /// ADR-008 throttle.
    fn status_check_due(inner: &mut Inner, workspace_id: WorkspaceId, now: Instant) -> bool {
        let due = inner
            .status_checks
            .get(&workspace_id)
            .is_none_or(|last| now.duration_since(*last) >= GIT_STATUS_THROTTLE);
        if due {
            inner.status_checks.insert(workspace_id, now);
        }
        due
    }

    // --- Sessions ---

    #[allow(clippy::too_many_arguments)]
    fn create_session(
        self: &Arc<Self>,
        workspace_id: WorkspaceId,
        kind: SessionKind,
        provider_id: Option<AgentProviderId>,
        profile_id: Option<AgentProfileId>,
        parent: Option<SessionId>,
        role: SessionRole,
        resume: Option<String>,
        initial_prompt: Option<String>,
        read_only: bool,
    ) -> Result<Response, ProtocolError> {
        let _span = tracing::info_span!("session.create", %workspace_id, ?kind).entered();

        let profile = self.launch_profile(kind, provider_id.as_ref(), profile_id)?;

        // Version probe is a subprocess: never under the core lock.
        let verified_program = match (kind, provider_id.as_ref()) {
            (SessionKind::Agent, Some(_)) if profile_has_executable(profile.as_ref()) => None,
            (SessionKind::Agent, Some(id)) => Some(self.verified_agent_executable(id)?),
            _ => None,
        };

        // The baseline is a subprocess too, so it is resolved between two lock
        // sections rather than inside one. A workspace that disappears in the
        // gap is caught by the `not_found` below; the worst case here is a
        // baseline read against a checkout that is about to go away.
        let base_commit = self
            .lock()
            .workspaces
            .get(&workspace_id)
            .map(|ws| ws.path.clone())
            .as_deref()
            .and_then(git_service::head_commit);

        let (cwd, spawn_spec, session) = {
            let mut inner = self.lock();
            let ws = inner
                .workspaces
                .get(&workspace_id)
                .cloned()
                .ok_or_else(|| ProtocolError::not_found("workspace"))?;

            let (parent_id, root_id) = Self::graph_placement(&inner, parent, workspace_id)?;

            let id = SessionId::new();
            let root_session_id = root_id.unwrap_or(id);

            let cwd = ws.path.clone();
            let spec = self.build_spawn_spec(
                &mut inner,
                kind,
                AgentLaunch::new(
                    provider_id.clone(),
                    profile.clone(),
                    verified_program,
                    resume.clone(),
                    initial_prompt.clone(),
                    read_only,
                ),
                &cwd,
                id,
            )?;

            let now = Timestamp::now();
            let session = Session {
                id,
                workspace_id,
                kind,
                role,
                parent_session_id: parent_id,
                root_session_id,
                terminal_id: None,
                agent_provider_id: provider_id,
                agent_profile_id: profile.as_ref().map(|p| p.id),
                title: SessionTitle::default(),
                state: SessionState::Starting,
                created_at: now,
                launch_command: None,
                last_activity_at: now,
                ended_at: None,
                base_commit,
            };
            inner.db.sessions().upsert(&session).map_err(db_err)?;
            inner.sessions.insert(id, session.clone());
            if let Some(resume) = resume {
                inner.resumed_from.insert(id, resume);
            }
            if read_only {
                inner.read_only.insert(id);
            }
            (cwd, spec, session)
        };

        self.registry
            .broadcast_domain(DaemonEvent::SessionCreated(session.clone()));

        match self.spawn_terminal(session.id, spawn_spec) {
            Ok(terminal_id) => Ok(Response::SessionCreated {
                session_id: session.id,
                terminal_id,
            }),
            Err(reason) => {
                self.mark_session_failed(session.id, reason.clone());
                let _ = cwd;
                Err(ProtocolError::new(ErrorCode::SpawnError, reason))
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn create_child_session(
        self: &Arc<Self>,
        parent_session_id: SessionId,
        kind: SessionKind,
        provider_id: Option<AgentProviderId>,
        profile_id: Option<AgentProfileId>,
        role: SessionRole,
        policy: ChildWorkspacePolicy,
        initial_prompt: Option<String>,
    ) -> Result<Response, ProtocolError> {
        let (parent_workspace_id, project_id) = {
            let inner = self.lock();
            let parent = inner
                .sessions
                .get(&parent_session_id)
                .ok_or_else(|| ProtocolError::not_found("parent session"))?;
            let workspace_id = parent.workspace_id;
            let project_id = inner.workspaces.get(&workspace_id).map(|w| w.project_id);
            (workspace_id, project_id)
        };

        let workspace_id = match policy {
            ChildWorkspacePolicy::SameWorkspace => parent_workspace_id,
            ChildWorkspacePolicy::ExistingWorkspace(ws) => {
                if !self.lock().workspaces.contains_key(&ws) {
                    return Err(ProtocolError::not_found("workspace"));
                }
                ws
            }
            ChildWorkspacePolicy::NewManagedWorktree { branch_hint, base } => {
                let project_id = project_id.ok_or_else(|| ProtocolError::not_found("workspace"))?;
                let branch = match branch_hint {
                    Some(hint) if !hint.trim().is_empty() => hint,
                    // v7 uuid leading bytes are a timestamp; the tail is unique per call.
                    _ => {
                        let id = SessionId::new().to_string();
                        format!("forge/child-{}", &id[id.len() - 12..])
                    }
                };
                self.create_managed_worktree(project_id, &branch, base.as_deref(), None)?
                    .id
            }
            _ => {
                return Err(ProtocolError::new(
                    ErrorCode::InvalidRequest,
                    "unsupported child workspace policy",
                ));
            }
        };
        self.create_session(
            workspace_id,
            kind,
            provider_id,
            profile_id,
            Some(parent_session_id),
            role,
            None,
            initial_prompt,
            // A harness child writes code; only a pull-request review asks for
            // the provider's read-only mode (§16.9).
            false,
        )
    }

    /// Persist an envelope and either paste it into a live PTY or spawn a child
    /// that starts with it (§8.3).
    #[allow(clippy::too_many_arguments)]
    fn send_context(
        self: &Arc<Self>,
        source_session_id: SessionId,
        target_session_id: Option<SessionId>,
        spawn: Option<SendContextSpawn>,
        summary: Option<String>,
        instructions: Option<String>,
        include_transcript: bool,
        max_transcript_bytes: Option<u32>,
    ) -> Result<Response, ProtocolError> {
        match (&target_session_id, &spawn) {
            (Some(_), None) | (None, Some(_)) => {}
            (None, None) => {
                return Err(ProtocolError::new(
                    ErrorCode::InvalidRequest,
                    "SendContext needs target_session_id or spawn",
                ));
            }
            (Some(_), Some(_)) => {
                return Err(ProtocolError::new(
                    ErrorCode::InvalidRequest,
                    "SendContext takes target_session_id or spawn, not both",
                ));
            }
        }

        let summary = crate::context_xfer::clamp_field(summary);
        let instructions = crate::context_xfer::clamp_field(instructions);
        if summary.is_none() && instructions.is_none() && !include_transcript {
            return Err(ProtocolError::new(
                ErrorCode::InvalidRequest,
                "SendContext needs summary, instructions, or include_transcript",
            ));
        }

        let source_label = {
            let inner = self.lock();
            let source = inner
                .sessions
                .get(&source_session_id)
                .ok_or_else(|| ProtocolError::not_found("source session"))?;
            source.title.resolve("session").to_owned()
        };

        let transcript_text = if include_transcript {
            match self.get_session_transcript(source_session_id, None, max_transcript_bytes)? {
                Response::SessionTranscript(t) if !t.text.trim().is_empty() => Some(t.text),
                Response::SessionTranscript(_) => None,
                _ => None,
            }
        } else {
            None
        };

        let mut artifacts: Vec<ContextArtifactRef> = Vec::new();
        if let Some(text) = transcript_text.as_deref() {
            artifacts.push(crate::context_xfer::transcript_artifact(text));
        }

        if let Some(spec) = spawn {
            let prompt = {
                let draft = ContextEnvelope {
                    id: ContextId::new(),
                    source_session_id,
                    target_session_id: None,
                    summary: summary.clone(),
                    instructions: instructions.clone(),
                    artifacts: artifacts.clone(),
                    git_context: None,
                    created_at: Timestamp::now(),
                };
                crate::context_xfer::format_delivery(
                    &draft,
                    &source_label,
                    transcript_text.as_deref(),
                )
            };
            let created = self.create_child_session(
                source_session_id,
                spec.kind,
                spec.provider_id,
                spec.profile_id,
                spec.role,
                spec.workspace_policy,
                Some(prompt),
            )?;
            let (child_id, _) = match &created {
                Response::SessionCreated {
                    session_id,
                    terminal_id,
                } => (*session_id, *terminal_id),
                _ => {
                    return Err(ProtocolError::new(
                        ErrorCode::Internal,
                        "CreateChildSession did not return SessionCreated",
                    ));
                }
            };
            let envelope = ContextEnvelope {
                id: ContextId::new(),
                source_session_id,
                target_session_id: Some(child_id),
                summary,
                instructions,
                artifacts,
                git_context: None,
                created_at: Timestamp::now(),
            };
            self.lock().db.context().insert(&envelope).map_err(db_err)?;
            return Ok(created);
        }

        let target_id = target_session_id.expect("validated above");
        if target_id == source_session_id {
            return Err(ProtocolError::new(
                ErrorCode::InvalidRequest,
                "cannot send context to the same session",
            ));
        }
        let terminal_id = {
            let inner = self.lock();
            let target = inner
                .sessions
                .get(&target_id)
                .ok_or_else(|| ProtocolError::not_found("target session"))?;
            target.terminal_id
        };

        let envelope = ContextEnvelope {
            id: ContextId::new(),
            source_session_id,
            target_session_id: Some(target_id),
            summary,
            instructions,
            artifacts,
            git_context: None,
            created_at: Timestamp::now(),
        };
        let paste = crate::context_xfer::format_delivery(
            &envelope,
            &source_label,
            transcript_text.as_deref(),
        );
        self.lock().db.context().insert(&envelope).map_err(db_err)?;

        if let Some(terminal_id) = terminal_id {
            let mut bytes = paste.into_bytes();
            if !bytes.ends_with(b"\n") {
                bytes.push(b'\n');
            }
            let _ = self.write_terminal_input(terminal_id, &bytes);
        }
        Ok(Response::Ack)
    }

    /// ADR-010: no cycle, depth ≤ 8, same project.
    fn graph_placement(
        inner: &Inner,
        parent: Option<SessionId>,
        workspace_id: WorkspaceId,
    ) -> Result<(Option<SessionId>, Option<SessionId>), ProtocolError> {
        let Some(parent_id) = parent else {
            return Ok((None, None));
        };
        let parent = inner
            .sessions
            .get(&parent_id)
            .ok_or_else(|| ProtocolError::not_found("parent session"))?;

        let child_project = inner.workspaces.get(&workspace_id).map(|w| w.project_id);
        let parent_project = inner
            .workspaces
            .get(&parent.workspace_id)
            .map(|w| w.project_id);
        if child_project.is_none() || child_project != parent_project {
            return Err(ProtocolError::new(
                ErrorCode::InvalidRequest,
                "child must be in the same project as its parent",
            ));
        }

        let mut depth = 1u32;
        let mut cur = Some(parent_id);
        let mut seen = HashSet::new();
        while let Some(id) = cur {
            if !seen.insert(id) {
                return Err(ProtocolError::new(
                    ErrorCode::InvalidRequest,
                    "cycle in session graph",
                ));
            }
            depth += 1;
            if depth > MAX_GRAPH_DEPTH {
                return Err(ProtocolError::new(
                    ErrorCode::InvalidRequest,
                    "session graph too deep",
                ));
            }
            cur = inner.sessions.get(&id).and_then(|s| s.parent_session_id);
        }
        Ok((Some(parent_id), Some(parent.root_session_id)))
    }

    /// Invalid transitions are races (EOF after `Failed`); log and keep the current state.
    fn set_session_state(
        inner: &mut Inner,
        session_id: SessionId,
        next: SessionState,
        terminal_id: Option<TerminalId>,
    ) -> Option<Session> {
        let Some(session) = inner.sessions.get(&session_id) else {
            tracing::warn!(%session_id, "state transition for an unknown session");
            return None;
        };
        if !session.state.can_transition_to(&next) {
            tracing::warn!(
                %session_id,
                from = ?session.state,
                to = ?next,
                "invalid session state transition rejected"
            );
            return None;
        }

        let now = Timestamp::now();
        let ended_at = next.is_terminal().then_some(now);
        let restarting = matches!(next, SessionState::Starting);
        let snapshot = {
            let session = inner.sessions.get_mut(&session_id)?;
            session.state = next;
            session.terminal_id = terminal_id;
            session.ended_at = ended_at;
            if restarting {
                session.last_activity_at = now;
            }
            session.clone()
        };
        inner.idle_warned.remove(&session_id);
        let _ = inner
            .db
            .sessions()
            .update_state(session_id, &snapshot.state, ended_at);
        Some(snapshot)
    }

    fn kill_session(self: &Arc<Self>, session_id: SessionId) -> Result<Response, ProtocolError> {
        let _span = tracing::info_span!("session.kill", %session_id).entered();
        let (pgid, kind) = {
            let inner = self.lock();
            let session = inner
                .sessions
                .get(&session_id)
                .ok_or_else(|| ProtocolError::not_found("session"))?;
            let Some(term_id) = session.terminal_id else {
                return Ok(Response::Ack); // nothing running
            };
            let rt = inner.terminals.get(&term_id);
            (rt.map(|r| r.process_group), session.kind)
        };

        let Some(pgid) = pgid else {
            return Ok(Response::Ack);
        };

        signal_group(pgid, first_kill_signal(kind));

        // Re-check pgid before SIGKILL: the kernel may have recycled it.
        let grace = self.config.kill_grace();
        let daemon = self.clone();
        std::thread::spawn(move || {
            std::thread::sleep(grace);
            if daemon.session_still_owns_pgid(session_id, pgid) {
                signal_group(pgid, nix::sys::signal::Signal::SIGKILL);
            }
        });
        Ok(Response::Ack)
    }

    /// Guards the delayed SIGKILL in [`Daemon::kill_session`].
    fn session_still_owns_pgid(&self, session_id: SessionId, pgid: i32) -> bool {
        let inner = self.lock();
        inner
            .sessions
            .get(&session_id)
            .and_then(|s| s.terminal_id)
            .and_then(|t| inner.terminals.get(&t))
            .is_some_and(|rt| rt.process_group == pgid)
    }

    /// Blocking kill on shutdown. Detached SIGKILL dies with the process.
    pub fn shutdown_sessions(&self) {
        // Snapshot first: killing is the point of no return, and a command
        // recorded after it would be the shell's, not the program's.
        self.snapshot_launch_commands();
        let targets: Vec<(i32, SessionKind)> = {
            let inner = self.lock();
            inner
                .sessions
                .values()
                .filter(|s| s.state.is_active())
                .filter_map(|s| {
                    let rt = inner.terminals.get(&s.terminal_id?)?;
                    Some((rt.process_group, s.kind))
                })
                .collect()
        };
        if targets.is_empty() {
            return;
        }
        tracing::info!(
            sessions = targets.len(),
            "killing live sessions on shutdown"
        );
        self.kill_groups_blocking(&targets);
    }

    /// Record what each live shell session is running, for resurrect on boot.
    ///
    /// Shells only: an agent session relaunches from its provider and profile,
    /// while a shell that only ever prompted stores nothing and restarts
    /// fresh. A command that cannot be read is not an error — the session
    /// simply restarts as a fresh shell.
    fn snapshot_launch_commands(&self) {
        let shells: Vec<(SessionId, u32)> = {
            let inner = self.lock();
            inner
                .sessions
                .values()
                .filter(|s| s.kind == SessionKind::Shell && s.state.is_active())
                .filter_map(|s| {
                    let rt = inner.terminals.get(&s.terminal_id?)?;
                    Some((s.id, rt.child_pid))
                })
                .collect()
        };
        if shells.is_empty() {
            return;
        }
        let commands = foreground_commands(&shells);
        if commands.is_empty() {
            return;
        }
        let mut inner = self.lock();
        for (id, command) in &commands {
            let Some(session) = inner.sessions.get_mut(id) else {
                continue;
            };
            session.launch_command = Some(command.clone());
            let snap = session.clone();
            if let Err(error) = inner.db.sessions().upsert(&snap) {
                tracing::warn!(%id, %error, "could not persist a session's launch command");
            }
        }
    }

    /// Active sessions that still own a live terminal.
    fn kill_targets(&self, session_ids: &[SessionId]) -> Vec<(i32, SessionKind)> {
        let inner = self.lock();
        session_ids
            .iter()
            .filter_map(|id| {
                let s = inner.sessions.get(id)?;
                s.state.is_active().then_some(())?;
                let rt = inner.terminals.get(&s.terminal_id?)?;
                Some((rt.process_group, s.kind))
            })
            .collect()
    }

    /// Blocking counterpart to [`Daemon::kill_session`]. Use before deleting rows
    /// so detached SIGKILL cannot skip a still-live child (P1).
    fn kill_groups_blocking(&self, targets: &[(i32, SessionKind)]) {
        if targets.is_empty() {
            return;
        }
        for (pgid, kind) in targets {
            signal_group(*pgid, first_kill_signal(*kind));
        }

        let deadline = Instant::now() + self.config.kill_grace();
        while Instant::now() < deadline {
            if !targets.iter().any(|(pgid, _)| group_alive(*pgid)) {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }

        for (pgid, _) in targets {
            if group_alive(*pgid) {
                tracing::warn!(pgid, "process group survived the kill grace; SIGKILL");
                signal_group(*pgid, nix::sys::signal::Signal::SIGKILL);
            }
        }
    }

    fn close_session(self: &Arc<Self>, session_id: SessionId) -> Result<Response, ProtocolError> {
        {
            let inner = self.lock();
            let session = inner
                .sessions
                .get(&session_id)
                .ok_or_else(|| ProtocolError::not_found("session"))?;
            if session.state.is_active() {
                return Err(ProtocolError::precondition_failed(
                    "session is active; kill it before closing",
                ));
            }
        }

        let updated = {
            let mut inner = self.lock();
            if !inner.sessions.contains_key(&session_id) {
                return Err(ProtocolError::not_found("session"));
            }
            Self::delete_session_locked(&mut inner, session_id)?
        };

        for session in updated {
            self.registry
                .broadcast_domain(DaemonEvent::SessionUpdated(session));
        }
        self.registry
            .broadcast_domain(DaemonEvent::SessionRemoved { session_id });
        Ok(Response::Ack)
    }

    /// Re-parent children (ADR-010) before deleting. Shared with workspace drop.
    fn delete_session_locked(
        inner: &mut Inner,
        session_id: SessionId,
    ) -> Result<Vec<Session>, ProtocolError> {
        let Some(session) = inner.sessions.get(&session_id).cloned() else {
            return Ok(Vec::new());
        };
        let grandparent = session.parent_session_id;
        let new_root =
            grandparent.and_then(|gp| inner.sessions.get(&gp).map(|s| s.root_session_id));

        let children: Vec<SessionId> = inner
            .sessions
            .values()
            .filter(|s| s.parent_session_id == Some(session_id))
            .map(|s| s.id)
            .collect();

        let mut updated: Vec<Session> = Vec::new();
        for cid in children {
            let Some(child) = inner.sessions.get_mut(&cid) else {
                continue;
            };
            child.parent_session_id = grandparent;
            child.root_session_id = new_root.unwrap_or(cid);
            let root = child.root_session_id;
            updated.push(child.clone());
            updated.extend(Self::rewrite_subtree_root(inner, cid, root));
        }
        // FK: persist re-parented subtree before deleting the closed row.
        for s in &updated {
            let _ = inner.db.sessions().upsert(s);
        }
        inner.db.sessions().delete(session_id).map_err(db_err)?;
        inner.sessions.remove(&session_id);
        inner.idle_warned.remove(&session_id);
        inner.resumed_from.remove(&session_id);
        inner.read_only.remove(&session_id);
        // Runtime is reaped on PTY EOF. Dropping it here races that and leaks a zombie (P1).
        Ok(updated)
    }

    /// Iterative: a corrupted DB row must not hang the daemon.
    fn rewrite_subtree_root(inner: &mut Inner, from: SessionId, root: SessionId) -> Vec<Session> {
        let mut children_by_parent: HashMap<SessionId, Vec<SessionId>> = HashMap::new();
        for s in inner.sessions.values() {
            if let Some(parent) = s.parent_session_id {
                children_by_parent.entry(parent).or_default().push(s.id);
            }
        }

        let mut updated = Vec::new();
        let mut seen: HashSet<SessionId> = HashSet::from([from]);
        let mut stack: Vec<SessionId> = children_by_parent.get(&from).cloned().unwrap_or_default();
        while let Some(id) = stack.pop() {
            if !seen.insert(id) {
                tracing::warn!(session_id = %id, "cycle in session graph while re-parenting");
                continue;
            }
            if let Some(session) = inner.sessions.get_mut(&id) {
                if session.root_session_id != root {
                    session.root_session_id = root;
                    updated.push(session.clone());
                }
            }
            if let Some(children) = children_by_parent.get(&id) {
                stack.extend(children.iter().copied());
            }
        }
        updated
    }

    fn restart_session(self: &Arc<Self>, session_id: SessionId) -> Result<Response, ProtocolError> {
        let (provider_id, profile_id, kind) = {
            let inner = self.lock();
            let session = inner
                .sessions
                .get(&session_id)
                .ok_or_else(|| ProtocolError::not_found("session"))?;
            if !session.state.is_terminal() {
                return Err(ProtocolError::precondition_failed(
                    "session is not in a terminal state",
                ));
            }
            match session.kind {
                SessionKind::Agent => (
                    session.agent_provider_id.clone(),
                    session.agent_profile_id,
                    session.kind,
                ),
                kind => (None, None, kind),
            }
        };
        // Missing profile is a refusal: dropping `CLAUDE_CONFIG_DIR` would log into someone else.
        let profile = self.launch_profile(kind, provider_id.as_ref(), profile_id)?;
        let verified_program = match provider_id.as_ref() {
            Some(_) if profile_has_executable(profile.as_ref()) => None,
            Some(id) => Some(self.verified_agent_executable(id)?),
            None => None,
        };

        let (spec, session) = {
            let mut inner = self.lock();
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
            let ws = inner
                .workspaces
                .get(&session.workspace_id)
                .cloned()
                .ok_or_else(|| ProtocolError::not_found("workspace"))?;
            let cwd = ws.path.clone();
            let resume = inner.resumed_from.get(&session_id).cloned();
            let read_only = inner.read_only.contains(&session_id);
            let spec = self.build_spawn_spec(
                &mut inner,
                session.kind,
                AgentLaunch::new(
                    session.agent_provider_id.clone(),
                    profile.clone(),
                    verified_program,
                    resume,
                    None,
                    read_only,
                ),
                &cwd,
                session_id,
            )?;

            let updated =
                Self::set_session_state(&mut inner, session_id, SessionState::Starting, None)
                    .ok_or_else(|| {
                        ProtocolError::precondition_failed(
                            "session cannot restart from its current state",
                        )
                    })?;
            // Consumed, not remembered: the command was unfinished business
            // from before the daemon went down. Once re-run it dissolves back
            // into a plain shell instead of haunting every later restart.
            if inner
                .sessions
                .get_mut(&session_id)
                .and_then(|s| s.launch_command.take())
                .is_some()
            {
                let snap = inner
                    .sessions
                    .get(&session_id)
                    .cloned()
                    .ok_or_else(|| ProtocolError::not_found("session"))?;
                inner.db.sessions().upsert(&snap).map_err(db_err)?;
            }
            (spec, updated)
        };

        self.registry
            .broadcast_domain(DaemonEvent::SessionUpdated(session));

        match self.spawn_terminal(session_id, spec) {
            Ok(_) => Ok(Response::Ack),
            Err(reason) => {
                self.mark_session_failed(session_id, reason.clone());
                Err(ProtocolError::new(ErrorCode::SpawnError, reason))
            }
        }
    }

    fn rename_session(
        self: &Arc<Self>,
        session_id: SessionId,
        title: Option<String>,
    ) -> Result<Response, ProtocolError> {
        let session = {
            let mut inner = self.lock();
            let session = inner
                .sessions
                .get_mut(&session_id)
                .ok_or_else(|| ProtocolError::not_found("session"))?;
            session.title.user = title;
            let snap = session.clone();
            inner.db.sessions().upsert(&snap).map_err(db_err)?;
            snap
        };
        self.registry
            .broadcast_domain(DaemonEvent::SessionUpdated(session));
        Ok(Response::Ack)
    }

    fn set_session_role(
        self: &Arc<Self>,
        session_id: SessionId,
        role: SessionRole,
    ) -> Result<Response, ProtocolError> {
        let session = {
            let mut inner = self.lock();
            let session = inner
                .sessions
                .get_mut(&session_id)
                .ok_or_else(|| ProtocolError::not_found("session"))?;
            session.role = role;
            let snap = session.clone();
            inner.db.sessions().upsert(&snap).map_err(db_err)?;
            snap
        };
        self.registry
            .broadcast_domain(DaemonEvent::SessionUpdated(session));
        Ok(Response::Ack)
    }

    // --- Terminals ---

    fn build_spawn_spec(
        &self,
        inner: &mut Inner,
        kind: SessionKind,
        agent: Option<AgentLaunch>,
        cwd: &Path,
        session_id: SessionId,
    ) -> Result<SpawnSpec, ProtocolError> {
        let env: ResolvedEnvironment = self.resolved_env(inner);
        let mut spec = match kind {
            SessionKind::Shell => {
                let mut vars: Vec<(String, String)> = env.vars.clone();
                let term = self.session_term(inner, &vars);
                upsert_var(&mut vars, "TERM", &term.term);
                if let Some(dir) = &term.terminfo_dir {
                    upsert_var(&mut vars, "TERMINFO", &dir.to_string_lossy());
                }
                upsert_var(&mut vars, "COLORTERM", "truecolor");
                let mut args = if self.config.sessions.login_shell {
                    vec!["-l".to_owned()]
                } else {
                    vec![]
                };
                // A resurrected shell re-runs what it ran when the daemon went
                // down. `-c` takes the whole line as one argument, so a
                // command with spaces and pipes survives intact. Absent at
                // creation time: a new shell just prompts.
                if let Some(command) = inner
                    .sessions
                    .get(&session_id)
                    .and_then(|s| s.launch_command.clone())
                    .map(|c| c.trim().to_owned())
                    .filter(|c| !c.is_empty())
                {
                    args.push("-c".to_owned());
                    args.push(command);
                }
                SpawnSpec {
                    program: env.shell.clone(),
                    args,
                    cwd: cwd.to_path_buf(),
                    env: vars,
                }
            }
            SessionKind::Agent => {
                let AgentLaunch {
                    provider_id,
                    profile,
                    verified_program,
                    resume,
                    initial_prompt,
                    read_only,
                } = agent.ok_or_else(|| {
                    ProtocolError::new(ErrorCode::InvalidRequest, "agent session needs a provider")
                })?;
                let profile = profile.as_ref();
                let _span =
                    tracing::info_span!("agent.spawn", provider_id = %provider_id, %session_id)
                        .entered();
                // Never re-pick off PATH by *candidate*: only the probed binary,
                // or the one the profile names, may run. A profile's bare name
                // is still looked up on PATH — that is where the wrapper script
                // standing in for a shell alias lives (§13.4).
                let program = match profile.and_then(|p| p.executable.clone()) {
                    Some(path) => agents::resolve_executable(&path, &env)
                        .filter(|resolved| is_executable_file(resolved))
                        .ok_or_else(|| {
                            ProtocolError::new(
                                ErrorCode::ProviderNotInstalled,
                                format!(
                                    "the profile's executable is gone or not runnable: {}",
                                    path.display()
                                ),
                            )
                        })?,
                    None => verified_program.ok_or_else(|| {
                        ProtocolError::new(
                            ErrorCode::ProviderNotInstalled,
                            format!("no verified executable for agent provider `{provider_id}`"),
                        )
                    })?,
                };
                let config_dir = profile.and_then(|p| profile_config_dir(p, &env));
                if let (Some(profile), Some(dir), Some(descriptor)) = (
                    profile,
                    config_dir.as_deref(),
                    inner.agents.descriptor(&provider_id).cloned(),
                ) {
                    agents::ensure_config_dir(&descriptor, dir).map_err(|e| {
                        ProtocolError::new(
                            ErrorCode::IoError,
                            format!(
                                "could not create `{}` for profile `{}`: {e}",
                                dir.display(),
                                profile.name
                            ),
                        )
                    })?;
                }
                let req = LaunchAgentRequest {
                    provider_id,
                    cwd: cwd.to_path_buf(),
                    extra_args: profile.map(|p| p.args.clone()).unwrap_or_default(),
                    executable_override: Some(program),
                    resume_session_id: resume,
                    initial_prompt,
                    read_only,
                };
                let mut spec = inner
                    .agents
                    .build_launch_with_config_dir(&req, &env, config_dir.as_deref())
                    .map_err(|e| match e {
                        agents::AgentError::NotInstalled(_) => {
                            ProtocolError::new(ErrorCode::ProviderNotInstalled, e.to_string())
                        }
                        agents::AgentError::ResumeUnsupported(_)
                        | agents::AgentError::PromptUnsupported(_)
                        | agents::AgentError::ReviewUnsupported(_) => {
                            ProtocolError::new(ErrorCode::InvalidRequest, e.to_string())
                        }
                        _ => ProtocolError::new(ErrorCode::SpawnError, e.to_string()),
                    })?;
                // Provider-shaped: agents decides whether this launch needs an
                // attention adapter. Daemon only supplies the on-disk assets (P2).
                if let Some(assets) = &self.attention_assets {
                    if let Some(descriptor) = inner.agents.descriptor(&req.provider_id) {
                        agents::inject_attention(descriptor, &mut spec, assets);
                    }
                }
                spec
            }
            _ => {
                return Err(ProtocolError::new(
                    ErrorCode::InvalidRequest,
                    "unsupported session kind",
                ))
            }
        };
        upsert_var(&mut spec.env, "FORGE_SESSION_ID", &session_id.to_string());
        upsert_var(&mut spec.env, "FORGE_WORKSPACE", &cwd.to_string_lossy());
        // Harness state lives at the repository root, not the worktree cwd.
        if let Some(root) = harness_root_in(inner, cwd) {
            upsert_var(&mut spec.env, "FORGE_HARNESS_ROOT", &root.to_string_lossy());
        }
        // Agents call `forge-daemon session|context …` with FORGE_SESSION_ID;
        // the daemon binary's directory must be on PATH inside the PTY.
        prepend_daemon_bin_to_path(&mut spec.env);
        Ok(spec)
    }

    fn spawn_terminal(
        self: &Arc<Self>,
        session_id: SessionId,
        spec: SpawnSpec,
    ) -> Result<TerminalId, String> {
        let _span = tracing::info_span!("terminal.spawn", %session_id).entered();
        let size = PtySize {
            cols: 80,
            rows: 24,
            pixel_width: 0,
            pixel_height: 0,
        }
        .sanitized();
        let scrollback = self.config.effective_scrollback() as usize;
        let mut handle = self
            .pty_backend
            .spawn(&spec, size)
            .map_err(|e| e.to_string())?;
        let reader = handle.reader();
        let writer: crate::terminal::SharedWriter = Arc::new(Mutex::new(handle.writer()));
        let child_pid = handle.child_pid();
        let process_group = handle.process_group();
        let terminal_id = TerminalId::new();

        let runtime = TerminalRuntime {
            id: terminal_id,
            session_id,
            pty: handle,
            writer,
            engine: AlacrittyEngine::with_scrollback(size, scrollback),
            delta_builder: DeltaBuilder::new(),
            emit_seq: 0,
            last_emit: None,
            pending_damage: false,
            size,
            child_pid,
            process_group,
            last_title: None,
            last_activity_broadcast: Instant::now(),
        };

        let session = {
            let mut inner = self.lock();
            inner.terminals.insert(terminal_id, runtime);
            let Some(snapshot) = Self::set_session_state(
                &mut inner,
                session_id,
                SessionState::Running,
                Some(terminal_id),
            ) else {
                inner.terminals.remove(&terminal_id);
                return Err("session left Starting during spawn".into());
            };
            snapshot
        };

        self.registry
            .broadcast_domain(DaemonEvent::SessionUpdated(session));

        let daemon = self.clone();
        std::thread::spawn(move || pty_loop(daemon, terminal_id, reader));

        Ok(terminal_id)
    }

    /// Adopt `size` only for the first subscriber. Later attaches use `ResizeTerminal`.
    fn attach_terminal(
        &self,
        terminal_id: TerminalId,
        size: PtySize,
    ) -> Result<Response, ProtocolError> {
        let _span = tracing::info_span!("terminal.attach", %terminal_id).entered();
        let size = size.sanitized();
        let shared = self.registry.subscriber_count(terminal_id) > 0;
        let (snapshot, update) = {
            let mut inner = self.lock();
            let rt = inner
                .terminals
                .get_mut(&terminal_id)
                .ok_or_else(|| ProtocolError::not_found("terminal"))?;
            if rt.size != size && !shared {
                let _ = rt.pty.resize(size);
                rt.engine.resize(size);
                rt.size = size;
            }
            let mut snap = rt
                .delta_builder
                .snapshot(&rt.engine, DEFAULT_SCROLLBACK_TAIL);
            snap.seq = rt.emit_seq;
            let update = Self::touch_activity(&mut inner, terminal_id, false);
            (snap, update)
        };
        if let Some(session) = update {
            self.registry
                .broadcast_domain(DaemonEvent::SessionUpdated(session));
        }
        Ok(Response::AttachAck { snapshot })
    }

    /// Clone the writer and release the core lock before the write: a full PTY buffer blocks.
    fn write_terminal_input(
        &self,
        terminal_id: TerminalId,
        bytes: &[u8],
    ) -> Result<Response, ProtocolError> {
        let writer = {
            let inner = self.lock();
            inner
                .terminals
                .get(&terminal_id)
                .map(|rt| Arc::clone(&rt.writer))
                .ok_or_else(|| ProtocolError::not_found("terminal"))?
        };
        crate::terminal::write_pty(&writer, bytes)
            .map_err(|e| ProtocolError::new(ErrorCode::IoError, e.to_string()))?;
        self.note_client_activity(terminal_id);
        Ok(Response::Ack)
    }

    fn resize_terminal(
        &self,
        terminal_id: TerminalId,
        size: PtySize,
    ) -> Result<Response, ProtocolError> {
        let size = size.sanitized();
        let mut inner = self.lock();
        let rt = inner
            .terminals
            .get_mut(&terminal_id)
            .ok_or_else(|| ProtocolError::not_found("terminal"))?;
        rt.pty
            .resize(size)
            .map_err(|e| ProtocolError::new(ErrorCode::IoError, e.to_string()))?;
        rt.engine.resize(size);
        rt.size = size;
        let update = Self::touch_activity(&mut inner, terminal_id, false);
        drop(inner);
        if let Some(session) = update {
            self.registry
                .broadcast_domain(DaemonEvent::SessionUpdated(session));
        }
        Ok(Response::Ack)
    }

    /// Clamp wire `from_line`/`count` before `rows` allocates under the core lock.
    fn fetch_scrollback(
        &self,
        terminal_id: TerminalId,
        from_line: i64,
        count: u32,
    ) -> Result<Response, ProtocolError> {
        if count == 0 {
            return Ok(Response::ScrollbackRows(ScrollbackRows {
                snapshot: None,
                generation: 0,
                from_line,
                rows: Vec::new(),
            }));
        }
        if count > MAX_SCROLLBACK_FETCH {
            return Err(ProtocolError::invalid_request(format!(
                "count {count} exceeds the maximum of {MAX_SCROLLBACK_FETCH} rows per fetch"
            )));
        }
        if from_line < 0 {
            return Err(ProtocolError::invalid_request(
                "from_line is an absolute scrollback index and cannot be negative",
            ));
        }

        let inner = self.lock();
        let rt = inner
            .terminals
            .get(&terminal_id)
            .ok_or_else(|| ProtocolError::not_found("terminal"))?;
        let len = rt.engine.scrollback_len() as i64;
        let start = from_line
            .checked_sub(len)
            .ok_or_else(|| ProtocolError::invalid_request("from_line is out of range"))?;
        let end = start
            .checked_add(i64::from(count))
            .ok_or_else(|| ProtocolError::invalid_request("from_line + count is out of range"))?;
        let rows = rt.engine.rows(start..end);
        Ok(Response::ScrollbackRows(ScrollbackRows {
            snapshot: None,
            generation: rt.engine.scrollback_generation(),
            from_line,
            rows,
        }))
    }

    fn send_signal(
        &self,
        session_id: SessionId,
        signal: Signal,
    ) -> Result<Response, ProtocolError> {
        let Some(num) = signal.number() else {
            return Err(ProtocolError::new(
                ErrorCode::InvalidRequest,
                "unknown signal",
            ));
        };
        let pgid = {
            let inner = self.lock();
            let session = inner
                .sessions
                .get(&session_id)
                .ok_or_else(|| ProtocolError::not_found("session"))?;
            session
                .terminal_id
                .and_then(|t| inner.terminals.get(&t))
                .map(|r| r.process_group)
        };
        if let Some(pgid) = pgid {
            if let Ok(sig) = nix::sys::signal::Signal::try_from(num) {
                signal_group(pgid, sig);
            }
        }
        Ok(Response::Ack)
    }

    // --- PTY thread callbacks ---

    pub fn has_subscribers(&self, terminal_id: TerminalId) -> bool {
        self.registry.has_subscribers(terminal_id)
    }

    pub(crate) fn recover_client(&self, client: domain::ClientId) {
        let inner = self.lock();
        self.registry.recover(client, |terminal_id| {
            let rt = inner.terminals.get(&terminal_id)?;
            let mut snapshot = rt.engine.snapshot(DEFAULT_SCROLLBACK_TAIL);
            snapshot.seq = rt.emit_seq;
            Some(DaemonEvent::TerminalResync {
                terminal_id,
                snapshot,
            })
        });
    }

    /// Feed and publish immediately, without ending an open synchronized frame.
    pub fn pump_terminal(
        self: &Arc<Self>,
        terminal_id: TerminalId,
        bytes: &[u8],
        note: bool,
    ) -> bool {
        self.pump_terminal_batch(terminal_id, bytes, note, true, false)
            .subscribed
    }

    pub(crate) fn pump_terminal_batch(
        self: &Arc<Self>,
        terminal_id: TerminalId,
        bytes: &[u8],
        note: bool,
        force_emit: bool,
        eof: bool,
    ) -> crate::terminal::PumpStatus {
        let subscribed = self.registry.has_subscribers(terminal_id);
        let mut next_wake = None;

        let mut reply = None;
        let mut bell = false;
        let mut session_update = None;

        {
            let mut inner = self.lock();
            let Some(rt) = inner.terminals.get_mut(&terminal_id) else {
                return crate::terminal::PumpStatus {
                    subscribed,
                    next_wake,
                };
            };

            let (replies, publish_damage) = rt.feed(bytes);
            rt.pending_damage |= publish_damage;
            if eof {
                rt.pending_damage |= rt.engine.finish_sync();
            }
            if !replies.is_empty() {
                reply = Some((Arc::clone(&rt.writer), replies));
            }

            let now = Instant::now();
            let emit_at = rt
                .last_emit
                .map_or(now, |last| last + crate::terminal::FRAME);
            let emit = subscribed && rt.pending_damage && (force_emit || now >= emit_at);
            if emit {
                rt.pending_damage = false;
                rt.last_emit = Some(now);
            }
            next_wake = rt.engine.sync_deadline();
            if subscribed && rt.pending_damage {
                next_wake = Some(next_wake.map_or(emit_at, |sync| sync.min(emit_at)));
            }
            if emit {
                // Once per emitted delta, not per engine feed.
                rt.emit_seq += 1;
                let seq = rt.emit_seq;
                let mut delta = rt.delta_builder.delta(&mut rt.engine);
                delta.seq = seq;
                bell = rt.engine.take_bell();
                // Empty OSC 2 is a reset, stored as `None` so resolve falls back.
                // `is_some()` on the raw string pinned a stale title; `Some("")`
                // would pin an empty one and blank the rail row.
                let title = SessionTitle::from_osc(rt.engine.title());
                let title_changed = title != rt.last_title;
                if title_changed {
                    rt.last_title = title.clone();
                }
                let session_id = rt.session_id;
                let delta_event = DaemonEvent::TerminalDelta { terminal_id, delta };

                {
                    let inner_ref = &*inner;
                    self.registry
                        .route_terminal_delta(terminal_id, &delta_event, || {
                            let rt = inner_ref.terminals.get(&terminal_id).unwrap();
                            let mut snap = rt.engine.snapshot(DEFAULT_SCROLLBACK_TAIL);
                            snap.seq = seq;
                            DaemonEvent::TerminalResync {
                                terminal_id,
                                snapshot: snap,
                            }
                        });
                }

                if title_changed {
                    if let Some(session) = inner.sessions.get_mut(&session_id) {
                        session.title.terminal = title;
                        let snap = session.clone();
                        // Known lock/I/O debt: title persistence shares Inner's SQLite connection.
                        let _ = inner.db.sessions().upsert(&snap);
                        session_update = Some(snap);
                    }
                }
            }

            if note {
                if let Some(session) =
                    Self::touch_activity(&mut inner, terminal_id, session_update.is_some())
                {
                    session_update = Some(session);
                }
            }
        }

        if let Some((writer, replies)) = reply {
            let _ = crate::terminal::write_pty(&writer, &replies);
        }
        if bell {
            self.registry
                .broadcast_domain(DaemonEvent::TerminalBell { terminal_id });
        }
        if let Some(session) = session_update {
            self.registry
                .broadcast_domain(DaemonEvent::SessionUpdated(session));
        }
        if note && !subscribed {
            self.registry
                .notify_non_subscribers(terminal_id, DaemonEvent::TerminalActivity { terminal_id });
        }
        crate::terminal::PumpStatus {
            subscribed,
            next_wake,
        }
    }

    /// Runtime-only `last_activity_at`. Never touches the DB. Caller holds the core lock.
    fn touch_activity(inner: &mut Inner, terminal_id: TerminalId, force: bool) -> Option<Session> {
        let now = Timestamp::now();
        let rt = inner.terminals.get_mut(&terminal_id)?;
        let session_id = rt.session_id;
        let due =
            force || rt.last_activity_broadcast.elapsed() >= crate::terminal::ACTIVITY_BROADCAST;
        if due {
            rt.last_activity_broadcast = Instant::now();
        }
        let session = inner.sessions.get_mut(&session_id)?;
        session.last_activity_at = now;
        let snapshot = due.then(|| session.clone());
        // Next quiet spell is a new event.
        inner.idle_warned.remove(&session_id);
        snapshot
    }

    /// Client-driven activity (keystrokes, attach, resize). Without this a quiet reader ages.
    fn note_client_activity(&self, terminal_id: TerminalId) {
        let update = {
            let mut inner = self.lock();
            Self::touch_activity(&mut inner, terminal_id, false)
        };
        if let Some(session) = update {
            self.registry
                .broadcast_domain(DaemonEvent::SessionUpdated(session));
        }
    }

    pub fn on_terminal_exited(self: &Arc<Self>, terminal_id: TerminalId) {
        // Reap outside the core lock.
        let Some(mut rt) = ({
            let mut inner = self.lock();
            if let Some(rt) = inner.terminals.get(&terminal_id) {
                self.registry.finish_terminal(terminal_id, || {
                    let mut snapshot = rt.engine.snapshot(DEFAULT_SCROLLBACK_TAIL);
                    snapshot.seq = rt.emit_seq;
                    DaemonEvent::TerminalResync {
                        terminal_id,
                        snapshot,
                    }
                });
            }
            inner.terminals.remove(&terminal_id)
        }) else {
            return;
        };
        let session_id = rt.session_id;
        let status = reap_child(rt.child_pid, rt.pty.as_mut());
        let state = SessionState::Exited {
            code: status.code,
            signal: status.signal,
        };

        let session = {
            let mut inner = self.lock();
            // Failed-spawn racing EOF stays Failed.
            Self::set_session_state(&mut inner, session_id, state, None)
        };
        if let Some(session) = session {
            // Transcript just finished: invalidate so History does not wait the TTL.
            if session.kind == SessionKind::Agent {
                self.external_agents
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .invalidate();
            }
            self.registry
                .broadcast_domain(DaemonEvent::SessionUpdated(session));
        }
    }

    fn mark_session_failed(self: &Arc<Self>, session_id: SessionId, reason: String) {
        let session = {
            let mut inner = self.lock();
            // Anything other than Starting → Failed is a race.
            Self::set_session_state(
                &mut inner,
                session_id,
                SessionState::Failed { reason },
                None,
            )
        };
        if let Some(session) = session {
            self.registry
                .broadcast_domain(DaemonEvent::SessionUpdated(session));
        }
    }

    // --- Agents ---

    /// Core lock released: probes are subprocesses and can take seconds.
    fn detect_agents(&self) -> Vec<DetectionResult> {
        let _span = tracing::info_span!("agent.detect").entered();
        let (descriptors, overrides, env) = {
            let mut inner = self.lock();
            let env = self.resolved_env(&mut inner);
            let descriptors: Vec<domain::AgentDescriptor> =
                inner.agents.descriptors().into_iter().cloned().collect();
            // Registry overrides match the persisted `SetProviderExecutable` rows.
            let overrides: HashMap<AgentProviderId, PathBuf> = inner
                .db
                .provider_overrides()
                .list()
                .unwrap_or_default()
                .into_iter()
                .collect();
            (descriptors, overrides, env)
        };

        let results: Vec<DetectionResult> = descriptors
            .iter()
            .map(|descriptor| {
                let over = overrides.get(&descriptor.id).map(PathBuf::as_path);
                agents::detect(descriptor, &env, over)
            })
            .collect();

        let mut inner = self.lock();
        for result in &results {
            inner
                .detections
                .insert(result.provider_id.clone(), result.clone());
        }
        results
    }

    /// Core lock released: each usage probe is a subprocess or HTTP call.
    ///
    /// One provider can be several logins: the default account plus every
    /// profile that moved its config directory (§13.4). Each is probed on its
    /// own, because each has its own allowance — reporting only the default one
    /// is what showed a profile's meter as somebody else's numbers. Profiles
    /// are few and the sweep is every five minutes, so the extra calls are
    /// bounded by what the user configured.
    fn collect_usage(&self) -> Vec<domain::ProviderUsage> {
        let (sources, env) = {
            let mut inner = self.lock();
            let env = self.resolved_env(&mut inner);
            let sources: Vec<UsageSource> = inner
                .agents
                .descriptors()
                .into_iter()
                .filter(|descriptor| descriptor.usage_source.is_some())
                .map(|descriptor| {
                    let executable = match inner.detections.get(&descriptor.id).map(|d| &d.status) {
                        Some(DetectionStatus::Installed { executable, .. }) => {
                            Some(executable.clone())
                        }
                        _ => None,
                    };
                    let mut accounts = vec![(None, None)];
                    accounts.extend(
                        inner
                            .profiles
                            .iter()
                            .filter(|profile| profile.provider_id == descriptor.id)
                            .filter_map(|profile| {
                                Some((Some(profile.id), Some(profile_config_dir(profile, &env)?)))
                            }),
                    );
                    UsageSource {
                        descriptor: descriptor.clone(),
                        executable,
                        accounts,
                    }
                })
                .collect();
            (sources, env)
        };

        let readings: Vec<domain::ProviderUsage> = sources
            .iter()
            .flat_map(|source| {
                source.accounts.iter().filter_map(|(profile_id, dir)| {
                    agents::collect_usage(
                        &source.descriptor,
                        source.executable.as_deref(),
                        &env,
                        &agents::UsageAccount {
                            profile_id: *profile_id,
                            config_dir: dir.as_deref(),
                        },
                    )
                })
            })
            .collect();

        self.lock().usage.clone_from(&readings);
        readings
    }

    /// Synchronous read behind [`crate::usage_stats::Cache`]. Core lock released.
    fn usage_analytics(&self, window_days: u16) -> domain::UsageAnalytics {
        let window_days = match window_days {
            0 => agents::usage::analytics::DEFAULT_WINDOW_DAYS,
            days => days.min(agents::usage::analytics::MAX_WINDOW_DAYS),
        };
        if let Some(cached) = self
            .usage_stats
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .fresh(window_days)
        {
            return cached;
        }

        let (env, profiles) = {
            let mut inner = self.lock();
            let env = self.resolved_env(&mut inner);
            let profiles = inner.profiles.clone();
            (env, profiles)
        };
        // Every account, not just the default one: a profile moves the
        // transcripts this reads along with the config directory (§13.4).
        let analytics = agents::collect_analytics(window_days, &env, &profiles);
        self.usage_stats
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .update(window_days, analytics.clone());
        analytics
    }

    /// Broadcast only when readings change. `collected_at` advancing is not a change.
    fn run_usage_sweeper(&self) {
        const PERIOD: Duration = Duration::from_secs(300);
        let mut previous: Vec<domain::ProviderUsage> = Vec::new();

        loop {
            let readings = self.collect_usage();
            if !same_usage(&previous, &readings) {
                self.registry
                    .broadcast_domain(DaemonEvent::ProviderUsageChanged {
                        usage: readings.clone(),
                    });
                previous = readings;
            }

            let jitter =
                Duration::from_millis((Timestamp::now().as_offset().nanosecond() % 30_000).into());
            let deadline = PERIOD + jitter;
            let step = Duration::from_secs(5);
            let mut slept = Duration::ZERO;
            while slept < deadline {
                if self.is_shutting_down() {
                    return;
                }
                std::thread::sleep(step.min(deadline - slept));
                slept += step;
            }
        }
    }

    fn run_pull_request_sweeper(self: &Arc<Self>) {
        let period = Duration::from_secs(self.config.github.refresh_secs);
        let step = Duration::from_secs(1);
        loop {
            let mut slept = Duration::ZERO;
            while slept < period {
                if self.is_shutting_down() {
                    return;
                }
                let sleep = step.min(period - slept);
                std::thread::sleep(sleep);
                slept += sleep;
            }
            if self.is_shutting_down() {
                return;
            }
            if let Err(error) = self.refresh_pull_requests() {
                tracing::warn!(
                    error = %error.message,
                    "could not start automatic pull-request refresh"
                );
            }
        }
    }

    // --- Idle ---

    /// Evaluate under the core lock; effects (`kill_session`) run without it.
    pub fn sweep_idle_sessions(self: &Arc<Self>) -> usize {
        let policy = self.config.idle_policy();
        if !policy.is_enabled() {
            return 0;
        }
        let now = Timestamp::now();

        let (warn, stop) = {
            let mut inner = self.lock();

            let settled: Vec<SessionId> = inner
                .idle_warned
                .iter()
                .filter(|id| {
                    inner
                        .sessions
                        .get(id)
                        .is_none_or(|s| policy.is_settled(s, now))
                })
                .copied()
                .collect();
            for id in settled {
                inner.idle_warned.remove(&id);
            }

            let attached_terminals = self.registry.subscribed_terminals();

            let mut warn: Vec<(Session, crate::idle::IdleReason)> = Vec::new();
            let mut stop: Vec<(Session, crate::idle::IdleReason)> = Vec::new();
            for session in inner.sessions.values() {
                let attached = session
                    .terminal_id
                    .is_some_and(|t| attached_terminals.contains(&t));
                let warned = inner.idle_warned.contains(&session.id);
                match policy.evaluate(session, now, attached, warned) {
                    crate::idle::IdleAction::Keep => {}
                    crate::idle::IdleAction::Warn(reason) => warn.push((session.clone(), reason)),
                    crate::idle::IdleAction::Stop(reason) => stop.push((session.clone(), reason)),
                }
            }
            for (session, _) in &warn {
                inner.idle_warned.insert(session.id);
            }
            (warn, stop)
        };

        for (session, reason) in warn {
            self.notice(
                NoticeLevel::Info,
                Self::describe_idle(&session, reason, false),
            );
        }

        let stopped = stop.len();
        for (session, reason) in stop {
            self.notice(
                NoticeLevel::Warning,
                Self::describe_idle(&session, reason, true),
            );
            if let Err(e) = self.kill_session(session.id) {
                tracing::warn!(session_id = %session.id, error = %e.message, "idle stop failed");
            }
        }
        stopped
    }

    fn describe_idle(session: &Session, reason: crate::idle::IdleReason, stopping: bool) -> String {
        let what = match session.kind {
            SessionKind::Agent => session
                .agent_provider_id
                .as_ref()
                .map_or("Agent", |p| p.as_str()),
            _ => "Shell",
        };
        let name = session.title.resolve(what);
        let how_long = crate::idle::humanize(reason.duration());
        match (reason, stopping) {
            (crate::idle::IdleReason::Inactive(_), false) => {
                format!("\"{name}\" has been idle for {how_long}.")
            }
            (crate::idle::IdleReason::Inactive(_), true) => {
                format!("Stopping \"{name}\": idle for {how_long} (sessions.idle_stop_after_secs).")
            }
            (crate::idle::IdleReason::LongRunning(_), _) => {
                format!("\"{name}\" has been open for {how_long}.")
            }
        }
    }

    fn run_idle_sweeper(self: &Arc<Self>) {
        const PERIOD: Duration = Duration::from_secs(30);
        loop {
            if self.is_shutting_down() {
                return;
            }
            self.sweep_idle_sessions();
            let step = Duration::from_secs(5);
            let mut slept = Duration::ZERO;
            while slept < PERIOD {
                if self.is_shutting_down() {
                    return;
                }
                std::thread::sleep(step.min(PERIOD - slept));
                slept += step;
            }
        }
    }

    fn detect_agent(
        &self,
        provider_id: &AgentProviderId,
    ) -> Result<DetectionResult, ProtocolError> {
        let (descriptor, over, env) = {
            let mut inner = self.lock();
            let descriptor = inner
                .agents
                .descriptors()
                .into_iter()
                .find(|d| d.id == *provider_id)
                .cloned()
                .ok_or_else(|| ProtocolError::not_found("agent provider"))?;
            let over = inner
                .db
                .provider_overrides()
                .get(provider_id)
                .ok()
                .flatten();
            let env = self.resolved_env(&mut inner);
            (descriptor, over, env)
        };

        let result = agents::detect(&descriptor, &env, over.as_deref());

        self.lock()
            .detections
            .insert(result.provider_id.clone(), result.clone());
        Ok(result)
    }

    /// Profile must belong to the provider being launched.
    fn launch_profile(
        &self,
        kind: SessionKind,
        provider_id: Option<&AgentProviderId>,
        profile_id: Option<AgentProfileId>,
    ) -> Result<Option<AgentProfile>, ProtocolError> {
        let Some(profile_id) = profile_id else {
            return Ok(None);
        };
        if kind != SessionKind::Agent {
            return Err(ProtocolError::new(
                ErrorCode::InvalidRequest,
                "only an agent session can use a launch profile",
            ));
        }
        let profile = self
            .lock()
            .profiles
            .iter()
            .find(|p| p.id == profile_id)
            .cloned()
            .ok_or_else(|| ProtocolError::not_found("agent profile"))?;
        if provider_id != Some(&profile.provider_id) {
            return Err(ProtocolError::new(
                ErrorCode::InvalidRequest,
                format!(
                    "profile `{}` belongs to provider `{}`",
                    profile.name, profile.provider_id
                ),
            ));
        }
        Ok(Some(profile))
    }

    /// Validate here so a broken profile never looks saved.
    fn save_agent_profile(&self, profile: AgentProfile) -> Result<Response, ProtocolError> {
        let descriptor = self
            .lock()
            .agents
            .descriptor(&profile.provider_id)
            .cloned()
            .ok_or_else(|| ProtocolError::not_found("agent provider"))?;
        let profile = validate_profile(profile, &descriptor)?;

        // Probe is a subprocess: core lock released.
        if let Some(path) = profile.executable.clone() {
            let env = {
                let mut inner = self.lock();
                self.resolved_env(&mut inner)
            };
            // A bare name is probed where it will actually be found at launch —
            // on the login shell's PATH, not under the daemon's cwd (§13.4).
            let resolved = agents::resolve_executable(&path, &env).unwrap_or_else(|| path.clone());
            let status = agents::detect(&descriptor, &env, Some(&resolved)).status;
            if !status.is_installed() {
                return Err(ProtocolError::with_details(
                    ErrorCode::ProviderNotInstalled,
                    format!(
                        "`{}` is not a usable {} executable",
                        path.display(),
                        descriptor.display_name
                    ),
                    describe_detection(&status),
                ));
            }
        }

        {
            let mut inner = self.lock();
            inner
                .db
                .agent_profiles()
                .upsert(&profile)
                .map_err(|e| profile_db_err(&profile.name, e))?;
            inner.profiles = inner.db.agent_profiles().list().map_err(db_err)?;
        }
        self.forget_account_scans();
        self.broadcast_profiles();
        Ok(Response::Ack)
    }

    /// Does not kill running sessions.
    fn remove_agent_profile(&self, profile_id: AgentProfileId) -> Result<Response, ProtocolError> {
        {
            let mut inner = self.lock();
            if !inner
                .db
                .agent_profiles()
                .delete(profile_id)
                .map_err(db_err)?
            {
                return Err(ProtocolError::not_found("agent profile"));
            }
            inner.profiles = inner.db.agent_profiles().list().map_err(db_err)?;
        }
        self.forget_account_scans();
        self.broadcast_profiles();
        Ok(Response::Ack)
    }

    /// Drop the scans that read a provider's own directories, because the set
    /// of profiles is the set of accounts they cover (§13.4). Discovery keys on
    /// the profiles itself; the token scan only knows its window.
    fn forget_account_scans(&self) {
        self.usage_stats
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .invalidate();
    }

    /// Whole list: a merge would strand a deleted profile.
    fn broadcast_profiles(&self) {
        let profiles = self.lock().profiles.clone();
        self.registry
            .broadcast_domain(DaemonEvent::AgentProfilesChanged { profiles });
    }

    /// The probed `Installed` binary. Never re-walk PATH. Probe now if detection has not run.
    pub(crate) fn verified_agent_executable(
        &self,
        provider_id: &AgentProviderId,
    ) -> Result<PathBuf, ProtocolError> {
        let cached = self.lock().detections.get(provider_id).cloned();
        let detection = match cached {
            Some(detection) => detection,
            None => {
                // Probe now; broadcast so the picker matches.
                let result = self.detect_agent(provider_id)?;
                self.registry
                    .broadcast_domain(DaemonEvent::AgentDetectionChanged {
                        results: vec![result.clone()],
                    });
                result
            }
        };
        match detection.status {
            DetectionStatus::Installed { executable, .. } => Ok(executable),
            other => Err(ProtocolError::new(
                ErrorCode::ProviderNotInstalled,
                format!(
                    "agent provider `{provider_id}` is not installed: {}",
                    describe_detection(&other)
                ),
            )),
        }
    }

    /// Cached. A fallback raises a notice rather than silently ignoring `[sessions].term`.
    fn session_term(&self, inner: &mut Inner, vars: &[(String, String)]) -> TermSelection {
        if let Some(cached) = &inner.term_selection {
            return cached.clone();
        }
        let selection =
            terminfo::resolve(&self.config.sessions.term, self.config.terminfo_dir(), vars);
        if let Some(requested) = &selection.fell_back_from {
            self.notice(
                NoticeLevel::Warning,
                format!(
                    "no terminfo entry for `{requested}`; shell sessions will use \
                     `{}` instead. Install the entry or point \
                     `[sessions].terminfo_dir` at the database that has it.",
                    selection.term
                ),
            );
        } else {
            tracing::info!(
                term = %selection.term,
                terminfo = ?selection.terminfo_dir,
                "resolved the session TERM"
            );
        }
        inner.term_selection = Some(selection.clone());
        selection
    }

    /// Cached login-shell env. Fallback notice once per daemon.
    fn resolved_env(&self, inner: &mut Inner) -> ResolvedEnvironment {
        let env = inner.env.get().clone();
        if env.source == EnvSource::ProcessFallback && !inner.env_fallback_noticed {
            inner.env_fallback_noticed = true;
            self.notice(
                NoticeLevel::Warning,
                format!(
                    "could not read the login-shell environment from {}; \
                     using the daemon environment with a widened PATH. \
                     Agent CLIs installed only in your shell profile may not be found.",
                    env.shell.display()
                ),
            );
        }
        env
    }

    fn provider_infos(&self) -> Vec<ProviderInfo> {
        let inner = self.lock();
        self.provider_infos_locked(&inner)
    }

    fn provider_infos_locked(&self, inner: &Inner) -> Vec<ProviderInfo> {
        inner
            .agents
            .descriptors()
            .into_iter()
            .map(|descriptor| {
                let detection = inner
                    .detections
                    .get(&descriptor.id)
                    .cloned()
                    .unwrap_or_else(|| DetectionResult {
                        provider_id: descriptor.id.clone(),
                        status: DetectionStatus::NotFound,
                        checked_at: Timestamp::now(),
                    });
                ProviderInfo {
                    descriptor: descriptor.clone(),
                    detection,
                }
            })
            .collect()
    }
}

/// Client-facing reason a provider is not launchable.
fn describe_detection(status: &DetectionStatus) -> String {
    match status {
        DetectionStatus::NotFound => "no candidate binary was found on PATH".to_owned(),
        DetectionStatus::Rejected { candidate, reason } => {
            format!("candidate `{candidate}` failed its version probe: {reason}")
        }
        DetectionStatus::ProbeTimeout => "the version probe timed out".to_owned(),
        // `Installed` never gets here.
        _ => "detection did not confirm an executable".to_owned(),
    }
}

/// `Ok(false)` if the source is absent. Refuses anything that is not a plain file inside both trees.
/// Releases a workspace's provisioning flag however the worker ends.
///
/// `Daemon::lock` recovers from poisoning, so a panicked worker that left the
/// flag set would make that workspace unprovisionable until the daemon
/// restarted — the same reason `fetching` and `pr_opening` are unwound by a
/// `Drop` impl rather than by the happy path.
struct ProvisioningGuard {
    daemon: Arc<Daemon>,
    workspace_id: WorkspaceId,
}

/// Releases a workspace's Juva flag however the worker ends, for the reason
/// [`ProvisioningGuard`] exists: a panic under a recovered lock would otherwise
/// leave that workspace unable to draft until the daemon restarted.
struct DraftingGuard {
    daemon: Arc<Daemon>,
    workspace_id: WorkspaceId,
}

impl Drop for DraftingGuard {
    fn drop(&mut self) {
        let mut inner = self.daemon.lock();
        inner.drafting.remove(&self.workspace_id);
    }
}

impl Drop for ProvisioningGuard {
    fn drop(&mut self) {
        let mut inner = self.daemon.lock();
        inner.provisioning.remove(&self.workspace_id);
    }
}

/// Out-of-range git dates become `None` rather than failing the listing.
/// How long the shutdown snapshot waits for `ps`.
const SNAPSHOT_PS_TIMEOUT: Duration = Duration::from_secs(2);
/// Longest command line remembered for resurrect.
const MAX_COMMAND_BYTES: usize = 4096;
/// Largest `ps` dump parsed; the table is consumed per line and dropped.
const MAX_PS_LINES: usize = 16_384;

/// Foreground command of each `(session, shell pid)` pair.
///
/// The deepest descendant of the shell wins; the shell itself means a fresh
/// prompt and yields nothing. Empty on any failure: a command that cannot be
/// read relaunches a fresh shell, which is the safe fallback, never an error.
fn foreground_commands(shells: &[(SessionId, u32)]) -> HashMap<SessionId, String> {
    let output = run_ps_table().unwrap_or_default();
    let table = parse_ps_table(&output);
    let mut out = HashMap::new();
    for (id, shell) in shells {
        if let Some(command) = foreground_below(*shell, &table) {
            out.insert(*id, command);
        }
    }
    out
}

/// One `ps -eo pid,ppid,args` snapshot, or `None` when it fails or stalls.
///
/// Same spawn discipline as `shares::apply`'s runner: own process group, bounded
/// wait, negative-pgid kill. This runs once per shutdown, never per frame.
fn run_ps_table() -> Option<String> {
    use std::os::unix::process::CommandExt as _;
    use std::process::{Command, Stdio};

    let child = Command::new("ps")
        .args(["-eo", "pid,ppid,args"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .ok()?;
    let pid = child.id();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(child.wait_with_output());
    });
    match rx.recv_timeout(SNAPSHOT_PS_TIMEOUT) {
        Ok(Ok(output)) if output.status.success() => {
            let text = String::from_utf8_lossy(&output.stdout);
            Some(
                text.lines()
                    .take(MAX_PS_LINES)
                    .collect::<Vec<_>>()
                    .join("\n"),
            )
        }
        // Stalled or failed: kill the group so no `ps` outlives the daemon.
        _ => {
            #[allow(clippy::cast_possible_wrap)]
            let raw = pid as i32;
            let _ = nix::sys::signal::kill(
                nix::unistd::Pid::from_raw(-raw),
                nix::sys::signal::Signal::SIGKILL,
            );
            None
        }
    }
}

/// `pid -> (ppid, args)` from a `ps -eo pid,ppid,args` dump.
///
/// Pure, so the shutdown path's parsing is unit-testable without spawning.
/// Argument spacing is preserved verbatim: collapsing it would rewrite quoted
/// strings before the relaunched shell ever saw them.
fn parse_ps_table(output: &str) -> HashMap<u32, (u32, String)> {
    let mut table = HashMap::new();
    for line in output.lines() {
        if let Some((pid, ppid, args)) = split_ps_line(line) {
            table.insert(pid, (ppid, args.to_owned()));
        }
    }
    table
}

/// One `ps` row: two leading pids, then the command line untouched.
fn split_ps_line(line: &str) -> Option<(u32, u32, &str)> {
    let line = line.trim_start();
    let pid_end = line.find(char::is_whitespace)?;
    let pid: u32 = line[..pid_end].parse().ok()?;
    let rest = line[pid_end..].trim_start();
    let ppid_end = rest.find(char::is_whitespace)?;
    let ppid: u32 = rest[..ppid_end].parse().ok()?;
    let args = rest[ppid_end..].trim_start();
    if args.is_empty() {
        return None;
    }
    Some((pid, ppid, args))
}

/// Deepest descendant of `shell` and its command line, or `None` when the
/// shell itself is the youngest (a fresh prompt) or the tree is unknown.
fn foreground_below(shell: u32, table: &HashMap<u32, (u32, String)>) -> Option<String> {
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for (pid, (ppid, _)) in table {
        children.entry(*ppid).or_default().push(*pid);
    }
    let mut best: Option<(usize, u32)> = None;
    let mut stack = vec![(shell, 0usize)];
    // A process table is not a tree: `ps` reports the kernel as `0 0`, its own
    // parent, and a reparented row can point back up. Without this the walk
    // pushes that row forever — a spin at 100% of a core, inside the path that
    // names a session after what it is running.
    let mut seen: HashSet<u32> = HashSet::new();
    while let Some((pid, depth)) = stack.pop() {
        if !seen.insert(pid) {
            continue;
        }
        if pid != shell {
            let replace = match best {
                None => true,
                Some((best_depth, best_pid)) => (depth, pid) > (best_depth, best_pid),
            };
            if replace {
                best = Some((depth, pid));
            }
        }
        if let Some(kids) = children.get(&pid) {
            let mut kids = kids.clone();
            kids.sort_unstable();
            for kid in kids {
                stack.push((kid, depth + 1));
            }
        }
    }
    best.and_then(|(_, pid)| table.get(&pid).map(|(_, args)| clamp_command(args)))
}

/// Cap a remembered command line before it is stored.
fn clamp_command(args: &str) -> String {
    let trimmed = args.trim();
    match trimmed.char_indices().nth(MAX_COMMAND_BYTES) {
        Some((idx, _)) => trimmed[..idx].to_owned(),
        None => trimmed.to_owned(),
    }
}

fn unix_to_timestamp(secs: i64) -> Option<Timestamp> {
    time::OffsetDateTime::from_unix_timestamp(secs)
        .ok()
        .map(Timestamp::from_offset)
}

/// Fall back to the path unchanged if it no longer exists.
fn canonical_or_self(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Seed a display name from the directory basename when it is shorter than the branch.
fn inferred_display_name(path: &Path, branch: Option<&str>) -> Option<String> {
    let basename = path.file_name()?.to_string_lossy();
    let basename = basename.trim();
    if basename.is_empty() {
        return None;
    }
    match branch {
        Some(branch) if basename == branch => None,
        Some(_) | None => Some(basename.to_owned()),
    }
}

/// Used for the out-of-home warning.
/// One provider's usage probe and every login it should be run for.
struct UsageSource {
    descriptor: domain::AgentDescriptor,
    executable: Option<PathBuf>,
    /// The default account first (`(None, None)`), then one entry per profile
    /// that moved the config directory.
    accounts: Vec<(Option<domain::AgentProfileId>, Option<PathBuf>)>,
}

/// A profile's config directory as an absolute path (§13.4), or `None` when it
/// shares the provider's default account.
///
/// Resolved against the *user's* home, not the daemon's working directory:
/// launchd starts it in `/`, where a relative `.claude-personal` fails on a
/// read-only file system.
fn profile_config_dir(profile: &AgentProfile, env: &ResolvedEnvironment) -> Option<PathBuf> {
    let dir = profile.config_dir.as_deref()?;
    let home = env.get("HOME").map(PathBuf::from).or_else(home_dir);
    Some(match home.as_deref() {
        Some(home) => AgentProfile::resolve_config_dir(dir, home),
        None => dir.to_path_buf(),
    })
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
}

/// SIGHUP for shells, SIGTERM for agents.
fn first_kill_signal(kind: SessionKind) -> nix::sys::signal::Signal {
    if kind == SessionKind::Shell {
        nix::sys::signal::Signal::SIGHUP
    } else {
        nix::sys::signal::Signal::SIGTERM
    }
}

/// `kill(-pgid, 0)`.
fn group_alive(pgid: i32) -> bool {
    nix::sys::signal::kill(nix::unistd::Pid::from_raw(-pgid), None).is_ok()
}

impl Daemon {
    /// Opaque GUI key. The harness step's agent lives here, not a second store.
    pub(crate) fn app_state_value(&self, key: &str) -> Option<String> {
        let inner = self.lock();
        inner.db.app_state().get(key).ok().flatten()
    }

    /// First installed provider that can run a job (picker order).
    pub(crate) fn first_headless_provider(&self) -> Option<AgentProviderId> {
        let inner = self.lock();
        inner
            .agents
            .descriptors()
            .into_iter()
            .find(|descriptor| {
                descriptor.headless.is_some()
                    && inner
                        .detections
                        .get(&descriptor.id)
                        .is_some_and(|detection| detection.status.is_installed())
            })
            .map(|descriptor| descriptor.id.clone())
    }

    /// Under the daemon log dir, not the project: removing a worktree must not drop the transcript.
    pub(crate) fn job_log_path(&self, id: domain::JobId) -> Result<PathBuf, ProtocolError> {
        let dir = crate::paths::logs_dir()
            .map_err(|e| ProtocolError::new(ErrorCode::IoError, e.to_string()))?
            .join("jobs");
        std::fs::create_dir_all(&dir)
            .map_err(|e| ProtocolError::new(ErrorCode::IoError, e.to_string()))?;
        Ok(dir.join(format!("{id}.jsonl")))
    }

    /// Same env as an interactive launch.
    pub(crate) fn job_environment(&self, cwd: &Path) -> (Vec<(String, String)>, Option<PathBuf>) {
        let mut inner = self.lock();
        let harness_root = harness_root_in(&inner, cwd);
        let env = self.resolved_env(&mut inner);
        (env.vars, harness_root)
    }
}

/// The repository root that owns `harness/`, for one project.
///
/// `git_root`, not `root_path`: a project is registered at whatever directory
/// the user picked, and that is routinely a subdirectory of the repository —
/// `apps/tauri/src-tauri` is one you would plausibly open on its own. But
/// `harness/` is metadata of the *repository*, like `.git`, so a `root_path`
/// one level down names a directory that has no `harness/` in it at all.
///
/// The agent side resolves the same root from `--git-common-dir`
/// (`harness/src/harness.ts`), and the two ends have to agree: this value is
/// exported as `FORGE_HARNESS_ROOT`, which wins there over any resolution of
/// its own. Disagreeing means the GUI reads a different `features.json` than
/// the agent writes, and — through the sandbox's writable dirs in `jobs.rs` —
/// that the agent is handed write access to a directory the harness never
/// touches while being denied the one it does.
///
/// A project outside a repository has no `git_root`; `root_path` is then the
/// only root it has, and the fallback keeps that case working.
pub(crate) fn harness_root_of(project: &Project) -> PathBuf {
    project
        .git_root
        .clone()
        .unwrap_or_else(|| project.root_path.clone())
}

/// Same root, reached from the checkout at `cwd` rather than a project id.
///
/// Runs under the core lock, so it stays a lookup: git is never spawned here.
pub(crate) fn harness_root_in(inner: &Inner, cwd: &Path) -> Option<PathBuf> {
    inner
        .workspaces
        .values()
        .find(|workspace| workspace.path == cwd)
        .and_then(|workspace| inner.projects.get(&workspace.project_id))
        .map(harness_root_of)
}

/// Negative pid: a single-pid kill orphans grandchildren.
pub(crate) fn signal_group(pgid: i32, sig: nix::sys::signal::Signal) {
    let _ = nix::sys::signal::kill(nix::unistd::Pid::from_raw(-pgid), sig);
}

/// Semantic equality: `collected_at` advancing is not a change.
fn same_usage(a: &[domain::ProviderUsage], b: &[domain::ProviderUsage]) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b)
            .all(|(left, right)| left.same_reading(right))
}

const REAP_WINDOW: Duration = Duration::from_millis(500);
const REAP_POLL: Duration = Duration::from_millis(5);

/// EOF and child exit are unordered; one `try_wait` leaves a zombie. `waitpid`
/// supplies the signal number (`portable-pty` only has the name). `try_wait`
/// remains the fallback for fakes and `ECHILD`.
fn reap_child(child_pid: u32, pty: &mut dyn terminal_core::PtyHandle) -> terminal_core::ExitStatus {
    use nix::sys::wait::{waitpid, WaitPidFlag, WaitStatus};

    // `waitpid(0)` is "any child in my group". Pid 0 must not reap an unrelated child.
    if child_pid == 0 {
        return pty.try_wait().ok().flatten().unwrap_or_default();
    }
    let pid = nix::unistd::Pid::from_raw(child_pid as i32);

    let deadline = Instant::now() + REAP_WINDOW;
    loop {
        match waitpid(pid, Some(WaitPidFlag::WNOHANG)) {
            Ok(WaitStatus::Exited(_, code)) => {
                return terminal_core::ExitStatus {
                    code: Some(code),
                    signal: None,
                }
            }
            Ok(WaitStatus::Signaled(_, sig, _)) => {
                return terminal_core::ExitStatus {
                    code: None,
                    signal: Some(sig as i32),
                }
            }
            Ok(_) => {}
            Err(_) => break,
        }
        if Instant::now() >= deadline {
            tracing::warn!(
                pid = child_pid,
                "pty reached EOF but the child is still alive; reaping in the background"
            );
            std::thread::spawn(move || {
                let _ = waitpid(pid, None);
            });
            break;
        }
        std::thread::sleep(REAP_POLL);
    }

    pty.try_wait().ok().flatten().unwrap_or_default()
}

/// Insert or replace a `KEY=VALUE` pair.
fn upsert_var(vars: &mut Vec<(String, String)>, key: &str, value: &str) {
    match vars.iter_mut().find(|(k, _)| k == key) {
        Some((_, v)) => *v = value.to_owned(),
        None => vars.push((key.to_owned(), value.to_owned())),
    }
}

/// Put the running `forge-daemon` binary's directory first on PATH.
fn prepend_daemon_bin_to_path(vars: &mut Vec<(String, String)>) {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let Some(dir) = exe.parent() else {
        return;
    };
    let dir = dir.to_string_lossy();
    match vars.iter_mut().find(|(k, _)| k == "PATH") {
        Some((_, path)) => {
            if path.split(':').any(|entry| entry == dir) {
                return;
            }
            *path = format!("{dir}:{path}");
        }
        None => vars.push(("PATH".to_owned(), dir.into_owned())),
    }
}

/// Install Forge attention assets beside the worktrees root.
///
/// Default layout puts worktrees under `data_dir/worktrees`, so the assets
/// land at `data_dir/agent-plugins/`. A custom `worktrees.root` gets a sibling
/// directory — still private to the daemon's tree.
fn install_attention_assets(worktrees_root: &Path) -> Option<agents::AttentionAssets> {
    let dir = worktrees_root
        .parent()
        .unwrap_or(worktrees_root)
        .join("agent-plugins");
    match agents::install_assets(&dir) {
        Ok(assets) => Some(assets),
        Err(error) => {
            tracing::warn!(
                %error,
                dir = %dir.display(),
                "could not install agent attention assets"
            );
            None
        }
    }
}

fn db_err(e: persistence::DbError) -> ProtocolError {
    ProtocolError::new(ErrorCode::Internal, format!("persistence: {e}"))
}

fn validate_project_group_name(name: &str) -> Result<&str, ProtocolError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(ProtocolError::new(
            ErrorCode::InvalidRequest,
            "workspace name cannot be empty",
        ));
    }
    if name.chars().count() > 128 {
        return Err(ProtocolError::new(
            ErrorCode::InvalidRequest,
            "workspace name cannot exceed 128 characters",
        ));
    }
    Ok(name)
}

struct AgentLaunch {
    provider_id: AgentProviderId,
    profile: Option<AgentProfile>,
    /// `None` when the profile brings its own binary.
    verified_program: Option<PathBuf>,
    resume: Option<String>,
    /// Fresh launch only. A restart must not re-send the original prompt.
    initial_prompt: Option<String>,
    /// Launch in the provider's own read-only mode (§16.9).
    read_only: bool,
}

impl AgentLaunch {
    #[allow(clippy::too_many_arguments)]
    fn new(
        provider_id: Option<AgentProviderId>,
        profile: Option<AgentProfile>,
        verified_program: Option<PathBuf>,
        resume: Option<String>,
        initial_prompt: Option<String>,
        read_only: bool,
    ) -> Option<Self> {
        Some(Self {
            provider_id: provider_id?,
            profile,
            verified_program,
            resume,
            initial_prompt,
            read_only,
        })
    }
}

fn profile_has_executable(profile: Option<&AgentProfile>) -> bool {
    profile.is_some_and(|p| p.executable.is_some())
}

fn is_executable_file(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.is_file() && meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        meta.is_file()
    }
}

fn validate_profile(
    mut profile: AgentProfile,
    descriptor: &AgentDescriptor,
) -> Result<AgentProfile, ProtocolError> {
    profile.name = profile.name.trim().to_owned();
    if profile.name.is_empty() {
        return Err(ProtocolError::new(
            ErrorCode::InvalidRequest,
            "a profile needs a name",
        ));
    }
    if profile.name.chars().count() > 64 {
        return Err(ProtocolError::new(
            ErrorCode::InvalidRequest,
            "a profile name cannot exceed 64 characters",
        ));
    }
    profile.config_dir = profile.config_dir.and_then(|dir| {
        let trimmed = dir.to_string_lossy().trim().to_owned();
        (!trimmed.is_empty()).then(|| PathBuf::from(trimmed))
    });
    // A directory the provider is never told about would look saved and change
    // nothing, which is the failure this whole screen exists to avoid.
    if profile.config_dir.is_some() && descriptor.config_dir.is_none() {
        return Err(ProtocolError::new(
            ErrorCode::InvalidRequest,
            format!(
                "{} has no config directory of its own to move",
                descriptor.display_name
            ),
        ));
    }
    profile.executable = profile.executable.and_then(|path| {
        let trimmed = path.to_string_lossy().trim().to_owned();
        (!trimmed.is_empty()).then(|| PathBuf::from(trimmed))
    });
    Ok(profile)
}

/// Turn a profile write error into a protocol error, recognizing the unique
/// index on `(provider_id, name)` as a name clash rather than an internal fault.
fn profile_db_err(name: &str, e: persistence::DbError) -> ProtocolError {
    let message = e.to_string();
    if message.contains("UNIQUE constraint failed") {
        return ProtocolError::conflict(format!(
            "another profile of this agent is already called `{name}`"
        ));
    }
    db_err(e)
}

/// The fallback a session's title falls back to, matching the tab strip.
fn session_fallback_title(session: &Session) -> &str {
    match session.kind {
        SessionKind::Agent => session
            .agent_provider_id
            .as_ref()
            .map_or("Agent", |id| id.as_str()),
        _ => "Shell",
    }
}

/// `(base to diff against, why it is that)` for a recorded baseline.
fn resolve_base(
    repo: &std::path::Path,
    recorded: Option<&str>,
) -> (Option<String>, domain::BaseOrigin) {
    match recorded {
        None => (None, domain::BaseOrigin::Missing),
        Some(base) if git_service::commit_exists(repo, base) => {
            (Some(base.to_owned()), domain::BaseOrigin::Recorded)
        }
        // A rebase or an amend can leave a baseline pointing at nothing. The
        // read still answers, against `HEAD`, and the caller shows why.
        Some(_) => (None, domain::BaseOrigin::Unreachable),
    }
}

fn commit_line(entry: git_service::CommitEntry) -> domain::CommitLine {
    domain::CommitLine {
        short_id: entry.short_id,
        subject: entry.subject,
    }
}

fn change_summary(
    base: Option<&str>,
    summary: git_service::SummaryOfChanges,
) -> domain::ChangeSummary {
    domain::ChangeSummary {
        branch: summary.branch,
        base: base.map(git_service::short_commit),
        files: summary
            .files
            .into_iter()
            .map(|file| domain::ChangeSummaryFile {
                path: file.path,
                status: file_change(file.status),
                additions: file.additions,
                deletions: file.deletions,
                binary: file.binary,
            })
            .collect(),
        commits: summary.commits.into_iter().map(commit_line).collect(),
        commit_count: summary.commit_count,
        truncated: summary.truncated,
    }
}

fn file_change(status: git_service::FileChange) -> domain::DiffStatus {
    match status {
        git_service::FileChange::Added => domain::DiffStatus::Added,
        git_service::FileChange::Deleted => domain::DiffStatus::Deleted,
        git_service::FileChange::Modified => domain::DiffStatus::Modified,
        git_service::FileChange::Conflicted => domain::DiffStatus::Conflicted,
    }
}

/// One terminal row as plain text, with its trailing blanks dropped.
///
/// A wide grapheme's continuation cell carries no text (§11.4), so pushing
/// every cell's `&str` is already correct for it — and allocates nothing per
/// cell beyond the string being built.
fn row_text(row: &domain::Row) -> String {
    let mut out = String::new();
    for cell in row.cells.iter() {
        out.push_str(cell.text.as_str());
    }
    while out.ends_with(' ') {
        out.pop();
    }
    out
}

fn diff_file(file: git_service::FileDiff) -> domain::DiffFile {
    domain::DiffFile {
        path: file.path,
        status: file_change(file.status),
        additions: file.additions,
        deletions: file.deletions,
        patch: file.patch,
        binary: file.binary,
        truncated: file.truncated,
    }
}

fn rebase_state(
    workspace_id: WorkspaceId,
    state: git_service::SequencerState,
) -> domain::RebaseState {
    domain::RebaseState {
        workspace_id,
        operation: state.operation.map(|op| match op {
            git_service::Sequencer::Rebase => domain::SequencerOp::Rebase,
            git_service::Sequencer::Merge => domain::SequencerOp::Merge,
            git_service::Sequencer::CherryPick => domain::SequencerOp::CherryPick,
            git_service::Sequencer::Revert => domain::SequencerOp::Revert,
        }),
        head: state.head,
        branch: state.branch,
        onto: state.onto,
        step: state.step,
        total: state.total,
        conflicts: state
            .conflicts
            .into_iter()
            .map(|c| domain::ConflictFile {
                path: c.path,
                code: c.code,
            })
            .collect(),
        truncated: state.truncated,
    }
}

fn git_err(e: git_service::GitError) -> ProtocolError {
    use git_service::GitError;
    match e {
        GitError::Conflict { .. } => ProtocolError::conflict(e.to_string()),
        GitError::InvalidBranchName { .. } => {
            ProtocolError::new(ErrorCode::InvalidRequest, e.to_string())
        }
        _ => ProtocolError::new(ErrorCode::GitError, e.to_string()),
    }
}

fn fs_err(e: fs_service::FsError) -> ProtocolError {
    match e {
        fs_service::FsError::EscapesWorkspace(_)
        | fs_service::FsError::NotFound(_)
        | fs_service::FsError::AlreadyExists(_) => {
            ProtocolError::new(ErrorCode::InvalidRequest, e.to_string())
        }
        fs_service::FsError::TooLarge { .. } | fs_service::FsError::Binary => {
            ProtocolError::new(ErrorCode::InvalidRequest, e.to_string())
        }
        fs_service::FsError::RevisionMismatch { .. } => {
            ProtocolError::precondition_failed(e.to_string())
        }
        fs_service::FsError::Git(g) => git_err(g),
        fs_service::FsError::Io(io) => ProtocolError::new(ErrorCode::IoError, io.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::ClientId;
    use protocol::DaemonMessage;
    use test_support::FakePtyBackend;

    fn usage(provider: &str, percent: u8) -> domain::ProviderUsage {
        domain::ProviderUsage {
            provider_id: AgentProviderId::new(provider),
            profile_id: None,
            windows: vec![domain::UsageWindow {
                used_percent: percent,
                window: "5h".to_owned(),
                resets_at: None,
            }],
            collected_at: Timestamp::now(),
        }
    }

    /// The sweeper only broadcasts when the reading changes; `collected_at`
    /// advancing on every sweep must not count as a change (§16.2).
    #[test]
    fn same_usage_ignores_collected_at_but_not_windows() {
        let a = usage("claude", 40);
        let mut later = a.clone();
        later.collected_at = Timestamp::now();
        assert!(
            same_usage(std::slice::from_ref(&a), std::slice::from_ref(&later)),
            "only the clock moved"
        );

        let changed = usage("claude", 55);
        assert!(
            !same_usage(std::slice::from_ref(&a), std::slice::from_ref(&changed)),
            "a different percent is a real change"
        );
        assert!(
            !same_usage(std::slice::from_ref(&a), &[]),
            "a provider dropping out is a real change"
        );
    }

    /// A daemon core over an in-memory DB, without the §15.3 startup scans or
    /// the agent-detection thread. The `TempDir` backs `worktrees_root` and is
    /// returned so it outlives the daemon.
    fn test_daemon() -> (Arc<Daemon>, tempfile::TempDir) {
        let (daemon, tmp, _backend) = test_daemon_with_pty(FakePtyBackend::empty());
        (daemon, tmp)
    }

    /// A daemon core driving a [`FakePtyBackend`], so the session lifecycle can
    /// be exercised without spawning real processes (§21 "Determinismo"). The
    /// backend handle is returned so a test can assert on what was written to
    /// the PTY and decide when the "child" exits.
    fn test_daemon_with_pty(
        backend: FakePtyBackend,
    ) -> (Arc<Daemon>, tempfile::TempDir, FakePtyBackend) {
        // `[sessions].term` defaults to Ghostty's entry, which exists on some
        // machines and not others; pinning it keeps the spawn assertions from
        // depending on what the test host has installed.
        let mut config = Config::default();
        config.sessions.term = terminfo::FALLBACK_TERM.to_owned();
        test_daemon_with_config(backend, config)
    }

    fn test_daemon_with_config(
        backend: FakePtyBackend,
        config: Config,
    ) -> (Arc<Daemon>, tempfile::TempDir, FakePtyBackend) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let db = Db::open_in_memory().expect("open db");
        let daemon = Daemon::load_with_backend(
            db,
            config,
            tmp.path().join("worktrees"),
            "test-instance".to_string(),
            "0.0.0-test".to_string(),
            Box::new(backend.clone()),
        )
        .expect("load daemon core");
        // The gate must not depend on the terminal it was launched from: a
        // resolved login shell inherits Forge's own exports (FORGE_*,
        // OPENCODE_CONFIG_CONTENT) from an agent PTY.
        {
            let mut inner = daemon.lock();
            inner.env.set_for_test(
                PathBuf::from("/bin/sh"),
                vec![
                    ("HOME".to_owned(), tmp.path().to_string_lossy().into_owned()),
                    (
                        "PATH".to_owned(),
                        "/usr/bin:/bin:/usr/sbin:/sbin".to_owned(),
                    ),
                ],
            );
        }
        (daemon, tmp, backend)
    }

    /// Seed a project in the model and the DB (foreign keys are ON, so the rows
    /// have to exist before workspaces and sessions can reference them).
    fn add_project_row(daemon: &Daemon, name: &str, root: Option<PathBuf>) -> ProjectId {
        let id = ProjectId::new();
        let now = Timestamp::now();
        let project = Project {
            id,
            project_group_id: None,
            name: name.to_string(),
            icon: None,
            // Deliberately non-existent unless the caller says otherwise.
            root_path: root.unwrap_or_else(|| PathBuf::from(format!("/nonexistent/forge/{id}"))),
            git_root: None,
            created_at: now,
            last_opened_at: now,
        };
        let mut inner = daemon.lock();
        inner
            .db
            .projects()
            .upsert(&project)
            .expect("persist project");
        inner.projects.insert(id, project);
        id
    }

    fn add_workspace_row(daemon: &Daemon, project_id: ProjectId, managed: bool) -> WorkspaceId {
        let id = WorkspaceId::new();
        let ws = Workspace {
            id,
            project_id,
            kind: WorkspaceKind::Main,
            path: PathBuf::from(format!("/nonexistent/forge/ws/{id}")),
            branch: None,
            display_name: None,
            managed_by_app: managed,
            created_at: Timestamp::now(),
            status: WorkspaceStatus::default(),
        };
        let mut inner = daemon.lock();
        inner
            .db
            .workspaces()
            .upsert(&ws)
            .expect("persist workspace");
        inner.workspaces.insert(id, ws);
        id
    }

    fn add_session_row(
        daemon: &Daemon,
        workspace_id: WorkspaceId,
        parent: Option<SessionId>,
        state: SessionState,
    ) -> SessionId {
        let mut inner = daemon.lock();
        let id = SessionId::new();
        let root_session_id = parent
            .and_then(|p| inner.sessions.get(&p).map(|s| s.root_session_id))
            .unwrap_or(id);
        let session = Session {
            id,
            workspace_id,
            kind: SessionKind::Shell,
            role: SessionRole::Generic,
            parent_session_id: parent,
            root_session_id,
            terminal_id: None,
            agent_provider_id: None,
            agent_profile_id: None,
            title: SessionTitle::default(),
            state,
            created_at: Timestamp::now(),
            launch_command: None,
            last_activity_at: Timestamp::now(),
            ended_at: None,
            base_commit: None,
        };
        inner
            .db
            .sessions()
            .upsert(&session)
            .expect("persist session");
        inner.sessions.insert(id, session);
        id
    }

    /// Backdate a session's idle clock without waiting for real time.
    fn backdate_activity(daemon: &Daemon, session_id: SessionId, secs: i64) {
        let mut inner = daemon.lock();
        let session = inner.sessions.get_mut(&session_id).expect("session");
        session.last_activity_at =
            Timestamp::from_offset(Timestamp::now().as_offset() - time::Duration::seconds(secs));
    }

    fn idle_config(warn_secs: u64, stop_secs: u64) -> Config {
        let mut config = Config::default();
        config.sessions.term = terminfo::FALLBACK_TERM.to_owned();
        config.sessions.idle_warn_after_secs = warn_secs;
        config.sessions.idle_stop_after_secs = stop_secs;
        // Every session in these tests is a shell over the fake PTY.
        config.sessions.idle_include_shells = true;
        config
    }

    // ---------------------------------------------------------------
    // Idle sessions (see [`crate::idle`])
    // ---------------------------------------------------------------

    #[test]
    fn a_quiet_session_is_reported_once_and_left_running() {
        let (daemon, tmp, _b) =
            test_daemon_with_config(FakePtyBackend::empty(), idle_config(60, 0));
        let ws = seeded_workspace(&daemon, tmp.path());
        daemon
            .create_session(
                ws,
                SessionKind::Shell,
                None,
                None,
                None,
                SessionRole::Generic,
                None,
                None,
                false,
            )
            .expect("create session");
        let session_id = only_session(&daemon).id;

        assert_eq!(daemon.sweep_idle_sessions(), 0, "brand new: nothing to do");
        assert!(daemon.take_pending_notices().is_empty());

        backdate_activity(&daemon, session_id, 3600);
        assert_eq!(daemon.sweep_idle_sessions(), 0, "warning never stops");
        let notices = daemon.take_pending_notices();
        assert_eq!(notices.len(), 1, "one notice for the quiet spell");
        assert!(
            matches!(&notices[0], DaemonEvent::DaemonNotice { message, .. } if message.contains("idle for")),
            "{:?}",
            notices[0]
        );
        assert!(
            only_session(&daemon).state.is_active(),
            "warning must not touch the process"
        );

        // Still quiet on the next pass: reported once, not once per sweep.
        daemon.sweep_idle_sessions();
        assert!(daemon.take_pending_notices().is_empty());
    }

    #[test]
    fn a_session_that_wakes_up_is_reported_again_next_time_it_goes_quiet() {
        let (daemon, tmp, _b) =
            test_daemon_with_config(FakePtyBackend::empty(), idle_config(60, 0));
        let ws = seeded_workspace(&daemon, tmp.path());
        daemon
            .create_session(
                ws,
                SessionKind::Shell,
                None,
                None,
                None,
                SessionRole::Generic,
                None,
                None,
                false,
            )
            .expect("create session");
        let session_id = only_session(&daemon).id;

        backdate_activity(&daemon, session_id, 3600);
        daemon.sweep_idle_sessions();
        assert_eq!(daemon.take_pending_notices().len(), 1);

        // The child produces output: the session is in use again.
        let terminal_id = only_session(&daemon).terminal_id.expect("terminal");
        daemon.pump_terminal(terminal_id, b"working\n", true);
        assert!(
            daemon.lock().idle_warned.is_empty(),
            "activity clears the flag"
        );

        // ...and then goes quiet a second time: a new event, so a new notice.
        backdate_activity(&daemon, session_id, 3600);
        daemon.sweep_idle_sessions();
        assert_eq!(daemon.take_pending_notices().len(), 1);
    }

    #[test]
    fn an_idle_session_is_stopped_only_when_a_stop_threshold_is_set() {
        let (daemon, tmp, backend) =
            test_daemon_with_config(FakePtyBackend::empty(), idle_config(60, 600));
        let ws = seeded_workspace(&daemon, tmp.path());
        daemon
            .create_session(
                ws,
                SessionKind::Shell,
                None,
                None,
                None,
                SessionRole::Generic,
                None,
                None,
                false,
            )
            .expect("create session");
        let session_id = only_session(&daemon).id;

        // Past the warning but short of the stop threshold: warn only.
        backdate_activity(&daemon, session_id, 120);
        assert_eq!(daemon.sweep_idle_sessions(), 0);
        assert!(only_session(&daemon).state.is_active());
        let _ = daemon.take_pending_notices();

        // Past the stop threshold: the session is killed through the normal path.
        backdate_activity(&daemon, session_id, 3600);
        assert_eq!(daemon.sweep_idle_sessions(), 1);
        let notices = daemon.take_pending_notices();
        assert!(
            notices.iter().any(|n| matches!(
                n,
                DaemonEvent::DaemonNotice { message, .. } if message.contains("Stopping")
            )),
            "the stop is announced, never silent: {notices:?}"
        );
        let _ = backend;
    }

    #[test]
    fn the_default_policy_never_stops_an_idle_session() {
        // The shipped defaults warn and nothing more, however long the silence.
        let mut config = Config::default();
        config.sessions.term = terminfo::FALLBACK_TERM.to_owned();
        config.sessions.idle_include_shells = true;
        let (daemon, tmp, _b) = test_daemon_with_config(FakePtyBackend::empty(), config);
        let ws = seeded_workspace(&daemon, tmp.path());
        daemon
            .create_session(
                ws,
                SessionKind::Shell,
                None,
                None,
                None,
                SessionRole::Generic,
                None,
                None,
                false,
            )
            .expect("create session");
        let session_id = only_session(&daemon).id;

        backdate_activity(&daemon, session_id, 30 * 24 * 3600);
        assert_eq!(daemon.sweep_idle_sessions(), 0);
        assert!(only_session(&daemon).state.is_active());
    }

    #[test]
    fn an_agent_session_is_swept_even_when_shells_are_exempt() {
        let mut config = idle_config(60, 0);
        config.sessions.idle_include_shells = false;
        let (daemon, tmp, _b) = test_daemon_with_config(FakePtyBackend::empty(), config);
        let ws = seeded_workspace(&daemon, tmp.path());
        daemon
            .create_session(
                ws,
                SessionKind::Shell,
                None,
                None,
                None,
                SessionRole::Generic,
                None,
                None,
                false,
            )
            .expect("create session");
        let shell_id = only_session(&daemon).id;
        backdate_activity(&daemon, shell_id, 3600);
        daemon.sweep_idle_sessions();
        assert!(
            daemon.take_pending_notices().is_empty(),
            "a shell at a prompt is the resting state of a terminal"
        );

        // The same session, relabelled as an agent, is not exempt.
        daemon
            .lock()
            .sessions
            .get_mut(&shell_id)
            .expect("session")
            .kind = SessionKind::Agent;
        daemon.sweep_idle_sessions();
        assert_eq!(daemon.take_pending_notices().len(), 1);
    }

    #[test]
    fn terminal_output_advances_the_idle_clock() {
        let (daemon, tmp, _b) =
            test_daemon_with_config(FakePtyBackend::empty(), idle_config(60, 0));
        let ws = seeded_workspace(&daemon, tmp.path());
        daemon
            .create_session(
                ws,
                SessionKind::Shell,
                None,
                None,
                None,
                SessionRole::Generic,
                None,
                None,
                false,
            )
            .expect("create session");
        let session = only_session(&daemon);
        let terminal_id = session.terminal_id.expect("terminal");

        backdate_activity(&daemon, session.id, 3600);
        let before = only_session(&daemon).last_activity_at;

        // `note = false` is the ≤1/s coalescing case: it must not pay for a
        // clock read, so the timestamp does not move.
        daemon.pump_terminal(terminal_id, b"quiet\n", false);
        assert_eq!(only_session(&daemon).last_activity_at, before);

        daemon.pump_terminal(terminal_id, b"loud\n", true);
        assert!(only_session(&daemon).last_activity_at > before);
    }

    #[test]
    fn restarting_a_session_restarts_its_idle_clock() {
        let (daemon, tmp, backend) =
            test_daemon_with_config(FakePtyBackend::empty(), idle_config(60, 0));
        let ws = seeded_workspace(&daemon, tmp.path());
        daemon
            .create_session(
                ws,
                SessionKind::Shell,
                None,
                None,
                None,
                SessionRole::Generic,
                None,
                None,
                false,
            )
            .expect("create session");
        let session_id = only_session(&daemon).id;

        backdate_activity(&daemon, session_id, 3600);
        daemon.sweep_idle_sessions();
        assert_eq!(daemon.take_pending_notices().len(), 1);

        // Kill it, then bring it back: the restart is the session being used.
        let terminal_id = only_session(&daemon).terminal_id.expect("terminal");
        daemon.kill_session(session_id).expect("kill");
        backend.set_exited(terminal_core::ExitStatus::default());
        daemon.on_terminal_exited(terminal_id);
        daemon.restart_session(session_id).expect("restart");

        let restarted = only_session(&daemon);
        assert!(restarted.idle_for(Timestamp::now()) < Duration::from_secs(5));
        daemon.sweep_idle_sessions();
        assert!(
            daemon.take_pending_notices().is_empty(),
            "a session just restarted is not idle"
        );
    }

    #[test]
    fn a_shell_alone_means_a_fresh_prompt_not_a_command() {
        let table = parse_ps_table("  PID  PPID COMMAND\n  100   1 -zsh\n");
        assert_eq!(foreground_below(100, &table), None);
    }

    #[test]
    fn the_deepest_descendant_wins_and_spacing_survives() {
        let table = parse_ps_table(
            "  PID  PPID COMMAND\n    1     0 /sbin/launchd\n  100     1 -zsh\n  200   100 npm run  dev\n  300   200 node  server.js\n",
        );
        assert_eq!(
            foreground_below(100, &table).as_deref(),
            Some("node  server.js")
        );
    }

    #[test]
    fn an_unknown_tree_and_a_kernel_row_yield_nothing() {
        let table = parse_ps_table("  PID  PPID COMMAND\n    0     0 [kernel]\n");
        assert_eq!(foreground_below(999, &table), None);
        assert_eq!(foreground_below(0, &table), None);
    }

    #[test]
    fn a_cycle_in_the_process_table_terminates_instead_of_spinning() {
        // `ps` reports the kernel as its own parent, and a reparented row can
        // point back up. The walk used to push such a row forever — a spin at
        // 100% of a core inside the path that names a session.
        let table = parse_ps_table(
            "  PID  PPID COMMAND\n  100     1 -zsh\n  200   100 node\n  100   200 -zsh\n",
        );
        assert!(foreground_below(100, &table).is_some());

        let self_parent = parse_ps_table("  PID  PPID COMMAND\n   42    42 loop\n");
        assert_eq!(foreground_below(42, &self_parent), None);
    }

    #[test]
    fn a_remembered_command_is_capped_before_it_is_stored() {
        let long = format!("node {}", "x".repeat(MAX_COMMAND_BYTES + 100));
        let table = parse_ps_table(&format!(
            "  PID  PPID COMMAND\n  100     1 -zsh\n  200   100 {long}\n"
        ));
        let command = foreground_below(100, &table).expect("a command");
        assert!(command.len() <= MAX_COMMAND_BYTES);
        assert!(command.starts_with("node "));
    }

    // ---------------------------------------------------------------
    // Session lifecycle over a fake PTY (§21)    // ---------------------------------------------------------------

    /// A workspace whose path really exists, so a spawn can use it as its cwd.
    fn seeded_workspace(daemon: &Daemon, dir: &std::path::Path) -> WorkspaceId {
        let project = add_project_row(daemon, "p", Some(dir.to_path_buf()));
        let id = WorkspaceId::new();
        let ws = Workspace {
            id,
            project_id: project,
            kind: WorkspaceKind::Main,
            path: dir.to_path_buf(),
            branch: None,
            display_name: None,
            managed_by_app: false,
            created_at: Timestamp::now(),
            status: WorkspaceStatus::default(),
        };
        let mut inner = daemon.lock();
        inner
            .db
            .workspaces()
            .upsert(&ws)
            .expect("persist workspace");
        inner.workspaces.insert(id, ws);
        id
    }

    fn only_session(daemon: &Daemon) -> Session {
        let inner = daemon.lock();
        let mut all: Vec<Session> = inner.sessions.values().cloned().collect();
        assert_eq!(all.len(), 1, "expected exactly one session");
        all.remove(0)
    }

    /// A `[sessions].term` with no terminfo entry must not reach the child: an
    /// unknown `TERM` leaves every ncurses program without capabilities.
    #[test]
    fn an_unresolvable_term_falls_back_and_never_exports_terminfo() {
        // An empty database, so `term` cannot resolve however the host is set up.
        let db = tempfile::tempdir().expect("tempdir");
        let mut config = Config::default();
        config.sessions.term = "xterm-nonesuch".to_owned();
        config.sessions.terminfo_dir = db.path().to_string_lossy().into_owned();
        config.sessions.login_shell = false;
        let (daemon, tmp, backend) = test_daemon_with_config(FakePtyBackend::empty(), config);
        let workspace = seeded_workspace(&daemon, tmp.path());

        daemon
            .create_session(
                workspace,
                SessionKind::Shell,
                None,
                None,
                None,
                SessionRole::Generic,
                None,
                None,
                false,
            )
            .expect("create shell session");

        let spec = backend.last_spawn().expect("a spawn happened");
        let var = |k: &str| {
            spec.env
                .iter()
                .find(|(name, _)| name == k)
                .map(|(_, v)| v.clone())
        };
        assert_eq!(var("TERM").as_deref(), Some(terminfo::FALLBACK_TERM));
        // Whatever the login-shell environment carried stays untouched: the
        // failed probe must not point the child at the database it just missed.
        assert_ne!(
            var("TERMINFO").as_deref(),
            Some(db.path().to_string_lossy().as_ref())
        );
        // `login_shell = false` is the escape hatch for shells that reject `-l`.
        assert!(spec.args.is_empty());
    }

    /// Clicking a discovered transcript re-enters the conversation: the launch
    /// carries the provider's own resume arguments (§13.5), and a restart of
    /// that session stays on the same conversation instead of quietly opening a
    /// blank one under a title that says otherwise.
    #[test]
    fn a_resumed_agent_relaunches_on_the_same_conversation() {
        let (daemon, tmp, backend) = test_daemon_with_pty(FakePtyBackend::empty());
        let workspace = seeded_workspace(&daemon, tmp.path());
        let provider = AgentProviderId::new("claude");
        let executable = test_support::write_fake_agent(tmp.path(), "claude", "1.0.0");
        {
            let mut inner = daemon.lock();
            inner.detections.insert(
                provider.clone(),
                DetectionResult {
                    provider_id: provider.clone(),
                    status: DetectionStatus::Installed {
                        executable: executable.clone(),
                        version: Some("1.0.0".to_owned()),
                    },
                    checked_at: Timestamp::now(),
                },
            );
        }

        daemon
            .create_session(
                workspace,
                SessionKind::Agent,
                Some(provider),
                None,
                None,
                SessionRole::Generic,
                Some("39c2ae5c-4fe3".to_owned()),
                None,
                false,
            )
            .expect("create a resumed agent session");

        let spec = backend.last_spawn().expect("a spawn happened");
        assert_eq!(spec.program.file_name().unwrap(), "claude");
        // Attention appends `--settings`; the resume spelling is the assertion.
        assert_eq!(spec.args[..2], ["--resume", "39c2ae5c-4fe3"]);

        let session = only_session(&daemon);
        let terminal_id = session
            .terminal_id
            .expect("a running session has a terminal");
        daemon.kill_session(session.id).expect("kill");
        backend.set_exited(terminal_core::ExitStatus::default());
        daemon.on_terminal_exited(terminal_id);
        daemon.restart_session(session.id).expect("restart");

        let respawn = backend.last_spawn().expect("a respawn happened");
        assert_eq!(respawn.args[..2], ["--resume", "39c2ae5c-4fe3"]);
    }

    /// A provider that declares no resume spelling is refused rather than
    /// started on a fresh conversation the caller did not ask for.
    #[test]
    fn resuming_a_provider_that_cannot_is_refused() {
        let (daemon, tmp, _backend) = test_daemon_with_pty(FakePtyBackend::empty());
        let workspace = seeded_workspace(&daemon, tmp.path());
        let provider = AgentProviderId::new("cursor");
        let executable = test_support::write_fake_agent(tmp.path(), "cursor-agent", "cursor 1.0");
        {
            let mut inner = daemon.lock();
            inner.detections.insert(
                provider.clone(),
                DetectionResult {
                    provider_id: provider.clone(),
                    status: DetectionStatus::Installed {
                        executable,
                        version: None,
                    },
                    checked_at: Timestamp::now(),
                },
            );
        }

        let error = daemon
            .create_session(
                workspace,
                SessionKind::Agent,
                Some(provider),
                None,
                None,
                SessionRole::Generic,
                Some("chat-1".to_owned()),
                None,
                false,
            )
            .expect_err("cursor cannot resume");
        assert_eq!(error.code, ErrorCode::InvalidRequest);
    }

    /// A pull-request review starts in a checkout the user is working in, so
    /// the read-only flags are the difference between a reader and something
    /// that can edit it — and a restart that dropped them would quietly turn
    /// one into the other (§16.9).
    #[test]
    fn a_read_only_agent_session_keeps_its_flags_across_a_restart() {
        let (daemon, tmp, backend) = test_daemon_with_pty(FakePtyBackend::empty());
        let workspace = seeded_workspace(&daemon, tmp.path());
        let provider = AgentProviderId::new("claude");
        let executable = test_support::write_fake_agent(tmp.path(), "claude", "1.0.0");
        {
            let mut inner = daemon.lock();
            inner.detections.insert(
                provider.clone(),
                DetectionResult {
                    provider_id: provider.clone(),
                    status: DetectionStatus::Installed {
                        executable,
                        version: Some("1.0.0".to_owned()),
                    },
                    checked_at: Timestamp::now(),
                },
            );
        }

        daemon
            .create_session(
                workspace,
                SessionKind::Agent,
                Some(provider),
                None,
                None,
                SessionRole::Reviewer,
                None,
                Some("review it".to_owned()),
                true,
            )
            .expect("create a read-only agent session");

        let spec = backend.last_spawn().expect("a spawn happened");
        // Attention appends `--settings`; the read-only flags are the assertion.
        assert_eq!(spec.args[..3], ["--permission-mode", "plan", "review it"]);

        let session = only_session(&daemon);
        let terminal_id = session
            .terminal_id
            .expect("a running session has a terminal");
        daemon.kill_session(session.id).expect("kill");
        backend.set_exited(terminal_core::ExitStatus::default());
        daemon.on_terminal_exited(terminal_id);
        daemon.restart_session(session.id).expect("restart");

        // The prompt is deliberately gone — a restart must not re-ask — while
        // the read-only mode is deliberately still there.
        let respawn = backend.last_spawn().expect("a respawn happened");
        assert_eq!(respawn.args[..2], ["--permission-mode", "plan"]);
    }

    /// OpenCode never rings BEL itself; Claude / Codex / Cursor / Grok need a
    /// Forge adapter too. Each launch carries its provider's attention wiring.
    #[test]
    fn an_opencode_launch_carries_the_attention_plugin() {
        let (daemon, tmp, backend) = test_daemon_with_pty(FakePtyBackend::empty());
        let workspace = seeded_workspace(&daemon, tmp.path());
        let assets = daemon
            .attention_assets
            .as_ref()
            .expect("attention assets installed beside the worktrees root");
        assert!(assets.opencode_plugin.exists());

        let provider = AgentProviderId::new("opencode");
        let executable = test_support::write_fake_agent(tmp.path(), "opencode", "1.18.29");
        {
            let mut inner = daemon.lock();
            inner.detections.insert(
                provider.clone(),
                DetectionResult {
                    provider_id: provider.clone(),
                    status: DetectionStatus::Installed {
                        executable,
                        version: Some("1.18.29".to_owned()),
                    },
                    checked_at: Timestamp::now(),
                },
            );
        }

        daemon
            .create_session(
                workspace,
                SessionKind::Agent,
                Some(provider),
                None,
                None,
                SessionRole::Generic,
                None,
                None,
                false,
            )
            .expect("create an opencode session");

        let spec = backend.last_spawn().expect("a spawn happened");
        let content = spec
            .env
            .iter()
            .find(|(k, _)| k == "OPENCODE_CONFIG_CONTENT")
            .map(|(_, v)| v.as_str())
            .expect("OPENCODE_CONFIG_CONTENT");
        assert!(
            content.contains(&assets.opencode_plugin.to_string_lossy().into_owned()),
            "plugin path missing from {content}"
        );
    }

    #[test]
    fn a_claude_launch_carries_forge_settings() {
        let (daemon, tmp, backend) = test_daemon_with_pty(FakePtyBackend::empty());
        let workspace = seeded_workspace(&daemon, tmp.path());
        let settings = daemon
            .attention_assets
            .as_ref()
            .expect("attention assets")
            .claude_settings
            .clone();
        let provider = AgentProviderId::new("claude");
        let executable = test_support::write_fake_agent(tmp.path(), "claude", "1.0.0");
        {
            let mut inner = daemon.lock();
            inner.detections.insert(
                provider.clone(),
                DetectionResult {
                    provider_id: provider.clone(),
                    status: DetectionStatus::Installed {
                        executable,
                        version: Some("1.0.0".to_owned()),
                    },
                    checked_at: Timestamp::now(),
                },
            );
        }

        daemon
            .create_session(
                workspace,
                SessionKind::Agent,
                Some(provider),
                None,
                None,
                SessionRole::Generic,
                None,
                None,
                false,
            )
            .expect("create a claude session");

        let spec = backend.last_spawn().expect("a spawn happened");
        assert!(spec
            .args
            .windows(2)
            .any(|w| { w[0] == "--settings" && w[1] == settings.to_string_lossy() }));
    }

    #[test]
    fn a_codex_launch_requests_bel_on_approval() {
        let (daemon, tmp, backend) = test_daemon_with_pty(FakePtyBackend::empty());
        let workspace = seeded_workspace(&daemon, tmp.path());
        let provider = AgentProviderId::new("codex");
        let executable = test_support::write_fake_agent(tmp.path(), "codex", "0.153.0");
        {
            let mut inner = daemon.lock();
            inner.detections.insert(
                provider.clone(),
                DetectionResult {
                    provider_id: provider.clone(),
                    status: DetectionStatus::Installed {
                        executable,
                        version: Some("0.153.0".to_owned()),
                    },
                    checked_at: Timestamp::now(),
                },
            );
        }

        daemon
            .create_session(
                workspace,
                SessionKind::Agent,
                Some(provider),
                None,
                None,
                SessionRole::Generic,
                None,
                None,
                false,
            )
            .expect("create a codex session");

        let spec = backend.last_spawn().expect("a spawn happened");
        assert!(spec
            .args
            .iter()
            .any(|a| a.contains(r#"notification_method="bel""#)));
        assert!(spec.args.iter().any(|a| a.contains("approval-requested")));
    }

    #[test]
    fn creating_a_shell_session_spawns_a_pty_and_marks_it_running() {
        let (daemon, tmp, backend) = test_daemon_with_pty(FakePtyBackend::empty());
        let workspace = seeded_workspace(&daemon, tmp.path());

        let response = daemon
            .create_session(
                workspace,
                SessionKind::Shell,
                None,
                None,
                None,
                SessionRole::Generic,
                None,
                None,
                false,
            )
            .expect("create shell session");

        let session = only_session(&daemon);
        assert_eq!(session.state, SessionState::Running);
        let terminal_id = session
            .terminal_id
            .expect("a running session has a terminal");
        // The response carries the freshly minted ids so a client can attach
        // without reloading the snapshot to guess which session is new (L3).
        assert_eq!(
            response,
            Response::SessionCreated {
                session_id: session.id,
                terminal_id,
            }
        );

        // §13.3: the spawn carries a complete environment with the daemon's own
        // variables injected, and runs in the workspace directory.
        let spec = backend.last_spawn().expect("a spawn happened");
        assert_eq!(spec.cwd, tmp.path());
        let var = |k: &str| {
            spec.env
                .iter()
                .find(|(name, _)| name == k)
                .map(|(_, v)| v.clone())
        };
        assert_eq!(var("TERM").as_deref(), Some("xterm-256color"));
        assert_eq!(var("COLORTERM").as_deref(), Some("truecolor"));
        // §15.4: shell sessions are login shells by default, so they source the
        // same profile the user's own terminal does.
        assert_eq!(spec.args, ["-l"]);
        assert_eq!(var("FORGE_SESSION_ID"), Some(session.id.to_string()));
        assert_eq!(
            var("FORGE_WORKSPACE"),
            Some(tmp.path().to_string_lossy().into_owned())
        );

        // Input reaches the PTY.
        daemon
            .write_terminal_input(terminal_id, b"echo hi\n")
            .expect("write input");
        assert_eq!(backend.written(), b"echo hi\n");
        backend.set_exited(terminal_core::ExitStatus::default());
    }

    #[test]
    fn an_active_session_cannot_be_closed_but_a_terminal_one_can() {
        let (daemon, tmp, backend) = test_daemon_with_pty(FakePtyBackend::empty());
        let workspace = seeded_workspace(&daemon, tmp.path());
        daemon
            .create_session(
                workspace,
                SessionKind::Shell,
                None,
                None,
                None,
                SessionRole::Generic,
                None,
                None,
                false,
            )
            .expect("create session");
        let session = only_session(&daemon);

        let err = daemon
            .close_session(session.id)
            .expect_err("a Running session must not be closed");
        assert_eq!(err.code, ErrorCode::PreconditionFailed);

        // The child exits: the single exit path marks the session Exited.
        let terminal_id = session.terminal_id.expect("terminal");
        daemon.on_terminal_exited(terminal_id);
        let exited = only_session(&daemon);
        assert!(exited.state.is_terminal(), "state is {:?}", exited.state);
        assert!(exited.terminal_id.is_none());
        assert!(exited.ended_at.is_some());

        daemon.close_session(session.id).expect("close after exit");
        assert!(daemon.lock().sessions.is_empty());
        backend.set_exited(terminal_core::ExitStatus::default());
    }

    #[test]
    fn a_closed_session_survives_a_context_envelope() {
        // Regression: `context_envelopes.source_session_id` had no referential
        // action, so a session that had produced an envelope could never be
        // deleted — `CloseSession` failed with a constraint error, permanently.
        let (daemon, tmp, _backend) = test_daemon_with_pty(FakePtyBackend::empty());
        let workspace = seeded_workspace(&daemon, tmp.path());
        let session_id = add_session_row(&daemon, workspace, None, SessionState::Orphaned);

        let envelope = domain::ContextEnvelope {
            id: domain::ContextId::new(),
            source_session_id: session_id,
            target_session_id: None,
            summary: Some("hand-off".to_string()),
            instructions: None,
            artifacts: vec![],
            git_context: None,
            created_at: Timestamp::now(),
        };
        daemon
            .lock()
            .db
            .context()
            .insert(&envelope)
            .expect("insert envelope");

        daemon
            .close_session(session_id)
            .expect("a session with an envelope must still be closeable");
        assert!(daemon.lock().sessions.is_empty());
    }

    #[test]
    fn send_context_pastes_into_a_live_target_and_lists_the_envelope() {
        let (daemon, tmp, backend) = test_daemon_with_pty(FakePtyBackend::empty());
        let workspace = seeded_workspace(&daemon, tmp.path());
        daemon
            .create_session(
                workspace,
                SessionKind::Shell,
                None,
                None,
                None,
                SessionRole::Generic,
                None,
                None,
                false,
            )
            .expect("source");
        let source = only_session(&daemon).id;
        daemon
            .create_session(
                workspace,
                SessionKind::Shell,
                None,
                None,
                None,
                SessionRole::Generic,
                None,
                None,
                false,
            )
            .expect("target");
        let target = daemon
            .lock()
            .sessions
            .values()
            .find(|s| s.id != source)
            .map(|s| s.id)
            .expect("target id");

        daemon
            .send_context(
                source,
                Some(target),
                None,
                Some("hand this over".into()),
                Some("do the next step".into()),
                false,
                None,
            )
            .expect("send");

        let bytes = backend.written();
        let written = String::from_utf8_lossy(&bytes);
        assert!(written.contains("--- forge context ---"));
        assert!(written.contains("Summary: hand this over"));
        assert!(written.contains("do the next step"));

        let listed = match daemon
            .handle_request(Request::ListContextEnvelopes { session_id: source })
            .expect("list")
        {
            Response::ContextEnvelopes(list) => list,
            other => panic!("expected ContextEnvelopes, got {other:?}"),
        };
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].target_session_id, Some(target));
        assert_eq!(listed[0].summary.as_deref(), Some("hand this over"));
    }

    // ---------------------------------------------------------------
    // Terminal request validation (§10.2)
    // ---------------------------------------------------------------

    #[test]
    fn fetch_scrollback_rejects_out_of_range_arguments() {
        let (daemon, tmp, backend) = test_daemon_with_pty(FakePtyBackend::empty());
        let workspace = seeded_workspace(&daemon, tmp.path());
        daemon
            .create_session(
                workspace,
                SessionKind::Shell,
                None,
                None,
                None,
                SessionRole::Generic,
                None,
                None,
                false,
            )
            .expect("create session");
        let terminal_id = only_session(&daemon).terminal_id.expect("terminal");

        // An unchecked `count` allocated one Row per index under the core lock:
        // u32::MAX rows hangs the daemon and then exhausts memory.
        let err = daemon
            .fetch_scrollback(terminal_id, 0, u32::MAX)
            .expect_err("an oversized count must be rejected");
        assert_eq!(err.code, ErrorCode::InvalidRequest);

        // An extreme `from_line` overflowed the index arithmetic — a panic under
        // the guard, which poisoned the core lock for every later request.
        let err = daemon
            .fetch_scrollback(terminal_id, i64::MIN, 1)
            .expect_err("a negative from_line must be rejected");
        assert_eq!(err.code, ErrorCode::InvalidRequest);

        // A request within the bounds is served.
        let response = daemon
            .fetch_scrollback(terminal_id, 0, 4)
            .expect("an in-range fetch succeeds");
        let Response::ScrollbackRows(rows) = response else {
            panic!("expected ScrollbackRows");
        };
        assert_eq!(rows.rows.len(), 4);
        assert_eq!(rows.from_line, 0);
        backend.set_exited(terminal_core::ExitStatus::default());
    }

    /// The capture is a fold, not a parse: the engine already consumed the
    /// escape sequences, so nothing that reaches the prompt can carry one.
    #[test]
    fn a_transcript_is_decoded_text_with_no_escape_sequences_left() {
        let (daemon, tmp, backend) = test_daemon_with_pty(FakePtyBackend::empty());
        let workspace = seeded_workspace(&daemon, tmp.path());
        daemon
            .create_session(
                workspace,
                SessionKind::Shell,
                None,
                None,
                None,
                SessionRole::Generic,
                None,
                None,
                false,
            )
            .expect("create session");
        let session = only_session(&daemon);
        let terminal_id = session.terminal_id.expect("terminal");

        // Colour, a cursor move and a title: everything a TUI agent paints with.
        daemon.pump_terminal(terminal_id, b"\x1b]0;title\x07", false);
        daemon.pump_terminal(terminal_id, b"\x1b[31mred\x1b[0m line\r\n", false);
        daemon.pump_terminal(terminal_id, b"second line\r\n", false);

        let Response::SessionTranscript(transcript) = daemon
            .get_session_transcript(session.id, None, None)
            .expect("transcript")
        else {
            panic!("expected SessionTranscript");
        };
        assert!(transcript.text.contains("red line"));
        assert!(transcript.text.contains("second line"));
        assert!(!transcript.text.contains('\x1b'));
        assert!(!transcript.text.contains("31m"));
        // Trailing blank rows are dropped rather than spending the budget.
        assert!(!transcript.text.ends_with('\n'));
        backend.set_exited(terminal_core::ExitStatus::default());
    }

    /// An unclamped wire `u32` asked the engine to materialise the whole
    /// scrollback under the core lock. The budget bounds what is built.
    #[test]
    fn a_transcript_clamps_its_budget_and_keeps_the_newest_lines() {
        let (daemon, tmp, backend) = test_daemon_with_pty(FakePtyBackend::empty());
        let workspace = seeded_workspace(&daemon, tmp.path());
        daemon
            .create_session(
                workspace,
                SessionKind::Shell,
                None,
                None,
                None,
                SessionRole::Generic,
                None,
                None,
                false,
            )
            .expect("create session");
        let session = only_session(&daemon);
        let terminal_id = session.terminal_id.expect("terminal");

        for n in 0..40 {
            daemon.pump_terminal(terminal_id, format!("line {n}\r\n").as_bytes(), false);
        }

        let Response::SessionTranscript(transcript) = daemon
            .get_session_transcript(session.id, Some(u32::MAX), Some(40))
            .expect("transcript")
        else {
            panic!("expected SessionTranscript");
        };
        assert!(
            transcript.truncated,
            "a 40-byte budget cannot hold 40 lines"
        );
        assert!(transcript.text.len() <= 40);
        // The tail is what matters: the newest line has to survive the cut.
        assert!(transcript.text.contains("line 39"));
        assert!(!transcript.text.contains("line 0\n"));
        backend.set_exited(terminal_core::ExitStatus::default());
    }

    /// A session records the commit it started from, and its own commits then
    /// count as its changes — which is the point of the baseline, and what a
    /// plain working-tree read cannot see.
    #[test]
    fn a_session_records_its_baseline_and_its_own_commits_count() {
        let (daemon, tmp, backend) = test_daemon_with_pty(FakePtyBackend::empty());
        // A subdirectory, so the attention assets beside the worktrees root are
        // not untracked files in this repo.
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).expect("create repo");
        let git = |args: &[&str]| {
            let ok = std::process::Command::new("git")
                .args(args)
                .current_dir(&repo)
                .status()
                .is_ok_and(|status| status.success());
            assert!(ok, "git {args:?}");
        };
        git(&["init"]);
        git(&["config", "user.email", "t@example.com"]);
        git(&["config", "user.name", "t"]);
        std::fs::write(repo.join("a.txt"), "one\n").expect("write");
        git(&["add", "a.txt"]);
        git(&["commit", "-m", "init"]);

        let workspace = seeded_workspace(&daemon, &repo);
        daemon
            .create_session(
                workspace,
                SessionKind::Shell,
                None,
                None,
                None,
                SessionRole::Generic,
                None,
                None,
                false,
            )
            .expect("create session");
        let session = only_session(&daemon);
        let base = session.base_commit.clone().expect("a baseline");

        // What this "session" did: one commit, and then one dirty file.
        std::fs::write(repo.join("a.txt"), "two\n").expect("write");
        git(&["commit", "-am", "change a"]);
        std::fs::write(repo.join("b.txt"), "new\n").expect("write");

        let Response::SessionChanges(changes) =
            daemon.get_session_changes(session.id).expect("changes")
        else {
            panic!("expected SessionChanges");
        };
        assert_eq!(changes.origin, domain::BaseOrigin::Recorded);
        assert_eq!(changes.summary.base.as_deref(), Some(&base[..7]));
        assert_eq!(changes.summary.commit_count, 1);
        assert_eq!(changes.sharing_sessions, 0);
        let mut paths: Vec<&str> = changes
            .summary
            .files
            .iter()
            .map(|file| file.path.as_str())
            .collect();
        paths.sort_unstable();
        // `a.txt` is clean on disk; only the baseline makes it visible.
        assert_eq!(paths, ["a.txt", "b.txt"]);

        let Response::WorkspaceReview(review) = daemon
            .get_workspace_review(workspace, None)
            .expect("review")
        else {
            panic!("expected WorkspaceReview");
        };
        assert_eq!(review.origin, domain::BaseOrigin::Recorded);
        assert_eq!(review.sessions.len(), 1);
        assert_eq!(review.sessions[0].commit_count, 1);
        assert_eq!(review.diff.files.len(), 2);
        backend.set_exited(terminal_core::ExitStatus::default());
    }

    /// A baseline git cannot resolve any more — a rebase, an amend — falls back
    /// to `HEAD` and says which base it used. A silent fallback would show a
    /// much smaller diff as if it were the whole session.
    #[test]
    fn an_unreachable_baseline_falls_back_to_head_and_says_so() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (base, origin) = resolve_base(dir.path(), Some("0".repeat(40).as_str()));
        assert_eq!(base, None);
        assert_eq!(origin, domain::BaseOrigin::Unreachable);

        let (base, origin) = resolve_base(dir.path(), None);
        assert_eq!(base, None);
        assert_eq!(origin, domain::BaseOrigin::Missing);
    }

    #[test]
    fn a_poisoned_core_lock_is_recovered_instead_of_killing_the_daemon() {
        let (daemon, _tmp) = test_daemon();

        // Poison the lock the way a panicking handler would.
        let poisoner = Arc::clone(&daemon);
        let _ = std::thread::spawn(move || {
            let _guard = poisoner.lock();
            panic!("handler panic");
        })
        .join();
        assert!(daemon.inner.is_poisoned());

        // Every later request used to panic here too, for every client.
        let Response::Snapshot { sessions, .. } = daemon.snapshot() else {
            panic!("expected a snapshot");
        };
        assert!(sessions.is_empty());
    }

    // ---------------------------------------------------------------
    // Graph validation (ADR-010, §21)
    // ---------------------------------------------------------------

    #[test]
    fn graph_placement_rejects_an_unknown_parent() {
        let (daemon, _tmp) = test_daemon();
        let project = add_project_row(&daemon, "a", None);
        let workspace = add_workspace_row(&daemon, project, false);

        let inner = daemon.lock();
        let err = Daemon::graph_placement(&inner, Some(SessionId::new()), workspace)
            .expect_err("an unknown parent must be rejected");
        assert_eq!(err.code, ErrorCode::NotFound);
    }

    #[test]
    fn graph_placement_rejects_a_parent_in_another_project() {
        let (daemon, _tmp) = test_daemon();
        let project_a = add_project_row(&daemon, "a", None);
        let project_b = add_project_row(&daemon, "b", None);
        let ws_a = add_workspace_row(&daemon, project_a, false);
        let ws_b = add_workspace_row(&daemon, project_b, false);
        let parent = add_session_row(&daemon, ws_a, None, SessionState::Running);

        let inner = daemon.lock();
        let err = Daemon::graph_placement(&inner, Some(parent), ws_b)
            .expect_err("crossing projects must be rejected");
        assert_eq!(err.code, ErrorCode::InvalidRequest);
    }

    #[test]
    fn graph_placement_allows_another_workspace_of_the_same_project() {
        let (daemon, _tmp) = test_daemon();
        let project = add_project_row(&daemon, "a", None);
        let ws_main = add_workspace_row(&daemon, project, false);
        let ws_worktree = add_workspace_row(&daemon, project, true);
        let parent = add_session_row(&daemon, ws_main, None, SessionState::Running);

        let inner = daemon.lock();
        let (parent_id, root_id) = Daemon::graph_placement(&inner, Some(parent), ws_worktree)
            .expect("a child may live in another workspace of the same project");
        assert_eq!(parent_id, Some(parent));
        assert_eq!(root_id, Some(parent));
    }

    #[test]
    fn graph_placement_rejects_a_chain_deeper_than_the_limit() {
        let (daemon, _tmp) = test_daemon();
        let project = add_project_row(&daemon, "a", None);
        let workspace = add_workspace_row(&daemon, project, false);

        // A chain of exactly MAX_GRAPH_DEPTH sessions: root is depth 1.
        let mut chain = Vec::new();
        let mut parent = None;
        for _ in 0..MAX_GRAPH_DEPTH {
            let id = add_session_row(&daemon, workspace, parent, SessionState::Running);
            chain.push(id);
            parent = Some(id);
        }

        let inner = daemon.lock();
        // A child of the second-to-last node still fits (depth 8).
        assert!(Daemon::graph_placement(&inner, Some(chain[chain.len() - 2]), workspace).is_ok());
        // A child of the deepest node would be depth 9.
        let err = Daemon::graph_placement(&inner, Some(chain[chain.len() - 1]), workspace)
            .expect_err("depth > MAX_GRAPH_DEPTH must be rejected");
        assert_eq!(err.code, ErrorCode::InvalidRequest);
    }

    /// §8.2: `NewManagedWorktree` "is implemented in the daemon (create
    /// worktree + session in a single operation)" — the UI may or may not
    /// expose it, the daemon must honour it. It used to be rejected outright.
    #[test]
    fn a_child_session_can_be_given_a_brand_new_managed_worktree() {
        let Ok(repo) = test_support::temp_repo::init_repo() else {
            return; // git is not installed: skip (§21).
        };
        let (daemon, _tmp) = test_daemon();
        daemon.add_project(repo.path()).expect("add a git project");
        let main_ws = {
            let inner = daemon.lock();
            inner
                .workspaces
                .values()
                .find(|w| w.kind == WorkspaceKind::Main)
                .expect("the Main workspace")
                .id
        };
        daemon
            .create_session(
                main_ws,
                SessionKind::Shell,
                None,
                None,
                None,
                SessionRole::Generic,
                None,
                None,
                false,
            )
            .expect("parent session");
        let parent = only_session(&daemon);

        daemon
            .create_child_session(
                parent.id,
                SessionKind::Shell,
                None,
                None,
                SessionRole::Executor,
                ChildWorkspacePolicy::NewManagedWorktree {
                    branch_hint: Some("feature/child".to_string()),
                    base: None,
                },
                None,
            )
            .expect("child session in a new managed worktree");

        let inner = daemon.lock();
        let child = inner
            .sessions
            .values()
            .find(|s| s.id != parent.id)
            .expect("the child session")
            .clone();
        assert_eq!(child.parent_session_id, Some(parent.id));
        assert_eq!(child.root_session_id, parent.id, "lineage kept (ADR-010)");
        assert_ne!(
            child.workspace_id, main_ws,
            "the child runs in the new worktree, not the parent's workspace"
        );

        let ws = &inner.workspaces[&child.workspace_id];
        assert_eq!(ws.kind, WorkspaceKind::GitWorktree);
        assert!(ws.managed_by_app, "created by Forge (§14.3)");
        assert_eq!(ws.branch.as_deref(), Some("feature/child"));
        assert!(ws.path.join(".git").exists(), "the worktree is on disk");
        assert_eq!(
            inner.workspaces[&main_ws].project_id, ws.project_id,
            "a child may change workspace but never project (ADR-010)"
        );
    }

    /// With no `branch_hint` the daemon still has to produce a valid, unique
    /// branch rather than fail (§8.2).
    #[test]
    fn a_new_managed_worktree_child_without_a_branch_hint_gets_a_generated_one() {
        let Ok(repo) = test_support::temp_repo::init_repo() else {
            return; // git is not installed: skip (§21).
        };
        let (daemon, _tmp) = test_daemon();
        daemon.add_project(repo.path()).expect("add a git project");
        let main_ws = {
            let inner = daemon.lock();
            inner
                .workspaces
                .values()
                .find(|w| w.kind == WorkspaceKind::Main)
                .expect("the Main workspace")
                .id
        };
        daemon
            .create_session(
                main_ws,
                SessionKind::Shell,
                None,
                None,
                None,
                SessionRole::Generic,
                None,
                None,
                false,
            )
            .expect("parent session");
        let parent = only_session(&daemon);

        for _ in 0..2 {
            daemon
                .create_child_session(
                    parent.id,
                    SessionKind::Shell,
                    None,
                    None,
                    SessionRole::Executor,
                    ChildWorkspacePolicy::NewManagedWorktree {
                        branch_hint: None,
                        base: None,
                    },
                    None,
                )
                .expect("child session with a generated branch");
        }

        let inner = daemon.lock();
        let worktrees: Vec<&Workspace> = inner
            .workspaces
            .values()
            .filter(|w| w.kind == WorkspaceKind::GitWorktree)
            .collect();
        assert_eq!(worktrees.len(), 2, "two distinct worktrees were created");
        assert_ne!(
            worktrees[0].branch, worktrees[1].branch,
            "generated branches must not collide"
        );
        for ws in worktrees {
            assert!(ws.path.join(".git").exists());
        }
    }

    // ---------------------------------------------------------------
    // Re-parenting on close (ADR-010, §21)
    // ---------------------------------------------------------------

    #[test]
    fn closing_a_root_reparents_children_and_fixes_the_whole_subtree() {
        let (daemon, _tmp) = test_daemon();
        let project = add_project_row(&daemon, "a", None);
        let workspace = add_workspace_row(&daemon, project, false);
        let root = add_session_row(&daemon, workspace, None, SessionState::Orphaned);
        let child_a = add_session_row(&daemon, workspace, Some(root), SessionState::Orphaned);
        let child_b = add_session_row(&daemon, workspace, Some(root), SessionState::Orphaned);
        let grandchild = add_session_row(&daemon, workspace, Some(child_a), SessionState::Orphaned);

        daemon.close_session(root).expect("close the root");

        let inner = daemon.lock();
        assert!(!inner.sessions.contains_key(&root));

        // Both children become roots (there is no grandparent).
        for child in [child_a, child_b] {
            let s = &inner.sessions[&child];
            assert_eq!(s.parent_session_id, None, "child re-parented to no one");
            assert_eq!(s.root_session_id, child, "child becomes its own root");
        }

        // Regression: the grandchild used to keep the *deleted* root's id, both
        // in memory and in SQLite (a dangling reference).
        let g = &inner.sessions[&grandchild];
        assert_eq!(g.parent_session_id, Some(child_a));
        assert_eq!(g.root_session_id, child_a);
        let persisted = inner
            .db
            .sessions()
            .get(grandchild)
            .expect("query")
            .expect("the grandchild row");
        assert_eq!(persisted.root_session_id, child_a);
    }

    #[test]
    fn closing_a_middle_session_reparents_to_the_grandparent() {
        let (daemon, _tmp) = test_daemon();
        let project = add_project_row(&daemon, "a", None);
        let workspace = add_workspace_row(&daemon, project, false);
        let root = add_session_row(&daemon, workspace, None, SessionState::Orphaned);
        let middle = add_session_row(&daemon, workspace, Some(root), SessionState::Orphaned);
        let child = add_session_row(&daemon, workspace, Some(middle), SessionState::Orphaned);
        let grandchild = add_session_row(&daemon, workspace, Some(child), SessionState::Orphaned);

        daemon.close_session(middle).expect("close the middle node");

        let inner = daemon.lock();
        assert_eq!(inner.sessions[&child].parent_session_id, Some(root));
        assert_eq!(inner.sessions[&child].root_session_id, root);
        assert_eq!(inner.sessions[&grandchild].root_session_id, root);
    }

    #[test]
    fn closing_an_active_session_is_rejected() {
        let (daemon, _tmp) = test_daemon();
        let project = add_project_row(&daemon, "a", None);
        let workspace = add_workspace_row(&daemon, project, false);
        let session = add_session_row(&daemon, workspace, None, SessionState::Running);

        let err = daemon
            .close_session(session)
            .expect_err("Running sessions must be killed first (§7.3)");
        assert_eq!(err.code, ErrorCode::PreconditionFailed);
        assert!(daemon.lock().sessions.contains_key(&session));
    }

    // ---------------------------------------------------------------
    // State machine (§7.3, §21)
    // ---------------------------------------------------------------

    #[test]
    fn invalid_state_transitions_are_rejected_and_change_nothing() {
        let (daemon, _tmp) = test_daemon();
        let project = add_project_row(&daemon, "a", None);
        let workspace = add_workspace_row(&daemon, project, false);
        let failed = SessionState::Failed {
            reason: "binary missing".to_string(),
        };
        let session = add_session_row(&daemon, workspace, None, failed.clone());

        let mut inner = daemon.lock();
        // The real race: the PTY loop reports EOF for a session whose spawn
        // already failed. It must stay `Failed`, not become `Exited`.
        assert!(Daemon::set_session_state(
            &mut inner,
            session,
            SessionState::Exited {
                code: Some(0),
                signal: None,
            },
            None,
        )
        .is_none());
        assert_eq!(inner.sessions[&session].state, failed);
        let persisted = inner
            .db
            .sessions()
            .get(session)
            .expect("query")
            .expect("the session row");
        // §15.2 persists the discriminant, not the `Failed` reason.
        assert!(matches!(persisted.state, SessionState::Failed { .. }));

        // Running is not reachable from Failed either; only a restart is.
        assert!(
            Daemon::set_session_state(&mut inner, session, SessionState::Running, None).is_none()
        );
        let restarted =
            Daemon::set_session_state(&mut inner, session, SessionState::Starting, None)
                .expect("Failed → Starting is the RestartSession transition");
        assert_eq!(restarted.state, SessionState::Starting);
        assert!(restarted.ended_at.is_none());
    }

    #[test]
    fn a_valid_transition_persists_state_and_ended_at() {
        let (daemon, _tmp) = test_daemon();
        let project = add_project_row(&daemon, "a", None);
        let workspace = add_workspace_row(&daemon, project, false);
        let session = add_session_row(&daemon, workspace, None, SessionState::Starting);
        let terminal_id = TerminalId::new();

        let mut inner = daemon.lock();
        let running = Daemon::set_session_state(
            &mut inner,
            session,
            SessionState::Running,
            Some(terminal_id),
        )
        .expect("Starting → Running");
        assert_eq!(running.terminal_id, Some(terminal_id));
        assert!(running.ended_at.is_none());

        let exited = Daemon::set_session_state(
            &mut inner,
            session,
            SessionState::Exited {
                code: Some(3),
                signal: None,
            },
            None,
        )
        .expect("Running → Exited");
        assert!(exited.terminal_id.is_none());
        assert!(exited.ended_at.is_some(), "terminal states stamp ended_at");
        let persisted = inner
            .db
            .sessions()
            .get(session)
            .expect("query")
            .expect("the session row");
        assert_eq!(
            persisted.state,
            SessionState::Exited {
                code: Some(3),
                signal: None
            }
        );
    }

    // ---------------------------------------------------------------
    // RemoveProjectPolicy (§10.2)
    // ---------------------------------------------------------------

    #[test]
    fn keep_everything_is_rejected_while_a_session_is_active() {
        let (daemon, _tmp) = test_daemon();
        let project = add_project_row(&daemon, "a", None);
        let workspace = add_workspace_row(&daemon, project, false);
        let _session = add_session_row(&daemon, workspace, None, SessionState::Running);

        let err = daemon
            .remove_project(project, RemoveProjectPolicy::KeepEverything)
            .expect_err("KeepEverything must refuse running sessions");
        assert_eq!(err.code, ErrorCode::PreconditionFailed);
        assert!(daemon.lock().projects.contains_key(&project));
    }

    #[test]
    fn keep_everything_removes_the_project_when_nothing_is_active() {
        let (daemon, _tmp) = test_daemon();
        let project = add_project_row(&daemon, "a", None);
        let workspace = add_workspace_row(&daemon, project, false);
        let session = add_session_row(&daemon, workspace, None, SessionState::Orphaned);

        daemon
            .remove_project(project, RemoveProjectPolicy::KeepEverything)
            .expect("no active session, nothing to refuse");

        let inner = daemon.lock();
        assert!(inner.projects.is_empty());
        assert!(inner.workspaces.is_empty());
        assert!(inner.sessions.is_empty());
        assert!(inner.db.sessions().get(session).expect("query").is_none());
    }

    #[test]
    fn kill_sessions_policies_remove_the_project_with_live_sessions() {
        // The sessions have no terminal, so `kill_session` is a no-op and the
        // policy branch is exercised without spawning a PTY.
        for policy in [
            RemoveProjectPolicy::KillSessionsKeepWorktrees,
            RemoveProjectPolicy::KillSessionsRemoveManagedWorktrees,
        ] {
            let (daemon, _tmp) = test_daemon();
            let project = add_project_row(&daemon, "a", None);
            let workspace = add_workspace_row(&daemon, project, true);
            let _session = add_session_row(&daemon, workspace, None, SessionState::Running);

            daemon
                .remove_project(project, policy)
                .expect("kill policies do not refuse running sessions");

            let inner = daemon.lock();
            assert!(inner.projects.is_empty(), "{policy:?} removed the project");
            assert!(inner.sessions.is_empty());
        }
    }

    // ---------------------------------------------------------------
    // git status throttle (ADR-008)
    // ---------------------------------------------------------------

    #[test]
    fn git_status_runs_at_most_once_every_two_seconds_per_workspace() {
        let (daemon, _tmp) = test_daemon();
        let project = add_project_row(&daemon, "a", None);
        let ws_a = add_workspace_row(&daemon, project, false);
        let ws_b = add_workspace_row(&daemon, project, false);
        let t0 = Instant::now();

        let mut inner = daemon.lock();
        assert!(Daemon::status_check_due(&mut inner, ws_a, t0), "first run");
        assert!(
            !Daemon::status_check_due(&mut inner, ws_a, t0 + Duration::from_millis(1_999)),
            "inside the window the cached status is served"
        );
        // The budget is per workspace, not global.
        assert!(Daemon::status_check_due(&mut inner, ws_b, t0));
        assert!(
            Daemon::status_check_due(&mut inner, ws_a, t0 + GIT_STATUS_THROTTLE),
            "the window has elapsed"
        );
    }

    // ---------------------------------------------------------------
    // Notices (§7.1, §15.3, §23)
    // ---------------------------------------------------------------

    #[test]
    fn a_project_group_contains_projects_and_appears_in_the_snapshot() {
        let (daemon, tmp) = test_daemon();
        let project_root = tmp.path().join("product-x-api");
        std::fs::create_dir_all(&project_root).expect("mkdir");

        daemon
            .create_project_group("  Product X  ")
            .expect("create project group");
        let group_id = daemon
            .lock()
            .project_groups
            .values()
            .next()
            .expect("created group")
            .id;
        daemon
            .add_project_to_group(&project_root, group_id)
            .expect("add grouped project");

        let Response::Snapshot {
            project_groups,
            projects,
            ..
        } = daemon.snapshot()
        else {
            panic!("expected snapshot");
        };
        assert_eq!(project_groups.len(), 1);
        assert_eq!(project_groups[0].name, "Product X");
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].project_group_id, Some(group_id));

        let inner = daemon.lock();
        assert_eq!(
            inner
                .db
                .projects()
                .get(projects[0].id)
                .expect("read project")
                .expect("persisted project")
                .project_group_id,
            Some(group_id)
        );
    }

    #[test]
    fn project_group_metadata_changes_never_require_the_project_folder() {
        let (daemon, _tmp) = test_daemon();
        daemon
            .create_project_group("Product X")
            .expect("create first group");
        daemon
            .create_project_group("Platform")
            .expect("create second group");
        let (product_x, platform) = {
            let inner = daemon.lock();
            let id = |name: &str| {
                inner
                    .project_groups
                    .values()
                    .find(|group| group.name == name)
                    .expect("group")
                    .id
            };
            (id("Product X"), id("Platform"))
        };
        let project = add_project_row(&daemon, "missing-project", None);
        let missing_path = daemon.lock().projects[&project].root_path.clone();
        assert!(!missing_path.exists());
        let workspace = add_workspace_row(&daemon, project, false);
        let session = add_session_row(&daemon, workspace, None, SessionState::Orphaned);

        daemon
            .move_project(project, Some(product_x))
            .expect("metadata move works without a directory");
        daemon
            .rename_project_group(product_x, "Product X Local")
            .expect("metadata rename works without a directory");
        daemon
            .move_project(project, Some(platform))
            .expect("move between groups");
        daemon
            .remove_project_group(platform)
            .expect("delete only the organizational group");

        let inner = daemon.lock();
        assert!(!missing_path.exists(), "no directory was created or moved");
        assert_eq!(inner.projects[&project].project_group_id, None);
        assert!(inner.workspaces.contains_key(&workspace));
        assert!(inner.sessions.contains_key(&session));
        assert_eq!(inner.project_groups[&product_x].name, "Product X Local");
        assert!(!inner.project_groups.contains_key(&platform));
        assert_eq!(
            inner
                .db
                .projects()
                .get(project)
                .expect("read project")
                .expect("project remains")
                .project_group_id,
            None
        );
    }

    /// The icon is metadata like the name: it survives in the row, reaches
    /// every client as an update, and an empty box asks for the initials back.
    #[test]
    fn a_project_icon_is_stored_and_cleared() {
        let (daemon, _tmp) = test_daemon();
        let project = add_project_row(&daemon, "iconed", None);

        daemon
            .set_project_icon(project, Some(" 🦀 "))
            .expect("set an icon");
        assert_eq!(
            daemon.lock().projects[&project].icon.as_deref(),
            Some("🦀"),
            "the stored icon is trimmed, not the raw keystrokes"
        );
        assert_eq!(
            daemon
                .lock()
                .db
                .projects()
                .get(project)
                .expect("read project")
                .expect("project row")
                .icon
                .as_deref(),
            Some("🦀"),
            "an icon that only lives in memory is lost on restart"
        );

        daemon
            .set_project_icon(project, Some("   "))
            .expect("an emptied box clears the icon");
        assert_eq!(daemon.lock().projects[&project].icon, None);

        daemon
            .set_project_icon(project, Some("🦀"))
            .expect("set it again");
        daemon.set_project_icon(project, None).expect("clear it");
        assert_eq!(daemon.lock().projects[&project].icon, None);
    }

    /// The daemon does not trust the client: a label is not a mark, and
    /// refusing is not the same as clearing.
    #[test]
    fn a_project_icon_that_is_not_a_mark_is_refused() {
        let (daemon, _tmp) = test_daemon();
        let project = add_project_row(&daemon, "iconed", None);
        daemon.set_project_icon(project, Some("🦀")).expect("set");

        let error = daemon
            .set_project_icon(project, Some("HP Backend"))
            .expect_err("a name is not an icon");
        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert_eq!(
            daemon.lock().projects[&project].icon.as_deref(),
            Some("🦀"),
            "a refused pick must not clear the icon that was there"
        );
    }

    #[test]
    fn setting_an_icon_on_an_unknown_project_is_rejected() {
        let (daemon, _tmp) = test_daemon();
        let error = daemon
            .set_project_icon(ProjectId::new(), Some("🦀"))
            .expect_err("unknown project must be rejected");
        assert_eq!(error.code, ErrorCode::NotFound);
    }

    #[test]
    fn adding_a_project_to_an_unknown_group_is_rejected() {
        let (daemon, tmp) = test_daemon();
        let project_root = tmp.path().join("orphan");
        std::fs::create_dir_all(&project_root).expect("mkdir");

        let error = daemon
            .add_project_to_group(&project_root, ProjectGroupId::new())
            .expect_err("unknown group must be rejected");
        assert_eq!(error.code, ErrorCode::NotFound);
        assert!(daemon.lock().projects.is_empty());
    }

    fn warning_containing(notices: &[DaemonEvent], needle: &str) -> bool {
        notices.iter().any(|e| {
            matches!(
                e,
                DaemonEvent::DaemonNotice {
                    level: NoticeLevel::Warning,
                    message,
                } if message.contains(needle)
            )
        })
    }

    #[test]
    fn a_nested_project_is_accepted_with_a_warning() {
        let (daemon, tmp) = test_daemon();
        let outer = tmp.path().join("outer");
        let nested = outer.join("nested");
        std::fs::create_dir_all(&nested).expect("mkdir");

        daemon.add_project(&outer).expect("outer project added");
        let _ = daemon.take_pending_notices();

        daemon
            .add_project(&nested)
            .expect("§7.1: nesting is allowed, only warned about");
        let notices = daemon.take_pending_notices();
        assert!(warning_containing(&notices, "nested"), "{notices:?}");
        assert_eq!(daemon.lock().projects.len(), 2);
    }

    #[test]
    fn a_project_outside_the_home_is_accepted_with_a_warning() {
        let (daemon, tmp) = test_daemon();
        let root = tmp.path().join("outside");
        std::fs::create_dir_all(&root).expect("mkdir");
        let canonical = std::fs::canonicalize(&root).expect("canonicalize");
        // Only meaningful when the temp dir really sits outside $HOME.
        if home_dir().is_some_and(|home| canonical.starts_with(home)) {
            return;
        }

        daemon
            .add_project(&root)
            .expect("§23: the path is accepted, not rejected");
        let notices = daemon.take_pending_notices();
        assert!(warning_containing(&notices, "outside"), "{notices:?}");
    }

    /// Step 4 removes a vanished worktree, so step 3 must not warn about it
    /// first: the user got a "keeping it in the model" toast per worktree they
    /// had already deleted, for rows that then disappeared anyway.
    #[test]
    fn startup_does_not_warn_about_a_vanished_worktree() {
        let (daemon, _tmp) = test_daemon();
        let project = add_project_row(&daemon, "gone", None);
        let id = WorkspaceId::new();
        {
            let mut inner = daemon.lock();
            let ws = Workspace {
                id,
                project_id: project,
                kind: WorkspaceKind::GitWorktree,
                path: PathBuf::from(format!("/nonexistent/forge/wt/{id}")),
                branch: Some("gone/by-hand".into()),
                display_name: None,
                managed_by_app: true,
                created_at: Timestamp::now(),
                status: WorkspaceStatus::default(),
            };
            inner
                .db
                .workspaces()
                .upsert(&ws)
                .expect("persist workspace");
            inner.workspaces.insert(id, ws);
        }

        daemon.validate_known_paths();

        let notices = daemon.take_pending_notices();
        assert_eq!(
            notices.len(),
            1,
            "only the project, never the worktree: {notices:?}"
        );
    }

    #[test]
    fn startup_warns_about_missing_paths_but_keeps_them() {
        let (daemon, _tmp) = test_daemon();
        let project = add_project_row(&daemon, "gone", None);
        let workspace = add_workspace_row(&daemon, project, false);

        daemon.validate_known_paths();

        let notices = daemon.take_pending_notices();
        assert!(warning_containing(&notices, "gone"), "{notices:?}");
        assert_eq!(notices.len(), 2, "one per missing project and workspace");
        // §15.3 step 3 never deletes.
        let inner = daemon.lock();
        assert!(inner.projects.contains_key(&project));
        assert!(inner.workspaces.contains_key(&workspace));
    }

    #[test]
    fn notices_are_broadcast_once_a_client_is_connected() {
        let (daemon, _tmp) = test_daemon();
        let client_id = domain::ClientId::new();
        let rx = daemon.registry().register(client_id);

        daemon.notice(NoticeLevel::Warning, "live warning");

        assert!(
            daemon.take_pending_notices().is_empty(),
            "with a client attached the notice is not queued"
        );
        assert_eq!(rx.len(), 1);
    }

    // ---------------------------------------------------------------
    // Worktree rescan (§15.3 step 4)
    // ---------------------------------------------------------------

    #[test]
    fn rescan_picks_up_worktrees_added_and_removed_outside_the_app() {
        let Ok(repo) = test_support::temp_repo::init_repo() else {
            return; // git is not installed: skip (§21).
        };
        let (daemon, tmp) = test_daemon();
        daemon.add_project(repo.path()).expect("add a git project");
        let _ = daemon.take_pending_notices();

        let (project, git_root) = {
            let inner = daemon.lock();
            let p = inner.projects.values().next().expect("the project");
            assert_eq!(inner.workspaces.len(), 1, "only Main so far");
            (p.id, p.git_root.clone().expect("a git project"))
        };

        // Someone adds a worktree from a plain terminal.
        let external = tmp.path().join("external");
        git_service::create(&git_root, &external, "feature/x", None).expect("git worktree add");

        daemon.rescan_worktrees();
        {
            let inner = daemon.lock();
            assert_eq!(inner.workspaces.len(), 2);
            let ws = inner
                .workspaces
                .values()
                .find(|w| w.kind == WorkspaceKind::GitWorktree)
                .expect("the rescanned worktree");
            assert!(!ws.managed_by_app, "found on disk, not created by Forge");
            assert_eq!(ws.branch.as_deref(), Some("feature/x"));
            assert_eq!(
                inner
                    .db
                    .workspaces()
                    .list_by_project(project)
                    .expect("query")
                    .len(),
                2,
                "the new workspace is persisted"
            );
        }

        // ...and removes it the same way.
        git_service::remove(&git_root, &external, true).expect("git worktree remove");

        daemon.rescan_worktrees();
        let inner = daemon.lock();
        assert_eq!(
            inner.workspaces.len(),
            1,
            "the vanished worktree is dropped"
        );
        assert_eq!(
            inner
                .db
                .workspaces()
                .list_by_project(project)
                .expect("query")
                .len(),
            1
        );
    }

    #[test]
    fn factory_reset_clears_the_model_database_and_client_replica() {
        let (daemon, _tmp) = test_daemon();
        let project = add_project_row(&daemon, "reset", None);
        let workspace = add_workspace_row(&daemon, project, false);
        add_session_row(&daemon, workspace, None, SessionState::Orphaned);
        daemon
            .create_project_group("Reset group")
            .expect("create group");
        daemon
            .set_app_state("ui.theme_base", "light")
            .expect("set app state");
        let client = ClientId::new();
        let events = daemon.registry.register(client);

        assert_eq!(
            daemon
                .handle_request(Request::FactoryReset)
                .expect("factory reset"),
            Response::Ack
        );

        let Response::Snapshot {
            project_groups,
            projects,
            workspaces,
            sessions,
            agent_profiles,
            app_state,
            jobs,
            ..
        } = daemon.snapshot()
        else {
            panic!("expected snapshot");
        };
        assert!(project_groups.is_empty());
        assert!(projects.is_empty());
        assert!(workspaces.is_empty());
        assert!(sessions.is_empty());
        assert!(agent_profiles.is_empty());
        assert!(app_state.is_empty());
        assert!(jobs.is_empty());
        assert!(matches!(
            events.try_recv(),
            Ok(DaemonMessage::Event(DaemonEvent::FactoryReset))
        ));

        let inner = daemon.lock();
        assert!(inner.db.projects().list().expect("projects").is_empty());
        assert!(inner.db.project_groups().list().expect("groups").is_empty());
        assert!(inner.db.app_state().list().expect("app state").is_empty());
    }

    // ---------------------------------------------------------------
    // Emitted-delta sequencing (§10.5)
    // ---------------------------------------------------------------

    /// The wire sequence a client checks for gaps counts **emitted deltas**, not
    /// engine feeds. Bytes that arrive while nobody is attached still reach the
    /// grid, but they must not burn sequence numbers — the first watcher would
    /// then see its very first delta already past a hole and ask for a resync.
    #[test]
    fn emit_seq_counts_emitted_deltas_not_engine_feeds() {
        let (daemon, tmp, _backend) = test_daemon_with_pty(FakePtyBackend::empty());
        let workspace = seeded_workspace(&daemon, tmp.path());
        daemon
            .create_session(
                workspace,
                SessionKind::Shell,
                None,
                None,
                None,
                SessionRole::Generic,
                None,
                None,
                false,
            )
            .expect("create shell session");
        let terminal_id = only_session(&daemon)
            .terminal_id
            .expect("a running session has a terminal");

        let state = |d: &Daemon| {
            let inner = d.lock();
            let rt = inner.terminals.get(&terminal_id).expect("live terminal");
            (rt.emit_seq, rt.engine.seq())
        };

        // Nobody attached: the bytes land in the grid and nothing is emitted.
        for _ in 0..3 {
            assert!(
                !daemon.pump_terminal(terminal_id, b"unwatched\r\n", false),
                "no subscriber should be reported"
            );
        }
        let (emitted, fed) = state(&daemon);
        assert_eq!(emitted, 0, "an unwatched chunk emits nothing");
        assert_eq!(fed, 3, "but it is still fed to the authoritative engine");

        let client = ClientId::new();
        let rx = daemon.registry.register(client);
        daemon.registry.subscribe(client, terminal_id);

        for _ in 0..3 {
            assert!(daemon.pump_terminal(terminal_id, b"watched\r\n", false));
        }

        let seqs: Vec<u64> = std::iter::from_fn(|| rx.try_recv().ok())
            .filter_map(|msg| match msg {
                DaemonMessage::Event(DaemonEvent::TerminalDelta { delta, .. }) => Some(delta.seq),
                _ => None,
            })
            .collect();
        assert_eq!(
            seqs,
            vec![1, 2, 3],
            "the first delta a client sees is seq 1, and they are consecutive"
        );

        let (emitted, fed) = state(&daemon);
        assert_eq!(emitted, 3);
        assert_eq!(
            fed, 6,
            "the engine seq counts every feed, the wire seq does not"
        );

        // DEC 2026 asks the terminal to hold a frame until ESU. The opening
        // mode change itself may publish, but the buffered body must neither
        // wake the client nor consume wire sequence numbers.
        daemon.pump_terminal(terminal_id, b"\x1b[?2026h", false);
        let opening = rx.try_recv().expect("the BSU mode change publishes");
        assert!(matches!(
            opening,
            DaemonMessage::Event(DaemonEvent::TerminalDelta { delta, .. }) if delta.seq == 4
        ));
        daemon.pump_terminal(terminal_id, b"one ", false);
        daemon.pump_terminal(terminal_id, b"frame", false);
        assert!(
            rx.try_recv().is_err(),
            "synchronized body chunks must stay invisible"
        );
        assert_eq!(state(&daemon), (4, 9));

        daemon.pump_terminal(terminal_id, b"\x1b[?2026l", false);
        let closing = rx.try_recv().expect("ESU publishes the complete frame");
        assert!(matches!(
            closing,
            DaemonMessage::Event(DaemonEvent::TerminalDelta { delta, .. }) if delta.seq == 5
        ));
        assert_eq!(state(&daemon), (5, 10));
    }
}
