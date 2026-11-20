use super::*;
use client::Store;
use domain::orchestration::ToolSurface;
use domain::spec_run::model::{
    AgentPick, Delivery, GateDecision, GateKind, NextAction, Placement, SpecArtifactKind,
    SpecOptions, SpecRunSource, SpecRunView, SpecStage,
};
use domain::spec_run::schema;
use domain::spec_run::{
    agent_refusal, classify, compose_planner_prompt, planner_effort, Coder, SpecError, SpecRun,
    SpecSource, MAX_INLINE_SPEC_BYTES,
};
use domain::{SpecRunId, WorkspaceId};

#[derive(Subcommand)]
pub(crate) enum SpecCmd {
    /// Start a spec run from a spec: a drafter resolves its criteria, then the plan, the coder
    /// rounds and the report follow, each behind a gate the person passes.
    Run(Box<RunArgs>),
    /// Start a spec run from an idea, a GitHub issue (#123 or its link), a file or an artifact.
    Draft(Box<RunArgs>),
    /// Print the planner brief, for the calling session to act as the planner itself, without gates.
    Brief(Box<SpecArgs>),
    /// Hand over the run's spec, research, plan or early exit.
    Save(SaveArgs),
    /// Hand over the run's report.
    Report(ReportArgs),
    /// Send findings for another coder round, or accept the current task.
    Round(RoundArgs),
    /// Pass, send back or reject the gate a run waits at. A person's action, refused to the run's agents.
    Gate(Box<GateArgs>),
    /// Spec runs and the stage each one is at.
    Status(StatusArgs),
    /// One spec run with its spec, plan and report.
    Show(ShowArgs),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, clap::ValueEnum)]
pub(crate) enum Preset {
    /// A Claude coder at medium effort.
    Claude,
    /// A Cursor coder, or the one `--coder` names (for example opencode).
    Mix,
}

impl Preset {
    fn coder(self) -> (&'static str, Option<&'static str>) {
        match self {
            Preset::Claude => domain::spec_run::Preset::Claude,
            Preset::Mix => domain::spec_run::Preset::Mix,
        }
        .coder()
    }
}

#[derive(Clone, clap::Args)]
pub(crate) struct SpecArgs {
    /// A Claude artifact link, a path to a Markdown file, the full spec text, or `-` for stdin.
    spec: String,
    #[arg(long, value_enum, default_value_t = Preset::Claude)]
    preset: Preset,
    /// Coder provider; replaces the preset's provider and its effort.
    #[arg(long)]
    coder: Option<String>,
    #[arg(long)]
    coder_effort: Option<String>,
    #[arg(long)]
    coder_model: Option<String>,
    /// Profile (name or UUID) of the coder's provider; the brief names it by UUID.
    #[arg(long)]
    coder_profile: Option<String>,
    /// Put the coder in a new worktree on this branch instead of the planner's checkout.
    #[arg(long)]
    worktree: Option<String>,
    #[arg(long, requires = "worktree")]
    base: Option<String>,
    /// Review rounds before the planner hands what remains to the person.
    #[arg(long, default_value_t = 3, value_parser = clap::value_parser!(u32).range(1..=10))]
    rounds: u32,
    /// Label for the session titles.
    #[arg(long, default_value = "spec")]
    name: String,
}

