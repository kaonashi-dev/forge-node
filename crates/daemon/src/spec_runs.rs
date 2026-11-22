//! Spec runs: the stages, gates and rounds of one spec, on their own lock.
//!
//! Agents are ordinary sessions and the coder is a ledger attempt. Files live under
//! `<forge home>/spec-runs/<id>/`; the row keeps metadata only. Lock order is `runs` before
//! `inner`, and no disk, git or session work happens while `runs` is held.

use std::collections::{BTreeSet, HashMap};
use std::io::{Read as _, Seek as _, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use domain::spec_run::model::{
    AgentPick, CheckRecord, Delivery, GateDecision, GateKind, NextAction, Placement, Route,
    SourceRef, SpecArtifactKind, SpecOptions, SpecRole, SpecRunDetail, SpecRunSource, SpecRunView,
    SpecSession, SpecStage, MAX_ROUNDS, MIN_ROUNDS,
};
use domain::spec_run::schema::{self, PlanDoc, PlannedCheck, SpecDoc, GATE};
use domain::spec_run::stages::{self, CoderInput, ReviewInput, StageContext};
use domain::spec_run::{compose_drafter_prompt, source_section, DrafterInput, SpecSource, Tools};
use domain::{
    ActionKind, ActionReady, ActionSource, ProjectAction, ProjectActionId, ProjectId, SessionId,
    SessionKind, SessionRole, SessionState, SpecRunId, Timestamp, WorkspaceId,
};
use persistence::SpecRunStore;
use protocol::{DaemonEvent, ErrorCode, PromptDelivery, ProtocolError, Request, Response};
use sha2::{Digest, Sha256};

use crate::core::Daemon;

const DIR: &str = "spec-runs";
const MAX_NAME_CHARS: usize = 80;
/// A gate's reason reaches the agent verbatim, inside one 8 KiB prompt.
const MAX_REASON_BYTES: usize = 4096;
const MAX_FINDINGS_BYTES: usize = 32 * 1024;
const MAX_DIRTY_PATHS: usize = 500;
/// The report as saved, with room for the deviations Forge appends to the validated text.
const REPORT_FILE_BYTES: usize = schema::MAX_REPORT_BYTES + 16 * 1024;
const DEFAULT_CHECK_TIMEOUT: Duration = Duration::from_secs(600);
const MAX_CHECK_TIMEOUT_SECS: u64 = 3600;
/// Past the kill grace, how long a killed check may take to be reaped.
const REAP_SLACK: Duration = Duration::from_secs(5);
/// The shell's own code for a command it could not find.
const COMMAND_NOT_FOUND: i32 = 127;

pub(crate) struct SpecRuns {
    runs: Mutex<HashMap<SpecRunId, SpecRunView>>,
    store: SpecRunStore,
}

impl SpecRuns {
    /// A run past its plan gate lost its sessions with the daemon; it says so.
    pub(crate) fn load(store: SpecRunStore) -> Result<Self, String> {
        let mut runs = HashMap::new();
        for mut run in store.list().map_err(|e| e.to_string())? {
            if !run.stage.is_closed() && !run.stage.is_gate() {
                run.attention = Some(
                    "the daemon restarted while this run's agents were working; their sessions \
                     ended"
                        .to_owned(),
                );
            }
            runs.insert(run.id, run);
        }
        Ok(Self {
            runs: Mutex::new(runs),
            store,
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<SpecRunId, SpecRunView>> {
        self.runs.lock().unwrap_or_else(PoisonError::into_inner)
    }

    #[must_use]
    pub(crate) fn list(&self) -> Vec<SpecRunView> {
        let mut runs: Vec<SpecRunView> = self.lock().values().cloned().collect();
        runs.sort_by_key(|run| run.created_at);
        runs
    }

    fn get(&self, id: SpecRunId) -> Result<SpecRunView, ProtocolError> {
        self.lock()
            .get(&id)
            .cloned()
            .ok_or_else(|| ProtocolError::not_found("spec run"))
    }

    /// The rows went with `Db::reset`; the files go with them.
    pub(crate) fn clear(&self, home: &Path) {
        self.lock().clear();
        let _ = std::fs::remove_dir_all(home.join(DIR));
    }

    /// The rows cascade with the project in SQLite; the map and the files are this owner's.
    pub(crate) fn forget_project(&self, project_id: ProjectId, home: &Path) {
        let gone: Vec<SpecRunId> = {
            let mut runs = self.lock();
            let gone = runs
                .values()
                .filter(|run| run.project_id == project_id)
                .map(|run| run.id)
                .collect::<Vec<_>>();
            for id in &gone {
                runs.remove(id);
            }
            gone
        };
        for id in gone {
            let _ = std::fs::remove_dir_all(home.join(DIR).join(id.to_string()));
        }
    }

    fn of_session(&self, session: SessionId) -> Option<SpecRunView> {
        self.lock()
            .values()
            .filter(|run| run.owns_session(session))
            .max_by_key(|run| run.created_at)
            .cloned()
    }
}

/// Change a run, persist and broadcast it under the runs lock (never the core lock), so two
/// changes reach SQLite and the clients in the order they were made.
fn update<T>(
    daemon: &Daemon,
    id: SpecRunId,
    change: impl FnOnce(&mut SpecRunView) -> Result<T, ProtocolError>,
) -> Result<T, ProtocolError> {
    let mut runs = daemon.spec_runs.lock();
    let run = runs
        .get_mut(&id)
        .ok_or_else(|| ProtocolError::not_found("spec run"))?;
    let value = change(run)?;
    run.updated_at = Timestamp::now();
    publish(daemon, run.clone());
    Ok(value)
}

fn publish(daemon: &Daemon, run: SpecRunView) {
    if let Err(error) = daemon.spec_runs.store.upsert(&run) {
        tracing::warn!(spec_run = %run.id, %error, "could not persist a spec run");
    }
    daemon
        .registry
        .broadcast_domain(DaemonEvent::SpecRunUpdated(Box::new(run)));
}

fn attention(daemon: &Daemon, id: SpecRunId, why: String) {
    let _ = update(daemon, id, |run| {
        run.attention = Some(why);
        Ok(())
    });
}

fn dir_of(daemon: &Daemon, id: SpecRunId) -> PathBuf {
    daemon.forge_paths().home.join(DIR).join(id.to_string())
}

fn io_err(what: &str, error: std::io::Error) -> ProtocolError {
    ProtocolError::new(ErrorCode::IoError, format!("{what}: {error}"))
}

fn ensure_dir(path: &Path) -> Result<(), ProtocolError> {
    use std::os::unix::fs::DirBuilderExt as _;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)
        .map_err(|e| io_err("spec run directory", e))
}

fn write_file(dir: &Path, name: &str, content: &str) -> Result<(), ProtocolError> {
    let path = dir.join(name);
    if let Some(parent) = path.parent() {
        ensure_dir(parent)?;
    }
    std::fs::write(&path, content).map_err(|e| io_err(name, e))
}

/// A run file, capped before it is read.
fn read_file(dir: &Path, name: &str, cap: usize) -> Option<String> {
    let path = dir.join(name);
    let len = std::fs::metadata(&path).ok()?.len();
    if len > cap as u64 {
        return None;
    }
    let mut text = String::new();
    std::fs::File::open(&path)
        .ok()?
        .take(cap as u64)
        .read_to_string(&mut text)
        .ok()?;
    Some(text)
}

fn sha256(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn invalid(what: &str, errors: Vec<String>) -> ProtocolError {
    let details = serde_json::json!({ "errors": errors }).to_string();
    ProtocolError::with_details(
        ErrorCode::InvalidRequest,
        format!("{what}: {}", errors.join("; ")),
        details,
    )
}

fn rounds_cap(daemon: &Daemon) -> u32 {
    MAX_ROUNDS.min(daemon.orchestration_config().limits().max_attempts_per_task)
}

fn check_options(daemon: &Daemon, options: &SpecOptions) -> Result<(), ProtocolError> {
    let cap = rounds_cap(daemon);
    if !(MIN_ROUNDS..=cap).contains(&options.rounds) {
        return Err(ProtocolError::invalid_request(format!(
            "rounds must be {MIN_ROUNDS}–{cap}: each round is a ledger attempt, and the ledger allows \
             {cap} per task"
        )));
    }
    if let Placement::Worktree { branch, .. } = &options.placement {
        git_service::validate_branch_name(branch)
            .map_err(|e| ProtocolError::invalid_request(e.to_string()))?;
    }
    Ok(())
}

fn tools(daemon: &Daemon, pick: &AgentPick) -> Tools {
    let surface = {
        let inner = daemon.lock();
        daemon.tool_surface(&inner, &pick.provider, false)
    };
    let ctl = crate::core::sibling_forgectl().unwrap_or_else(|| PathBuf::from("forgectl"));
    Tools::new(surface, &ctl)
}

fn project_actions(daemon: &Daemon, project_id: ProjectId) -> Vec<ProjectAction> {
    daemon
        .lock()
        .project_actions
        .get(&project_id)
        .cloned()
        .unwrap_or_default()
}

// ----- Start -----

/// Read what Forge can read of the source; a link or an artifact stays a reference.
fn read_source(source: &SpecRunSource) -> Result<(SourceRef, Option<String>), ProtocolError> {
    let cap = schema::MAX_SPEC_BYTES;
    let (kind, reference, text) = match source {
        SpecRunSource::Inline { text } => {
            if text.trim().is_empty() {
                return Err(ProtocolError::invalid_request("the spec is empty"));
            }
            ("inline", String::new(), Some(text.clone()))
        }
        SpecRunSource::File { path } => {
            let len = std::fs::metadata(path)
                .map_err(|_| ProtocolError::not_found(format!("spec file {path}")))?
                .len();
            if len > cap as u64 {
                return Err(ProtocolError::invalid_request(format!(
                    "a spec file is capped at {cap} bytes"
                )));
            }
            if !Path::new(path).is_absolute() {
                return Err(ProtocolError::invalid_request(
                    "a spec file path is absolute",
                ));
            }
            let mut text = String::new();
            std::fs::File::open(path)
                .and_then(|file| file.take(cap as u64).read_to_string(&mut text))
                .map_err(|_| ProtocolError::invalid_request("the spec file is not UTF-8 text"))?;
            ("file", path.clone(), Some(text))
        }
        SpecRunSource::ClaudeArtifact { url } => ("claude_artifact", url.clone(), None),
        SpecRunSource::Link { url } => ("link", url.clone(), None),
        SpecRunSource::Issue { reference } => ("issue", reference.clone(), None),
        _ => return Err(ProtocolError::invalid_request("unknown spec source")),
    };
    if text.as_ref().is_some_and(|t| t.len() > cap) {
        return Err(ProtocolError::invalid_request(format!(
            "a spec is capped at {cap} bytes"
        )));
    }
    let sha256 = text.as_deref().map(sha256);
    Ok((
        SourceRef {
            kind: kind.to_owned(),
            reference,
            sha256,
        },
        text,
    ))
}

fn drafter_source(dir: &Path, source: &SourceRef) -> String {
    match source.kind.as_str() {
        "claude_artifact" => source_section(&SpecSource::ClaudeArtifact(source.reference.clone())),
        "link" => source_section(&SpecSource::Link(source.reference.clone())),
        kind => {
            let what = match kind {
                "issue" => format!(
                    "GitHub issue {}, as `gh issue view` printed it",
                    source.reference
                ),
                "file" => format!("a copy of {}", source.reference),
                _ => "the text the person gave".to_owned(),
            };
            format!(
                "The source is {what}: the file {}. Read all of it.",
                dir.join("source.md").display()
            )
        }
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "the request's fields, passed through once"
)]
pub(crate) fn start(
    daemon: &Arc<Daemon>,
    workspace_id: WorkspaceId,
    name: String,
    source: SpecRunSource,
    planner: AgentPick,
    coder: AgentPick,
    options: SpecOptions,
) -> Result<Response, ProtocolError> {
    let name: String = name.trim().chars().take(MAX_NAME_CHARS).collect();
    let name = if name.is_empty() {
        "spec".to_owned()
    } else {
        name
    };
    let (project_id, root) = {
        let inner = daemon.lock();
        let ws = inner
            .workspaces
            .get(&workspace_id)
            .ok_or_else(|| ProtocolError::not_found("workspace"))?;
        (ws.project_id, ws.path.clone())
    };
    check_options(daemon, &options)?;
    let (source, text) = read_source(&source)?;
    let id = SpecRunId::new();
    let dir = dir_of(daemon, id);
    ensure_dir(&dir)?;
    if let Some(text) = &text {
        write_file(&dir, "source.md", text)?;
    }
    let now = Timestamp::now();
    let run = SpecRunView {
        id,
        project_id,
        workspace_id,
        name,
        stage: SpecStage::Drafting,
        route: Route::Standard,
        source,
        planner,
        coder,
        options,
        sessions: Vec::new(),
        coder_workspace_id: None,
        base_commit: None,
        dirty_at_launch: Vec::new(),
        ledger_run: None,
        tasks: Vec::new(),
        current_task: 0,
        round: 0,
        gate: None,
        checks: Vec::new(),
        deviations: Vec::new(),
        review_id: None,
        spec_sha256: None,
        plan_sha256: None,
        has_research: false,
        has_plan: false,
        has_report: false,
        spec_artifact_url: None,
        report_artifact_url: None,
        outcome: None,
        manual_checked: Vec::new(),
        attention: None,
        last_reason: None,
        created_at: now,
        updated_at: now,
    };
    {
        let mut runs = daemon.spec_runs.lock();
        runs.insert(id, run.clone());
        publish(daemon, run.clone());
    }
    if run.source.kind != "issue" {
        let session_id = launch_drafter(daemon, &run, &dir)?;
        return Ok(Response::SpecRunStarted {
            spec_run_id: id,
            session_id: Some(session_id),
        });
    }
    // `gh issue view` opens a socket: the request answers now, and the drafter starts once the
    // issue is read.
    let daemon_for_thread = Arc::clone(daemon);
    let spawned = std::thread::Builder::new()
        .name("forge-spec-issue".into())
        .spawn(move || {
            let daemon = &daemon_for_thread;
            let read =
                git_service::issue_markdown(&daemon.github_cli(), &root, &run.source.reference)
                    .map_err(|e| {
                        ProtocolError::new(ErrorCode::GitError, format!("gh issue view: {e}"))
                    })
                    .and_then(|text| {
                        write_file(&dir, "source.md", &text)?;
                        let sha = sha256(&text);
                        update(daemon, id, |run| {
                            run.source.sha256 = Some(sha);
                            Ok(run.clone())
                        })
                    });
            match read {
                Ok(run) => {
                    let _ = launch_drafter(daemon, &run, &dir);
                }
                Err(error) => fail_start(
                    daemon,
                    id,
                    &format!("the issue could not be read: {}", error.message),
                ),
            }
        });
    if let Err(error) = spawned {
        fail_start(
            daemon,
            id,
            &format!("the issue reader did not start: {error}"),
        );
    }
    Ok(Response::SpecRunStarted {
        spec_run_id: id,
        session_id: None,
    })
}

fn fail_start(daemon: &Daemon, id: SpecRunId, why: &str) {
    let _ = update(daemon, id, |run| {
        run.stage = SpecStage::Rejected;
        run.attention = Some(why.to_owned());
        Ok(())
    });
}

fn launch_drafter(
    daemon: &Arc<Daemon>,
    run: &SpecRunView,
    dir: &Path,
) -> Result<SessionId, ProtocolError> {
    let tools = tools(daemon, &run.planner);
    let source = drafter_source(dir, &run.source);
    let prompt = compose_drafter_prompt(&DrafterInput {
        spec_run_id: &run.id.to_string(),
        source: &source,
        tools: &tools,
        publish_artifact: run.options.publish_spec,
    });
    launch_agent(daemon, run, SpecRole::Drafter, prompt).inspect_err(|error| {
        fail_start(
            daemon,
            run.id,
            &format!("the drafter did not start: {}", error.message),
        );
    })
}

fn launch_agent(
    daemon: &Arc<Daemon>,
    run: &SpecRunView,
    role: SpecRole,
    prompt: String,
) -> Result<SessionId, ProtocolError> {
    let pick = &run.planner;
    let (label, session_role) = match role {
        SpecRole::Drafter => ("drafter", SessionRole::Researcher),
        _ => ("planner", SessionRole::Planner),
    };
    let created = daemon.create_session_with_options(
        run.workspace_id,
        SessionKind::Agent,
        Some(pick.provider.clone()),
        pick.profile,
        None,
        session_role,
        None,
        Some(prompt),
        false,
        domain::AgentLaunchOptions {
            model: pick.model.clone(),
            effort: pick.effort.clone(),
        },
        Some(format!("{} {label}", run.name)),
        None,
        false,
    )?;
    let Response::SessionCreated { session_id, .. } = created else {
        return Err(ProtocolError::internal("session creation returned no id"));
    };
    let stage = run.stage;
    let round = run.round;
    update(daemon, run.id, |run| {
        run.sessions.push(SpecSession {
            role,
            session_id,
            stage,
            round,
        });
        Ok(())
    })?;
    Ok(session_id)
}

// ----- Delivery -----

/// Hand a stage prompt to the planner: queued into its session by default, or as the first
/// prompt of a new session per stage. The prompt is also kept under `prompts/`.
fn deliver_to_planner(daemon: &Arc<Daemon>, id: SpecRunId, label: &str, prompt: String) {
    let run = match daemon.spec_runs.get(id) {
        Ok(run) => run,
        Err(_) => return,
    };
    let dir = dir_of(daemon, id);
    let file = format!("prompts/{:02}-{label}.md", run.sessions.len());
    let _ = write_file(&dir, &file, &prompt);
    let existing = run.session(SpecRole::Planner);
    let result = match existing {
        Some(session) if !run.options.session_per_stage => queue(
            daemon,
            session,
            format!(
                "Forge, spec run {id}: {label}. Read {} and follow it.",
                dir.join(&file).display()
            ),
        ),
        _ => launch_agent(daemon, &run, SpecRole::Planner, prompt).map(|_| ()),
    };
    if let Err(error) = result {
        attention(
            daemon,
            id,
            format!(
                "could not hand the {label} prompt to the planner ({}); it is in {}",
                error.message,
                dir.join(&file).display()
            ),
        );
    }
}

fn queue(daemon: &Daemon, session: SessionId, text: String) -> Result<(), ProtocolError> {
    crate::prompts::send(daemon, None, session, text, true, PromptDelivery::Queue).map(|_| ())
}

/// A gate decision's text, verbatim, to the agent that has to act on it.
fn send_reason(
    daemon: &Arc<Daemon>,
    run: &SpecRunView,
    role: SpecRole,
    what: &str,
    reason: &str,
    then: &str,
) {
    let text = stages::reason_prompt(&run.id.to_string(), what, reason, then);
    let dir = dir_of(daemon, run.id);
    let file = format!("prompts/{:02}-{what}.md", run.sessions.len());
    let _ = write_file(&dir, &file.replace(' ', "-"), &text);
    let Some(session) = run.session(role) else {
        return;
    };
    if let Err(error) = queue(daemon, session, text) {
        attention(
            daemon,
            run.id,
            format!(
                "could not hand the gate's reason to the agent: {}",
                error.message
            ),
        );
    }
}

// ----- Artifacts -----

fn resolve(
    daemon: &Daemon,
    id: Option<SpecRunId>,
    caller: Option<SessionId>,
) -> Result<SpecRunView, ProtocolError> {
    match (id, caller) {
        (Some(id), _) => daemon.spec_runs.get(id),
        (None, Some(caller)) => daemon
            .spec_runs
            .of_session(caller)
            .ok_or_else(|| ProtocolError::not_found("no spec run owns this session")),
        (None, None) => Err(ProtocolError::invalid_request("name the spec run")),
    }
}

fn author(
    run: &SpecRunView,
    caller: Option<SessionId>,
    role: SpecRole,
) -> Result<(), ProtocolError> {
    let Some(caller) = caller else { return Ok(()) };
    let allowed = run
        .sessions
        .iter()
        .any(|s| s.session_id == caller && s.role == role);
    if allowed {
        Ok(())
    } else {
        Err(ProtocolError::precondition_failed(format!(
            "only the run's {} hands this over",
            match role {
                SpecRole::Drafter => "drafter",
                _ => "planner",
            }
        )))
    }
}

fn expect_stage(run: &SpecRunView, stages: &[SpecStage]) -> Result<(), ProtocolError> {
    if stages.contains(&run.stage) {
        Ok(())
    } else {
        Err(ProtocolError::conflict(format!(
            "spec run {} is at {}",
            run.id,
            run.stage.as_str()
        )))
    }
}

/// A run file as it was saved: the hash kept at save time must still match, so an agent that
/// edits the approved spec or plan in place stops the run instead of changing what it checks.
fn pinned(dir: &Path, name: &str, cap: usize, sha: Option<&str>) -> Result<String, ProtocolError> {
    let text = read_file(dir, name, cap).ok_or_else(|| {
        ProtocolError::precondition_failed(format!("the run has no saved {name}"))
    })?;
    match sha {
        Some(sha) if sha256(&text) != sha => Err(ProtocolError::precondition_failed(format!(
            "{name} changed on disk after it was saved; save it again through Forge"
        ))),
        _ => Ok(text),
    }
}

fn approved_spec(dir: &Path, sha: Option<&str>) -> Result<(String, SpecDoc), ProtocolError> {
    let text = pinned(dir, "spec.md", schema::MAX_SPEC_BYTES, sha)?;
    let doc = schema::parse_spec(&text).map_err(|e| invalid("the saved spec", e))?;
    Ok((text, doc))
}

fn approved_plan(daemon: &Daemon, run: &SpecRunView) -> Result<(SpecDoc, PlanDoc), ProtocolError> {
    let dir = dir_of(daemon, run.id);
    let (spec_text, doc) = approved_spec(&dir, run.spec_sha256.as_deref())?;
    let plan_text = if run.route == Route::Short {
        spec_text
    } else {
        pinned(
            &dir,
            "plan.md",
            schema::MAX_PLAN_BYTES,
            run.plan_sha256.as_deref(),
        )?
    };
    let actions = project_actions(daemon, run.project_id);
    let (plan, _) =
        schema::parse_plan(&plan_text, &doc, &actions).map_err(|e| invalid("the saved plan", e))?;
    Ok((doc, plan))
}

pub(crate) fn save(
    daemon: &Arc<Daemon>,
    id: Option<SpecRunId>,
    caller: Option<SessionId>,
    kind: SpecArtifactKind,
    content: String,
    artifact_url: Option<String>,
) -> Result<Response, ProtocolError> {
    let run = resolve(daemon, id, caller)?;
    if let Some(url) = &artifact_url {
        schema::validate_artifact_url(url).map_err(ProtocolError::invalid_request)?;
    }
    let dir = dir_of(daemon, run.id);
    match kind {
        SpecArtifactKind::Spec => {
            author(&run, caller, SpecRole::Drafter)?;
            expect_stage(&run, &[SpecStage::Drafting, SpecStage::SpecGate])?;
            let doc = schema::parse_spec(&content).map_err(|e| invalid("spec refused", e))?;
            let gate = if doc.route == Route::Short {
                let actions = project_actions(daemon, run.project_id);
                Some(
                    schema::parse_plan(&content, &doc, &actions)
                        .map_err(|e| invalid("a short-route spec carries its plan; refused", e))?
                        .1,
                )
            } else {
                None
            };
            write_file(&dir, "spec.md", &content)?;
            let sha = sha256(&content);
            update(daemon, run.id, |run| {
                expect_stage(run, &[SpecStage::Drafting, SpecStage::SpecGate])?;
                run.route = doc.route;
                run.spec_sha256 = Some(sha);
                run.has_plan = gate.is_some();
                run.gate = gate;
                if artifact_url.is_some() {
                    run.spec_artifact_url = artifact_url;
                }
                run.stage = SpecStage::SpecGate;
                run.attention = None;
                Ok(())
            })?;
        }
        SpecArtifactKind::Exit => {
            author(&run, caller, SpecRole::Drafter)?;
            expect_stage(&run, &[SpecStage::Drafting, SpecStage::SpecGate])?;
            schema::parse_exit(&content).map_err(|e| invalid("early exit refused", vec![e]))?;
            if content.len() > schema::MAX_SPEC_BYTES {
                return Err(ProtocolError::invalid_request(
                    "an early exit is capped like a spec",
                ));
            }
            write_file(&dir, "exit.md", &content)?;
            update(daemon, run.id, |run| {
                run.stage = SpecStage::Exited;
                run.attention = None;
                Ok(())
            })?;
        }
        SpecArtifactKind::Research => {
            author(&run, caller, SpecRole::Planner)?;
            expect_stage(&run, &[SpecStage::Planning, SpecStage::PlanGate])?;
            if content.len() > schema::MAX_RESEARCH_BYTES {
                return Err(ProtocolError::invalid_request(format!(
                    "research is capped at {} bytes",
                    schema::MAX_RESEARCH_BYTES
                )));
            }
            write_file(&dir, "research.md", &content)?;
            update(daemon, run.id, |run| {
                run.has_research = true;
                Ok(())
            })?;
        }
        SpecArtifactKind::Plan => {
            author(&run, caller, SpecRole::Planner)?;
            expect_stage(&run, &[SpecStage::Planning, SpecStage::PlanGate])?;
            let (_, doc) = approved_spec(&dir, run.spec_sha256.as_deref())?;
            let actions = project_actions(daemon, run.project_id);
            let (_, gate) = schema::parse_plan(&content, &doc, &actions)
                .map_err(|e| invalid("plan refused", e))?;
            write_file(&dir, "plan.md", &content)?;
            let sha = sha256(&content);
            update(daemon, run.id, |run| {
                expect_stage(run, &[SpecStage::Planning, SpecStage::PlanGate])?;
                run.plan_sha256 = Some(sha);
                run.gate = Some(gate);
                run.has_plan = true;
                run.stage = SpecStage::PlanGate;
                run.attention = None;
                Ok(())
            })?;
        }
        _ => return Err(ProtocolError::invalid_request("unknown artifact kind")),
    }
    Ok(Response::Ack)
}

// ----- Gates -----

pub(crate) fn gate(
    daemon: &Arc<Daemon>,
    id: SpecRunId,
    caller: Option<SessionId>,
    kind: GateKind,
    decision: GateDecision,
) -> Result<Response, ProtocolError> {
    let run = daemon.spec_runs.get(id)?;
    if caller.is_some_and(|caller| run.owns_session(caller)) {
        return Err(ProtocolError::precondition_failed(
            "a gate is passed by a person in Forge, never by the run's own agents",
        ));
    }
    // A rejection also ends a run stuck outside a gate (a lost coder, a restart), so it only needs
    // an open run.
    if matches!(decision, GateDecision::Reject { .. }) {
        if run.stage.is_closed() {
            return Err(ProtocolError::conflict(format!(
                "spec run {} is already {}",
                run.id,
                run.stage.as_str()
            )));
        }
    } else {
        expect_stage(&run, &[kind.stage()])?;
    }
    if let GateDecision::Changes { reason } | GateDecision::Reject { reason } = &decision {
        check_reason(reason, "asking for changes or rejecting carries its reason")?;
    }
    match (kind, decision) {
        (_, GateDecision::Reject { reason }) => {
            let role = match run.stage {
                SpecStage::Drafting | SpecStage::SpecGate => SpecRole::Drafter,
                _ => SpecRole::Planner,
            };
            claim(daemon, id, run.stage, SpecStage::Rejected, Some(&reason))?;
            stop_execution(daemon, id);
            send_reason(
                daemon,
                &run,
                role,
                "the person rejected the run",
                &reason,
                "Stop here.",
            );
        }
        (GateKind::Spec, GateDecision::Changes { reason }) => {
            claim(
                daemon,
                id,
                SpecStage::SpecGate,
                SpecStage::Drafting,
                Some(&reason),
            )?;
            send_reason(
                daemon,
                &run,
                SpecRole::Drafter,
                "the person asked for changes to the spec",
                &reason,
                "Write each answer into the section it affects and save the spec again.",
            );
        }
        (GateKind::Plan, GateDecision::Changes { reason }) => {
            claim(
                daemon,
                id,
                SpecStage::PlanGate,
                SpecStage::Planning,
                Some(&reason),
            )?;
            send_reason(
                daemon,
                &run,
                SpecRole::Planner,
                "the person asked for changes to the plan",
                &reason,
                "Revise the plan and save it again.",
            );
        }
        (GateKind::Result, GateDecision::Changes { reason }) => {
            another_round(daemon, &run, reason)?;
        }
        (
            GateKind::Spec,
            GateDecision::Approve {
                options,
                save_gate_action,
                ..
            },
        ) => {
            set_options(daemon, &run, options)?;
            claim(daemon, id, SpecStage::SpecGate, SpecStage::Planning, None)?;
            let run = daemon.spec_runs.get(id)?;
            if run.route == Route::Short {
                let started = if save_gate_action {
                    save_gate(daemon, &run).and_then(|()| execute(daemon, &run))
                } else {
                    execute(daemon, &run)
                };
                if let Err(error) = started {
                    reopen(daemon, id, SpecStage::SpecGate, &error);
                    return Err(error);
                }
            } else {
                let dir = dir_of(daemon, id);
                let tools = tools(daemon, &run.planner);
                let prompt = stages::plan_stage_prompt(
                    &StageContext {
                        id: &id.to_string(),
                        name: &run.name,
                        dir: &dir.to_string_lossy(),
                        tools: &tools,
                    },
                    gate_action(daemon, run.project_id).as_deref(),
                    run.route,
                );
                deliver_to_planner(daemon, id, "plan", prompt);
            }
        }
        (
            GateKind::Plan,
            GateDecision::Approve {
                options,
                save_gate_action,
                ..
            },
        ) => {
            set_options(daemon, &run, options)?;
            claim(daemon, id, SpecStage::PlanGate, SpecStage::Executing, None)?;
            let run = daemon.spec_runs.get(id)?;
            if save_gate_action {
                if let Err(error) = save_gate(daemon, &run) {
                    reopen(daemon, id, SpecStage::PlanGate, &error);
                    return Err(error);
                }
            }
            if let Err(error) = execute(daemon, &run) {
                reopen(daemon, id, SpecStage::PlanGate, &error);
                return Err(error);
            }
        }
        (
            GateKind::Result,
            GateDecision::Approve {
                manual_checked,
                next,
                ..
            },
        ) => {
            finish(
                daemon,
                &run,
                manual_checked,
                next.unwrap_or(NextAction::Finish),
            )?;
        }
        _ => return Err(ProtocolError::invalid_request("unknown gate decision")),
    }
    Ok(Response::Ack)
}

fn check_reason(reason: &str, empty: &str) -> Result<(), ProtocolError> {
    if reason.trim().is_empty() {
        return Err(ProtocolError::invalid_request(empty));
    }
    if reason.len() > MAX_REASON_BYTES {
        return Err(ProtocolError::invalid_request(format!(
            "a reason is capped at {MAX_REASON_BYTES} bytes"
        )));
    }
    Ok(())
}

/// Move `from` → `to` atomically, so two gate clicks cannot both pass.
fn claim(
    daemon: &Daemon,
    id: SpecRunId,
    from: SpecStage,
    to: SpecStage,
    reason: Option<&str>,
) -> Result<(), ProtocolError> {
    update(daemon, id, |run| {
        expect_stage(run, &[from])?;
        run.stage = to;
        run.attention = None;
        if let Some(reason) = reason {
            run.last_reason = Some(reason.to_owned());
        }
        Ok(())
    })
}

/// Stop what a rejected run still has working: the coder's ledger attempt and any check.
fn stop_execution(daemon: &Arc<Daemon>, id: SpecRunId) {
    let Ok(run) = daemon.spec_runs.get(id) else {
        return;
    };
    if let Some(ledger) = run.ledger_run {
        if let Err(error) = crate::orchestration::cancel_spec_ledger(daemon, ledger) {
            tracing::warn!(spec_run = %id, error = %error.message, "could not cancel the coder");
        }
    }
    for check in run.sessions.iter().filter(|s| s.role == SpecRole::Check) {
        let _ = daemon.kill_session(check.session_id);
    }
}

/// A run rejected while its gate was proceeding stays rejected.
fn reopen(daemon: &Daemon, id: SpecRunId, stage: SpecStage, error: &ProtocolError) {
    let _ = update(daemon, id, |run| {
        if run.stage.is_closed() {
            return Ok(());
        }
        run.stage = stage;
        run.attention = Some(format!("the gate could not proceed: {}", error.message));
        Ok(())
    });
}

fn set_options(
    daemon: &Daemon,
    run: &SpecRunView,
    options: Option<SpecOptions>,
) -> Result<(), ProtocolError> {
    let Some(options) = options else {
        return Ok(());
    };
    check_options(daemon, &options)?;
    update(daemon, run.id, |run| {
        run.options = options;
        Ok(())
    })
}

fn gate_action(daemon: &Daemon, project_id: ProjectId) -> Option<String> {
    project_actions(daemon, project_id)
        .into_iter()
        .find(|a| a.gate)
        .map(|a| a.command)
}

/// Keep the approved gate as the project's gate action, the only one marked.
fn save_gate(daemon: &Arc<Daemon>, run: &SpecRunView) -> Result<(), ProtocolError> {
    let Some(gate) = run.gate.clone().filter(|g| g.action_id.is_none()) else {
        return Ok(());
    };
    let mut actions = project_actions(daemon, run.project_id);
    for action in &mut actions {
        action.gate = false;
    }
    let taken: BTreeSet<String> = actions.iter().map(|a| a.label.to_lowercase()).collect();
    let label = ["gate", "repository gate", "spec gate"]
        .into_iter()
        .find(|label| !taken.contains(*label))
        .unwrap_or("spec run gate")
        .to_owned();
    let id = ProjectActionId::new();
    actions.push(ProjectAction {
        id,
        project_id: run.project_id,
        label,
        command: gate.command.clone(),
        kind: ActionKind::Test,
        cwd: None,
        single_instance: true,
        pinned: false,
        gate: true,
        ready: ActionReady::default(),
        port: None,
        depends_on: Vec::new(),
        source: ActionSource::Manual,
        position: u32::try_from(actions.len()).unwrap_or(u32::MAX),
        created_at: Timestamp::now(),
    });
    daemon.handle_request(Request::SetProjectActions {
        project_id: run.project_id,
        actions,
    })?;
    update(daemon, run.id, |run| {
        if let Some(gate) = &mut run.gate {
            gate.action_id = Some(id);
        }
        Ok(())
    })
}

// ----- Execution -----

/// The stages a gate approval leaves the run in while `execute` places the coder: the short
/// route approves the spec and its plan together.
const LAUNCHING: &[SpecStage] = &[SpecStage::Planning, SpecStage::Executing];

/// Place the coder, open its ledger and start round 1. After a launch that failed, the
/// worktree and the ledger already exist and only the round is started again.
fn execute(daemon: &Arc<Daemon>, run: &SpecRunView) -> Result<(), ProtocolError> {
    let (spec, plan) = approved_plan(daemon, run)?;
    if run.ledger_run.is_some() {
        let run = update(daemon, run.id, |run| {
            expect_stage(run, LAUNCHING)?;
            run.stage = SpecStage::Executing;
            Ok(run.clone())
        })?;
        return start_round(daemon, &run, None);
    }
    // A worktree from an approval that failed later is the one to retry in: creating it again
    // would find its branch checked out.
    let placed = run
        .coder_workspace_id
        .and_then(|ws| daemon.workspace_path(ws).ok().map(|path| (ws, path)));
    let (workspace, path) = match (placed, &run.options.placement) {
        (Some(placed), Placement::Worktree { .. }) => placed,
        (None, Placement::Worktree { branch, base }) => {
            let ws =
                daemon.create_managed_worktree(run.project_id, branch, base.as_deref(), None)?;
            update(daemon, run.id, |run| {
                run.coder_workspace_id = Some(ws.id);
                Ok(())
            })?;
            (ws.id, ws.path)
        }
        (_, Placement::Checkout) => (run.workspace_id, daemon.workspace_path(run.workspace_id)?),
    };
    let dirty = match run.options.placement {
        Placement::Checkout => git_service::change_summary(&path, None)
            .map(|summary| {
                summary
                    .files
                    .into_iter()
                    .map(|f| f.path)
                    .take(MAX_DIRTY_PATHS)
                    .collect()
            })
            .unwrap_or_default(),
        Placement::Worktree { .. } => Vec::new(),
    };
    let base = git_service::head_commit(&path);
    let dir = dir_of(daemon, run.id);
    let pointer = |criteria: &[String]| {
        format!(
            "Spec: {}\nPlan: {}\nCriteria: {}",
            dir.join("spec.md").display(),
            dir.join(if run.route == Route::Short {
                "spec.md"
            } else {
                "plan.md"
            })
            .display(),
            criteria.join(", ")
        )
    };
    let all: Vec<String> = spec.criteria.iter().map(|c| c.id.clone()).collect();
    let tasks: Vec<(String, String, String, Vec<usize>)> = if plan.tasks.is_empty() {
        vec![(run.name.clone(), pointer(&all), all.join(", "), Vec::new())]
    } else {
        plan.tasks
            .iter()
            .map(|task| {
                let after = task
                    .after
                    .iter()
                    .filter_map(|a| plan.tasks.iter().position(|t| &t.id == a))
                    .collect();
                (
                    task.title.clone(),
                    pointer(&task.criteria),
                    task.criteria.join(", "),
                    after,
                )
            })
            .collect()
    };
    let planner = run.session(SpecRole::Planner);
    let objective = if spec.title.is_empty() {
        run.name.clone()
    } else {
        spec.title.clone()
    };
    let (ledger, task_ids) = crate::orchestration::open_spec_ledger(
        daemon,
        planner,
        run.project_id,
        objective,
        workspace,
        tasks,
    )?;
    let opened = update(daemon, run.id, |run| {
        expect_stage(run, LAUNCHING)?;
        run.stage = SpecStage::Executing;
        run.coder_workspace_id = Some(workspace);
        run.base_commit = base;
        run.dirty_at_launch = dirty;
        run.ledger_run = Some(ledger);
        run.tasks = task_ids;
        run.current_task = 0;
        run.round = 0;
        Ok(())
    });
    if let Err(error) = opened {
        let _ = crate::orchestration::cancel_spec_ledger(daemon, ledger);
        return Err(error);
    }
    let run = daemon.spec_runs.get(run.id)?;
    start_round(daemon, &run, None)?;
    {
        let run = daemon.spec_runs.get(run.id)?;
        let tools = tools(daemon, &run.planner);
        let coder = run
            .session(SpecRole::Coder)
            .map(|s| s.to_string())
            .unwrap_or_default();
        let prompt = stages::watch_stage_prompt(
            &StageContext {
                id: &run.id.to_string(),
                name: &run.name,
                dir: &dir.to_string_lossy(),
                tools: &tools,
            },
            &coder,
            &path.to_string_lossy(),
        );
        deliver_to_planner(daemon, run.id, "watch", prompt);
    }
    Ok(())
}

fn task_criteria(
    daemon: &Daemon,
    run: &SpecRunView,
) -> Result<(String, Vec<String>), ProtocolError> {
    let (spec, plan) = approved_plan(daemon, run)?;
    Ok(match plan.tasks.get(run.current_task) {
        Some(task) => (task.title.clone(), task.criteria.clone()),
        None => (
            run.name.clone(),
            spec.criteria.iter().map(|c| c.id.clone()).collect(),
        ),
    })
}

fn start_round(
    daemon: &Arc<Daemon>,
    run: &SpecRunView,
    findings: Option<String>,
) -> Result<(), ProtocolError> {
    let task_index = run.current_task;
    let workspace = run
        .coder_workspace_id
        .ok_or_else(|| ProtocolError::precondition_failed("the coder has no workspace yet"))?;
    let task = *run
        .tasks
        .get(run.current_task)
        .ok_or_else(|| ProtocolError::precondition_failed("the run has no task left"))?;
    let (title, criteria) = task_criteria(daemon, run)?;
    let round = run.round + 1;
    let dir = dir_of(daemon, run.id);
    let tools = tools(daemon, &run.coder);
    let prompt = stages::coder_prompt(&CoderInput {
        ctx: &StageContext {
            id: &run.id.to_string(),
            name: &run.name,
            dir: &dir.to_string_lossy(),
            tools: &tools,
        },
        round,
        task: &title,
        criteria: &criteria,
        commit: run.options.delivery == Delivery::Commit,
        findings: findings.as_deref(),
    });
    let (_, session_id) = crate::orchestration::start_spec_attempt(
        daemon,
        task,
        &run.coder,
        workspace,
        findings,
        crate::orchestration::CoderLaunch {
            prompt,
            options: domain::AgentLaunchOptions {
                model: run.coder.model.clone(),
                effort: run.coder.effort.clone(),
            },
            title: Some(format!("{} coder r{round}", run.name)),
        },
    )?;
    let started = update(daemon, run.id, |run| {
        expect_stage(run, &[SpecStage::Executing])?;
        run.current_task = task_index;
        run.round = round;
        run.attention = None;
        run.sessions.push(SpecSession {
            role: SpecRole::Coder,
            session_id,
            stage: SpecStage::Executing,
            round,
        });
        Ok(())
    });
    if started.is_err() {
        let _ = daemon.kill_session(session_id);
    }
    started
}

/// Refuse an agent launch by a run's drafter or planner before its plan gate: the coder starts
/// when the person approves the plan, never earlier.
pub(crate) fn refuse_early_coder(
    daemon: &Daemon,
    launchers: impl IntoIterator<Item = SessionId>,
) -> Result<(), ProtocolError> {
    for launcher in launchers {
        let Some(run) = daemon.spec_runs.of_session(launcher) else {
            continue;
        };
        let planning = run.sessions.iter().any(|s| {
            s.session_id == launcher && matches!(s.role, SpecRole::Drafter | SpecRole::Planner)
        });
        if planning && !run.stage.past_plan_gate() && !run.stage.is_closed() {
            return Err(ProtocolError::precondition_failed(format!(
                "spec run {} is at {}: Forge launches the coder once the person approves the plan",
                run.id,
                run.stage.as_str()
            )));
        }
    }
    Ok(())
}

// ----- Verification -----

/// The coder's attempt settled: run the plan's checks off every lock, then hand the review over.
pub(crate) fn attempt_reported(daemon: &Arc<Daemon>, attempt_id: domain::AttemptId) {
    let Some((task_id, session, report)) =
        crate::orchestration::spec_attempt_facts(daemon, attempt_id)
    else {
        return;
    };
    let Some(run) = session.and_then(|s| daemon.spec_runs.of_session(s)) else {
        return;
    };
    if run.tasks.get(run.current_task) != Some(&task_id) || run.session(SpecRole::Coder) != session
    {
        return;
    }
    if claim(
        daemon,
        run.id,
        SpecStage::Executing,
        SpecStage::Verifying,
        None,
    )
    .is_err()
    {
        return;
    }
    let daemon_for_thread = Arc::clone(daemon);
    let id = run.id;
    let spawned = std::thread::Builder::new()
        .name("forge-spec-checks".into())
        .spawn(move || {
            let _guard = VerifyGuard {
                daemon: &daemon_for_thread,
                id,
            };
            verify(&daemon_for_thread, id, report);
        });
    if let Err(error) = spawned {
        attention(daemon, id, format!("could not start the checks: {error}"));
    }
}

/// A verification that stopped early leaves the run waiting on the person, not latched in
/// Verifying; after a normal one the stage has already moved on.
struct VerifyGuard<'a> {
    daemon: &'a Arc<Daemon>,
    id: SpecRunId,
}

impl Drop for VerifyGuard<'_> {
    fn drop(&mut self) {
        let _ = update(self.daemon, self.id, |run| {
            if run.stage == SpecStage::Verifying {
                run.stage = SpecStage::Reviewing;
                run.attention = Some("the checks stopped before they finished".to_owned());
            }
            Ok(())
        });
    }
}

fn verify(
    daemon: &Arc<Daemon>,
    id: SpecRunId,
    report: Option<domain::orchestration::AttemptReport>,
) {
    let Ok(run) = daemon.spec_runs.get(id) else {
        return;
    };
    let Some(workspace) = run.coder_workspace_id else {
        return;
    };
    let Ok(path) = daemon.workspace_path(workspace) else {
        return;
    };
    let (planned, unreadable) = match planned_checks(daemon, &run) {
        Ok(planned) => (planned, None),
        Err(error) => (
            Vec::new(),
            Some(format!(
                "the plan's checks could not be read: {}",
                error.message
            )),
        ),
    };
    let dir = dir_of(daemon, id);
    let mut records: Vec<CheckRecord> = Vec::with_capacity(planned.len());
    for (index, check) in planned.iter().enumerate() {
        if !daemon
            .spec_runs
            .get(id)
            .is_ok_and(|run| run.stage == SpecStage::Verifying)
        {
            return;
        }
        records.push(run_check(daemon, &run, workspace, &dir, index, check));
    }
    let branch = match &run.options.placement {
        Placement::Worktree { branch, .. } => Some(branch.clone()),
        Placement::Checkout => None,
    };
    let current = git_service::current_branch(&path).ok().flatten();
    let deviation = schema::delivery_deviation(
        run.options.delivery,
        run.base_commit.as_deref().unwrap_or(""),
        report
            .as_ref()
            .and_then(|r| r.reported_head.as_deref())
            .or(git_service::head_commit(&path).as_deref()),
        branch.as_deref(),
        current.as_deref(),
    );
    let repeated: Vec<String> = records
        .iter()
        .filter(|record| !record.passed() && record.not_verified.is_none())
        .filter(|record| {
            run.checks.iter().any(|earlier| {
                earlier.criterion == record.criterion
                    && earlier.task == record.task
                    && earlier.round + 1 == record.round
                    && !earlier.passed()
                    && earlier.not_verified.is_none()
            })
        })
        .map(|record| record.criterion.clone())
        .collect();
    let mut deviations = run.deviations.clone();
    if let Some(deviation) = deviation {
        if !deviations.contains(&deviation) {
            deviations.push(deviation);
        }
    }
    let updated = update(daemon, id, |run| {
        expect_stage(run, &[SpecStage::Verifying])?;
        run.checks.extend(records.iter().cloned());
        run.deviations.clone_from(&deviations);
        run.stage = SpecStage::Reviewing;
        run.attention = unreadable.or_else(|| {
            (!repeated.is_empty()).then(|| {
                format!(
                    "{} failed two rounds in a row; the planner takes the premise to you",
                    repeated.join(", ")
                )
            })
        });
        Ok(run.clone())
    });
    let Ok(run) = updated else { return };
    let tools = tools(daemon, &run.planner);
    let task = if run.tasks.len() > 1 {
        task_criteria(daemon, &run)
            .ok()
            .map(|(title, _)| (title, run.current_task + 1 < run.tasks.len()))
    } else {
        None
    };
    let coder = run
        .session(SpecRole::Coder)
        .map(|s| s.to_string())
        .unwrap_or_default();
    let summary = report
        .as_ref()
        .map(|r| r.summary.clone())
        .unwrap_or_default();
    let report_file = report.as_ref().and_then(|r| r.result_path.clone());
    let prompt = stages::review_stage_prompt(&ReviewInput {
        ctx: &StageContext {
            id: &id.to_string(),
            name: &run.name,
            dir: &dir.to_string_lossy(),
            tools: &tools,
        },
        round: run.round,
        rounds: run.options.rounds,
        coder_session: &coder,
        workspace: &path.to_string_lossy(),
        base_commit: run.base_commit.as_deref().unwrap_or("HEAD"),
        dirty: &run.dirty_at_launch,
        report_summary: &summary,
        report_file: report_file.as_deref(),
        checks: &records,
        repeated: &repeated,
        deviations: &run.deviations,
        publish_report: run.options.publish_report,
        task: task.as_ref().map(|(title, more)| (title.as_str(), *more)),
    });
    deliver_to_planner(daemon, id, &format!("review-r{}", run.round), prompt);
}

/// This round's checks: the current task's criteria, and the gate after the last task.
fn planned_checks(daemon: &Daemon, run: &SpecRunView) -> Result<Vec<PlannedCheck>, ProtocolError> {
    let (_, plan) = approved_plan(daemon, run)?;
    let (_, criteria) = task_criteria(daemon, run)?;
    let mut checks = plan.checks(Some(&criteria));
    let last = run.current_task + 1 >= run.tasks.len();
    if let (true, Some(gate)) = (last, &run.gate) {
        checks.push(PlannedCheck {
            criterion: GATE.to_owned(),
            command: gate.command.clone(),
            timeout_secs: None,
        });
    }
    Ok(checks)
}

fn quotable(path: &Path) -> Option<String> {
    let text = path.to_str()?;
    (!text.contains(['"', '$', '\\', '`'])).then(|| text.to_owned())
}

/// One check as a `Task` session: `/bin/sh` runs it, keeps its exit code and at most the last
/// 16 KiB of its output, and a check past its timeout is killed with its whole process group.
fn run_check(
    daemon: &Arc<Daemon>,
    run: &SpecRunView,
    workspace: WorkspaceId,
    dir: &Path,
    index: usize,
    check: &PlannedCheck,
) -> CheckRecord {
    let round = run.round;
    // Two steps may serve one criterion, and every task restarts its rounds.
    let stem = format!(
        "checks/{}-t{}-r{round}-{}",
        check.criterion,
        run.current_task + 1,
        index + 1
    );
    let log = format!("{stem}.log");
    let mut record = CheckRecord {
        criterion: check.criterion.clone(),
        task: run.current_task,
        round,
        command: check.command.clone(),
        exit_code: None,
        timed_out: false,
        not_verified: None,
        duration_ms: 0,
        log: log.clone(),
    };
    let (script, rc, tail) = (
        dir.join(format!("{stem}.sh")),
        dir.join(format!("{stem}.rc")),
        dir.join(format!("{stem}.out")),
    );
    let (Some(script_q), Some(rc_q), Some(tail_q)) =
        (quotable(&script), quotable(&rc), quotable(&tail))
    else {
        record.not_verified = Some("the run directory's path cannot be quoted for /bin/sh".into());
        write_log(dir, &record, "");
        return record;
    };
    let body = format!(
        "{{ {command}\n  echo $? > \"{rc_q}\"\n}} 2>&1 | /usr/bin/tee /dev/stderr | /usr/bin/tail -c {cap} > \"{tail_q}\"\nrc=1\nread rc < \"{rc_q}\"\nexit \"$rc\"\n",
        command = check.command,
        cap = schema::MAX_CHECK_LOG_BYTES,
    );
    if let Err(error) = write_file(dir, &format!("{stem}.sh"), &body) {
        record.not_verified = Some(error.message);
        write_log(dir, &record, "");
        return record;
    }
    let timeout = check
        .timeout_secs
        .map(|secs| Duration::from_secs(secs.clamp(1, MAX_CHECK_TIMEOUT_SECS)))
        .unwrap_or(DEFAULT_CHECK_TIMEOUT);
    let started = Instant::now();
    let session = match daemon.launch_check(
        workspace,
        format!("{} check {} r{round}", run.name, check.criterion),
        format!("/bin/sh \"{script_q}\""),
    ) {
        Ok(session) => session,
        Err(error) => {
            record.not_verified = Some(format!("the check did not start: {}", error.message));
            write_log(dir, &record, "");
            return record;
        }
    };
    let _ = update(daemon, run.id, |run| {
        run.sessions.push(SpecSession {
            role: SpecRole::Check,
            session_id: session,
            stage: SpecStage::Verifying,
            round,
        });
        Ok(())
    });
    let mut state = wait_for_exit(daemon, session, started + timeout);
    if state.is_none() {
        record.timed_out = true;
        let _ = daemon.kill_session(session);
        state = wait_for_exit(
            daemon,
            session,
            Instant::now() + daemon.kill_grace() + REAP_SLACK,
        );
    }
    record.duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    if let Some(SessionState::Exited { code, .. }) = state {
        record.exit_code = code;
    }
    if record.exit_code == Some(COMMAND_NOT_FOUND) {
        record.not_verified = Some("the command was not found in the check's environment".into());
    }
    let output = read_tail(&tail);
    write_log(dir, &record, &output);
    if let Err(error) = daemon.close_session(session) {
        tracing::debug!(%session, error = %error.message, "check session left open");
    }
    for path in [&script, &rc, &tail] {
        let _ = std::fs::remove_file(path);
    }
    record
}

fn wait_for_exit(daemon: &Daemon, session: SessionId, deadline: Instant) -> Option<SessionState> {
    let (wake_tx, wake_rx) = flume::bounded(1);
    daemon.lock().task_waiters.push(wake_tx);
    loop {
        {
            let inner = daemon.lock();
            match inner.sessions.get(&session) {
                None => {
                    return Some(SessionState::Exited {
                        code: None,
                        signal: None,
                    })
                }
                Some(s) if s.state.is_terminal() => return Some(s.state.clone()),
                Some(_) => {}
            }
        }
        if wake_rx.recv_deadline(deadline).is_err() {
            return None;
        }
    }
}

fn read_tail(path: &Path) -> String {
    let Ok(mut file) = std::fs::File::open(path) else {
        return String::new();
    };
    let cap = schema::MAX_CHECK_LOG_BYTES as u64;
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    let _ = file.seek(SeekFrom::Start(len.saturating_sub(cap)));
    let mut bytes = Vec::new();
    let _ = file.take(cap).read_to_end(&mut bytes);
    String::from_utf8_lossy(&bytes).into_owned()
}

fn write_log(dir: &Path, record: &CheckRecord, output: &str) {
    let exit = match (record.timed_out, record.exit_code) {
        (true, _) => "timed out, killed".to_owned(),
        (false, Some(code)) => code.to_string(),
        (false, None) => "none".to_owned(),
    };
    let mut text = format!(
        "command: {}\nexit: {exit}\nduration_ms: {}\n",
        record.command, record.duration_ms
    );
    if let Some(reason) = &record.not_verified {
        text.push_str(&format!("not_verified: {reason}\n"));
    }
    text.push_str("\n--- last 16 KiB of output ---\n");
    text.push_str(output);
    let _ = write_file(dir, &record.log, &text);
}

// ----- Rounds and report -----

pub(crate) fn round(
    daemon: &Arc<Daemon>,
    id: Option<SpecRunId>,
    caller: Option<SessionId>,
    findings: Option<String>,
) -> Result<Response, ProtocolError> {
    let run = resolve(daemon, id, caller)?;
    author(&run, caller, SpecRole::Planner)?;
    expect_stage(&run, &[SpecStage::Reviewing])?;
    match findings {
        Some(findings) => {
            if findings.trim().is_empty() || findings.len() > MAX_FINDINGS_BYTES {
                return Err(ProtocolError::invalid_request(format!(
                    "findings are 1–{MAX_FINDINGS_BYTES} bytes"
                )));
            }
            if run.round >= run.options.rounds {
                return Err(ProtocolError::precondition_failed(format!(
                    "round {} of {} is spent: write the report",
                    run.round, run.options.rounds
                )));
            }
            write_file(
                &dir_of(daemon, run.id),
                &format!("findings-r{}.md", run.round),
                &findings,
            )?;
            claim(
                daemon,
                run.id,
                SpecStage::Reviewing,
                SpecStage::Executing,
                None,
            )?;
            if let Err(error) = start_round(daemon, &run, Some(findings)) {
                reopen(daemon, run.id, SpecStage::Reviewing, &error);
                return Err(error);
            }
        }
        None => {
            if run.current_task + 1 >= run.tasks.len() {
                return Err(ProtocolError::precondition_failed(
                    "there is no further task: write the report",
                ));
            }
            let task = run.tasks[run.current_task];
            crate::orchestration::accept_spec_task(daemon, task)?;
            claim(
                daemon,
                run.id,
                SpecStage::Reviewing,
                SpecStage::Executing,
                None,
            )?;
            let mut next = run.clone();
            next.current_task += 1;
            next.round = 0;
            if let Err(error) = start_round(daemon, &next, None) {
                reopen(daemon, run.id, SpecStage::Reviewing, &error);
                return Err(error);
            }
        }
    }
    Ok(Response::Ack)
}

/// Each task's checks from its last round: rounds restart with every task, so this round's
/// number alone would mix one task's first round with another's.
fn final_checks(checks: &[CheckRecord]) -> Vec<CheckRecord> {
    let mut last: HashMap<usize, u32> = HashMap::new();
    for check in checks {
        let round = last.entry(check.task).or_insert(0);
        *round = (*round).max(check.round);
    }
    checks
        .iter()
        .filter(|c| last.get(&c.task) == Some(&c.round))
        .cloned()
        .collect()
}

fn reviewed_paths(daemon: &Daemon, run: &SpecRunView) -> Result<BTreeSet<String>, ProtocolError> {
    let workspace = run
        .coder_workspace_id
        .ok_or_else(|| ProtocolError::precondition_failed("the coder has no workspace"))?;
    let path = daemon.workspace_path(workspace)?;
    let summary = git_service::change_summary(&path, run.base_commit.as_deref())
        .map_err(|e| ProtocolError::new(ErrorCode::GitError, e.to_string()))?;
    let dirty: BTreeSet<&str> = run.dirty_at_launch.iter().map(String::as_str).collect();
    Ok(summary
        .files
        .into_iter()
        .map(|file| file.path)
        .filter(|path| !dirty.contains(path.as_str()))
        .collect())
}

pub(crate) fn report(
    daemon: &Arc<Daemon>,
    id: Option<SpecRunId>,
    caller: Option<SessionId>,
    content: String,
    artifact_url: Option<String>,
) -> Result<Response, ProtocolError> {
    let run = resolve(daemon, id, caller)?;
    author(&run, caller, SpecRole::Planner)?;
    expect_stage(&run, &[SpecStage::Reviewing])?;
    tasks_done(&run)?;
    if let Some(url) = &artifact_url {
        schema::validate_artifact_url(url).map_err(ProtocolError::invalid_request)?;
    }
    let dir = dir_of(daemon, run.id);
    let (_, spec) = approved_spec(&dir, run.spec_sha256.as_deref())?;
    let diff = reviewed_paths(daemon, &run)?;
    let checks = final_checks(&run.checks);
    let valid = schema::validate_report(
        &content,
        &schema::ReportContext {
            spec: &spec,
            diff_paths: &diff,
            checks: &checks,
        },
    )
    .map_err(|e| invalid("report refused", e))?;
    let mut text = content;
    if !run.deviations.is_empty() {
        text.push_str("\n\n## Deviations Forge found\n");
        for deviation in &run.deviations {
            if text.len() + deviation.len() + 3 > REPORT_FILE_BYTES {
                text.push_str("- (more deviations left out)\n");
                break;
            }
            text.push_str(&format!("- {deviation}\n"));
        }
    }
    write_file(&dir, "report.md", &text)?;
    update(daemon, run.id, |run| {
        expect_stage(run, &[SpecStage::Reviewing])?;
        tasks_done(run)?;
        run.has_report = true;
        run.outcome = Some(valid.outcome);
        if artifact_url.is_some() {
            run.report_artifact_url = artifact_url;
        }
        run.stage = SpecStage::ResultGate;
        run.attention = None;
        Ok(())
    })?;
    if run.options.forge_review {
        start_forge_review(daemon, &run);
    }
    Ok(Response::Ack)
}

/// The report closes the last task: before it, the remaining tasks and the gate have not run.
fn tasks_done(run: &SpecRunView) -> Result<(), ProtocolError> {
    if run.current_task + 1 < run.tasks.len() {
        return Err(ProtocolError::precondition_failed(format!(
            "task {} of {} is under review: start the next task before the report",
            run.current_task + 1,
            run.tasks.len()
        )));
    }
    Ok(())
}

fn start_forge_review(daemon: &Arc<Daemon>, run: &SpecRunView) {
    let Some(workspace_id) = run.coder_workspace_id else {
        return;
    };
    let started = daemon.start_review(
        domain::ReviewTarget::Workspace {
            workspace_id,
            base: run.base_commit.clone(),
        },
        None,
        None,
        None,
        None,
        domain::AgentLaunchOptions::default(),
        domain::ReviewerOptions {
            count: Some(1),
            hypotheses: Vec::new(),
        },
    );
    match started {
        Ok(Response::ReviewStarted { review_id }) => {
            let _ = update(daemon, run.id, |run| {
                run.review_id = Some(review_id);
                Ok(())
            });
        }
        Ok(_) => {}
        Err(error) => attention(
            daemon,
            run.id,
            format!("Forge Review did not start: {}", error.message),
        ),
    }
}

fn another_round(
    daemon: &Arc<Daemon>,
    run: &SpecRunView,
    text: String,
) -> Result<(), ProtocolError> {
    check_reason(&text, "another round carries what to change")?;
    if run.round >= run.options.rounds {
        return Err(ProtocolError::precondition_failed(format!(
            "round {} of {} is spent",
            run.round, run.options.rounds
        )));
    }
    claim(
        daemon,
        run.id,
        SpecStage::ResultGate,
        SpecStage::Executing,
        Some(&text),
    )?;
    if let Err(error) = start_round(daemon, run, Some(text)) {
        reopen(daemon, run.id, SpecStage::ResultGate, &error);
        return Err(error);
    }
    Ok(())
}

fn finish(
    daemon: &Arc<Daemon>,
    run: &SpecRunView,
    manual_checked: Vec<String>,
    next: NextAction,
) -> Result<(), ProtocolError> {
    match next {
        NextAction::AnotherRound { text } => return another_round(daemon, run, text),
        NextAction::OpenDraftPr => {
            let workspace = run
                .coder_workspace_id
                .ok_or_else(|| ProtocolError::precondition_failed("the coder has no workspace"))?;
            let report = read_file(&dir_of(daemon, run.id), "report.md", REPORT_FILE_BYTES)
                .ok_or_else(|| ProtocolError::precondition_failed("the run has no report"))?;
            daemon.open_pull_request_from(
                workspace,
                run.planner.provider.clone(),
                run.planner.profile,
                true,
                Some(report),
            )?;
        }
        NextAction::DiscardWorktree => {
            if let (Placement::Worktree { .. }, Some(workspace)) =
                (&run.options.placement, run.coder_workspace_id)
            {
                daemon.remove_worktree(workspace, true)?;
            }
        }
        NextAction::Finish => {}
        _ => return Err(ProtocolError::invalid_request("unknown next action")),
    }
    claim(daemon, run.id, SpecStage::ResultGate, SpecStage::Done, None)?;
    update(daemon, run.id, |run| {
        run.manual_checked = manual_checked;
        Ok(())
    })
}

/// The coder's session ended before it reported: the person decides what follows.
pub(crate) fn note_session_exit(daemon: &Daemon, session: SessionId) {
    let Some(run) = daemon.spec_runs.of_session(session) else {
        return;
    };
    if run.stage == SpecStage::Executing && run.session(SpecRole::Coder) == Some(session) {
        attention(
            daemon,
            run.id,
            "the coder's session ended without a report".to_owned(),
        );
    }
}

pub(crate) fn detail(daemon: &Daemon, id: SpecRunId) -> Result<Response, ProtocolError> {
    let run = daemon.spec_runs.get(id)?;
    let dir = dir_of(daemon, id);
    let spec = read_file(&dir, "spec.md", schema::MAX_SPEC_BYTES);
    let doc = spec.as_deref().and_then(|s| schema::parse_spec(s).ok());
    let planned_checks = approved_plan(daemon, &run)
        .map(|(_, plan)| {
            let mut checks: Vec<(String, String)> = plan
                .checks(None)
                .into_iter()
                .map(|c| (c.criterion, c.command))
                .collect();
            if let Some(gate) = &run.gate {
                checks.push((GATE.to_owned(), gate.command.clone()));
            }
            checks
        })
        .unwrap_or_default();
    let manual_checks = doc
        .map(|doc| {
            doc.criteria
                .into_iter()
                .filter(|c| c.manual)
                .map(|c| format!("{}: {}", c.id, c.text))
                .collect()
        })
        .unwrap_or_default();
    Ok(Response::SpecRun(Box::new(SpecRunDetail {
        spec,
        research: read_file(&dir, "research.md", schema::MAX_RESEARCH_BYTES),
        plan: read_file(&dir, "plan.md", schema::MAX_PLAN_BYTES),
        report: read_file(&dir, "report.md", REPORT_FILE_BYTES),
        exit: read_file(&dir, "exit.md", schema::MAX_SPEC_BYTES),
        planned_checks,
        manual_checks,
        run,
    })))
}
