use super::*;
use client::Store;
use domain::{ActivityState, Project, Session, SessionKind, SessionRole, SessionState, Workspace};
use places::{load, workspace_json};
use protocol::{PromptDelivery, Signal};

const MAX_INPUT_BYTES: usize = 8_192;
/// A launch prompt travels as one argument, not a paste, so it may be larger than a sent prompt.
const MAX_LAUNCH_PROMPT_BYTES: usize = 64 * 1024;

#[derive(Subcommand)]
pub(crate) enum SessionCmd {
    /// Launch an agent.
    Spawn(Box<SpawnArgs>),
    /// Open an interactive shell.
    Shell(ShellArgs),
    Rename {
        session: String,
        title: String,
    },
    /// Paste a bounded prompt into an agent.
    #[command(group(clap::ArgGroup::new("prompt").args(["text", "file", "stdin"]).required(true)))]
    Send {
        session: String,
        #[arg(long, conflicts_with_all = ["file", "stdin"])]
        text: Option<String>,
        #[arg(long, conflicts_with = "stdin")]
        file: Option<PathBuf>,
        #[arg(value_parser = ["-"])]
        stdin: Option<String>,
        #[arg(long)]
        no_submit: bool,
        #[arg(long, conflicts_with = "now")]
        queue: bool,
        #[arg(long)]
        now: bool,
    },
    /// Run one command line in a shell that is at its prompt.
    #[command(group(clap::ArgGroup::new("command").args(["text", "stdin"]).required(true)))]
    Exec {
        session: String,
        #[arg(long, conflicts_with = "stdin")]
        text: Option<String>,
        #[arg(value_parser = ["-"])]
        stdin: Option<String>,
    },
    Wait {
        session: String,
        /// One or more of idle, waiting, working, exit; the first to hold wins.
        #[arg(long, required = true, value_delimiter = ',', value_parser = ["idle", "waiting", "working", "exit"])]
        r#for: Vec<String>,
        #[arg(long)]
        timeout: Option<String>,
    },
    List {
        #[arg(long)]
        project: Option<String>,
        #[arg(long, value_enum)]
        kind: Option<KindArg>,
        /// Only sessions whose process is starting or running.
        #[arg(long)]
        active: bool,
    },
    Show {
        session: String,
    },
    Read {
        session: String,
        #[arg(long)]
        max_bytes: Option<u32>,
        /// Lines back from the bottom of the terminal.
        #[arg(long)]
        lines: Option<u32>,
    },
    /// Files the session changed since its base commit.
    Changes {
        session: String,
    },
    Kill {
        session: String,
    },
    /// Relaunch an ended session in its terminal; an agent's initial prompt is not re-sent.
    Restart {
        session: String,
    },
    /// Remove an ended session from the tree.
    Close {
        session: String,
    },
    /// Deliver a signal to the session's process group.
    Signal {
        session: String,
        #[arg(long, value_enum)]
        signal: SignalArg,
        /// Signal the terminal's foreground job, as a typed Ctrl-C would.
        #[arg(long)]
        foreground: bool,
    },
}

#[derive(clap::Args)]
pub(crate) struct Placement {
    /// Project name (case-insensitive), root path or UUID. Inside a Forge
    /// session it defaults to that session's project and checkout.
    #[arg(long)]
    project: Option<String>,
    /// Existing checkout: UUID or unique prefix. Defaults to the main checkout.
    #[arg(long, conflicts_with = "worktree")]
    workspace: Option<String>,
    /// Create a managed worktree on this branch first.
    #[arg(long)]
    worktree: Option<String>,
    #[arg(long, requires = "worktree")]
    base: Option<String>,
    /// Session title. The returned id stays the stable reference.
    #[arg(long)]
    name: Option<String>,
    #[arg(long, value_enum, default_value_t = RoleArg::Generic)]
    role: RoleArg,
    /// Graph parent in the same project: a session reference, or `self`.
    #[arg(long)]
    parent: Option<String>,
}

#[derive(clap::Args)]
pub(crate) struct SpawnArgs {
    #[command(flatten)]
    placement: Placement,
    /// Inside a Forge agent session it defaults to that session's provider and profile.
    #[arg(long)]
    provider: Option<String>,
    #[arg(long)]
    profile: Option<String>,
    #[arg(long, conflicts_with = "prompt_file")]
    prompt: Option<String>,
    #[arg(long)]
    prompt_file: Option<PathBuf>,
    /// Handed-over context, appended to the prompt under a heading naming the caller.
    #[arg(long, conflicts_with = "context_file")]
    context: Option<String>,
    #[arg(long)]
    context_file: Option<PathBuf>,
    #[arg(long)]
    read_only: bool,
    #[arg(long)]
    model: Option<String>,
    #[arg(long)]
    effort: Option<String>,
    /// Provider-owned conversation id, not a Forge session id.
    #[arg(long)]
    resume: Option<String>,
}