#[derive(clap::Args)]
pub(crate) struct RunArgs {
    #[command(flatten)]
    spec: SpecArgs,
    /// Project for the planner. Inside a Forge session it defaults to the caller's checkout.
    #[arg(long)]
    project: Option<String>,
    #[arg(long)]
    workspace: Option<String>,
    #[arg(long, default_value = "claude")]
    planner: String,
    /// Defaults to `high` when the planner's provider offers it.
    #[arg(long)]
    planner_effort: Option<String>,
    #[arg(long)]
    planner_model: Option<String>,
    /// Profile (name or UUID) of the planner's provider.
    #[arg(long)]
    planner_profile: Option<String>,
    /// Let the coder work in this checkout instead of a new worktree; what is already modified is
    /// left out of the review.
    #[arg(long, conflicts_with = "worktree")]
    checkout: bool,
    #[arg(long, value_enum, default_value_t = DeliveryArg::NoCommit)]
    delivery: DeliveryArg,
    /// Skip the Forge Review that follows the report.
    #[arg(long)]
    no_forge_review: bool,
    /// Start a new planner session for every stage instead of prompting the same one.
    #[arg(long)]
    session_per_stage: bool,
    /// Ask the drafter to publish the spec as a Claude artifact too.
    #[arg(long)]
    publish_spec: bool,
    /// Ask the planner to publish the report as a Claude artifact too.
    #[arg(long)]
    publish_report: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub(crate) enum DeliveryArg {
    /// The coder leaves its changes uncommitted.
    NoCommit,
    /// The coder commits on its worktree branch.
    Commit,
    /// Uncommitted, and Forge opens a draft pull request after the result gate.
    DraftPr,
}

impl DeliveryArg {
    fn domain(self) -> Delivery {
        match self {
            Self::NoCommit => Delivery::NoCommit,
            Self::Commit => Delivery::Commit,
            Self::DraftPr => Delivery::DraftPr,
        }
    }
}

#[derive(Clone, Copy, clap::ValueEnum)]
pub(crate) enum KindArg {
    Spec,
    Research,
    Plan,
    Exit,
}

#[derive(clap::Args)]
pub(crate) struct SaveArgs {
    /// Spec run id or prefix; inside a run's session it defaults to that run.
    run: Option<String>,
    #[arg(long, value_enum)]
    kind: KindArg,
    #[arg(long)]
    file: PathBuf,
    #[arg(long)]
    artifact_url: Option<String>,
}

#[derive(clap::Args)]
pub(crate) struct ReportArgs {
    run: Option<String>,
    #[arg(long)]
    file: PathBuf,
    #[arg(long)]
    artifact_url: Option<String>,
}

#[derive(clap::Args)]
pub(crate) struct RoundArgs {
    run: Option<String>,
    /// Numbered findings for the next coder round.
    #[arg(long, conflicts_with = "accept")]
    file: Option<PathBuf>,
    /// Accept the current task and start the next one.
    #[arg(long, required_unless_present = "file")]
    accept: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub(crate) enum NextArg {
    Finish,
    DraftPr,
    AnotherRound,
    Discard,
}

#[derive(clap::Args)]
pub(crate) struct GateArgs {
    run: String,
    #[arg(long, group = "decision")]
    approve: bool,
    /// Send the stage back with this text for its agent.
    #[arg(long, group = "decision", value_name = "REASON")]
    changes: Option<String>,
    #[arg(long, group = "decision", value_name = "REASON")]
    reject: Option<String>,
    /// Plan gate: the coder works in this checkout instead of a worktree.
    #[arg(long, conflicts_with = "worktree")]
    checkout: bool,
    #[arg(long)]
    worktree: Option<String>,
    #[arg(long, requires = "worktree")]
    base: Option<String>,
    #[arg(long, value_enum)]
    delivery: Option<DeliveryArg>,
    #[arg(long, value_parser = clap::value_parser!(u32).range(1..=5))]
    rounds: Option<u32>,
    #[arg(long, action = clap::ArgAction::Set)]
    forge_review: Option<bool>,
    #[arg(long, action = clap::ArgAction::Set)]
    session_per_stage: Option<bool>,
    /// Plan gate: keep the approved gate as the project's gate action.
    #[arg(long)]
    save_gate_action: bool,
    /// Result gate: a manual check the person ran (repeatable).
    #[arg(long = "manual")]
    manual: Vec<String>,
    /// Result gate: what follows.
    #[arg(long, value_enum)]
    next: Option<NextArg>,
    /// Result gate: the text for another round.
    #[arg(long)]
    text: Option<String>,
}

#[derive(clap::Args)]
pub(crate) struct StatusArgs {
    run: Option<String>,
    /// Only runs that wait on a person.
    #[arg(long)]
    attention: bool,
}

#[derive(clap::Args)]
pub(crate) struct ShowArgs {
    run: String,
}

pub(crate) fn dispatch(app: &App, client: &Client, cmd: SpecCmd) -> Result<(), u8> {
    match cmd {
        SpecCmd::Run(args) => start(app, client, *args, false),
        SpecCmd::Draft(args) => start(app, client, *args, true),
        SpecCmd::Save(args) => save(app, client, args),
        SpecCmd::Report(args) => report(app, client, args),
        SpecCmd::Round(args) => round(app, client, args),
        SpecCmd::Gate(args) => gate(app, client, *args),
        SpecCmd::Status(args) => status(app, client, args),
        SpecCmd::Show(args) => show(app, client, args),
        SpecCmd::Brief(args) => {
            let store = places::load(app, client)?;
            let surface = if app.capture.is_some() {
                ToolSurface::Mcp
            } else {
                ToolSurface::Cli
            };
            let run = prepare(app, &store, &args, env_session().is_some(), surface)?;
            let brief = compose_planner_prompt(&run);
            if app.json {
                app.print_ok(json!({ "brief": brief, "coder": coder_json(&run.coder),
                    "spec": source_json(&run.source) }));
            } else {
                println!("{brief}");
            }
            Ok(())
        }
    }
}

fn prepare(
    app: &App,
    store: &Store,
    args: &SpecArgs,
    planner_is_session: bool,
    surface: ToolSurface,
) -> Result<SpecRun, u8> {
    let help = ["forgectl", "spec", "--help"];
    let source = if args.spec == "-" {
        SpecSource::Inline(
            read_capped_stdin(MAX_INLINE_SPEC_BYTES).map_err(|e| usage_code(app, &e, &help))?,
        )
    } else {
        classify(&args.spec, Path::is_file).map_err(|error| match error {
            SpecError::MissingFile(_) => {
                app.fail(4, "not_found", &error.to_string(), json!({}), next(&help))
            }
            other => usage_code(app, &other.to_string(), &help),
        })?
    };
    let source = match source {
        SpecSource::File(path) => SpecSource::File(std::fs::canonicalize(&path).unwrap_or(path)),
        SpecSource::Inline(text) if text.len() > MAX_INLINE_SPEC_BYTES => {
            return Err(usage_code(
                app,
                &format!(
                    "an inline spec is capped at {MAX_INLINE_SPEC_BYTES} bytes; save it to a file"
                ),
                &help,
            ))
        }
        other => other,
    };
    let (provider, effort) = match &args.coder {
        Some(provider) => (provider.clone(), args.coder_effort.clone()),
        None => {
            let (provider, effort) = args.preset.coder();
            (
                provider.to_owned(),
                args.coder_effort.clone().or(effort.map(str::to_owned)),
            )
        }
    };
    check_agent(
        app,
        store,
        &provider,
        effort.as_deref(),
        args.coder_model.as_deref(),
        "coder",
    )?;
    let coder_profile = args
        .coder_profile
        .as_deref()
        .map(|raw| profile(app, store, &provider, raw, "coder"))
        .transpose()?;
    let forgectl = std::env::current_exe()
        .and_then(std::fs::canonicalize)
        .unwrap_or_else(|_| PathBuf::from("forgectl"));
    Ok(SpecRun {
        source,
        coder: Coder {
            provider,
            profile: coder_profile,
            effort,
            model: args.coder_model.clone(),
        },
        forgectl,
        name: args.name.trim().to_owned(),
        worktree: args.worktree.clone(),
        base: args.base.clone(),
        rounds: args.rounds,
        planner_is_session,
        surface,
    })
}

fn provider<'a>(
    app: &App,
    store: &'a Store,
    id: &str,
    role: &str,
) -> Result<&'a domain::AgentDescriptor, u8> {
    let info = store
        .providers
        .iter()
        .find(|p| p.descriptor.id.as_str() == id)
        .ok_or_else(|| {
            app.fail(
                4,
                "not_found",
                &format!("{role} provider {id} is unknown"),
                json!({ "role": role, "provider": id }),
                next(&["forgectl", "providers"]),
            )
        })?;
    Ok(&info.descriptor)
}

