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
         slug=$(printf '%s' \"$FORGE_WORKSPACE\" | /usr/bin/tr -c 'A-Za-z0-9' '-')\n\
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
        model: None,
        effort: None,
    }) {
        Ok(Response::ReviewStarted { review_id }) => Ok(review_id),
        Ok(other) => panic!("expected ReviewStarted, got {other:?}"),
        Err(client::ClientError::Protocol(error)) => Err(error.code),
        Err(other) => panic!("transport: {other}"),
    }
}

fn get(client: &client::Client, review_id: ReviewId) -> (ReviewRun, Vec<domain::Finding>) {
    match client.request(Request::GetReview { review_id }) {
        Ok(Response::Review { run, findings, .. }) => (run, findings),
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
    assert!(
        !args.contains("--model") && !args.contains("--effort"),
        "unset leaves the provider's own default: {args}"
    );
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
        model: None,
        effort: None,
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
    let restarted = start(&client, ws[1], None).expect("a cancelled run must not be joined");
    assert_ne!(restarted, second);
    client
        .request(Request::CancelReview {
            review_id: restarted,
        })
        .expect("cancel");
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
        start(&client, workspace, Some("opencode")),
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
        model: None,
        effort: None,
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
        dir.join("presets/security.toml"),
        "name = \"Nuestra seguridad\"\n",
    )
    .expect("security");
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
    assert_eq!(done.preset, "standard");
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

    assert_eq!(
        start_with(&client, workspace, Some("custom"), Some("  ".into())),
        Err(ErrorCode::InvalidRequest),
        "custom brings no wording of its own"
    );

    let review_id = start_with(&client, workspace, Some("security"), None).expect("security");
    let done = wait_run(&events, review_id, |run| run.status.is_finished());
    assert_eq!(done.status, ReviewStatus::Completed, "{:?}", done.error);
    assert_eq!(done.preset, "security");
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
    let second = start_with(&client, workspace, Some("deep"), None).expect("second");
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
    assert_eq!(ids, ["standard", "deep", "security", "custom", "propio"]);
    assert_eq!(settings.presets[2].name, "Nuestra seguridad");
    assert_eq!(
        settings.presets[1].detail,
        "Reads the surrounding code too — design, edge cases, cost."
    );
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

fn start_launching(
    client: &client::Client,
    workspace_id: WorkspaceId,
    model: Option<&str>,
    effort: Option<&str>,
) -> Result<ReviewId, ErrorCode> {
    match client.request(Request::StartReview {
        target: ReviewTarget::Workspace {
            workspace_id,
            base: None,
        },
        tier: None,
        provider: None,
        preset: None,
        instructions: None,
        model: model.map(str::to_owned),
        effort: effort.map(str::to_owned),
    }) {
        Ok(Response::ReviewStarted { review_id }) => Ok(review_id),
        Ok(other) => panic!("expected ReviewStarted, got {other:?}"),
        Err(client::ClientError::Protocol(error)) => Err(error.code),
        Err(other) => panic!("transport: {other}"),
    }
}

fn boot_configured(
    harness: &common::Harness,
    configure: impl FnOnce(&mut daemon::config::Config),
) -> common::TestDaemon {
    transcript_store();
    let daemon = harness.boot_with(configure);
    let client = daemon.connect("scenario-review");
    common::wait_for_detection(&client, |providers| {
        providers
            .iter()
            .any(|p| p.descriptor.id.as_str() == "claude" && p.detection.status.is_installed())
    });
    daemon
}

/// The fake's recorded argv once the reviewer of `review_id` has started.
fn reviewer_args(
    harness: &common::Harness,
    events: &flume::Receiver<DaemonEvent>,
    review_id: ReviewId,
) -> (ReviewRun, String) {
    let run = wait_run(events, review_id, |run| {
        run.session_id.is_some() || run.status.is_finished()
    });
    assert!(run.session_id.is_some(), "{:?}", run.error);
    assert!(common::poll_until(common::DEADLINE, || harness
        .root()
        .join("written")
        .exists()));
    let args = std::fs::read_to_string(harness.root().join("args")).expect("args");
    let _ = std::fs::remove_file(harness.root().join("written"));
    (run, args)
}

#[test]
fn the_reviewer_takes_model_and_effort_from_the_config_then_the_run() {
    let harness = common::Harness::new();
    install_reviewer(&harness, "", Reviewer::Silent);
    let repo = seeded_repo();
    let daemon = boot_configured(&harness, |config| {
        config.review.model = Some("opus".into());
        config.review.effort = Some("high".into());
    });
    let client = daemon.connect("scenario-review");
    let events = client.events();
    let workspace = workspace_at(&client, repo.path());

    let first = start_launching(&client, workspace, None, None).expect("configured");
    let (run, args) = reviewer_args(&harness, &events, first);
    assert!(args.contains("--model opus"), "{args}");
    assert!(args.contains("--effort high"), "{args}");
    assert!(
        run.coverage
            .notes
            .iter()
            .any(|note| note == "reviewer: claude, model opus, effort high"),
        "{:?}",
        run.coverage.notes
    );
    assert_eq!(
        start_launching(&client, workspace, None, None),
        Ok(first),
        "the same launch joins"
    );

    let second = start_launching(&client, workspace, Some("sonnet"), Some("low"))
        .expect("the run's own options");
    assert_ne!(second, first, "another model supersedes");
    let old = wait_run(&events, first, |run| run.status.is_finished());
    assert_eq!(old.status, ReviewStatus::Cancelled);
    let (_, args) = reviewer_args(&harness, &events, second);
    assert!(args.contains("--model sonnet"), "{args}");
    assert!(args.contains("--effort low"), "{args}");
    client
        .request(Request::CancelReview { review_id: second })
        .expect("cancel");
}

#[test]
fn an_effort_the_provider_does_not_list_is_refused_before_any_run() {
    let harness = common::Harness::new();
    install_reviewer(&harness, "", Reviewer::Silent);
    let repo = seeded_repo();
    let daemon = boot_configured(&harness, |config| {
        config.review.effort = Some("turbo".into());
    });
    let client = daemon.connect("scenario-review");
    let workspace = workspace_at(&client, repo.path());

    assert_eq!(
        start_launching(&client, workspace, None, None),
        Err(ErrorCode::InvalidRequest),
        "from the config"
    );
    match client.request(Request::StartReview {
        target: ReviewTarget::Workspace {
            workspace_id: workspace,
            base: None,
        },
        tier: None,
        provider: None,
        preset: None,
        instructions: None,
        model: None,
        effort: Some("turbo".into()),
    }) {
        Err(client::ClientError::Protocol(error)) => {
            assert_eq!(error.code, ErrorCode::InvalidRequest);
            assert!(error.message.contains("effort"), "{}", error.message);
        }
        other => panic!("expected InvalidRequest, got {other:?}"),
    }
    let Response::Reviews(runs) = client
        .request(Request::ListReviews {
            workspace_id: workspace,
            limit: 10,
        })
        .expect("ListReviews")
    else {
        panic!("expected Reviews");
    };
    assert!(runs.is_empty(), "{runs:?}");
}

#[test]
fn global_and_repository_instructions_reach_the_reviewer_in_order() {
    let harness = common::Harness::new();
    install_reviewer(&harness, "", Reviewer::Silent);
    let repo = seeded_repo();
    std::fs::create_dir_all(repo.path().join(".forge/review")).expect("dir");
    std::fs::write(
        repo.path().join(".forge/review/prompt.md"),
        "REPO_PROMPT_MARKER\n",
    )
    .expect("prompt.md");
    git(repo.path(), &["add", ".forge"]);
    git(repo.path(), &["commit", "-qm", "review prompt"]);
    let daemon = boot_configured(&harness, |config| {
        config.review.instructions = "GLOBAL_PROMPT_MARKER".into();
    });
    let client = daemon.connect("scenario-review");
    let events = client.events();
    let workspace = workspace_at(&client, repo.path());

    let review_id = start(&client, workspace, None).expect("StartReview");
    let (_, args) = reviewer_args(&harness, &events, review_id);
    let section = args
        .find("## Additional instructions")
        .expect("one instructions section");
    let global = args.find("GLOBAL_PROMPT_MARKER").expect("global");
    let local = args.find("REPO_PROMPT_MARKER").expect("repository");
    let preset = args.find("Review the change the way").expect("preset");
    assert!(
        section < global && global < local && local < preset,
        "{args}"
    );
    client
        .request(Request::CancelReview { review_id })
        .expect("cancel");
}

#[test]
fn oversized_instructions_are_reported_and_left_out() {
    let harness = common::Harness::new();
    install_reviewer(&harness, "", Reviewer::Silent);
    let repo = seeded_repo();
    std::fs::create_dir_all(repo.path().join(".forge/review")).expect("dir");
    let big = format!("OVERSIZED_MARKER\n{}", "x".repeat(16 * 1024));
    std::fs::write(repo.path().join(".forge/review/prompt.md"), big).expect("prompt.md");
    let daemon = boot_configured(&harness, |config| {
        config.review.instructions = format!("GLOBAL_OVERSIZED{}", "y".repeat(4096));
    });
    let client = daemon.connect("scenario-review");
    let events = client.events();
    let workspace = workspace_at(&client, repo.path());

    let Response::ReviewConfig(settings) = client
        .request(Request::GetReviewConfig {
            workspace_id: workspace,
        })
        .expect("GetReviewConfig")
    else {
        panic!("expected ReviewConfig");
    };
    assert_eq!(settings.errors.len(), 2, "{:?}", settings.errors);
    assert!(
        settings.errors.iter().any(|e| e.contains("prompt.md")),
        "{:?}",
        settings.errors
    );
    assert!(
        settings
            .errors
            .iter()
            .any(|e| e.contains("[review] instructions")),
        "{:?}",
        settings.errors
    );

    let review_id = start(&client, workspace, None).expect("StartReview");
    let (run, args) = reviewer_args(&harness, &events, review_id);
    assert!(!args.contains("OVERSIZED_MARKER"), "{args}");
    assert!(!args.contains("GLOBAL_OVERSIZED"), "{args}");
    assert!(
        run.coverage.notes.iter().any(|n| n.contains("prompt.md")),
        "{:?}",
        run.coverage.notes
    );
    client
        .request(Request::CancelReview { review_id })
        .expect("cancel");
}

#[test]
fn agents_md_is_project_context_unless_the_repository_opts_out() {
    let harness = common::Harness::new();
    install_reviewer(&harness, "", Reviewer::Silent);
    let repo = seeded_repo();
    std::fs::write(repo.path().join("AGENTS.md"), "AGENTS_MARKER\n").expect("AGENTS.md");
    git(repo.path(), &["add", "AGENTS.md"]);
    git(repo.path(), &["commit", "-qm", "agents"]);
    let daemon = boot(&harness);
    let client = daemon.connect("scenario-review");
    let events = client.events();
    let workspace = workspace_at(&client, repo.path());
    let pack = |review_id: ReviewId| {
        reviewer_args(&harness, &events, review_id);
        std::fs::read_to_string(
            harness
                .root()
                .join("review-packs")
                .join(review_id.to_string())
                .join("pack.md"),
        )
        .expect("pack.md while the model runs")
    };

    let config = |text: &str| {
        std::fs::create_dir_all(repo.path().join(".forge/review")).expect("dir");
        std::fs::write(repo.path().join(".forge/review/config.toml"), text).expect("config");
    };
    let context_blocks = |text: &str| text.matches("<project_context path=\"AGENTS.md\">").count();
    let finish = |review_id: ReviewId| {
        client
            .request(Request::CancelReview { review_id })
            .expect("cancel");
        wait_run(&events, review_id, |run| run.status.is_finished());
    };

    let first = start(&client, workspace, None).expect("default");
    let text = pack(first);
    assert_eq!(context_blocks(&text), 1, "{text}");
    assert!(text.contains("AGENTS_MARKER"), "{text}");
    assert!(!text.contains("CLAUDE.md"), "an absent file is skipped");
    finish(first);

    config("[context]\ninclude = [\"AGENTS.md\"]\n");
    let second = start(&client, workspace, None).expect("listed again");
    let text = pack(second);
    assert_eq!(
        context_blocks(&text),
        1,
        "listed again in `include`, read once: {text}"
    );
    finish(second);

    config("[context]\nproject_docs = false\n");
    let third = start(&client, workspace, None).expect("opted out");
    let text = pack(third);
    assert!(!text.contains("AGENTS_MARKER"), "{text}");
    finish(third);
}

/// A run from start to `Completed`, the reviewer answering whatever
/// `install_reviewer` last wrote.
fn review_to_completion(
    harness: &common::Harness,
    client: &client::Client,
    events: &flume::Receiver<DaemonEvent>,
    target: ReviewTarget,
) -> ReviewId {
    let _ = std::fs::remove_file(harness.root().join("written"));
    let review_id = match client.request(Request::StartReview {
        target,
        tier: None,
        provider: None,
        preset: None,
        instructions: None,
        model: None,
        effort: None,
    }) {
        Ok(Response::ReviewStarted { review_id }) => review_id,
        other => panic!("expected ReviewStarted, got {other:?}"),
    };
    let model = wait_run(events, review_id, |run| run.session_id.is_some());
    assert!(common::poll_until(common::DEADLINE, || harness
        .root()
        .join("written")
        .exists()));
    report_idle(client, model.session_id.expect("reviewer"));
    let done = wait_run(events, review_id, |run| run.status.is_finished());
    assert_eq!(done.status, ReviewStatus::Completed, "{:?}", done.error);
    review_id
}

struct Report {
    findings: Vec<domain::Finding>,
    resolved: Vec<domain::Finding>,
    previous: Option<ReviewId>,
}

fn report(client: &client::Client, review_id: ReviewId) -> Report {
    match client.request(Request::GetReview { review_id }) {
        Ok(Response::Review {
            findings,
            resolved,
            previous,
            ..
        }) => Report {
            findings,
            resolved,
            previous,
        },
        other => panic!("expected Review, got {other:?}"),
    }
}

fn lineage_of(report: &Report, category: &str) -> domain::Lineage {
    report
        .findings
        .iter()
        .find(|f| f.category == category)
        .unwrap_or_else(|| panic!("no {category} finding in {:?}", report.findings))
        .lineage
}

#[test]
fn lineage_marks_new_persisting_and_resolved() {
    let harness = common::Harness::new();
    let a = finding(2, "P1", "logic", "price + qty");
    let b = finding(2, "P2", "performance", "price + qty");
    let c = finding(1, "P2", "bug", "fn total");
    install_reviewer(&harness, &answer(&[a, b.clone()]), Reviewer::Answers);
    let repo = seeded_repo();
    let daemon = boot(&harness);
    let client = daemon.connect("scenario-review");
    let events = client.events();
    let workspace = workspace_at(&client, repo.path());
    let target = ReviewTarget::Workspace {
        workspace_id: workspace,
        base: None,
    };

    let first = review_to_completion(&harness, &client, &events, target.clone());
    let one = report(&client, first);
    assert_eq!(one.previous, None);
    let diff = review_diff(&client, first);
    assert_eq!(diff.workspace_id, workspace);
    let main = diff
        .files
        .iter()
        .find(|f| f.path == "main.rs")
        .expect("main.rs");
    assert!(main.patch.contains("+    price + qty"), "{}", main.patch);
    assert!(matches!(
        client.request(Request::GetReviewDiff { review_id: ReviewId::new() }),
        Err(client::ClientError::Protocol(error)) if error.code == ErrorCode::NotFound
    ));
    assert!(one.resolved.is_empty());
    assert_eq!(lineage_of(&one, "logic"), domain::Lineage::New);

    std::fs::write(repo.path().join("notes.txt"), "moved\n").expect("notes");
    git(repo.path(), &["add", "notes.txt"]);
    git(repo.path(), &["commit", "-qm", "move head"]);
    install_reviewer(&harness, &answer(&[b.clone(), c]), Reviewer::Answers);
    let second = review_to_completion(&harness, &client, &events, target.clone());
    let two = report(&client, second);
    assert_eq!(two.previous, Some(first));
    assert_eq!(lineage_of(&two, "performance"), domain::Lineage::Persisting);
    assert_eq!(lineage_of(&two, "bug"), domain::Lineage::New);
    let resolved: Vec<&str> = two.resolved.iter().map(|f| f.category.as_str()).collect();
    assert_eq!(resolved, ["logic"]);

    git(repo.path(), &["checkout", "-qb", "feature"]);
    install_reviewer(&harness, &answer(&[b]), Reviewer::Answers);
    let third = review_to_completion(&harness, &client, &events, target);
    let three = report(&client, third);
    assert_eq!(three.previous, None, "another branch is another lineage");
    assert_eq!(lineage_of(&three, "performance"), domain::Lineage::New);
    assert!(three.resolved.is_empty());
}

fn react(
    client: &client::Client,
    review_id: ReviewId,
    finding_id: u32,
    reaction: Option<domain::FindingReaction>,
) {
    assert_eq!(
        client
            .request(Request::SetFindingFeedback {
                review_id,
                finding_id,
                reaction,
            })
            .expect("SetFindingFeedback"),
        Response::Ack
    );
}

#[test]
fn feedback_dismiss_recomputes_score_and_verdict() {
    let harness = common::Harness::new();
    install_reviewer(
        &harness,
        &answer(&[finding(2, "P0", "logic", "price + qty")]),
        Reviewer::Answers,
    );
    let repo = seeded_repo();
    let daemon = boot(&harness);
    let client = daemon.connect("scenario-review");
    let events = client.events();
    let workspace = workspace_at(&client, repo.path());
    let review_id = review_to_completion(
        &harness,
        &client,
        &events,
        ReviewTarget::Workspace {
            workspace_id: workspace,
            base: None,
        },
    );
    let (run, findings) = get(&client, review_id);
    assert_eq!(run.verdict, Some(Verdict::Block));
    let id = findings[0].id;

    react(
        &client,
        review_id,
        id,
        Some(domain::FindingReaction::Dismiss),
    );
    let dismissed = wait_run(&events, review_id, |run| run.verdict == Some(Verdict::Ok));
    assert_eq!(dismissed.score, Some(5.0));
    let (run, findings) = get(&client, review_id);
    assert_eq!((run.verdict, run.score), (Some(Verdict::Ok), Some(5.0)));
    assert_eq!(findings[0].reaction, Some(domain::FindingReaction::Dismiss));

    react(&client, review_id, id, None);
    wait_run(&events, review_id, |run| {
        run.verdict == Some(Verdict::Block)
    });
    let (run, findings) = get(&client, review_id);
    assert_eq!((run.verdict, run.score), (Some(Verdict::Block), Some(2.5)));
    assert_eq!(findings[0].reaction, None);

    assert!(matches!(
        client.request(Request::SetFindingFeedback {
            review_id,
            finding_id: 99,
            reaction: Some(domain::FindingReaction::Up),
        }),
        Err(client::ClientError::Protocol(error)) if error.code == ErrorCode::NotFound
    ));
}

fn config(client: &client::Client, workspace_id: WorkspaceId) -> domain::ReviewSettings {
    match client.request(Request::GetReviewConfig { workspace_id }) {
        Ok(Response::ReviewConfig(settings)) => settings,
        other => panic!("expected ReviewConfig, got {other:?}"),
    }
}

#[test]
fn dismissals_suppress_a_rule_until_undone() {
    let harness = common::Harness::new();
    let ruled = |category: &str| {
        let mut value = ruled_finding("money", "P1");
        value["category"] = category.into();
        value
    };
    install_reviewer(
        &harness,
        &answer(&[ruled("logic"), ruled("performance"), ruled("bug")]),
        Reviewer::Answers,
    );
    let repo = seeded_repo();
    seed_review_dir(repo.path());
    let daemon = boot(&harness);
    let client = daemon.connect("scenario-review");
    let events = client.events();
    let workspace = workspace_at(&client, repo.path());
    let target = ReviewTarget::Workspace {
        workspace_id: workspace,
        base: None,
    };
    let first = review_to_completion(&harness, &client, &events, target.clone());
    let (_, findings) = get(&client, first);
    assert_eq!(findings.len(), 3);
    assert!(config(&client, workspace).suppressions.is_empty());
    for finding in &findings {
        react(
            &client,
            first,
            finding.id,
            Some(domain::FindingReaction::Dismiss),
        );
    }
    let suppressions = config(&client, workspace).suppressions;
    let rule = suppressions
        .iter()
        .find(|s| s.kind == domain::SuppressionKind::Rule)
        .cloned()
        .expect("the rule is suppressed");
    assert_eq!(rule.value, "money");

    install_reviewer(
        &harness,
        &answer(&[ruled("naming"), finding(2, "P2", "performance", "qty")]),
        Reviewer::Answers,
    );
    let second = review_to_completion(&harness, &client, &events, target);
    let (run, findings) = get(&client, second);
    let lineage = report(&client, second);
    assert_eq!(lineage.previous, Some(first));
    assert!(
        lineage.resolved.is_empty(),
        "dismissed findings were hidden, not resolved: {:?}",
        lineage.resolved
    );
    assert!(findings.iter().all(|f| f.rule_id.is_none()), "{findings:?}");
    assert!(
        run.coverage
            .notes
            .iter()
            .any(|note| note == "1 finding(s) suppressed by feedback"),
        "{:?}",
        run.coverage.notes
    );

    assert_eq!(
        client
            .request(Request::ClearReviewSuppression {
                workspace_id: workspace,
                suppression: rule,
            })
            .expect("ClearReviewSuppression"),
        Response::Ack
    );
    assert!(config(&client, workspace).suppressions.is_empty());
}

fn rev_parse(repo: &Path, rev: &str) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["rev-parse", rev])
        .output()
        .expect("git rev-parse");
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