/// An agent launched on a caller's behalf, such as a spec run's planner.
pub(crate) struct Launch {
    pub project: Option<String>,
    pub workspace: Option<String>,
    pub name: String,
    pub role: RoleArg,
    pub provider: String,
    pub effort: Option<String>,
    pub model: Option<String>,
    pub prompt: String,
}

impl From<Launch> for SpawnArgs {
    fn from(launch: Launch) -> Self {
        Self {
            placement: Placement {
                project: launch.project,
                workspace: launch.workspace,
                worktree: None,
                base: None,
                name: Some(launch.name),
                role: launch.role,
                parent: None,
            },
            provider: Some(launch.provider),
            profile: None,
            prompt: Some(launch.prompt),
            prompt_file: None,
            context: None,
            context_file: None,
            read_only: false,
            model: launch.model,
            effort: launch.effort,
            resume: None,
        }
    }
}

#[derive(clap::Args)]
pub(crate) struct ShellArgs {
    #[command(flatten)]
    placement: Placement,
}

#[derive(Clone, Copy, clap::ValueEnum)]
pub(crate) enum RoleArg {
    Generic,
    Orchestrator,
    Planner,
    Researcher,
    Executor,
    Reviewer,
    Tester,
}

impl From<RoleArg> for SessionRole {
    fn from(role: RoleArg) -> Self {
        match role {
            RoleArg::Generic => SessionRole::Generic,
            RoleArg::Orchestrator => SessionRole::Orchestrator,
            RoleArg::Planner => SessionRole::Planner,
            RoleArg::Researcher => SessionRole::Researcher,
            RoleArg::Executor => SessionRole::Executor,
            RoleArg::Reviewer => SessionRole::Reviewer,
            RoleArg::Tester => SessionRole::Tester,
        }
    }
}

#[derive(Clone, Copy, clap::ValueEnum)]
pub(crate) enum KindArg {
    Agent,
    Shell,
    Editor,
    Task,
}

impl From<KindArg> for SessionKind {
    fn from(kind: KindArg) -> Self {
        match kind {
            KindArg::Agent => SessionKind::Agent,
            KindArg::Shell => SessionKind::Shell,
            KindArg::Editor => SessionKind::Editor,
            KindArg::Task => SessionKind::Task,
        }
    }
}

#[derive(Clone, Copy, clap::ValueEnum)]
pub(crate) enum SignalArg {
    Int,
    Term,
    Hup,
    Kill,
}

impl From<SignalArg> for Signal {
    fn from(signal: SignalArg) -> Self {
        match signal {
            SignalArg::Int => Signal::SigInt,
            SignalArg::Term => Signal::SigTerm,
            SignalArg::Hup => Signal::SigHup,
            SignalArg::Kill => Signal::SigKill,
        }
    }
}