/// The UUID of `raw`, a profile of `provider` named by UUID or case-insensitive name.
fn profile(app: &App, store: &Store, provider: &str, raw: &str, role: &str) -> Result<String, u8> {
    store
        .agent_profiles
        .iter()
        .find(|p| {
            p.provider_id.as_str() == provider
                && (p.id.to_string() == raw || p.name.eq_ignore_ascii_case(raw))
        })
        .map(|p| p.id.to_string())
        .ok_or_else(|| {
            app.fail(
                4,
                "not_found",
                &format!("{role} profile {raw} is not a profile of {provider}"),
                json!({ "role": role, "provider": provider, "profile": raw }),
                next(&["forgectl", "providers"]),
            )
        })
}

fn check_agent(
    app: &App,
    store: &Store,
    id: &str,
    effort: Option<&str>,
    model: Option<&str>,
    role: &str,
) -> Result<(), u8> {
    match agent_refusal(provider(app, store, id, role)?, effort, model) {
        Some(reason) => Err(app.fail(
            1,
            "invalid_request",
            &format!("{role} provider {id} {reason}"),
            json!({ "role": role, "provider": id }),
            next(&["forgectl", "providers"]),
        )),
        None => Ok(()),
    }
}

/// `#123`, `123` or an issue link; anything else is not an issue.
fn issue_ref(raw: &str) -> Option<String> {
    let raw = raw.trim();
    let digits = raw.strip_prefix('#').unwrap_or(raw);
    if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) {
        return Some(digits.to_owned());
    }
    (raw.starts_with("https://") && raw.contains("/issues/") && !raw.contains(char::is_whitespace))
        .then(|| raw.to_owned())
}

