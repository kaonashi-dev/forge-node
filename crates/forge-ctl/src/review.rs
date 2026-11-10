//! Forge Review from the command line, and the recall bench over known fixes.

use super::*;
use client::Store;
use domain::{ReviewId, ReviewRun, ReviewStatus, ReviewTarget, Tier, Workspace};
use protocol::RemoveProjectPolicy;
use serde::Deserialize;

#[derive(Subcommand)]
pub(crate) enum ReviewCmd {
    /// Review a workspace's changes, uncommitted ones included, against its merge base.
    Start {
        /// Workspace UUID or unique prefix. Inside a Forge session, the caller's checkout.
        #[arg(long)]
        workspace: Option<String>,
        /// Branch or commit to diff against; defaults to the repository's default branch.
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
        /// Block until the run ends and print it with its findings.
        #[arg(long)]
        wait: bool,
    },
    /// A run and its findings.
    Show { review: String },
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
    /// Review each corpus case's reintroduced bug and report how many were found.
    Bench {
        corpus: PathBuf,
        /// Defaults to `./review-bench-<unix seconds>`.
        #[arg(long)]
        out: Option<PathBuf>,
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
            base,
            tier,
            provider,
            preset,
            instructions,
            model,
            effort,
            wait,
        } => {
            let store = places::load(app, client)?;
            let workspace = target_workspace(app, &store, workspace.as_deref())?;
            let start = Request::StartReview {
                target: ReviewTarget::Workspace {
                    workspace_id: workspace.id,
                    base,
                },
                tier: tier.map(Tier::from),
                provider,
                preset,
                instructions,
                model,
                effort,
            };
            if !wait {
                let response = app.call(client, start, true)?;
                app.print_ok(flatten(&response));
                return Ok(());
            }
            let (run, findings) = run_to_end(app, client, start, Wait::Leave)?;
            print_finished(app, &run, &findings)
        }
        ReviewCmd::Show { review } => {
            let review_id = review_id(app, &review)?;
            let response = app.call(client, Request::GetReview { review_id }, false)?;
            app.print_ok(flatten(&response));
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
        ReviewCmd::Bench { corpus, out } => bench(app, client, &corpus, out),
    }
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
        Response::Review { run, findings } => Ok((run, findings)),
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

fn bench(app: &App, client: &Client, corpus: &Path, out: Option<PathBuf>) -> Result<(), u8> {
    let bad = |message: String| {
        app.fail(
            2,
            "usage",
            &message,
            json!({ "corpus": corpus }),
            next(&["forgectl", "review", "bench", "--help"]),
        )
    };
    let text = std::fs::read_to_string(corpus)
        .map_err(|error| bad(format!("cannot read the corpus: {error}")))?;
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
        let row = match bench_case(app, client, &repo, &worktree, case) {
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