pub(crate) fn dispatch(app: &App, client: &Client, cmd: SessionCmd) -> Result<(), u8> {
    match cmd {
        SessionCmd::Spawn(args) => {
            let spawned = spawn(app, client, *args)?;
            app.print_ok(spawned);
            Ok(())
        }
        SessionCmd::Shell(args) => shell(app, client, args),
        SessionCmd::List {
            project,
            kind,
            active,
        } => {
            let store = load(app, client)?;
            let project = project
                .as_deref()
                .map(|raw| places::project(app, &store, raw).map(|p| p.id))
                .transpose()?;
            let kind = kind.map(SessionKind::from);
            let sessions: Vec<Value> = store
                .sessions
                .iter()
                .filter(|s| {
                    kind.is_none_or(|kind| s.kind == kind)
                        && (!active || s.state.is_active())
                        && project
                            .is_none_or(|id| project_for(&store, s).is_some_and(|p| p.id == id))
                })
                .map(|s| candidate(&store, s))
                .collect();
            app.print_ok(json!({ "sessions": sessions }));
            Ok(())
        }
        SessionCmd::Show { session } => {
            let store = load(app, client)?;
            let session = resolve(app, &store, &session)?;
            app.print_ok(serde_json::to_value(session).unwrap_or(Value::Null));
            Ok(())
        }
        SessionCmd::Read {
            session,
            max_bytes,
            lines,
        } => {
            let id = app.resolve_session(client, &session)?;
            let response = app.call(
                client,
                Request::GetSessionTranscript {
                    session_id: id,
                    max_lines: lines,
                    max_bytes,
                },
                false,
            )?;
            app.print_ok(flatten(&response));
            Ok(())
        }
        SessionCmd::Changes { session } => {
            let id = app.resolve_session(client, &session)?;
            let response =
                app.call(client, Request::GetSessionChanges { session_id: id }, false)?;
            app.print_ok(flatten(&response));
            Ok(())
        }
        SessionCmd::Kill { session } => {
            let id = app.resolve_session(client, &session)?;
            acked(app, client, Request::KillSession { session_id: id })
        }
        SessionCmd::Restart { session } => {
            let id = app.resolve_session(client, &session)?;
            acked(app, client, Request::RestartSession { session_id: id })
        }
        SessionCmd::Close { session } => {
            let id = app.resolve_session(client, &session)?;
            acked(app, client, Request::CloseSession { session_id: id })
        }
        SessionCmd::Signal {
            session,
            signal,
            foreground,
        } => {
            let id = app.resolve_session(client, &session)?;
            acked(
                app,
                client,
                Request::SendSignal {
                    session_id: id,
                    signal: signal.into(),
                    foreground,
                },
            )
        }
        SessionCmd::Rename { session, title } => {
            let title = title.trim();
            if title.is_empty() {
                return Err(usage_code(
                    app,
                    "title cannot be empty",
                    &["forgectl", "session", "rename", "--help"],
                ));
            }
            let id = app.resolve_session(client, &session)?;
            acked(
                app,
                client,
                Request::RenameSession {
                    session_id: id,
                    title: Some(title.into()),
                },
            )
        }
        SessionCmd::Send {
            session,
            text,
            file,
            stdin: _,
            no_submit,
            queue,
            now,
        } => {
            let text = input(app, text, file, MAX_INPUT_BYTES)?;
            let id = app.resolve_session(client, &session)?;
            let response = app.call(
                client,
                Request::SendSessionPrompt {
                    caller_session_id: env_session(),
                    session_id: id,
                    text,
                    submit: !no_submit,
                    when: if now {
                        PromptDelivery::Now
                    } else if queue {
                        PromptDelivery::Queue
                    } else {
                        PromptDelivery::IfIdle
                    },
                },
                true,
            )?;
            app.print_ok(flatten(&response));
            Ok(())
        }
        SessionCmd::Exec {
            session,
            text,
            stdin: _,
        } => {
            let command = input(app, text, None, MAX_INPUT_BYTES)?;
            let command = command
                .strip_suffix('\n')
                .map(|line| line.strip_suffix('\r').unwrap_or(line))
                .unwrap_or(&command)
                .to_owned();
            let id = app.resolve_session(client, &session)?;
            let response = app.call(
                client,
                Request::SendShellCommand {
                    caller_session_id: env_session(),
                    session_id: id,
                    command,
                },
                true,
            )?;
            app.print_ok(flatten(&response));
            Ok(())
        }
        SessionCmd::Wait {
            session,
            r#for,
            timeout,
        } => wait(app, client, &session, &r#for, timeout.as_deref()),
    }
}

fn acked(app: &App, client: &Client, request: Request) -> Result<(), u8> {
    let response = app.call(client, request, true)?;
    app.print_ok(flatten(&response));
    Ok(())
}

pub(crate) fn resolve<'a>(app: &App, store: &'a Store, raw: &str) -> Result<&'a Session, u8> {
    resolve_ref(store, raw).map_err(|candidates| {
        let ambiguous = !candidates.is_empty();
        app.fail(if ambiguous { 5 } else { 4 }, if ambiguous { "ambiguous" } else { "not_found" },
            if ambiguous { "session reference is ambiguous" } else { "session not found" },
            json!({ "candidates": candidates.iter().map(|s| candidate(store, s)).collect::<Vec<_>>() }),
            next(&["forgectl", "session", "list"]))
    })
}

fn resolve_ref<'a>(store: &'a Store, raw: &str) -> Result<&'a Session, Vec<&'a Session>> {
    if let Ok(id) = raw.parse::<SessionId>() {
        return store
            .sessions
            .iter()
            .find(|s| s.id == id)
            .ok_or_else(Vec::new);
    }
    let prefix: Vec<_> = if raw.len() >= domain::orchestration::MIN_ID_PREFIX
        && raw.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-')
    {
        store
            .sessions
            .iter()
            .filter(|s| s.id.to_string().starts_with(&raw.to_ascii_lowercase()))
            .collect()
    } else {
        vec![]
    };
    if !prefix.is_empty() {
        return sole(prefix);
    }
    let (project, title) = raw
        .split_once('/')
        .map_or((None, raw), |(p, t)| (Some(p), t));
    sole(
        store
            .sessions
            .iter()
            .filter(|s| {
                s.title.resolve("session").eq_ignore_ascii_case(title)
                    && project.is_none_or(|name| {
                        project_for(store, s).is_some_and(|p| {
                            p.name.eq_ignore_ascii_case(name) || p.id.to_string() == name
                        })
                    })
            })
            .collect(),
    )
}