fn caller_session(store: &Store) -> Option<&domain::Session> {
    let id = env_session()?;
    store.sessions.iter().find(|s| s.id == id)
}

/// `--project`/`--workspace`, else the caller's checkout.
fn place(
    app: &App,
    store: &Store,
    project: Option<&str>,
    workspace: Option<&str>,
) -> Result<WorkspaceId, u8> {
    let help = ["forgectl", "spec", "run", "--help"];
    match (project, workspace, caller_session(store)) {
        (Some(project), workspace, _) => {
            let project = places::project(app, store, project)?;
            Ok(places::workspace(app, store, Some(project.id), workspace)?.id)
        }
        (None, Some(workspace), _) => Ok(places::workspace(app, store, None, Some(workspace))?.id),
        (None, None, Some(caller)) => Ok(caller.workspace_id),
        (None, None, None) => Err(usage_code(
            app,
            "pass --project, or run inside a Forge session",
            &help,
        )),
    }
}

fn default_branch(name: &str) -> String {
    let slug: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    let slug = if slug.is_empty() {
        "spec".to_owned()
    } else {
        slug
    };
    let unique = SpecRunId::new().to_string();
    format!("spec/{slug}-{}", &unique[unique.len() - 6..])
}

fn profile_id(raw: Option<&str>) -> Option<domain::AgentProfileId> {
    raw.and_then(|raw| raw.parse().ok())
}

