//! Forge Review end to end: a real daemon, a seeded repository and a fake
//! `claude` that writes a transcript where the real one would and then waits,
//! the way an interactive reviewer sits idle after answering.

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use domain::{
    ActivityState, ReviewId, ReviewRun, ReviewStage, ReviewStatus, ReviewTarget, Severity, Verdict,
    WorkspaceId,
};
use protocol::{DaemonEvent, ErrorCode, Request, Response};

/// The daemon finds transcripts through its own `CLAUDE_CONFIG_DIR`, a
/// process-wide variable, so every test in this binary shares one store; the
/// session ids Forge mints keep their files apart.
fn transcript_store() -> &'static Path {
    static STORE: OnceLock<PathBuf> = OnceLock::new();
    STORE.get_or_init(|| {
        let dir = tempfile::tempdir_in("/tmp").expect("store").keep();
        std::env::set_var("CLAUDE_CONFIG_DIR", &dir);
        dir
    })
}

#[derive(Clone, Copy)]
enum Reviewer {
    /// Writes the answer, then blocks like an interactive session.
    Answers,
    /// Writes the answer and exits.
    AnswersAndExits,
    /// Never writes anything.
    Silent,
}

/// Install a fake `claude` that writes `answer` as its last assistant message
/// to `<store>/projects/<slug>/<--session-id>.jsonl`, and records its args.
fn install_reviewer(harness: &common::Harness, answer: &str, mode: Reviewer) {
    let store = transcript_store();
    let record = serde_json::json!({
        "type": "assistant",
        "message": {"role": "assistant", "content": [{"type": "text", "text": answer}]},
    })
    .to_string();
    let answer_file = harness.root().join("answer.jsonl");
    std::fs::write(&answer_file, format!("{record}\n")).expect("answer");
    let write = match mode {
        Reviewer::Silent => String::new(),
        _ => format!(
            "/bin/cat {answer} > \"$dir/$id.jsonl\"\n",
            answer = common::sh_quote(&answer_file.to_string_lossy())
        ),
    };
    let tail = match mode {
        Reviewer::AnswersAndExits => "exit 0\n",
        _ => "exec /bin/cat\n",
    };
    let script = format!(
        "#!/bin/sh\n\
         if [ \"$1\" = --version ]; then echo '2.1.0 (Claude Code)'; exit 0; fi\n\
         id=''\n\
         prev=''\n\
         for a in \"$@\"; do [ \"$prev\" = --session-id ] && id=$a; prev=$a; done\n\
         printf '%s\\n' \"$*\" > {args}\n\
         slug=$(printf '%s' \"$FORGE_WORKSPACE\" | /usr/bin/tr '/.' '--')\n\
         dir={store}/projects/$slug\n\
         /bin/mkdir -p \"$dir\"\n\
         {write}\
         /usr/bin/touch {written}\n\
         {tail}",
        args = common::sh_quote(&harness.root().join("args").to_string_lossy()),
        store = common::sh_quote(&store.to_string_lossy()),
        written = common::sh_quote(&harness.root().join("written").to_string_lossy()),
    );
    common::write_executable(harness.bin(), "claude", &script);
}

/// Retries while the daemon's own `git status` holds `index.lock`.
fn git(repo: &Path, args: &[&str]) {
    let mut last = None;
    for _ in 0..50 {
        let out = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .expect("git");
        if out.status.success() {
            return;
        }
        if !String::from_utf8_lossy(&out.stderr).contains("index.lock") {
            panic!("git {args:?}: {out:?}");
        }
        last = Some(out);
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    panic!("git {args:?} kept finding index.lock: {last:?}");
}

fn porcelain(repo: &Path) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["status", "--porcelain"])
        .output()
        .expect("git status");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// `main` sits far below `total`, so it is outside the hunk's context lines.
fn source(op: &str) -> String {
    let filler: String = (1..=20).map(|n| format!("// filler {n}\n")).collect();
    format!(
        "fn total(price: u32, qty: u32) -> u32 {{\n    price {op} qty\n}}\n{filler}\nfn main() {{\n    println!(\"{{}}\", total(2, 3));\n}}\n"
    )
}