fn sole(matches: Vec<&Session>) -> Result<&Session, Vec<&Session>> {
    if matches.len() == 1 {
        Ok(matches[0])
    } else {
        Err(matches)
    }
}

fn project_for<'a>(store: &'a Store, session: &Session) -> Option<&'a Project> {
    let workspace = store
        .workspaces
        .iter()
        .find(|w| w.id == session.workspace_id)?;
    store.projects.iter().find(|p| p.id == workspace.project_id)
}

/// `kind` and `state` stay display strings for existing scripts; the other fields are structured.
fn candidate(store: &Store, session: &Session) -> Value {
    let (exit_code, signal) = match session.state {
        SessionState::Exited { code, signal } => (code, signal),
        _ => (None, None),
    };
    json!({ "id": session.id, "prefix": &session.id.to_string()[..8],
        "project": project_for(store, session).map(|p| &p.name), "workspace": session.workspace_id,
        "title": session.title.resolve("session"), "provider": session.agent_provider_id,
        "kind": format!("{:?}", session.kind), "state": format!("{:?}", session.state),
        "active": session.state.is_active(), "ended": session.state.is_terminal(),
        "exit_code": exit_code, "signal": signal,
        "role": session.role, "parent": session.parent_session_id,
        "activity": session.activity.state })
}

fn input(app: &App, text: Option<String>, file: Option<PathBuf>, cap: usize) -> Result<String, u8> {
    let result = match (text, file) {
        (Some(text), None) if text.len() <= cap => Ok(text),
        (Some(_), None) => Err(format!("text exceeds {cap} bytes")),
        (None, Some(path)) if path != Path::new("-") => client::read_capped(&path, cap)
            .map_err(|e| e.to_string())
            .and_then(|read| {
                if read.truncated {
                    Err(format!("file exceeds {cap} bytes"))
                } else {
                    String::from_utf8(read.bytes).map_err(|e| e.to_string())
                }
            }),
        (None, _) => read_capped_stdin(cap),
        _ => Err("text and file are mutually exclusive".into()),
    };
    result.map_err(|message| usage_code(app, &message, &["forgectl", "session", "--help"]))
}

struct Target<'a> {
    project: &'a Project,
    /// The caller's checkout, used when neither `--project` nor `--workspace` names one.
    default_workspace: Option<WorkspaceId>,
    caller: Option<&'a Session>,
    title: Option<String>,
    role: SessionRole,
    parent: Option<SessionId>,
}

/// The session `FORGE_SESSION_ID` names; one this daemon does not know is an error, not "no caller".
fn caller<'a>(app: &App, store: &'a Store, help: &[&str]) -> Result<Option<&'a Session>, u8> {
    let Some(id) = env_session() else {
        return Ok(None);
    };
    store
        .sessions
        .iter()
        .find(|s| s.id == id)
        .map(Some)
        .ok_or_else(|| {
            usage_code(
                app,
                "FORGE_SESSION_ID names no session of this daemon",
                help,
            )
        })
}

/// Every check that needs no daemon round trip, so a refusal never leaves a worktree behind.
fn target<'a>(
    app: &App,
    store: &'a Store,
    placement: &Placement,
    help: &[&str],
) -> Result<Target<'a>, u8> {
    let title = placement.name.as_deref().map(str::trim);
    if title == Some("") {
        return Err(usage_code(app, "name cannot be empty", help));
    }
    let caller = caller(app, store, help)?;
    let (project, default_workspace) = match (&placement.project, caller) {
        (Some(raw), _) => (places::project(app, store, raw)?, None),
        (None, Some(caller)) => {
            let project = project_for(store, caller).ok_or_else(|| {
                usage_code(
                    app,
                    "the calling session has no project; pass --project",
                    help,
                )
            })?;
            (project, Some(caller.workspace_id))
        }
        (None, None) => {
            return Err(usage_code(
                app,
                "pass --project, or run inside a Forge session",
                help,
            ))
        }
    };
    if let Some(raw) = &placement.workspace {
        places::workspace(app, store, Some(project.id), Some(raw))?;
    }
    let parent = match placement.parent.as_deref() {
        None => None,
        Some("self") => Some(
            env_session()
                .ok_or_else(|| usage_code(app, "--parent self needs FORGE_SESSION_ID", help))?,
        ),
        Some(raw) => Some(resolve(app, store, raw)?.id),
    };
    Ok(Target {
        project,
        default_workspace,
        caller,
        title: title.map(str::to_owned),
        role: placement.role.into(),
        parent,
    })
}