fn start(app: &App, client: &Client, args: RunArgs, draft: bool) -> Result<(), u8> {
    let store = places::load(app, client)?;
    let issue = if draft {
        issue_ref(&args.spec.spec)
    } else {
        None
    };
    let spec = if issue.is_some() {
        prepare(
            app,
            &store,
            &SpecArgs {
                spec: "issue".into(),
                ..args.spec.clone()
            },
            true,
            ToolSurface::Cli,
        )?
    } else {
        prepare(app, &store, &args.spec, true, ToolSurface::Cli)?
    };
    let effort = match args.planner_effort {
        Some(effort) => Some(effort),
        None => planner_effort(provider(app, &store, &args.planner, "planner")?),
    };
    check_agent(
        app,
        &store,
        &args.planner,
        effort.as_deref(),
        args.planner_model.as_deref(),
        "planner",
    )?;
    let planner_profile = args
        .planner_profile
        .as_deref()
        .map(|raw| profile(app, &store, &args.planner, raw, "planner"))
        .transpose()?;
    let workspace_id = place(
        app,
        &store,
        args.project.as_deref(),
        args.workspace.as_deref(),
    )?;
    let source = match (issue, &spec.source) {
        (Some(reference), _) => SpecRunSource::Issue { reference },
        (None, SpecSource::Inline(text)) => SpecRunSource::Inline { text: text.clone() },
        (None, SpecSource::File(path)) => SpecRunSource::File {
            path: path.to_string_lossy().into_owned(),
        },
        (None, SpecSource::ClaudeArtifact(url)) => {
            SpecRunSource::ClaudeArtifact { url: url.clone() }
        }
        (None, SpecSource::Link(url)) => SpecRunSource::Link { url: url.clone() },
    };
    let placement = if args.checkout {
        Placement::Checkout
    } else {
        Placement::Worktree {
            branch: spec
                .worktree
                .clone()
                .unwrap_or_else(|| default_branch(&spec.name)),
            base: spec.base.clone(),
        }
    };
    let options = SpecOptions {
        placement,
        delivery: args.delivery.domain(),
        rounds: spec.rounds,
        forge_review: !args.no_forge_review,
        session_per_stage: args.session_per_stage,
        publish_spec: args.publish_spec,
        publish_report: args.publish_report,
    };
    let planner = AgentPick {
        provider: domain::AgentProviderId::new(&args.planner),
        profile: profile_id(planner_profile.as_deref()),
        effort,
        model: args.planner_model.clone(),
    };
    let coder = AgentPick {
        provider: domain::AgentProviderId::new(&spec.coder.provider),
        profile: profile_id(spec.coder.profile.as_deref()),
        effort: spec.coder.effort.clone(),
        model: spec.coder.model.clone(),
    };
    let response = app.call(
        client,
        Request::StartSpecRun {
            workspace_id,
            name: spec.name.clone(),
            source,
            planner,
            coder,
            options: options.clone(),
        },
        true,
    )?;
    let Response::SpecRunStarted {
        spec_run_id,
        session_id,
    } = response
    else {
        return Err(unexpected(app));
    };
    app.print_ok(json!({ "spec_run": spec_run_id, "drafter": session_id,
        "coder": coder_json(&spec.coder), "spec": source_json(&spec.source),
        "rounds": spec.rounds, "placement": options.placement, "delivery": options.delivery }));
    Ok(())
}

fn unexpected(app: &App) -> u8 {
    app.fail(1, "protocol", "unexpected response", json!({}), vec![])
}

/// A run named by UUID or a prefix of at least 8 characters; `None` leaves it to the caller's session.
fn run_ref(app: &App, store: &Store, raw: Option<&str>) -> Result<Option<SpecRunId>, u8> {
    let Some(raw) = raw else { return Ok(None) };
    let found: Vec<SpecRunId> = store
        .spec_runs
        .iter()
        .map(|run| run.id)
        .filter(|id| {
            let text = id.to_string();
            text == raw || (raw.len() >= 8 && text.starts_with(raw))
        })
        .collect();
    match found.as_slice() {
        [one] => Ok(Some(*one)),
        [] => Err(app.fail(
            4,
            "not_found",
            &format!("spec run {raw} is unknown"),
            json!({}),
            next(&["forgectl", "spec", "status"]),
        )),
        _ => Err(app.fail(
            5,
            "ambiguous",
            &format!("spec run {raw} is ambiguous"),
            json!({ "candidates": found }),
            next(&["forgectl", "spec", "status"]),
        )),
    }
}

fn read_text(app: &App, path: &Path, cap: usize) -> Result<String, u8> {
    let help = ["forgectl", "spec", "--help"];
    let read =
        client::read_capped(path, cap).map_err(|e| usage_code(app, &e.to_string(), &help))?;
    if read.truncated {
        return Err(usage_code(
            app,
            &format!("{} exceeds {cap} bytes", path.display()),
            &help,
        ));
    }
    String::from_utf8(read.bytes).map_err(|_| usage_code(app, "the file is not UTF-8", &help))
}

