//! Forge Review from the command line, and the recall bench over known fixes.

use super::*;
use client::Store;
use domain::{ReviewId, ReviewRun, ReviewStatus, ReviewTarget, Tier, Workspace};
use protocol::RemoveProjectPolicy;
use serde::Deserialize;

#[derive(Subcommand)]
pub(crate) enum ReviewCmd {
    /// Review a workspace's changes, uncommitted ones included, against its merge
    /// base, or with `--pr` a pull request's head fetched from `origin`.
    Start {
        /// Workspace UUID or unique prefix. Inside a Forge session, the caller's checkout.
        #[arg(long)]
        workspace: Option<String>,
        /// A pull request number; the checkout stays where it is.
        #[arg(long)]
        pr: Option<u32>,
        /// Branch or commit to diff against; with `--pr`, the pull request's base
        /// branch. Defaults to the repository's default branch.
        #[arg(long)]
        base: Option<String>,
        #[arg(long, value_enum)]
        tier: Option<TierArg>,
        /// Reviewer provider; defaults to `[review] provider`.
        #[arg(long)]
        provider: Option<String>,
        /// A preset id; defaults to the repository's `default_preset`.
        #[arg(long)]
        preset: Option<String>,
        /// Added after the preset's instructions; at most 4096 bytes.
        #[arg(long)]
        instructions: Option<String>,
        /// The reviewer's model; defaults to `[review] model`, then the provider's own.
        #[arg(long)]
        model: Option<String>,
        /// The reviewer's effort level; defaults to `[review] effort`, then the provider's own.
        #[arg(long)]
        effort: Option<String>,
        /// Reviewer agents, 1 to 8; defaults to `[review] swarm_default`.
        #[arg(long, value_parser = clap::value_parser!(u8).range(1..=8))]
        swarm_size: Option<u8>,
        /// A hypothesis for one agent (`AuthBypass`, `RaceCondition`, …); repeat for more.
        #[arg(long = "hypothesis", alias = "hypotheses", value_parser = parse_hypothesis)]
        hypotheses: Vec<domain::Hypothesis>,
        /// Block until the run ends and print it with its findings.
        #[arg(long)]
        wait: bool,
    },
    /// A run and its findings, or with `--diagram` one diagram of what it reviewed.
    Show {
        review: String,
        #[arg(long, value_enum)]
        diagram: Option<DiagramArg>,
        /// `forge` is Forge's own JSON; `mermaid` its mermaid text.
        #[arg(long, value_enum, default_value = "forge", requires = "diagram")]
        format: DiagramFormatArg,
    },
    /// A workspace's runs, newest first.
    Status {
        #[arg(long)]
        workspace: Option<String>,
    },
    /// Stop a queued or running review.
    Cancel { review: String },
    /// The workspace's `.forge/review/` rules and presets.
    Rules {
        #[command(subcommand)]
        cmd: RulesCmd,
    },
    /// Latency by tier and the graph, swarm, knowledge-base and sandbox rates.
    Stats {
        /// One workspace; every one when left out.
        #[arg(long)]
        workspace: Option<String>,
    },
    /// Send a review's findings to a new writable agent and review again,
    /// until nothing is found or `--max-iters` reviews ran. Asks first.
    FixLoop {
        #[arg(long)]
        workspace: Option<String>,
        /// Defaults to the workspace's latest completed review.
        #[arg(long)]
        review: Option<String>,
        #[arg(long, default_value_t = 3, value_parser = clap::value_parser!(u32).range(1..=10))]
        max_iters: u32,
        /// Start without asking.
        #[arg(long)]
        yes: bool,
    },
    /// `.forge/review/sandbox.toml`'s targets, run under the macOS sandbox.
    Sandbox {
        #[command(subcommand)]
        cmd: SandboxCmd,
    },
    /// The project's review knowledge base.
    Kb {
        #[command(subcommand)]
        cmd: KbCmd,
    },
    /// The project's codebase graph.
    Graph {
        #[command(subcommand)]
        cmd: GraphCmd,
    },
    /// Review each corpus case's reintroduced bug and report how many were found.
    Bench {
        corpus: PathBuf,
        /// Defaults to `./review-bench-<unix seconds>`.
        #[arg(long)]
        out: Option<PathBuf>,
        /// Reviewer agents per case, 1 to 8; defaults to `[review] swarm_default`.
        #[arg(long, value_parser = clap::value_parser!(u8).range(1..=8))]
        swarm_size: Option<u8>,
    },
}

#[derive(Subcommand)]
pub(crate) enum RulesCmd {
    /// Print the rules, presets and errors; exits 1 when any file is invalid.
    Check {
        #[arg(long)]
        workspace: Option<String>,
    },
}