const PR_FILE: &str = "fn pr_only() -> u32 {\n    40 + 2\n}\n";

/// A bare `origin` whose `refs/pull/1/head` adds `pr.rs` on a commit no branch
/// holds, a `--no-local` clone of it that has never seen that commit, and the
/// commit's oid.
fn pull_request_checkout() -> (tempfile::TempDir, tempfile::TempDir, String) {
    let source = seeded_repo();
    git(source.path(), &["checkout", "-q", "--", "main.rs"]);
    git(source.path(), &["checkout", "-qb", "topic"]);
    std::fs::write(source.path().join("pr.rs"), PR_FILE).expect("pr.rs");
    git(source.path(), &["add", "pr.rs"]);
    git(source.path(), &["commit", "-qm", "add pr_only"]);
    let head = rev_parse(source.path(), "HEAD");
    let origin = tempfile::tempdir_in("/tmp").expect("origin");
    let bare = origin.path().join("origin.git");
    git(
        source.path(),
        &["clone", "-q", "--bare", ".", bare.to_str().expect("utf-8")],
    );
    git(&bare, &["update-ref", "refs/pull/1/head", &head]);
    git(&bare, &["branch", "-D", "topic"]);
    let checkout = tempfile::tempdir_in("/tmp").expect("checkout");
    git(
        checkout.path(),
        &[
            "clone",
            "-q",
            "--no-local",
            bare.to_str().expect("utf-8"),
            ".",
        ],
    );
    git(checkout.path(), &["config", "user.email", "t@example.com"]);
    git(checkout.path(), &["config", "user.name", "T"]);
    (origin, checkout, head)
}