/// A repository whose `main.rs` was committed and then broken in the working tree.
fn seeded_repo() -> test_support::TempRepo {
    let repo = test_support::init_repo().expect("repo");
    std::fs::write(repo.path().join("main.rs"), source("*")).expect("write");
    git(repo.path(), &["add", "main.rs"]);
    git(repo.path(), &["commit", "-qm", "add total"]);
    std::fs::write(repo.path().join("main.rs"), source("+")).expect("write");
    repo
}

fn answer(findings: &[serde_json::Value]) -> String {
    let body = serde_json::json!({
        "schema": "forge.review/v1",
        "summary": "Changes `total`.",
        "findings": findings,
    });
    format!("Here is the review:\n```json\n{body}\n```\n")
}

fn finding(line: u32, severity: &str, category: &str, evidence: &str) -> serde_json::Value {
    serde_json::json!({
        "path": "main.rs", "line": line, "end_line": null, "side": "new",
        "severity": severity, "category": category, "rule_id": null,
        "confidence": 0.9, "title": "t", "body": "b", "evidence": evidence,
        "suggestion": null,
    })
}

fn workspace_at(client: &client::Client, repo: &Path) -> WorkspaceId {
    let canonical = std::fs::canonicalize(repo).expect("canonical");
    let response = client
        .request(Request::AddProject {
            path: repo.to_path_buf(),
        })
        .expect("AddProject");
    assert_eq!(response, Response::Ack);
    common::workspaces(client)
        .into_iter()
        .find(|w| std::fs::canonicalize(&w.path).ok().as_deref() == Some(canonical.as_path()))
        .expect("the repository's workspace")
        .id
}

fn start(
    client: &client::Client,
    workspace_id: WorkspaceId,
    provider: Option<&str>,
) -> Result<ReviewId, ErrorCode> {
    match client.request(Request::StartReview {
        target: ReviewTarget::Workspace {
            workspace_id,
            base: None,
        },
        tier: None,
        provider: provider.map(str::to_owned),
        preset: None,
        instructions: None,
    }) {
        Ok(Response::ReviewStarted { review_id }) => Ok(review_id),
        Ok(other) => panic!("expected ReviewStarted, got {other:?}"),
        Err(client::ClientError::Protocol(error)) => Err(error.code),
        Err(other) => panic!("transport: {other}"),
    }
}

fn get(client: &client::Client, review_id: ReviewId) -> (ReviewRun, Vec<domain::Finding>) {
    match client.request(Request::GetReview { review_id }) {
        Ok(Response::Review { run, findings }) => (run, findings),
        other => panic!("expected Review, got {other:?}"),
    }
}

fn wait_run(
    events: &flume::Receiver<DaemonEvent>,
    review_id: ReviewId,
    mut pred: impl FnMut(&ReviewRun) -> bool,
) -> ReviewRun {
    let event = common::wait_for(events, common::DEADLINE, |event| {
        matches!(event, DaemonEvent::ReviewUpdated { run } if run.id == review_id && pred(run))
    })
    .expect("the review never reached the expected state");
    let DaemonEvent::ReviewUpdated { run } = event else {
        unreachable!()
    };
    run
}

fn boot(harness: &common::Harness) -> common::TestDaemon {
    transcript_store();
    let daemon = harness.boot();
    let client = daemon.connect("scenario-review");
    common::wait_for_detection(&client, |providers| {
        providers
            .iter()
            .any(|p| p.descriptor.id.as_str() == "claude" && p.detection.status.is_installed())
    });
    daemon
}

fn report_idle(client: &client::Client, session_id: domain::SessionId) {
    client
        .request(Request::ReportAgentActivity {
            session_id,
            state: ActivityState::Idle,
            source: "test".into(),
        })
        .expect("ReportAgentActivity");
}

fn pack_dirs(harness: &common::Harness) -> usize {
    std::fs::read_dir(harness.root().join("review-packs"))
        .map(|dir| dir.count())
        .unwrap_or(0)
}