#[derive(Subcommand)]
pub(crate) enum SandboxCmd {
    /// Run every target (or one) and wait; exit 1 when one failed.
    Run {
        #[arg(long)]
        workspace: Option<String>,
        #[arg(long)]
        target: Option<String>,
    },
    /// A sandbox run's targets and output tails.
    Results { id: String },
    /// Let TREX run the checkout's `sandbox.toml` for its project, as it is
    /// now. Shows the file and asks first; a later change needs approving again.
    Approve {
        #[arg(long)]
        workspace: Option<String>,
        /// Approve without asking.
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Subcommand)]
pub(crate) enum KbCmd {
    /// Every file, or one with `--file`.
    Show {
        #[arg(long)]
        workspace: Option<String>,
        #[arg(long)]
        file: Option<String>,
    },
    /// Edit `architecture.md` or `patterns.json` in `$VISUAL`, `$EDITOR`, or
    /// the `forge-editor` beside this binary; saved only when it changed.
    Edit {
        #[arg(long)]
        workspace: Option<String>,
        file: String,
    },
    /// All four files as one Markdown document, to stdout or `--output`.
    Export {
        #[arg(long)]
        workspace: Option<String>,
        #[arg(long)]
        output: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
pub(crate) enum GraphCmd {
    /// Bring the graph up to date with the checkout; `--full` re-parses every file.
    Rebuild {
        #[arg(long)]
        workspace: Option<String>,
        #[arg(long)]
        full: bool,
    },
    /// Ask the graph one question. Without a daemon, reads the checkout's
    /// `.forge/review/graph/cache.bin`, refusing one older than `HEAD`.
    Query {
        #[arg(long)]
        workspace: Option<String>,
        /// Who calls the function.
        #[arg(long, group = "question")]
        callers: bool,
        /// What the function calls.
        #[arg(long, group = "question")]
        callees: bool,
        /// Functions whose callees overlap the function's.
        #[arg(long, group = "question")]
        similar: bool,
        /// The call chain from the first function to the second.
        #[arg(long, group = "question")]
        path: bool,
        /// The files the file imports.
        #[arg(long, group = "question")]
        imports: bool,
        /// The external calls the file makes.
        #[arg(long, group = "question")]
        external_calls: bool,
        /// The function (or file); two functions for `--path`.
        #[arg(required = true, num_args = 1..=2)]
        names: Vec<String>,
    },
    /// The whole graph as text, at most 2 000 nodes.
    Show {
        #[arg(long)]
        workspace: Option<String>,
        #[arg(long, value_enum, default_value = "dot")]
        format: GraphFormatArg,
    },
}

#[derive(Clone, Copy, clap::ValueEnum)]
pub(crate) enum DiagramArg {
    Sequence,
    Er,
    Class,
    Flow,
}

#[derive(Clone, Copy, clap::ValueEnum)]
pub(crate) enum DiagramFormatArg {
    Forge,
    Mermaid,
}

#[derive(Clone, Copy, clap::ValueEnum)]
pub(crate) enum GraphFormatArg {
    Dot,
    Mermaid,
}

#[derive(Clone, Copy, clap::ValueEnum)]
pub(crate) enum TierArg {
    Base,
    Plus,
    Apex,
}

impl From<TierArg> for Tier {
    fn from(tier: TierArg) -> Self {
        match tier {
            TierArg::Base => Tier::Base,
            TierArg::Plus => Tier::Plus,
            TierArg::Apex => Tier::Apex,
        }
    }
}

/// Lines a bench finding may sit from the expected one and still count.
const HIT_SLACK: u32 = 3;
const STATUS_LIMIT: u32 = 20;

pub(crate) fn dispatch(app: &App, client: &Client, cmd: ReviewCmd) -> Result<(), u8> {
    match cmd {
        ReviewCmd::Start {
            workspace,
            pr,
            base,
            tier,
            provider,
            preset,
            instructions,
            model,
            effort,
            swarm_size,
            hypotheses,
            wait,
        } => {
            let store = places::load(app, client)?;
            let workspace = target_workspace(app, &store, workspace.as_deref())?;
            let target = match pr {
                Some(number) => ReviewTarget::PullRequest {
                    workspace_id: workspace.id,
                    number,
                    base,
                },
                None => ReviewTarget::Workspace {
                    workspace_id: workspace.id,
                    base,
                },
            };
            let start = Request::StartReview {
                target,
                tier: tier.map(Tier::from),
                provider,
                preset,
                instructions,
                model,
                effort,
                swarm: domain::SwarmOptions {
                    size: swarm_size,
                    hypotheses,
                },
            };
            if !wait {
                let response = app.call(client, start, true)?;
                app.print_ok(flatten(&response));
                return Ok(());
            }
            let (run, findings) = run_to_end(app, client, start, Wait::Leave)?;
            print_finished(app, &run, &findings)
        }
        ReviewCmd::Show {
            review,
            diagram: None,
            ..
        } => {
            let review_id = review_id(app, &review)?;
            let response = app.call(client, Request::GetReview { review_id }, false)?;
            app.print_ok(flatten(&response));
            Ok(())
        }
        ReviewCmd::Show {
            review,
            diagram: Some(kind),
            format,
        } => {
            let review_id = review_id(app, &review)?;
            let kind = match kind {
                DiagramArg::Sequence => domain::DiagramKind::Sequence,
                DiagramArg::Er => domain::DiagramKind::Er,
                DiagramArg::Class => domain::DiagramKind::Class,
                DiagramArg::Flow => domain::DiagramKind::Flow,
            };
            let response =
                app.call(client, Request::GetReviewDiagram { review_id, kind }, false)?;
            let Response::ReviewDiagram(diagram) = response else {
                return Err(app.fail(
                    1,
                    "internal",
                    "GetReviewDiagram was not understood",
                    json!({}),
                    vec![],
                ));
            };
            let Some(diagram) = diagram else {
                return Err(app.fail(
                    4,
                    "not_found",
                    "the reviewed change gives no such diagram",
                    json!({ "review_id": review_id }),
                    vec![],
                ));
            };
            match format {
                DiagramFormatArg::Forge => app.print_ok(json!(diagram)),
                DiagramFormatArg::Mermaid if !app.json => println!("{}", diagram.to_mermaid()),
                DiagramFormatArg::Mermaid => {
                    app.print_ok(json!({ "mermaid": diagram.to_mermaid() }))
                }
            }
            Ok(())
        }
        ReviewCmd::Status { workspace } => {
            let store = places::load(app, client)?;
            let workspace = target_workspace(app, &store, workspace.as_deref())?;
            let response = app.call(
                client,
                Request::ListReviews {
                    workspace_id: workspace.id,
                    limit: STATUS_LIMIT,
                },
                false,
            )?;
            app.print_ok(json!({ "reviews": flatten(&response) }));
            Ok(())
        }
        ReviewCmd::Cancel { review } => {
            let review_id = review_id(app, &review)?;
            let response = app.call(client, Request::CancelReview { review_id }, true)?;
            app.print_ok(flatten(&response));
            Ok(())
        }
        ReviewCmd::Rules {
            cmd: RulesCmd::Check { workspace },
        } => {
            let store = places::load(app, client)?;
            let workspace = target_workspace(app, &store, workspace.as_deref())?;
            let response = app.call(
                client,
                Request::GetReviewConfig {
                    workspace_id: workspace.id,
                },
                false,
            )?;
            let Response::ReviewConfig(settings) = response else {
                return Err(app.fail(
                    1,
                    "internal",
                    "GetReviewConfig was not understood",
                    json!({}),
                    vec![],
                ));
            };
            if settings.errors.is_empty() {
                app.print_ok(json!(settings));
                return Ok(());
            }
            Err(app.fail(
                1,
                "review_rules_invalid",
                &settings.errors.join("; "),
                json!(settings),
                vec![],
            ))
        }
        ReviewCmd::Graph { cmd } => graph(app, client, cmd),
        ReviewCmd::Kb { cmd } => kb(app, client, cmd),
        ReviewCmd::Sandbox { cmd } => sandbox(app, client, cmd),
        ReviewCmd::Stats { workspace } => {
            let workspace_id = match workspace {
                Some(raw) => {
                    let store = places::load(app, client)?;
                    Some(target_workspace(app, &store, Some(&raw))?.id)
                }
                None => None,
            };
            let response = app.call(client, Request::GetReviewStats { workspace_id }, false)?;
            app.print_ok(flatten(&response));
            Ok(())
        }
        ReviewCmd::FixLoop {
            workspace,
            review,
            max_iters,
            yes,
        } => fix_loop(
            app,
            client,
            workspace.as_deref(),
            review.as_deref(),
            max_iters,
            yes,
        ),
        ReviewCmd::Bench {
            corpus,
            out,
            swarm_size,
        } => bench(app, client, &corpus, out, swarm_size),
    }
}

fn graph(app: &App, client: &Client, cmd: GraphCmd) -> Result<(), u8> {
    let store = places::load(app, client)?;
    let response = match cmd {
        GraphCmd::Rebuild { workspace, full } => {
            let workspace = target_workspace(app, &store, workspace.as_deref())?;
            let request = Request::RebuildGraph {
                workspace_id: workspace.id,
                full,
            };
            flatten(&app.call(client, request, true)?)
        }
        GraphCmd::Query { ref workspace, .. } => {
            let query = graph_question(app, &cmd)?;
            let workspace = target_workspace(app, &store, workspace.as_deref())?;
            let request = Request::QueryGraph {
                workspace_id: workspace.id,
                query,
            };
            json!({ "nodes": flatten(&app.call(client, request, false)?), "source": "daemon" })
        }
        GraphCmd::Show { workspace, format } => {
            let workspace = target_workspace(app, &store, workspace.as_deref())?;
            let request = Request::ExportGraph {
                workspace_id: workspace.id,
                format: match format {
                    GraphFormatArg::Dot => domain::GraphFormat::Dot,
                    GraphFormatArg::Mermaid => domain::GraphFormat::Mermaid,
                },
            };
            let text = flatten(&app.call(client, request, false)?);
            if !app.json {
                println!("{}", text.as_str().unwrap_or_default());
                return Ok(());
            }
            json!({ "text": text })
        }
    };
    app.print_ok(response);
    Ok(())
}

fn fix_loop(
    app: &App,
    client: &Client,
    workspace: Option<&str>,
    review: Option<&str>,
    max_iters: u32,
    yes: bool,
) -> Result<(), u8> {
    let review_id = match review {
        Some(raw) => review_id(app, raw)?,
        None => {
            let store = places::load(app, client)?;
            let workspace = target_workspace(app, &store, workspace)?;
            let request = Request::ListReviews {
                workspace_id: workspace.id,
                limit: STATUS_LIMIT,
            };
            let Response::Reviews(runs) = app.call(client, request, false)? else {
                return Err(app.fail(
                    1,
                    "internal",
                    "ListReviews was not understood",
                    json!({}),
                    vec![],
                ));
            };
            runs.into_iter()
                .find(|run| {
                    run.status == ReviewStatus::Completed
                        && matches!(run.target, ReviewTarget::Workspace { .. })
                })
                .map(|run| run.id)
                .ok_or_else(|| {
                    app.fail(
                        4,
                        "not_found",
                        "the workspace has no completed review",
                        json!({}),
                        next(&["forgectl", "review", "start"]),
                    )
                })?
        }
    };
    let Response::Review { findings, .. } =
        app.call(client, Request::GetReview { review_id }, false)?
    else {
        return Err(app.fail(
            1,
            "internal",
            "GetReview was not understood",
            json!({}),
            vec![],
        ));
    };
    let standing = findings
        .iter()
        .filter(|finding| finding.reaction != Some(domain::FindingReaction::Dismiss))
        .count();
    if !yes {
        use std::io::{BufRead as _, IsTerminal as _, Write as _};
        if !std::io::stdin().is_terminal() {
            return Err(usage_code(
                app,
                "the fix loop launches writable agents: confirm on a terminal or pass --yes",
                &["forgectl", "review", "fix-loop", "--yes"],
            ));
        }
        eprint!(
            "Send {standing} finding(s) to a new writable agent and review again, up to {max_iters} time(s)? [y/N] "
        );
        let _ = std::io::stderr().flush();
        let mut answer = String::new();
        let _ = std::io::stdin().lock().read_line(&mut answer);
        if !matches!(answer.trim(), "y" | "Y" | "yes") {
            return Err(app.fail(
                1,
                "declined",
                "the fix loop was not started",
                json!({}),
                vec![],
            ));
        }
    }
    let request = Request::StartFixLoop {
        review_id,
        max_iterations: Some(max_iters),
        provider: None,
        profile_id: None,
    };
    let response = app.call(client, request, true)?;
    app.print_ok(json!({ "started": flatten(&response), "review_id": review_id, "findings": standing, "max_iterations": max_iters }));
    Ok(())
}

fn approve_sandbox(
    app: &App,
    client: &Client,
    workspace: Option<&str>,
    yes: bool,
) -> Result<(), u8> {
    use sha2::Digest as _;
    use std::io::{BufRead as _, IsTerminal as _, Read as _, Write as _};

    let store = places::load(app, client)?;
    let workspace = target_workspace(app, &store, workspace)?;
    let path = workspace.path.join(runtime_sandbox::CONFIG_PATH);
    let unreadable = |message: String| app.fail(4, "not_found", &message, json!({}), vec![]);
    let size = std::fs::metadata(&path)
        .map_err(|error| unreadable(format!("{}: {error}", path.display())))?
        .len();
    if size > runtime_sandbox::MAX_CONFIG_BYTES {
        return Err(app.fail(
            1,
            "invalid_request",
            &format!(
                "{} is over {} bytes",
                runtime_sandbox::CONFIG_PATH,
                runtime_sandbox::MAX_CONFIG_BYTES
            ),
            json!({}),
            vec![],
        ));
    }
    let mut text = String::new();
    std::fs::File::open(&path)
        .and_then(|file| {
            file.take(runtime_sandbox::MAX_CONFIG_BYTES)
                .read_to_string(&mut text)
        })
        .map_err(|error| unreadable(format!("{}: {error}", path.display())))?;
    let file_hash: String = sha2::Sha256::digest(text.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    if !yes {
        if !std::io::stdin().is_terminal() {
            return Err(usage_code(
                app,
                "approving sandbox.toml lets every apex review run its commands: confirm on a terminal or pass --yes",
                &["forgectl", "review", "sandbox", "approve", "--yes"],
            ));
        }
        eprintln!("{}:\n\n{text}", path.display());
        eprint!("Run these commands in the sandbox for this project's reviews? [y/N] ");
        let _ = std::io::stderr().flush();
        let mut answer = String::new();
        let _ = std::io::stdin().lock().read_line(&mut answer);
        if !matches!(answer.trim(), "y" | "Y" | "yes") {
            return Err(app.fail(
                1,
                "declined",
                "sandbox.toml was not approved",
                json!({}),
                vec![],
            ));
        }
    }
    let request = Request::ApproveSandboxConfig {
        workspace_id: workspace.id,
        file_hash: file_hash.clone(),
    };
    app.call(client, request, true)?;
    app.print_ok(json!({ "workspace_id": workspace.id, "file_hash": file_hash }));
    Ok(())
}

fn sandbox(app: &App, client: &Client, cmd: SandboxCmd) -> Result<(), u8> {
    let run = match cmd {
        SandboxCmd::Approve { workspace, yes } => {
            return approve_sandbox(app, client, workspace.as_deref(), yes)
        }
        SandboxCmd::Results { id } => {
            match app.call(client, Request::GetSandboxRun { sandbox_run_id: id }, false)? {
                Response::SandboxRun(run) => *run,
                _ => {
                    return Err(app.fail(
                        1,
                        "internal",
                        "GetSandboxRun was not understood",
                        json!({}),
                        vec![],
                    ))
                }
            }
        }
        SandboxCmd::Run { workspace, target } => {
            let store = places::load(app, client)?;
            let workspace = target_workspace(app, &store, workspace.as_deref())?;
            let until = Instant::now() + app.review_wait;
            let events = client.events();
            let request = Request::RunSandbox {
                workspace_id: workspace.id,
                target,
            };
            let Response::SandboxStarted { sandbox_run_id } = app.call(client, request, true)?
            else {
                return Err(app.fail(
                    1,
                    "internal",
                    "RunSandbox was not understood",
                    json!({}),
                    vec![],
                ));
            };
            let mut finished = None;
            let woke = app.await_event(&events, until, |event| match event {
                DaemonEvent::SandboxFinished(run) if run.id == sandbox_run_id => {
                    finished = Some((**run).clone());
                    true
                }
                _ => false,
            })?;
            match (woke, finished) {
                (Woke::Deadline, _) | (_, None) => {
                    return Err(app.fail(
                        124,
                        "timeout",
                        "the sandbox is still running",
                        json!({ "sandbox_run_id": sandbox_run_id }),
                        next(&["forgectl", "review", "sandbox", "results", &sandbox_run_id]),
                    ));
                }
                (_, Some(run)) => run,
            }
        }
    };
    if run.failed() {
        return Err(app.fail(
            1,
            "sandbox_failed",
            run.error.as_deref().unwrap_or("a target failed"),
            json!(run),
            next(&["forgectl", "review", "sandbox", "results", &run.id]),
        ));
    }
    app.print_ok(json!(run));
    Ok(())
}

fn kb_files(
    app: &App,
    client: &Client,
    workspace_id: domain::WorkspaceId,
) -> Result<Vec<domain::KbFile>, u8> {
    match app.call(client, Request::GetKb { workspace_id }, false)? {
        Response::Kb(files) => Ok(files),
        _ => Err(app.fail(1, "internal", "GetKb was not understood", json!({}), vec![])),
    }
}

fn kb(app: &App, client: &Client, cmd: KbCmd) -> Result<(), u8> {
    let store = places::load(app, client)?;
    match cmd {
        KbCmd::Show { workspace, file } => {
            let workspace = target_workspace(app, &store, workspace.as_deref())?;
            let files = kb_files(app, client, workspace.id)?;
            match file {
                None => app.print_ok(json!({ "files": files })),
                Some(name) => {
                    let found = files.into_iter().find(|f| f.file == name).ok_or_else(|| {
                        app.fail(
                            4,
                            "not_found",
                            &format!("no knowledge-base file {name}"),
                            json!({}),
                            next(&["forgectl", "review", "kb", "show"]),
                        )
                    })?;
                    app.print_ok(json!(found));
                }
            }
            Ok(())
        }
        KbCmd::Export { workspace, output } => {
            let workspace = target_workspace(app, &store, workspace.as_deref())?;
            let response = app.call(
                client,
                Request::ExportKb {
                    workspace_id: workspace.id,
                },
                false,
            )?;
            let Response::KbExport(text) = response else {
                return Err(app.fail(
                    1,
                    "internal",
                    "ExportKb was not understood",
                    json!({}),
                    vec![],
                ));
            };
            match output {
                Some(path) => {
                    std::fs::write(&path, &text).map_err(|error| {
                        app.fail(
                            1,
                            "io",
                            &format!("cannot write {}: {error}", path.display()),
                            json!({}),
                            vec![],
                        )
                    })?;
                    app.print_ok(json!({ "output": path, "bytes": text.len() }));
                }
                None if app.json => app.print_ok(json!({ "text": text })),
                None => print!("{text}"),
            }
            Ok(())
        }
        KbCmd::Edit { workspace, file } => {
            if !["architecture.md", "patterns.json"].contains(&file.as_str()) {
                return Err(usage_code(
                    app,
                    &format!("{file} is derived from feedback; only architecture.md and patterns.json can be edited"),
                    &["forgectl", "review", "kb", "show"],
                ));
            }
            let workspace = target_workspace(app, &store, workspace.as_deref())?;
            let current = kb_files(app, client, workspace.id)?
                .into_iter()
                .find(|f| f.file == file)
                .map(|f| f.content)
                .unwrap_or_default();
            let edited = edit_in_editor(&file, &current).map_err(|error| {
                app.fail(1, "editor_failed", &error, json!({ "file": file }), vec![])
            })?;
            if edited == current {
                app.print_ok(json!({ "file": file, "changed": false }));
                return Ok(());
            }
            let request = Request::UpdateKbEntry {
                workspace_id: workspace.id,
                file: file.clone(),
                content: edited,
            };
            app.call(client, request, true)?;
            app.print_ok(json!({ "file": file, "changed": true }));
            Ok(())
        }
    }
}

const MAX_EDITED_BYTES: usize = 256 * 1024;

/// `text` in a private temp file, opened in the person's editor; what it holds after.
fn edit_in_editor(name: &str, text: &str) -> Result<String, String> {
    let set = |name: &str| {
        std::env::var(name)
            .ok()
            .filter(|value| !value.trim().is_empty())
    };
    let editor = set("VISUAL")
        .or_else(|| set("EDITOR"))
        .or_else(|| {
            let sibling = std::env::current_exe().ok()?.with_file_name("forge-editor");
            sibling
                .exists()
                .then(|| sibling.to_string_lossy().into_owned())
        })
        .ok_or_else(|| "set $VISUAL or $EDITOR".to_owned())?;
    let dir = tempfile_dir()?;
    let path = dir.join(name.replace('/', "_"));
    std::fs::write(&path, text).map_err(|error| error.to_string())?;
    let status = std::process::Command::new("/bin/sh")
        .arg("-c")
        .arg(format!("{editor} \"$1\""))
        .arg("forge-kb-edit")
        .arg(&path)
        .status()
        .map_err(|error| format!("cannot start {editor}: {error}"))?;
    let read = client::read_capped(&path, MAX_EDITED_BYTES).map_err(|error| error.to_string());
    let _ = std::fs::remove_dir_all(&dir);
    if !status.success() {
        return Err(format!("{editor} exited with {status}"));
    }
    let read = read?;
    if read.truncated {
        return Err(format!("{name} is over {} KiB", MAX_EDITED_BYTES / 1024));
    }
    String::from_utf8(read.bytes).map_err(|error| error.to_string())
}

fn tempfile_dir() -> Result<PathBuf, String> {
    use std::os::unix::fs::DirBuilderExt;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let dir = std::env::temp_dir().join(format!("forge-kb-{}-{stamp}", std::process::id()));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&dir)
        .map_err(|error| error.to_string())?;
    Ok(dir)
}

fn graph_question(app: &App, cmd: &GraphCmd) -> Result<domain::GraphQuery, u8> {
    let GraphCmd::Query {
        callers,
        callees,
        similar,
        path,
        imports,
        external_calls,
        names,
        ..
    } = cmd
    else {
        return Err(usage_code(
            app,
            "not a query",
            &["forgectl", "review", "graph", "query", "--help"],
        ));
    };
    let first = names.first().cloned().unwrap_or_default();
    let usage = |message: &str| {
        usage_code(
            app,
            message,
            &["forgectl", "review", "graph", "query", "--help"],
        )
    };
    if *path {
        return match names.as_slice() {
            [from, to] => Ok(domain::GraphQuery::Path(from.clone(), to.clone())),
            _ => Err(usage("--path takes two functions")),
        };
    }
    if names.len() != 1 {
        return Err(usage("the question takes one name"));
    }
    Ok(if *callers {
        domain::GraphQuery::Callers(first)
    } else if *callees {
        domain::GraphQuery::Callees(first)
    } else if *similar {
        domain::GraphQuery::Similar(first)
    } else if *imports {
        domain::GraphQuery::Imports(first)
    } else if *external_calls {
        domain::GraphQuery::ExternalCalls(first)
    } else {
        return Err(usage(
            "say which question: --callers, --callees, --similar, --path, --imports or --external-calls",
        ));
    })
}

const MAX_CACHE_BYTES: usize = 512 * 1024 * 1024;

/// `review graph query` with no daemon: the checkout's own `cache.bin`, as of its `HEAD`.
pub(crate) fn offline_graph_query(app: &App, cmd: &GraphCmd) -> Result<(), u8> {
    let query = graph_question(app, cmd)?;
    let fail = |code: u8, kind: &str, message: &str| {
        app.fail(
            code,
            kind,
            message,
            json!({}),
            next(&["forgectl", "review", "graph", "rebuild"]),
        )
    };
    let here = std::env::var_os("FORGE_WORKSPACE")
        .map(PathBuf::from)
        .or_else(|| std::env::current_dir().ok())
        .ok_or_else(|| {
            fail(
                3,
                "unreachable",
                "the daemon is unreachable and there is no checkout",
            )
        })?;
    let top = git(&here, &["rev-parse", "--show-toplevel"]).map_err(|error| {
        fail(
            3,
            "unreachable",
            &format!("the daemon is unreachable and {error}"),
        )
    })?;
    let top = PathBuf::from(top.trim());
    let head = git(&top, &["rev-parse", "HEAD"]).map_err(|error| fail(1, "no_graph", &error))?;
    let read = client::read_capped(&top.join(".forge/review/graph/cache.bin"), MAX_CACHE_BYTES)
        .map_err(|error| {
            fail(
                1,
                "no_graph",
                &format!("the daemon is unreachable and cache.bin cannot be read: {error}"),
            )
        })?;
    if read.truncated {
        return Err(fail(1, "no_graph", "cache.bin is over its budget"));
    }
    let graph = codebase_graph::Graph::decode(&read.bytes)
        .map_err(|error| fail(1, "no_graph", &format!("cache.bin is not a graph: {error}")))?;
    if graph.git_oid != head.trim() {
        return Err(app.fail(
            1,
            "stale_graph",
            "cache.bin was built for another HEAD; rebuild it with the daemon running",
            json!({ "cache": graph.git_oid, "head": head.trim() }),
            next(&["forgectl", "review", "graph", "rebuild"]),
        ));
    }
    let nodes: Vec<domain::GraphNode> = graph
        .index()
        .query(&query)
        .into_iter()
        .map(codebase_graph::Node::to_wire)
        .collect();
    app.print_ok(json!({ "nodes": nodes, "source": "cache" }));
    Ok(())
}

fn review_id(app: &App, raw: &str) -> Result<ReviewId, u8> {
    raw.parse().map_err(|_| {
        usage_code(
            app,
            "a review is named by its full id",
            &["forgectl", "review", "status"],
        )
    })
}

/// `--workspace`, else the checkout `FORGE_WORKSPACE` names.
fn target_workspace<'a>(
    app: &App,
    store: &'a Store,
    raw: Option<&str>,
) -> Result<&'a Workspace, u8> {
    if raw.is_some() {
        return places::workspace(app, store, None, raw);
    }
    let here = std::env::var_os("FORGE_WORKSPACE")
        .map(PathBuf::from)
        .ok_or_else(|| {
            usage_code(
                app,
                "review needs --workspace outside a Forge session",
                &["forgectl", "workspace", "list"],
            )
        })?;
    workspace_at(store, &here).ok_or_else(|| {
        app.fail(
            4,
            "not_found",
            "FORGE_WORKSPACE is not a known checkout",
            json!({ "path": here }),
            next(&["forgectl", "workspace", "list"]),
        )
    })
}