#[test]
fn a_pull_request_is_reviewed_from_its_fetched_head() {
    let harness = common::Harness::new();
    let mut pr_finding = finding(2, "P1", "logic", "40 + 2");
    pr_finding["path"] = "pr.rs".into();
    install_reviewer(&harness, &answer(&[pr_finding]), Reviewer::Answers);
    let (_origin, checkout, pr_head) = pull_request_checkout();
    let head_before = rev_parse(checkout.path(), "HEAD");
    let porcelain_before = porcelain(checkout.path());
    let daemon = boot(&harness);
    let client = daemon.connect("scenario-review");
    let events = client.events();
    let workspace = workspace_at(&client, checkout.path());

    let review_id = review_to_completion(
        &harness,
        &client,
        &events,
        ReviewTarget::PullRequest {
            workspace_id: workspace,
            number: 1,
            base: Some("main".into()),
        },
    );
    let (run, findings) = get(&client, review_id);
    assert_eq!(run.head_oid, pr_head);
    assert_eq!(run.coverage.included, ["pr.rs"]);
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert_eq!(
        (findings[0].path.as_str(), findings[0].line),
        ("pr.rs", Some(2))
    );
    let diff = review_diff(&client, review_id);
    let paths: Vec<&str> = diff.files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(
        paths,
        ["pr.rs"],
        "the reviewed range, not the clean working tree"
    );
    assert!(
        diff.files[0].patch.contains("+    40 + 2"),
        "{}",
        diff.files[0].patch
    );
    assert_eq!(rev_parse(checkout.path(), "HEAD"), head_before);
    assert_eq!(porcelain(checkout.path()), porcelain_before);
}