#[test]
fn a_review_completes_with_anchored_findings_and_leaves_the_tree_alone() {
    let harness = common::Harness::new();
    install_reviewer(
        &harness,
        &answer(&[
            finding(2, "P1", "logic", "price + qty"),
            finding(26, "P2", "performance", "println!"),
            finding(2, "P0", "logic", "not in the file"),
        ]),
        Reviewer::Answers,
    );
    let repo = seeded_repo();
    let before = porcelain(repo.path());
    let daemon = boot(&harness);
    let client = daemon.connect("scenario-review");
    let events = client.events();
    let workspace = workspace_at(&client, repo.path());

    let review_id = start(&client, workspace, None).expect("StartReview");
    let mut stages = Vec::new();
    let model = wait_run(&events, review_id, |run| {
        if let Some(stage) = run.stage {
            if stages.last() != Some(&stage) {
                stages.push(stage);
            }
        }
        run.stage == Some(ReviewStage::Model) && run.session_id.is_some()
    });
    assert_eq!(model.status, ReviewStatus::Running);
    assert!(common::poll_until(common::DEADLINE, || harness
        .root()
        .join("written")
        .exists()));
    let args = std::fs::read_to_string(harness.root().join("args")).expect("args");
    assert!(
        args.contains("--permission-mode plan --session-id "),
        "{args}"
    );
    assert!(args.contains("--add-dir "), "{args}");
    assert_eq!(
        pack_dirs(&harness),
        1,
        "the pack exists while the model runs"
    );

    report_idle(&client, model.session_id.expect("reviewer"));
    let done = wait_run(&events, review_id, |run| {
        if let Some(stage) = run.stage {
            if stages.last() != Some(&stage) {
                stages.push(stage);
            }
        }
        run.status.is_finished()
    });
    assert_eq!(
        stages,
        [
            ReviewStage::Diff,
            ReviewStage::Pack,
            ReviewStage::Model,
            ReviewStage::Validate
        ]
    );
    assert_eq!(done.status, ReviewStatus::Completed, "{:?}", done.error);
    assert_eq!(done.verdict, Some(Verdict::Review));
    assert_eq!(done.score, Some(3.5));
    assert!(done.finished_at.is_some());
    assert_eq!(done.coverage.included, ["main.rs"]);

    let (run, findings) = get(&client, review_id);
    assert_eq!(run, done);
    assert_eq!(findings.len(), 2, "the finding without evidence is dropped");
    let anchored = findings.iter().find(|f| f.category == "logic").unwrap();
    assert_eq!((anchored.line, anchored.severity), (Some(2), Severity::P1));
    let general = findings
        .iter()
        .find(|f| f.category == "performance")
        .unwrap();
    assert_eq!(general.line, None, "line 26 is outside every hunk");

    let Response::Reviews(listed) = client
        .request(Request::ListReviews {
            workspace_id: workspace,
            limit: 10,
        })
        .expect("ListReviews")
    else {
        panic!("expected Reviews");
    };
    assert_eq!(listed, [done]);
    assert_eq!(porcelain(repo.path()), before);
    assert!(common::poll_until(common::DEADLINE, || pack_dirs(&harness) == 0));
    let reviewer = model.session_id.expect("reviewer");
    assert!(
        common::poll_until(common::DEADLINE, || {
            common::session(&client, reviewer).is_some_and(|s| !s.state.is_active())
        }),
        "a finished review stops its reviewer"
    );
}

#[test]
fn an_idle_reviewer_without_a_review_fails_without_waiting_out_the_budget() {
    let harness = common::Harness::new();
    install_reviewer(&harness, "I looked and it is fine.", Reviewer::Answers);
    let repo = seeded_repo();
    let daemon = boot(&harness);
    let client = daemon.connect("scenario-review");
    let events = client.events();
    let workspace = workspace_at(&client, repo.path());

    let review_id = start(&client, workspace, None).expect("StartReview");
    let model = wait_run(&events, review_id, |run| run.session_id.is_some());
    assert!(common::poll_until(common::DEADLINE, || harness
        .root()
        .join("written")
        .exists()));
    report_idle(&client, model.session_id.expect("reviewer"));
    let failed = wait_run(&events, review_id, |run| run.status.is_finished());
    assert_eq!(failed.status, ReviewStatus::Failed);
    assert!(
        failed
            .error
            .as_deref()
            .is_some_and(|e| e.contains("stopped without a valid review")),
        "{:?}",
        failed.error
    );
}