fn workspace_at<'a>(store: &'a Store, path: &Path) -> Option<&'a Workspace> {
    let canonical = std::fs::canonicalize(path).ok();
    store.workspaces.iter().find(|w| {
        w.path == path || canonical.is_some() && std::fs::canonicalize(&w.path).ok() == canonical
    })
}

/// What a run still in flight at the deadline is left as.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Wait {
    Leave,
    /// The bench removes the case's project next, so the run must not outlive it.
    Cancel,
}

/// Start a review and block until it ends; events are subscribed before the
/// request so the final update cannot slip between the two.
fn run_to_end(
    app: &App,
    client: &Client,
    start: Request,
    on_deadline: Wait,
) -> Result<(ReviewRun, Vec<domain::Finding>), u8> {
    let until = Instant::now() + app.review_wait;
    let events = client.events();
    let response = app.call(client, start, true)?;
    let Response::ReviewStarted { review_id } = response else {
        return Err(app.fail(
            1,
            "internal",
            "StartReview was not understood",
            json!({}),
            vec![],
        ));
    };
    let woke = app.await_event(&events, until, |event| {
        matches!(event, DaemonEvent::ReviewUpdated { run } if run.id == review_id && run.status.is_finished())
    })?;
    if woke == Woke::Deadline {
        if on_deadline == Wait::Cancel {
            let _ = app.call(client, Request::CancelReview { review_id }, true);
        }
        return Err(app.fail(
            124,
            "timeout",
            "the review is still running",
            json!({ "review_id": review_id }),
            next(&["forgectl", "review", "show", &review_id.to_string()]),
        ));
    }
    match app.call(client, Request::GetReview { review_id }, false)? {
        Response::Review { run, findings, .. } => Ok((*run, findings)),
        _ => Err(app.fail(
            1,
            "internal",
            "GetReview was not understood",
            json!({}),
            vec![],
        )),
    }
}