#[test]
fn a_pull_request_with_an_invalid_base_is_refused() {
    let harness = common::Harness::new();
    install_reviewer(&harness, "", Reviewer::Silent);
    let repo = seeded_repo();
    let daemon = boot(&harness);
    let client = daemon.connect("scenario-review");
    let workspace = workspace_at(&client, repo.path());
    for base in ["--upload-pack=x", "bad..name"] {
        let refused = client.request(Request::StartReview {
            target: ReviewTarget::PullRequest {
                workspace_id: workspace,
                number: 1,
                base: Some(base.into()),
            },
            tier: None,
            provider: None,
            preset: None,
            instructions: None,
            model: None,
            effort: None,
        });
        assert!(
            matches!(&refused, Err(client::ClientError::Protocol(error)) if error.code == ErrorCode::InvalidRequest),
            "{base}: {refused:?}"
        );
    }
}

fn review_diff(client: &client::Client, review_id: ReviewId) -> domain::WorkspaceDiff {
    match client.request(Request::GetReviewDiff { review_id }) {
        Ok(Response::WorkspaceDiff(diff)) => diff,
        other => panic!("expected WorkspaceDiff, got {other:?}"),
    }
}

#[test]
fn a_finding_prompt_reaches_an_idle_agent_and_queues_for_a_busy_one() {
    let harness = common::Harness::new();
    install_reviewer(&harness, "", Reviewer::Silent);
    let repo = seeded_repo();
    let daemon = boot(&harness);
    let client = daemon.connect("scenario-review");
    let events = client.events();
    let workspace = workspace_at(&client, repo.path());
    let (agent, _) = common::create_agent_session(&client, &events, workspace, "claude");

    client
        .request(Request::ReportAgentActivity {
            session_id: agent,
            state: ActivityState::Working,
            source: "test".into(),
        })
        .expect("ReportAgentActivity");
    let queued = client
        .send_session_prompt(agent, "Fix main.rs:2".into(), client::PromptDelivery::Queue)
        .expect("SendSessionPrompt");
    assert!(queued, "a working agent gets the prompt at its next idle");
    report_idle(&client, agent);
    // Submitting the queued prompt makes the agent busy again until its hook says otherwise.
    report_idle(&client, agent);
    let queued = client
        .send_session_prompt(agent, "Fix it again".into(), client::PromptDelivery::Queue)
        .expect("SendSessionPrompt");
    assert!(!queued, "an idle agent takes it now");
}