fn start_against(
    client: &client::Client,
    workspace_id: WorkspaceId,
    base: &str,
) -> Result<ReviewId, ErrorCode> {
    match client.request(Request::StartReview {
        target: ReviewTarget::Workspace {
            workspace_id,
            base: Some(base.to_owned()),
        },
        tier: None,
        provider: None,
        preset: None,
        instructions: None,
    }) {
        Ok(Response::ReviewStarted { review_id }) => Ok(review_id),
        Ok(other) => panic!("expected ReviewStarted, got {other:?}"),
        Err(client::ClientError::Protocol(error)) => Err(error.code),
        Err(other) => panic!("transport: {other}"),
    }
}

#[test]
fn a_base_is_checked_up_front_and_another_base_replaces_the_run() {
    let harness = common::Harness::new();
    install_reviewer(&harness, "", Reviewer::Silent);
    let repo = seeded_repo();
    let daemon = boot(&harness);
    let client = daemon.connect("scenario-review");
    let events = client.events();
    let workspace = workspace_at(&client, repo.path());

    for base in ["mian", "--output=x"] {
        assert_eq!(
            start_against(&client, workspace, base),
            Err(ErrorCode::InvalidRequest),
            "{base}"
        );
    }

    let first = start(&client, workspace, None).expect("default base");
    let other = start_against(&client, workspace, "HEAD").expect("HEAD");
    assert_ne!(other, first, "another base is another review");
    let old = wait_run(&events, first, |run| run.status.is_finished());
    assert_eq!(old.status, ReviewStatus::Cancelled);
    assert_eq!(start_against(&client, workspace, "HEAD"), Ok(other));
    client
        .request(Request::CancelReview { review_id: other })
        .expect("cancel");
}

#[test]
fn reviews_coalesce_cap_at_two_and_a_newer_head_wins() {
    let harness = common::Harness::new();
    install_reviewer(&harness, "", Reviewer::Silent);
    let repos = [seeded_repo(), seeded_repo(), seeded_repo()];
    let daemon = boot(&harness);
    let client = daemon.connect("scenario-review");
    let events = client.events();
    let ws: Vec<WorkspaceId> = repos
        .iter()
        .map(|repo| workspace_at(&client, repo.path()))
        .collect();

    let first = start(&client, ws[0], None).expect("first");
    assert_eq!(
        start(&client, ws[0], None),
        Ok(first),
        "same head coalesces"
    );
    let second = start(&client, ws[1], None).expect("second");
    assert_eq!(start(&client, ws[2], None), Err(ErrorCode::Conflict));

    git(repos[0].path(), &["commit", "-qam", "move head"]);
    let newer = start(&client, ws[0], None).expect("newer head");
    assert_ne!(newer, first);
    let old = wait_run(&events, first, |run| run.status.is_finished());
    assert_eq!(old.status, ReviewStatus::Cancelled);

    for id in [newer, second] {
        client
            .request(Request::CancelReview { review_id: id })
            .expect("cancel");
    }
}

#[test]
fn refusals_are_errors_and_failures_are_runs() {
    let harness = common::Harness::new();
    install_reviewer(
        &harness,
        "I looked and it is fine.",
        Reviewer::AnswersAndExits,
    );
    let repo = seeded_repo();
    let daemon = boot(&harness);
    let client = daemon.connect("scenario-review");
    let events = client.events();
    let workspace = workspace_at(&client, repo.path());

    assert_eq!(
        start(&client, workspace, Some("codex")),
        Err(ErrorCode::InvalidRequest)
    );
    assert_eq!(
        start(&client, WorkspaceId::new(), None),
        Err(ErrorCode::NotFound)
    );

    let review_id = start(&client, workspace, None).expect("StartReview");
    let failed = wait_run(&events, review_id, |run| run.status.is_finished());
    assert_eq!(failed.status, ReviewStatus::Failed);
    assert!(
        failed
            .error
            .as_deref()
            .is_some_and(|e| e.contains("without a valid review")),
        "{:?}",
        failed.error
    );
}