fn checkout(
    app: &App,
    client: &Client,
    store: &Store,
    target: &Target<'_>,
    placement: &Placement,
) -> Result<(Workspace, bool), u8> {
    match &placement.worktree {
        Some(branch) => Ok((
            create_worktree(
                app,
                client,
                target.project.id,
                branch,
                placement.base.clone(),
            )?,
            true,
        )),
        None => {
            let default = target.default_workspace.map(|id| id.to_string());
            let raw = placement.workspace.as_deref().or(default.as_deref());
            Ok((
                places::workspace(app, store, Some(target.project.id), raw)?.clone(),
                false,
            ))
        }
    }
}

/// A launch that fails after creating a worktree names it, so the caller can remove it.
fn created_details(workspace: &Workspace, created: bool) -> (Value, Vec<Vec<String>>) {
    if !created {
        return (Value::Null, vec![]);
    }
    (
        json!({ "worktree_created": workspace }),
        next(&["forgectl", "workspace", "remove", &workspace.id.to_string()]),
    )
}

fn created(
    app: &App,
    client: &Client,
    request: Request,
    workspace: &Workspace,
    worktree: bool,
) -> Result<(SessionId, domain::TerminalId), u8> {
    let (extra, hint) = created_details(workspace, worktree);
    match app.call_with(client, request, true, extra.clone(), hint.clone())? {
        Response::SessionCreated {
            session_id,
            terminal_id,
        } => Ok((session_id, terminal_id)),
        _ => Err(app.fail(1, "internal", "unexpected spawn response", extra, hint)),
    }
}

pub(crate) fn spawn(app: &App, client: &Client, args: SpawnArgs) -> Result<Value, u8> {
    let help = ["forgectl", "session", "spawn", "--help"];
    let stdin = Some(Path::new("-"));
    if args.prompt_file.as_deref() == stdin && args.context_file.as_deref() == stdin {
        return Err(usage_code(
            app,
            "only one of --prompt-file and --context-file can read stdin",
            &help,
        ));
    }
    let prompt = if args.prompt.is_some() || args.prompt_file.is_some() {
        Some(input(
            app,
            args.prompt,
            args.prompt_file,
            MAX_LAUNCH_PROMPT_BYTES,
        )?)
    } else {
        None
    };
    let context = if args.context.is_some() || args.context_file.is_some() {
        Some(input(
            app,
            args.context,
            args.context_file,
            MAX_LAUNCH_PROMPT_BYTES,
        )?)
    } else {
        None
    };
    let store = load(app, client)?;
    let target = target(app, &store, &args.placement, &help)?;
    let prompt = launch_prompt(prompt, context, target.caller);
    if prompt
        .as_ref()
        .is_some_and(|p| p.len() > MAX_LAUNCH_PROMPT_BYTES)
    {
        return Err(usage_code(
            app,
            &format!("prompt and context exceed {MAX_LAUNCH_PROMPT_BYTES} bytes together"),
            &help,
        ));
    }
    let (provider, inherited) = match (&args.provider, target.caller) {
        (Some(raw), _) => (AgentProviderId::new(raw), false),
        (
            None,
            Some(Session {
                agent_provider_id: Some(provider),
                ..
            }),
        ) => (provider.clone(), true),
        _ => {
            return Err(usage_code(
                app,
                "pass --provider, or run inside a Forge agent session",
                &help,
            ))
        }
    };
    let profile = match args.profile.as_deref() {
        None if inherited => target.caller.and_then(|caller| caller.agent_profile_id),
        None => None,
        Some(raw) => {
            let profile = store
                .agent_profiles
                .iter()
                .find(|p| {
                    p.provider_id == provider
                        && (p.id.to_string() == raw || p.name.eq_ignore_ascii_case(raw))
                })
                .ok_or_else(|| {
                    app.fail(
                        4,
                        "not_found",
                        "profile not found for provider",
                        json!({}),
                        vec![],
                    )
                })?;
            Some(profile.id)
        }
    };
    let descriptor = store
        .providers
        .iter()
        .find(|p| p.descriptor.id == provider)
        .map(|p| &p.descriptor)
        .ok_or_else(|| {
            app.fail(
                4,
                "not_found",
                "provider not found",
                json!({}),
                next(&["forgectl", "providers"]),
            )
        })?;
    let unsupported = if args.read_only && !descriptor.capabilities.supports_review {
        Some("provider has no read-only launch mode")
    } else if args.resume.is_some() && !descriptor.capabilities.supports_resume {
        Some("provider cannot resume a conversation")
    } else if args.model.is_some() && descriptor.model.is_none()
        || args.effort.is_some() && descriptor.effort.is_none()
    {
        Some("provider does not support model/effort selection")
    } else {
        None
    };
    if let Some(message) = unsupported {
        return Err(app.fail(1, "invalid_request", message, json!({}), vec![]));
    }
    let (workspace, worktree) = checkout(app, client, &store, &target, &args.placement)?;
    let (session_id, terminal_id) = created(
        app,
        client,
        Request::CreateAgentSession {
            workspace_id: workspace.id,
            provider_id: provider.clone(),
            profile_id: profile,
            parent: target.parent,
            role: target.role.clone(),
            resume: args.resume,
            initial_prompt: prompt,
            read_only: args.read_only,
            model: args.model,
            effort: args.effort,
            title: target.title.clone(),
        },
        &workspace,
        worktree,
    )?;
    Ok(
        json!({ "session_id": session_id, "terminal_id": terminal_id, "project": target.project,
        "workspace": workspace_json(&store, &workspace), "title": target.title, "provider": provider,
        "profile": profile, "role": target.role, "parent": target.parent, "spawned_by": env_session() }),
    )
}