fn save(app: &App, client: &Client, args: SaveArgs) -> Result<(), u8> {
    let store = places::load(app, client)?;
    let spec_run_id = run_ref(app, &store, args.run.as_deref())?;
    let (kind, cap) = match args.kind {
        KindArg::Spec => (SpecArtifactKind::Spec, schema::MAX_SPEC_BYTES),
        KindArg::Research => (SpecArtifactKind::Research, schema::MAX_RESEARCH_BYTES),
        KindArg::Plan => (SpecArtifactKind::Plan, schema::MAX_PLAN_BYTES),
        KindArg::Exit => (SpecArtifactKind::Exit, schema::MAX_SPEC_BYTES),
    };
    let content = read_text(app, &args.file, cap)?;
    app.call(
        client,
        Request::SaveSpecArtifact {
            spec_run_id,
            caller_session_id: env_session(),
            kind,
            content,
            artifact_url: args.artifact_url,
        },
        true,
    )?;
    app.print_ok(json!({ "saved": kind }));
    Ok(())
}

fn report(app: &App, client: &Client, args: ReportArgs) -> Result<(), u8> {
    let store = places::load(app, client)?;
    let spec_run_id = run_ref(app, &store, args.run.as_deref())?;
    let content = read_text(app, &args.file, schema::MAX_REPORT_BYTES)?;
    app.call(
        client,
        Request::SubmitSpecReport {
            spec_run_id,
            caller_session_id: env_session(),
            content,
            artifact_url: args.artifact_url,
        },
        true,
    )?;
    app.print_ok(json!({ "reported": true }));
    Ok(())
}

fn round(app: &App, client: &Client, args: RoundArgs) -> Result<(), u8> {
    let store = places::load(app, client)?;
    let spec_run_id = run_ref(app, &store, args.run.as_deref())?;
    let findings = args
        .file
        .as_deref()
        .map(|file| read_text(app, file, 32 * 1024))
        .transpose()?;
    let accepted = findings.is_none();
    app.call(
        client,
        Request::SpecRound {
            spec_run_id,
            caller_session_id: env_session(),
            findings,
        },
        true,
    )?;
    app.print_ok(json!({ "accepted": accepted }));
    Ok(())
}

fn gate(app: &App, client: &Client, args: GateArgs) -> Result<(), u8> {
    let help = ["forgectl", "spec", "gate", "--help"];
    let store = places::load(app, client)?;
    let Some(id) = run_ref(app, &store, Some(&args.run))? else {
        return Err(usage_code(app, "name the spec run", &help));
    };
    let Some(run) = store.spec_runs.iter().find(|run| run.id == id) else {
        return Err(unexpected(app));
    };
    let kind = match run.stage {
        SpecStage::SpecGate => GateKind::Spec,
        SpecStage::PlanGate => GateKind::Plan,
        SpecStage::ResultGate => GateKind::Result,
        _ if args.reject.is_some() && !run.stage.is_closed() => GateKind::Result,
        stage => {
            return Err(app.fail(
                5,
                "precondition_failed",
                &format!("spec run {id} is at {}, not at a gate", stage.as_str()),
                json!({ "stage": stage }),
                next(&["forgectl", "spec", "status", &id.to_string()]),
            ))
        }
    };
    let mut options = run.options.clone();
    let mut changed = false;
    if args.checkout {
        options.placement = Placement::Checkout;
        changed = true;
    }
    if let Some(branch) = args.worktree {
        options.placement = Placement::Worktree {
            branch,
            base: args.base,
        };
        changed = true;
    }
    if let Some(delivery) = args.delivery {
        options.delivery = delivery.domain();
        changed = true;
    }
    if let Some(rounds) = args.rounds {
        options.rounds = rounds;
        changed = true;
    }
    if let Some(forge_review) = args.forge_review {
        options.forge_review = forge_review;
        changed = true;
    }
    if let Some(session_per_stage) = args.session_per_stage {
        options.session_per_stage = session_per_stage;
        changed = true;
    }
    let decision = match (args.approve, args.changes, args.reject) {
        (true, None, None) => {
            let next = match (args.next, args.text) {
                (Some(NextArg::AnotherRound), Some(text)) => {
                    Some(NextAction::AnotherRound { text })
                }
                (Some(NextArg::AnotherRound), None) => {
                    return Err(usage_code(app, "another round takes --text", &help))
                }
                (Some(NextArg::DraftPr), _) => Some(NextAction::OpenDraftPr),
                (Some(NextArg::Discard), _) => Some(NextAction::DiscardWorktree),
                (Some(NextArg::Finish), _) => Some(NextAction::Finish),
                (None, _) => None,
            };
            GateDecision::Approve {
                options: changed.then_some(options),
                save_gate_action: args.save_gate_action,
                manual_checked: args.manual,
                next,
            }
        }
        (false, Some(reason), None) => GateDecision::Changes { reason },
        (false, None, Some(reason)) => GateDecision::Reject { reason },
        _ => {
            return Err(usage_code(
                app,
                "pass one of --approve, --changes <reason> or --reject <reason>",
                &help,
            ))
        }
    };
    app.call(
        client,
        Request::SpecGate {
            spec_run_id: id,
            caller_session_id: env_session(),
            gate: kind,
            decision,
        },
        true,
    )?;
    app.print_ok(json!({ "spec_run": id, "gate": kind }));
    Ok(())
}