#[test]
fn mostly_invalid_anchors_fail_the_run() {
    let harness = common::Harness::new();
    install_reviewer(
        &harness,
        &answer(&[
            finding(26, "P2", "a", "println!"),
            finding(25, "P2", "b", "fn main"),
            finding(2, "P1", "c", "price + qty"),
        ]),
        Reviewer::AnswersAndExits,
    );
    let repo = seeded_repo();
    let daemon = boot(&harness);
    let client = daemon.connect("scenario-review");
    let events = client.events();
    let workspace = workspace_at(&client, repo.path());

    let review_id = start(&client, workspace, None).expect("StartReview");
    let failed = wait_run(&events, review_id, |run| run.status.is_finished());
    assert_eq!(failed.status, ReviewStatus::Failed);
    assert!(
        failed
            .error
            .as_deref()
            .is_some_and(|e| e.contains("anchor")),
        "{:?}",
        failed.error
    );
}

#[test]
fn cancel_stops_the_reviewer() {
    let harness = common::Harness::new();
    install_reviewer(&harness, "", Reviewer::Silent);
    let repo = seeded_repo();
    let daemon = boot(&harness);
    let client = daemon.connect("scenario-review");
    let events = client.events();
    let workspace = workspace_at(&client, repo.path());

    let review_id = start(&client, workspace, None).expect("StartReview");
    let running = wait_run(&events, review_id, |run| run.session_id.is_some());
    assert_eq!(
        client
            .request(Request::CancelReview { review_id })
            .expect("CancelReview"),
        Response::Ack
    );
    let cancelled = wait_run(&events, review_id, |run| run.status.is_finished());
    assert_eq!(cancelled.status, ReviewStatus::Cancelled);
    let session_id = running.session_id.expect("reviewer");
    assert!(common::poll_until(common::DEADLINE, || {
        common::session(&client, session_id).is_some_and(|s| !s.state.is_active())
    }));
}

#[test]
fn a_reviewer_that_never_answers_times_out() {
    let harness = common::Harness::new();
    install_reviewer(&harness, "", Reviewer::Silent);
    let repo = seeded_repo();
    transcript_store();
    let daemon = harness.boot_with(|config| config.review.timeout_secs = 1);
    let client = daemon.connect("scenario-review");
    common::wait_for_detection(&client, |providers| {
        providers
            .iter()
            .any(|p| p.descriptor.id.as_str() == "claude" && p.detection.status.is_installed())
    });
    let events = client.events();
    let workspace = workspace_at(&client, repo.path());

    let review_id = start(&client, workspace, None).expect("StartReview");
    let failed = wait_run(&events, review_id, |run| run.status.is_finished());
    assert_eq!(failed.status, ReviewStatus::Failed);
    assert_eq!(failed.error.as_deref(), Some("timed out"));
}

#[test]
fn a_run_in_flight_is_interrupted_by_a_restart() {
    let harness = common::Harness::new();
    install_reviewer(&harness, "", Reviewer::Silent);
    let repo = seeded_repo();
    let first = boot(&harness);
    let client = first.connect("scenario-review");
    let events = client.events();
    let workspace = workspace_at(&client, repo.path());
    let review_id = start(&client, workspace, None).expect("StartReview");
    wait_run(&events, review_id, |run| run.session_id.is_some());
    drop(client);
    first.crash();

    let second = harness.boot();
    let client = second.connect("scenario-review");
    let (run, _) = get(&client, review_id);
    assert_eq!(run.status, ReviewStatus::Interrupted);
    assert!(run.finished_at.is_some());
    first.kill_leftover_sessions();
}