/// The context follows the task under a heading naming who handed it over.
fn launch_prompt(
    prompt: Option<String>,
    context: Option<String>,
    caller: Option<&Session>,
) -> Option<String> {
    let Some(context) = context else {
        return prompt;
    };
    let from = caller.map_or_else(
        || "the caller".to_owned(),
        |caller| {
            format!(
                "Forge session \"{}\" ({})",
                caller.title.resolve("session"),
                caller.id
            )
        },
    );
    let handover = format!("Context handed over from {from}:\n\n{context}");
    Some(match prompt {
        Some(prompt) => format!("{prompt}\n\n{handover}"),
        None => handover,
    })
}

fn shell(app: &App, client: &Client, args: ShellArgs) -> Result<(), u8> {
    let help = ["forgectl", "session", "shell", "--help"];
    let store = load(app, client)?;
    let target = target(app, &store, &args.placement, &help)?;
    let (workspace, worktree) = checkout(app, client, &store, &target, &args.placement)?;
    let (session_id, terminal_id) = created(
        app,
        client,
        Request::CreateShellSession {
            workspace_id: workspace.id,
            parent: target.parent,
            role: target.role.clone(),
            title: target.title.clone(),
        },
        &workspace,
        worktree,
    )?;
    app.print_ok(
        json!({ "session_id": session_id, "terminal_id": terminal_id, "project": target.project,
        "workspace": workspace_json(&store, &workspace), "title": target.title, "kind": "shell",
        "role": target.role, "parent": target.parent, "spawned_by": env_session() }),
    );
    Ok(())
}

fn create_worktree(
    app: &App,
    client: &Client,
    project: domain::ProjectId,
    branch: &str,
    base: Option<String>,
) -> Result<Workspace, u8> {
    let events = client.events();
    app.call(
        client,
        Request::CreateWorktree {
            project_id: project,
            branch: branch.into(),
            base,
            name: None,
        },
        true,
    )?;
    let deadline = Instant::now() + app.timeout;
    let mut found = None;
    app.await_event(&events, deadline, |event| match event {
        DaemonEvent::WorkspaceCreated(workspace)
            if workspace.project_id == project && workspace.branch.as_deref() == Some(branch) =>
        {
            found = Some(workspace.clone());
            true
        }
        _ => false,
    })?;
    found.ok_or_else(|| {
        app.fail(
            3,
            "unreachable",
            "worktree creation was acknowledged but not observed",
            json!({ "uncertain": true }),
            next(&["forgectl", "workspace", "list"]),
        )
    })
}