fn print_finished(app: &App, run: &ReviewRun, findings: &[domain::Finding]) -> Result<(), u8> {
    if run.status == ReviewStatus::Completed {
        app.print_ok(json!({ "run": run, "findings": findings }));
        return Ok(());
    }
    Err(app.fail(
        1,
        "review_failed",
        run.error
            .as_deref()
            .unwrap_or("the review did not complete"),
        json!({ "run": run }),
        next(&["forgectl", "review", "show", &run.id.to_string()]),
    ))
}

#[derive(Deserialize)]
struct Corpus {
    repo: Option<PathBuf>,
    #[serde(rename = "case", default)]
    cases: Vec<Case>,
}

#[derive(Deserialize)]
struct Case {
    id: String,
    commit: String,
    path: String,
    line: u32,
    #[serde(default)]
    note: String,
}

const MAX_CORPUS_BYTES: usize = 1024 * 1024;

fn parse_hypothesis(name: &str) -> Result<domain::Hypothesis, String> {
    domain::Hypothesis::from_name(name).ok_or_else(|| {
        let names: Vec<&str> = domain::Hypothesis::ALL.iter().map(|h| h.name()).collect();
        format!("unknown hypothesis `{name}`; one of {}", names.join(", "))
    })
}

fn bench(
    app: &App,
    client: &Client,
    corpus: &Path,
    out: Option<PathBuf>,
    swarm_size: Option<u8>,
) -> Result<(), u8> {
    let bad = |message: String| {
        app.fail(
            2,
            "usage",
            &message,
            json!({ "corpus": corpus }),
            next(&["forgectl", "review", "bench", "--help"]),
        )
    };
    let read = client::read_capped(corpus, MAX_CORPUS_BYTES)
        .map_err(|error| bad(format!("cannot read the corpus: {error}")))?;
    if read.truncated {
        return Err(bad(format!(
            "the corpus is over {} KiB",
            MAX_CORPUS_BYTES / 1024
        )));
    }
    let text = String::from_utf8(read.bytes)
        .map_err(|error| bad(format!("the corpus is not UTF-8: {error}")))?;
    let parsed: Corpus =
        toml::from_str(&text).map_err(|error| bad(format!("invalid corpus: {error}")))?;
    let corpus_dir = corpus
        .parent()
        .filter(|dir| !dir.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let repo = match parsed.repo {
        Some(repo) => corpus_dir.join(repo),
        None => git(corpus_dir, &["rev-parse", "--show-toplevel"])
            .map(|top| PathBuf::from(top.trim()))
            .map_err(|error| {
                bad(format!(
                    "the corpus is not inside a git repository: {error}"
                ))
            })?,
    };
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let out = out.unwrap_or_else(|| PathBuf::from(format!("review-bench-{stamp}")));
    let scratch = std::env::temp_dir().join(format!("forge-review-bench-{stamp}"));

    let mut rows = Vec::new();
    for case in &parsed.cases {
        let started = Instant::now();
        let worktree = scratch.join(&case.id);
        let row = match bench_case(app, client, &repo, &worktree, case, swarm_size) {
            Ok((run, findings)) => {
                let hit = findings.iter().any(|f| {
                    f.path == case.path
                        && f.line.is_some_and(|l| l.abs_diff(case.line) <= HIT_SLACK)
                });
                json!({
                    "id": case.id, "hit": hit, "status": run.status, "findings": findings.len(),
                    "score": run.score, "error": run.error, "review_id": run.id,
                })
            }
            Err(error) => json!({
                "id": case.id, "hit": false, "status": "Error", "findings": 0,
                "score": null, "error": error, "review_id": null,
            }),
        };
        let _ = git(
            &repo,
            &["worktree", "remove", "--force", &worktree.to_string_lossy()],
        );
        let mut row = row;
        row["note"] = json!(case.note);
        row["duration_secs"] = json!(started.elapsed().as_secs_f64());
        rows.push(row);
    }
    let _ = std::fs::remove_dir_all(&scratch);

    let hits = rows.iter().filter(|row| row["hit"] == true).count();
    #[allow(clippy::cast_precision_loss, reason = "a corpus has tens of cases")]
    let recall = if rows.is_empty() {
        0.0
    } else {
        hits as f64 / rows.len() as f64
    };
    let report = json!({ "cases": rows, "total": rows.len(), "hits": hits, "recall": recall });
    let mut markdown = format!(
        "# Forge Review bench\n\nRecall: {hits}/{} ({:.0}%)\n\n| Case | Hit | Status | Findings | Score | Seconds |\n|---|---|---|---|---|---|\n",
        rows.len(),
        recall * 100.0
    );
    for row in &rows {
        markdown.push_str(&format!(
            "| {} | {} | {} | {} | {} | {:.0} |\n",
            row["id"].as_str().unwrap_or(""),
            if row["hit"] == true { "yes" } else { "no" },
            row["status"].as_str().unwrap_or(""),
            row["findings"],
            row["score"],
            row["duration_secs"].as_f64().unwrap_or(0.0),
        ));
    }
    std::fs::create_dir_all(&out)
        .and_then(|()| {
            std::fs::write(
                out.join("report.json"),
                serde_json::to_string_pretty(&report).unwrap_or_default(),
            )
        })
        .and_then(|()| std::fs::write(out.join("report.md"), markdown))
        .map_err(|error| {
            app.fail(
                1,
                "io",
                &format!("cannot write the report: {error}"),
                json!({ "out": out }),
                vec![],
            )
        })?;
    app.print_ok(json!({ "out": out, "total": rows.len(), "hits": hits, "recall": recall }));
    Ok(())
}

/// Check out the fix's commit, reverse-apply the fix as an uncommitted change,
/// and review it in a project of its own.
fn bench_case(
    app: &App,
    client: &Client,
    repo: &Path,
    worktree: &Path,
    case: &Case,
    swarm_size: Option<u8>,
) -> Result<(ReviewRun, Vec<domain::Finding>), String> {
    let path = worktree.to_string_lossy();
    git(repo, &["worktree", "add", "--detach", &path, &case.commit])?;
    let parent = format!("{}^", case.commit);
    let fix = git(worktree, &["diff", &parent, &case.commit, "--", &case.path])?;
    let patch = worktree.join(".forge-bench.patch");
    std::fs::write(&patch, fix).map_err(|e| e.to_string())?;
    let applied = git(worktree, &["apply", "-R", &patch.to_string_lossy()]);
    let _ = std::fs::remove_file(&patch);
    applied?;

    app.call(
        client,
        Request::AddProject {
            path: worktree.to_path_buf(),
        },
        true,
    )
    .map_err(|_| "AddProject failed".to_owned())?;
    let store = places::load(app, client).map_err(|_| "cannot load the store".to_owned())?;
    let workspace = workspace_at(&store, worktree)
        .ok_or_else(|| "the worktree was not added".to_owned())?
        .clone();
    let result = run_to_end(
        app,
        client,
        Request::StartReview {
            target: ReviewTarget::Workspace {
                workspace_id: workspace.id,
                base: Some("HEAD".into()),
            },
            tier: None,
            provider: None,
            preset: None,
            instructions: None,
            model: None,
            effort: None,
            swarm: domain::SwarmOptions {
                size: swarm_size,
                hypotheses: Vec::new(),
            },
        },
        Wait::Cancel,
    )
    .map_err(|code| format!("review did not finish (exit {code})"));
    let _ = app.call(
        client,
        Request::RemoveProject {
            project_id: workspace.project_id,
            policy: RemoveProjectPolicy::KillSessionsKeepWorktrees,
        },
        true,
    );
    result
}

fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = git_service::run_git(Some(dir), args).map_err(|error| error.to_string())?;
    if !out.success() {
        return Err(format!("git {}: {}", args.join(" "), out.stderr.trim()));
    }
    if out.truncated {
        return Err(format!(
            "git {}: output over the capture budget",
            args.join(" ")
        ));
    }
    Ok(out.stdout)
}