fn start_with(
    client: &client::Client,
    workspace_id: WorkspaceId,
    preset: Option<&str>,
    instructions: Option<String>,
) -> Result<ReviewId, ErrorCode> {
    match client.request(Request::StartReview {
        target: ReviewTarget::Workspace {
            workspace_id,
            base: None,
        },
        tier: None,
        provider: None,
        preset: preset.map(str::to_owned),
        instructions,
    }) {
        Ok(Response::ReviewStarted { review_id }) => Ok(review_id),
        Ok(other) => panic!("expected ReviewStarted, got {other:?}"),
        Err(client::ClientError::Protocol(error)) => Err(error.code),
        Err(other) => panic!("transport: {other}"),
    }
}

const MONEY_RULE: &str = "---
id: money
title: Totals never add a price to a quantity
severity: P1
category: logic
scope: [\"**/*.rs\"]
---
MONEY_RULE_BODY: a total multiplies price by quantity.
";

const DOCS_RULE: &str = "---
id: docs-only
title: Docs stay short
severity: P2
category: docs
scope: [\"docs/**\"]
---
DOCS_RULE_BODY
";

/// `.forge/review/` with an in-scope rule, an out-of-scope one, a broken one,
/// a repository preset that replaces a built-in and the project context.
fn seed_review_dir(repo: &Path) {
    let dir = repo.join(".forge/review");
    std::fs::create_dir_all(dir.join("rules")).expect("rules");
    std::fs::create_dir_all(dir.join("presets")).expect("presets");
    std::fs::write(dir.join("rules/money.md"), MONEY_RULE).expect("money");
    std::fs::write(dir.join("rules/docs.md"), DOCS_RULE).expect("docs");
    std::fs::write(dir.join("rules/broken.md"), "no frontmatter here\n").expect("broken");
    std::fs::write(
        dir.join("presets/negocio.toml"),
        "name = \"Nuestro negocio\"\n",
    )
    .expect("negocio");
    std::fs::write(dir.join("presets/propio.toml"), "rules = [\"money\"]\n").expect("propio");
    std::fs::write(
        dir.join("context.md"),
        "CONTEXT_MARKER: prices are in cents.\n",
    )
    .expect("context");
    // Committed, so the reviewed diff is `main.rs` alone and the rule bodies
    // reach the pack only through <rules>.
    git(repo, &["add", ".forge"]);
    git(repo, &["commit", "-qm", "review rules"]);
}

fn ruled_finding(rule_id: &str, severity: &str) -> serde_json::Value {
    let mut value = finding(2, severity, "logic", "price + qty");
    value["rule_id"] = serde_json::Value::String(rule_id.to_owned());
    value
}

#[test]
fn a_rule_backed_review_applies_scope_context_and_ceilings() {
    let harness = common::Harness::new();
    let mut ghost = ruled_finding("ghost", "P2");
    ghost["category"] = "security".into();
    install_reviewer(
        &harness,
        &answer(&[ruled_finding("money", "P0"), ghost]),
        Reviewer::Answers,
    );
    let repo = seeded_repo();
    seed_review_dir(repo.path());
    let daemon = boot(&harness);
    let client = daemon.connect("scenario-review");
    let events = client.events();
    let workspace = workspace_at(&client, repo.path());

    let review_id = start(&client, workspace, None).expect("StartReview");
    let model = wait_run(&events, review_id, |run| {
        run.session_id.is_some() || run.status.is_finished()
    });
    assert!(model.session_id.is_some(), "{:?}", model.error);
    let packs = harness.root().join("review-packs");
    let pack = std::fs::read_to_string(packs.join(review_id.to_string()).join("pack.md"))
        .expect("pack.md while the model runs");
    assert!(pack.contains("MONEY_RULE_BODY"), "{pack}");
    assert!(pack.contains("CONTEXT_MARKER"), "{pack}");
    assert!(!pack.contains("DOCS_RULE_BODY"), "{pack}");

    assert!(common::poll_until(common::DEADLINE, || harness
        .root()
        .join("written")
        .exists()));
    report_idle(&client, model.session_id.expect("reviewer"));
    let done = wait_run(&events, review_id, |run| run.status.is_finished());
    assert_eq!(done.status, ReviewStatus::Completed, "{:?}", done.error);
    assert_eq!(done.preset, "estandar");
    assert_eq!(done.coverage.rules, ["money"]);
    assert!(
        done.coverage
            .notes
            .iter()
            .any(|note| note.contains("broken.md")),
        "{:?}",
        done.coverage.notes
    );

    let (_, findings) = get(&client, review_id);
    let money = findings
        .iter()
        .find(|f| f.rule_id.as_deref() == Some("money"))
        .expect("the money finding");
    assert_eq!(
        money.severity,
        Severity::P1,
        "lowered to the rule's ceiling"
    );
    let ghost = findings
        .iter()
        .find(|f| f.category == "security")
        .expect("the ghost finding");
    assert_eq!(ghost.rule_id, None);
}

#[test]
fn presets_and_instructions_are_checked_and_change_the_run() {
    let harness = common::Harness::new();
    install_reviewer(&harness, &answer(&[]), Reviewer::AnswersAndExits);
    let repo = seeded_repo();
    let daemon = boot(&harness);
    let client = daemon.connect("scenario-review");
    let events = client.events();
    let workspace = workspace_at(&client, repo.path());

    assert_eq!(
        start_with(&client, workspace, Some("nope"), None),
        Err(ErrorCode::InvalidRequest)
    );
    assert_eq!(
        start_with(&client, workspace, None, Some("x".repeat(4097))),
        Err(ErrorCode::InvalidRequest)
    );

    let review_id = start_with(&client, workspace, Some("seguridad"), None).expect("seguridad");
    let done = wait_run(&events, review_id, |run| run.status.is_finished());
    assert_eq!(done.status, ReviewStatus::Completed, "{:?}", done.error);
    assert_eq!(done.preset, "seguridad");
}

#[test]
fn another_preset_on_the_same_head_supersedes_the_running_review() {
    let harness = common::Harness::new();
    install_reviewer(&harness, "", Reviewer::Silent);
    let repo = seeded_repo();
    let daemon = boot(&harness);
    let client = daemon.connect("scenario-review");
    let events = client.events();
    let workspace = workspace_at(&client, repo.path());

    let first = start_with(&client, workspace, None, None).expect("first");
    assert_eq!(start_with(&client, workspace, None, None), Ok(first));
    let second = start_with(&client, workspace, Some("profundo"), None).expect("second");
    assert_ne!(second, first);
    let old = wait_run(&events, first, |run| run.status.is_finished());
    assert_eq!(old.status, ReviewStatus::Cancelled);
    client
        .request(Request::CancelReview { review_id: second })
        .expect("cancel");
}

#[test]
fn get_review_config_lists_presets_rules_and_errors() {
    let harness = common::Harness::new();
    install_reviewer(&harness, "", Reviewer::Silent);
    let repo = seeded_repo();
    seed_review_dir(repo.path());
    std::fs::write(
        repo.path().join(".forge/review/config.toml"),
        "default_preset = \"propio\"\n",
    )
    .expect("config");
    let daemon = boot(&harness);
    let client = daemon.connect("scenario-review");
    let workspace = workspace_at(&client, repo.path());

    let Response::ReviewConfig(settings) = client
        .request(Request::GetReviewConfig {
            workspace_id: workspace,
        })
        .expect("GetReviewConfig")
    else {
        panic!("expected ReviewConfig");
    };
    let ids: Vec<&str> = settings.presets.iter().map(|p| p.id.as_str()).collect();
    assert_eq!(
        ids,
        ["estandar", "seguridad", "negocio", "profundo", "propio"]
    );
    assert_eq!(settings.presets[2].name, "Nuestro negocio");
    assert_eq!(settings.default_preset, "propio");
    let rules: Vec<&str> = settings.rules.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(rules, ["docs-only", "money"]);
    assert_eq!(settings.rules[1].severity, Severity::P1);
    assert_eq!(settings.errors.len(), 1, "{:?}", settings.errors);
    assert!(settings.errors[0].starts_with("broken.md: "));

    match client.request(Request::GetReviewConfig {
        workspace_id: WorkspaceId::new(),
    }) {
        Err(client::ClientError::Protocol(error)) => assert_eq!(error.code, ErrorCode::NotFound),
        other => panic!("expected NotFound, got {other:?}"),
    }
}
