use super::*;
use client::Store;
use domain::spec_run::{
    agent_refusal, classify, compose_planner_prompt, planner_effort, Coder, SpecError, SpecRun,
    SpecSource, MAX_INLINE_SPEC_BYTES,
};
use sessions::{Launch, RoleArg};

#[derive(Subcommand)]
pub(crate) enum SpecCmd {
    /// Launch a planner session that plans the spec, starts a coder and reviews its work.
    Run(Box<RunArgs>),
    /// Print the planner brief, for the calling session to act as the planner itself.
    Brief(Box<SpecArgs>),
}

#[derive(Clone, Copy, clap::ValueEnum)]
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

#[derive(clap::Args)]
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
}

pub(crate) fn dispatch(app: &App, client: &Client, cmd: SpecCmd) -> Result<(), u8> {
    match cmd {
        SpecCmd::Run(args) => run(app, client, *args),
        SpecCmd::Brief(args) => {
            let store = places::load(app, client)?;
            let run = prepare(app, &store, &args, env_session().is_some())?;
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

fn run(app: &App, client: &Client, args: RunArgs) -> Result<(), u8> {
    let store = places::load(app, client)?;
    let spec = prepare(app, &store, &args.spec, true)?;
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
    let planner = sessions::spawn(
        app,
        client,
        Launch {
            project: args.project,
            workspace: args.workspace,
            name: format!("{} planner", spec.name),
            role: RoleArg::Planner,
            provider: args.planner,
            effort,
            model: args.planner_model,
            prompt: compose_planner_prompt(&spec),
        }
        .into(),
    )?;
    app.print_ok(
        json!({ "planner": planner, "coder": coder_json(&spec.coder),
        "spec": source_json(&spec.source), "rounds": spec.rounds, "worktree": spec.worktree }),
    );
    Ok(())
}

fn prepare(
    app: &App,
    store: &Store,
    args: &SpecArgs,
    planner_is_session: bool,
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
    let forgectl = std::env::current_exe()
        .and_then(std::fs::canonicalize)
        .unwrap_or_else(|_| PathBuf::from("forgectl"));
    Ok(SpecRun {
        source,
        coder: Coder {
            provider,
            effort,
            model: args.coder_model.clone(),
        },
        forgectl,
        name: args.name.trim().to_owned(),
        worktree: args.worktree.clone(),
        base: args.base.clone(),
        rounds: args.rounds,
        planner_is_session,
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

fn coder_json(coder: &Coder) -> Value {
    json!({ "provider": coder.provider, "effort": coder.effort, "model": coder.model })
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
        assert!(Cli::try_parse_from(["forgectl", "spec", "brief", "-", "--rounds", "0"]).is_err());
        assert!(Cli::try_parse_from(["forgectl", "spec", "brief", "x", "--base", "main"]).is_err());
        assert_eq!(Preset::Claude.coder(), ("claude", Some("medium")));
        assert_eq!(Preset::Mix.coder(), ("cursor", None));
    }
}