fn wait(
    app: &App,
    client: &Client,
    raw: &str,
    states: &[String],
    timeout: Option<&str>,
) -> Result<(), u8> {
    let help = ["forgectl", "session", "wait", "--help"];
    let mut wanted = Vec::new();
    let mut exit = false;
    for state in states {
        match state.as_str() {
            "idle" => wanted.push(ActivityState::Idle),
            "waiting" => wanted.push(ActivityState::Waiting),
            "working" => wanted.push(ActivityState::Working),
            "exit" => exit = true,
            _ => return Err(usage_code(app, "unknown activity", &help)),
        }
    }
    let deadline = Instant::now() + app.wait_budget(timeout, &help)?;
    let events = client.events();
    let store = load(app, client)?;
    let session = resolve(app, &store, raw)?;
    if !exit && session.kind != SessionKind::Agent {
        return Err(app.fail(
            5,
            "precondition_failed",
            "only agent sessions report activity; wait for exit instead",
            json!({ "session_id": session.id, "kind": session.kind }),
            vec![vec![
                "forgectl".into(),
                "session".into(),
                "wait".into(),
                session.id.to_string(),
                "--for".into(),
                "exit".into(),
            ]],
        ));
    }
    let id = session.id;
    let mut current = session.clone();
    let mut removed = false;
    for event in events.try_iter() {
        observe(id, &event, &mut current, &mut removed);
    }
    loop {
        if removed {
            return Err(app.fail(4, "not_found", "session removed", json!({}), vec![]));
        }
        if current.state.is_terminal() {
            if exit {
                app.print_ok(json!({ "session_id": id, "matched": "exit",
                    "state": current.state, "activity": current.activity }));
                return Ok(());
            }
            return Err(app.fail(
                5,
                "precondition_failed",
                "session ended before wait condition",
                json!({ "session_id": id, "state": current.state }),
                vec![],
            ));
        }
        if session.kind == SessionKind::Agent && wanted.contains(&current.activity.state) {
            app.print_ok(json!({ "session_id": id, "matched": current.activity.state,
                "state": current.state, "activity": current.activity }));
            return Ok(());
        }
        let woke = app.await_event(&events, deadline, |event| {
            observe(id, event, &mut current, &mut removed)
        })?;
        if woke == Woke::Deadline {
            return Err(app.fail(
                124,
                "timeout",
                "session wait timed out",
                json!({ "session_id": id }),
                vec![],
            ));
        }
    }
}