fn run_json(run: &SpecRunView) -> Value {
    json!({ "id": run.id, "name": run.name, "stage": run.stage, "route": run.route,
        "round": run.round, "rounds": run.options.rounds, "needs_attention": run.needs_attention(),
        "attention": run.attention, "outcome": run.outcome, "since": run.updated_at,
        "sessions": run.sessions, "coder_workspace": run.coder_workspace_id })
}

fn status(app: &App, client: &Client, args: StatusArgs) -> Result<(), u8> {
    let store = places::load(app, client)?;
    let only = run_ref(app, &store, args.run.as_deref())?;
    let runs: Vec<Value> = store
        .spec_runs
        .iter()
        .filter(|run| only.is_none_or(|id| run.id == id))
        .filter(|run| !args.attention || run.needs_attention())
        .map(run_json)
        .collect();
    app.print_ok(json!({ "spec_runs": runs }));
    Ok(())
}

fn show(app: &App, client: &Client, args: ShowArgs) -> Result<(), u8> {
    let store = places::load(app, client)?;
    let Some(spec_run_id) = run_ref(app, &store, Some(&args.run))? else {
        return Err(unexpected(app));
    };
    let Response::SpecRun(detail) = app.call(client, Request::GetSpecRun { spec_run_id }, false)?
    else {
        return Err(unexpected(app));
    };
    app.print_ok(serde_json::to_value(&*detail).unwrap_or(Value::Null));
    Ok(())
}

fn coder_json(coder: &Coder) -> Value {
    json!({ "provider": coder.provider, "profile": coder.profile, "effort": coder.effort,
        "model": coder.model })
}

fn source_json(source: &SpecSource) -> Value {
    let reference = match source {
        SpecSource::ClaudeArtifact(url) | SpecSource::Link(url) => json!(url),
        SpecSource::File(path) => json!(path),
        SpecSource::Inline(text) => json!({ "bytes": text.len() }),
    };
    json!({ "kind": source.kind(), "ref": reference })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_and_overrides_parse() {
        assert!(Cli::try_parse_from(["forgectl", "spec", "run", "spec.md"]).is_ok());
        assert!(Cli::try_parse_from([
            "forgectl", "spec", "run", "spec.md", "--preset", "mix", "--coder", "opencode"
        ])
        .is_ok());
        assert!(Cli::try_parse_from([
            "forgectl",
            "spec",
            "run",
            "spec.md",
            "--coder-profile",
            "Work",
            "--planner-profile",
            "Personal"
        ])
        .is_ok());
        assert!(
            Cli::try_parse_from([
                "forgectl",
                "spec",
                "brief",
                "spec.md",
                "--planner-profile",
                "Personal"
            ])
            .is_err(),
            "a brief launches no planner"
        );
        assert!(Cli::try_parse_from(["forgectl", "spec", "brief", "-", "--rounds", "0"]).is_err());
        assert!(Cli::try_parse_from(["forgectl", "spec", "brief", "x", "--base", "main"]).is_err());
        assert_eq!(Preset::Claude.coder(), ("claude", Some("medium")));
        assert_eq!(Preset::Mix.coder(), ("cursor", None));
    }
}