/// Events queued before the snapshot only count when they are newer than its activity clock.
fn observe(id: SessionId, event: &DaemonEvent, current: &mut Session, removed: &mut bool) -> bool {
    match event {
        DaemonEvent::SessionUpdated(s)
            if s.id == id && s.activity.since >= current.activity.since =>
        {
            *current = s.clone();
            true
        }
        DaemonEvent::SessionRemoved { session_id } if *session_id == id => {
            *removed = true;
            true
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::{ProjectId, WorkspaceKind};

    fn fixture() -> Store {
        let mut store = Store::new();
        let now = domain::Timestamp::now();
        for name in ["api", "web"] {
            let project = domain::Project {
                id: ProjectId::new(),
                project_group_id: None,
                name: name.into(),
                icon: None,
                root_path: format!("/dev/{name}").into(),
                git_root: None,
                created_at: now,
                last_opened_at: now,
            };
            let workspace = Workspace {
                id: WorkspaceId::new(),
                project_id: project.id,
                kind: WorkspaceKind::Main,
                path: project.root_path.clone(),
                branch: None,
                display_name: None,
                managed_by_app: false,
                created_at: now,
                status: Default::default(),
            };
            let id: SessionId = if name == "api" {
                "11111111-1111-7111-8111-111111111111"
            } else {
                "22222222-2222-7222-8222-222222222222"
            }
            .parse()
            .unwrap();
            store.sessions.push(Session {
                id,
                workspace_id: workspace.id,
                kind: domain::SessionKind::Agent,
                role: SessionRole::Generic,
                parent_session_id: None,
                root_session_id: id,
                terminal_id: None,
                editor: None,
                agent_provider_id: Some(AgentProviderId::new("claude")),
                agent_profile_id: None,
                title: domain::SessionTitle {
                    user: Some("Implement".into()),
                    ..Default::default()
                },
                state: domain::SessionState::Running,
                created_at: now,
                last_activity_at: now,
                ended_at: None,
                launch_command: None,
                base_commit: None,
                activity: domain::AgentActivity::starting(now),
                action: None,
            });
            store.projects.push(project);
            store.workspaces.push(workspace);
        }
        store
    }

    #[test]
    fn references_resolve_ids_prefixes_and_qualified_titles_or_list_ambiguity() {
        let mut store = fixture();
        let id = store.sessions[0].id;
        assert_eq!(resolve_ref(&store, &id.to_string()).unwrap().id, id);
        assert_eq!(resolve_ref(&store, &id.to_string()[..8]).unwrap().id, id);
        assert_eq!(resolve_ref(&store, "API/implement").unwrap().id, id);
        assert_eq!(resolve_ref(&store, "IMPLEMENT").unwrap_err().len(), 2);
        assert!(resolve_ref(&store, "absent").unwrap_err().is_empty());
        store.sessions[1].title.user = Some(id.to_string());
        assert_eq!(resolve_ref(&store, &id.to_string()).unwrap().id, id);
        store.sessions[1].title.user = Some("Review".into());
        assert_eq!(resolve_ref(&store, "implement").unwrap().id, id);
    }

    #[test]
    fn handed_over_context_follows_the_task_and_names_the_caller() {
        let store = fixture();
        let caller = &store.sessions[0];
        assert_eq!(
            launch_prompt(Some("Do X".into()), None, Some(caller)).as_deref(),
            Some("Do X")
        );
        let both =
            launch_prompt(Some("Do X".into()), Some("We chose Y".into()), Some(caller)).unwrap();
        assert!(both.starts_with("Do X\n\nContext handed over from Forge session \"Implement\""));
        assert!(both.ends_with(":\n\nWe chose Y"));
        let alone = launch_prompt(None, Some("We chose Y".into()), None).unwrap();
        assert_eq!(alone, "Context handed over from the caller:\n\nWe chose Y");
    }

    #[test]
    fn listing_retains_string_kind_and_state_and_adds_structured_exit() {
        let mut store = fixture();
        let running = candidate(&store, &store.sessions[0]);
        assert_eq!(running["kind"], "Agent");
        assert_eq!(running["state"], "Running");
        assert_eq!(running["active"], true);
        store.sessions[0].state = domain::SessionState::Exited {
            code: Some(3),
            signal: None,
        };
        let exited = candidate(&store, &store.sessions[0]);
        assert_eq!(exited["state"], "Exited { code: Some(3), signal: None }");
        assert_eq!(exited["exit_code"], 3);
        assert_eq!(exited["ended"], true);
        assert_eq!(exited["active"], false);
    }

    #[test]
    fn projects_match_names_case_insensitively_and_paths_exactly() {
        let store = fixture();
        let api = store.projects[0].id;
        assert_eq!(places::matching_projects(&store, "API")[0].id, api);
        assert_eq!(places::matching_projects(&store, "/dev/api")[0].id, api);
        assert_eq!(
            places::matching_projects(&store, &api.to_string())[0].id,
            api
        );
        assert!(places::matching_projects(&store, "/dev").is_empty());
    }

    #[test]
    fn spawn_shell_exec_and_send_flags_enforce_exclusive_inputs() {
        assert!(Cli::try_parse_from([
            "forgectl",
            "session",
            "spawn",
            "--project",
            "api",
            "--provider",
            "claude",
            "--workspace",
            "1234abcd",
            "--worktree",
            "feature"
        ])
        .is_err());
        assert!(Cli::try_parse_from([
            "forgectl",
            "session",
            "shell",
            "--project",
            "api",
            "--base",
            "main"
        ])
        .is_err());
        assert!(Cli::try_parse_from([
            "forgectl",
            "session",
            "shell",
            "--project",
            "api",
            "--role",
            "tester",
            "--parent",
            "self"
        ])
        .is_ok());
        assert!(Cli::try_parse_from(["forgectl", "session", "spawn", "--context", "c"]).is_ok());
        assert!(Cli::try_parse_from([
            "forgectl",
            "session",
            "spawn",
            "--context",
            "c",
            "--context-file",
            "f"
        ])
        .is_err());
        assert!(Cli::try_parse_from([
            "forgectl",
            "session",
            "wait",
            "w",
            "--for",
            "idle,waiting,exit"
        ])
        .is_ok());
        assert!(
            Cli::try_parse_from(["forgectl", "session", "wait", "w", "--for", "idle,done"])
                .is_err()
        );
        assert!(Cli::try_parse_from([
            "forgectl", "session", "send", "worker", "--text", "hello", "--file", "file"
        ])
        .is_err());
        assert!(Cli::try_parse_from([
            "forgectl", "session", "send", "worker", "--queue", "--now", "--text", "hello"
        ])
        .is_err());
        assert!(Cli::try_parse_from(["forgectl", "session", "send", "worker", "-"]).is_ok());
        assert!(Cli::try_parse_from(["forgectl", "session", "send", "worker"]).is_err());
        assert!(Cli::try_parse_from(["forgectl", "session", "exec", "sh"]).is_err());
        assert!(
            Cli::try_parse_from(["forgectl", "session", "exec", "sh", "--text", "ls", "-"])
                .is_err()
        );
        assert!(Cli::try_parse_from(["forgectl", "session", "signal", "sh"]).is_err());
        assert!(
            Cli::try_parse_from(["forgectl", "session", "signal", "sh", "--signal", "int"]).is_ok()
        );
    }
}
