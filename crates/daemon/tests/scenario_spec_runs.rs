//! A spec run end to end against a real daemon: gates, the coder's ledger attempt, the checks
//! Forge runs as task sessions, the validated report, a restart, and the draft pull request.
//! Agents are fake CLIs that record their arguments; the test plays drafter, planner and coder
//! by sending the requests their tools would send.

mod common;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use domain::orchestration::ReportOutcome;
use domain::spec_run::model::{
    AgentPick, Delivery, GateDecision, GateKind, NextAction, Placement, SpecArtifactKind,
    SpecOptions, SpecRole, SpecRunDetail, SpecRunSource, SpecRunView, SpecStage,
};
use domain::{AgentProviderId, SessionId, SpecRunId, WorkspaceId};
use protocol::{DaemonEvent, ErrorCode, Request, Response};

fn keep_history(cfg: &mut daemon::config::Config) {
    cfg.sessions.persist_history = true;
}

/// A Claude stand-in that writes its arguments to `<root>/args/<session>` and stays alive.
fn install_claude(harness: &common::Harness) -> PathBuf {
    let args = harness.root().join("args");
    fs::create_dir_all(&args).expect("fixture");
    common::write_executable(
        harness.bin(),
        "claude",
        &format!(
            "#!/bin/sh\nif [ \"$1\" = --version ]; then printf '%s\\n' 'Claude Code 1.0.0'; exit 0; fi\n\
             printf '%s\\n' \"$@\" > {dir}/\"$FORGE_SESSION_ID\"\nexec /bin/cat\n",
            dir = common::sh_quote(&args.to_string_lossy()),
        ),
    );
    args
}

/// A connection whose events nobody reads would overflow its bounded queue and be dropped.
fn connect(daemon: &common::TestDaemon, name: &str) -> client::Client {
    let client = daemon.connect(name);
    let events = client.events();
    std::thread::spawn(move || while events.recv().is_ok() {});
    client
}

fn pick() -> AgentPick {
    AgentPick {
        provider: AgentProviderId::new("claude"),
        profile: None,
        effort: None,
        model: None,
    }
}

fn options(placement: Placement) -> SpecOptions {
    SpecOptions {
        placement,
        delivery: Delivery::NoCommit,
        rounds: 3,
        forge_review: false,
        session_per_stage: false,
        publish_spec: false,
        publish_report: false,
    }
}

fn worktree(branch: &str) -> Placement {
    Placement::Worktree {
        branch: branch.into(),
        base: None,
    }
}

const SPEC: &str = "# Loader cache\nRoute: standard · Source: idea\n\n## Problem\nSlow.\n\n\
## Acceptance criteria\n\
- AC-1: WHEN the loader runs THE SYSTEM SHALL write done.txt\n  - Automatic validation: `test -f done.txt` → exit 0\n\
- AC-2: WHEN asked THE SYSTEM SHALL say fast\n  - Automatic validation: `grep -q fast done.txt` → exit 0\n\n\
## Out of scope\nNone\n";

fn plan(ac1: &str, ac2: &str, gate: Option<&str>) -> String {
    let gate = gate
        .map(|g| format!("\"gate\": {{\"command\": \"{g}\"}},"))
        .unwrap_or_default();
    format!(
        "# Plan\n\n## What we are not doing\nNone\n\n```json\n{{\"schema\": \"forge.spec_plan/v1\", {gate} \
\"steps\": [{{\"id\": \"S1\", \"criteria\": [\"AC-1\"], \"check\": {ac1}}}, \
{{\"id\": \"S2\", \"criteria\": [\"AC-2\"], \"check\": {ac2}}}]}}\n```\n"
    )
}

fn check(command: &str) -> String {
    serde_json::json!({ "command": command }).to_string()
}

fn detail(client: &client::Client, id: SpecRunId) -> SpecRunDetail {
    match client.request(Request::GetSpecRun { spec_run_id: id }) {
        Ok(Response::SpecRun(detail)) => *detail,
        other => panic!("GetSpecRun: {other:?}"),
    }
}

fn wait_stage(client: &client::Client, id: SpecRunId, stage: SpecStage) -> SpecRunView {
    let mut last = None;
    let reached = common::poll_until(Duration::from_secs(60), || {
        let run = detail(client, id).run;
        let done = run.stage == stage;
        last = Some(run);
        done
    });
    let run = last.expect("polled once");
    assert!(reached, "never reached {stage:?}: {run:#?}");
    run
}

fn start(
    client: &client::Client,
    workspace: WorkspaceId,
    options: SpecOptions,
) -> (SpecRunId, SessionId) {
    match client.request(Request::StartSpecRun {
        workspace_id: workspace,
        name: "cache".into(),
        source: SpecRunSource::Inline {
            text: "Make the loader fast.".into(),
        },
        planner: pick(),
        coder: pick(),
        options,
    }) {
        Ok(Response::SpecRunStarted {
            spec_run_id,
            session_id,
        }) => (
            spec_run_id,
            session_id.expect("an inline source starts its drafter at once"),
        ),
        other => panic!("StartSpecRun: {other:?}"),
    }
}

fn save(
    client: &client::Client,
    id: SpecRunId,
    caller: SessionId,
    kind: SpecArtifactKind,
    content: &str,
) -> Result<Response, protocol::ProtocolError> {
    client
        .request(Request::SaveSpecArtifact {
            spec_run_id: Some(id),
            caller_session_id: Some(caller),
            kind,
            content: content.into(),
            artifact_url: None,
        })
        .map_err(|e| match e {
            client::ClientError::Protocol(error) => error,
            other => panic!("transport: {other:?}"),
        })
}

fn gate(
    client: &client::Client,
    id: SpecRunId,
    kind: GateKind,
    decision: GateDecision,
) -> Result<Response, protocol::ProtocolError> {
    client
        .request(Request::SpecGate {
            spec_run_id: id,
            caller_session_id: None,
            gate: kind,
            decision,
        })
        .map_err(|e| match e {
            client::ClientError::Protocol(error) => error,
            other => panic!("transport: {other:?}"),
        })
}

fn approve() -> GateDecision {
    GateDecision::Approve {
        options: None,
        save_gate_action: false,
        manual_checked: Vec::new(),
        next: None,
    }
}

fn workspace_path(client: &client::Client, id: WorkspaceId) -> PathBuf {
    common::workspaces(client)
        .into_iter()
        .find(|w| w.id == id)
        .expect("coder workspace")
        .path
}

/// Report the coder's attempt the way `forgectl report` does from its session.
fn coder_reports(client: &client::Client, coder: SessionId) {
    let Response::Snapshot { runs, .. } = client.request(Request::GetSnapshot).expect("fixture")
    else {
        panic!("snapshot");
    };
    let attempt = runs
        .iter()
        .flat_map(|view| view.attempts.iter())
        .find(|attempt| attempt.session_id == Some(coder))
        .expect("the coder is a ledger attempt")
        .id;
    client
        .request(Request::ReportAttempt {
            request_id: None,
            caller_session_id: Some(coder),
            attempt_id: attempt,
            outcome: ReportOutcome::Done,
            summary: "AC-1 and AC-2 done; tests pass".into(),
            verification: None,
            result_file: None,
        })
        .expect("report");
}

/// Drafter saves the spec, G1 passes, the planner saves `plan`, G2 passes with `g2`, the coder
/// writes `files` and reports; returns the run once Forge has run the checks.
fn drive_to_review(
    client: &client::Client,
    workspace: WorkspaceId,
    launch: SpecOptions,
    plan_text: &str,
    g2: GateDecision,
    files: &[(&str, &str)],
) -> (SpecRunId, SpecRunView) {
    let (id, drafter) = start(client, workspace, launch);
    save(client, id, drafter, SpecArtifactKind::Spec, SPEC).expect("spec saved");
    gate(client, id, GateKind::Spec, approve()).expect("G1");
    let run = wait_stage(client, id, SpecStage::Planning);
    let planner = run.session(SpecRole::Planner).expect("planner launched");
    save(client, id, planner, SpecArtifactKind::Plan, plan_text).expect("plan saved");
    gate(client, id, GateKind::Plan, g2).expect("G2");
    let run = wait_stage(client, id, SpecStage::Executing);
    let coder = run.session(SpecRole::Coder).expect("coder launched");
    let path = workspace_path(client, run.coder_workspace_id.expect("placed"));
    for (name, text) in files {
        fs::write(path.join(name), text).expect("fixture");
    }
    coder_reports(client, coder);
    (id, wait_stage(client, id, SpecStage::Reviewing))
}

fn report(ac2_state: &str, paths: &[&str]) -> String {
    let paths = serde_json::to_string(paths).expect("fixture");
    format!(
        "# Loader cache · blocked\n\n## What was done\n- done.txt\n\n## Criteria\n| ID | State |\n|---|---|\n| AC-1 | met |\n| AC-2 | {ac2_state} |\n\n\
```json\n{{\"schema\": \"forge.spec_run/v1\", \"criteria\": [\
{{\"id\": \"AC-1\", \"state\": \"met\", \"paths\": {paths}, \"evidence\": {{\"command\": \"test -f done.txt\", \"exit_code\": 0, \"excerpt\": \"ok\"}}}}, \
{{\"id\": \"AC-2\", \"state\": \"{ac2_state}\", \"evidence\": {{\"command\": \"grep -q fast done.txt\", \"exit_code\": 1, \"excerpt\": \"no match\"}}}}], \
\"gate\": {{\"id\": \"gate\", \"state\": \"met\", \"evidence\": {{\"command\": \"true\", \"exit_code\": 0, \"excerpt\": \"\"}}}}}}\n```\n"
    )
}

fn submit(
    client: &client::Client,
    id: SpecRunId,
    planner: SessionId,
    text: &str,
) -> Result<Response, protocol::ProtocolError> {
    client
        .request(Request::SubmitSpecReport {
            spec_run_id: Some(id),
            caller_session_id: Some(planner),
            content: text.into(),
            artifact_url: None,
        })
        .map_err(|e| match e {
            client::ClientError::Protocol(error) => error,
            other => panic!("transport: {other:?}"),
        })
}

fn run_dir(harness: &common::Harness, id: SpecRunId) -> PathBuf {
    harness.forge_home().join("spec-runs").join(id.to_string())
}

fn prompts(harness: &common::Harness, id: SpecRunId) -> String {
    let dir = run_dir(harness, id).join("prompts");
    let mut all = String::new();
    for entry in fs::read_dir(dir).expect("fixture") {
        all.push_str(&fs::read_to_string(entry.expect("fixture").path()).expect("fixture"));
    }
    all
}

#[test]
fn a_spec_run_passes_its_gates_and_meets_only_what_a_check_proved() {
    let harness = common::Harness::new();
    install_claude(&harness);
    let repo = test_support::init_repo().expect("git repo");
    let daemon = harness.boot_with(keep_history);
    let client = connect(&daemon, "spec-run");
    let workspace = common::add_main_workspace(&client, repo.path());

    let (id, drafter) = start(&client, workspace, options(worktree("spec/cache")));
    let source = fs::read_to_string(run_dir(&harness, id).join("source.md")).expect("fixture");
    assert_eq!(source, "Make the loader fast.");
    let refused = save(
        &client,
        id,
        drafter,
        SpecArtifactKind::Spec,
        "# Cache\nNo criteria.",
    )
    .expect_err("refused");
    assert_eq!(refused.code, ErrorCode::InvalidRequest);
    save(&client, id, drafter, SpecArtifactKind::Spec, SPEC).expect("spec saved");

    // AC-11: no reason is refused; a reason reaches the drafter verbatim.
    let bare = gate(
        &client,
        id,
        GateKind::Spec,
        GateDecision::Changes {
            reason: "  ".into(),
        },
    )
    .expect_err("refused");
    assert_eq!(bare.code, ErrorCode::InvalidRequest);
    gate(
        &client,
        id,
        GateKind::Spec,
        GateDecision::Changes {
            reason: "Name the cache size in AC-1.".into(),
        },
    )
    .expect("changes");
    assert_eq!(detail(&client, id).run.stage, SpecStage::Drafting);
    assert!(prompts(&harness, id).contains("Name the cache size in AC-1."));
    let own = client
        .request(Request::SpecGate {
            spec_run_id: id,
            caller_session_id: Some(drafter),
            gate: GateKind::Spec,
            decision: approve(),
        })
        .expect_err("refused");
    assert!(
        matches!(own, client::ClientError::Protocol(ref e) if e.code == ErrorCode::Conflict || e.code == ErrorCode::PreconditionFailed),
        "{own:?}"
    );
    save(&client, id, drafter, SpecArtifactKind::Spec, SPEC).expect("spec saved again");
    gate(&client, id, GateKind::Spec, approve()).expect("G1");
    let run = wait_stage(&client, id, SpecStage::Planning);
    let planner = run.session(SpecRole::Planner).expect("planner");

    // AC-10: the planner cannot launch a coder before the plan gate.
    let early = client
        .request(Request::CreateAgentSession {
            workspace_id: workspace,
            provider_id: AgentProviderId::new("claude"),
            profile_id: None,
            parent: Some(planner),
            role: domain::SessionRole::Executor,
            resume: None,
            initial_prompt: Some("code it".into()),
            read_only: false,
            model: None,
            effort: None,
            title: None,
        })
        .expect_err("refused");
    assert!(
        matches!(early, client::ClientError::Protocol(ref e) if e.code == ErrorCode::PreconditionFailed),
        "{early:?}"
    );

    // AC-16: without a gate action, a plan that proposes no gate is refused.
    let gateless = save(
        &client,
        id,
        planner,
        SpecArtifactKind::Plan,
        &plan(
            &check("test -f done.txt"),
            &check("/usr/bin/grep -q fast done.txt"),
            None,
        ),
    )
    .expect_err("refused");
    assert!(
        gateless.message.contains("no repository gate"),
        "{gateless:?}"
    );
    save(
        &client,
        id,
        planner,
        SpecArtifactKind::Plan,
        &plan(
            &check("test -f done.txt"),
            &check("/usr/bin/grep -q fast done.txt"),
            Some("true"),
        ),
    )
    .expect("plan saved");
    gate(
        &client,
        id,
        GateKind::Plan,
        GateDecision::Approve {
            options: None,
            save_gate_action: true,
            manual_checked: Vec::new(),
            next: None,
        },
    )
    .expect("G2");
    let Response::Snapshot {
        project_actions, ..
    } = client.request(Request::GetSnapshot).expect("fixture")
    else {
        panic!()
    };
    let saved = project_actions
        .iter()
        .find(|a| a.gate)
        .expect("gate action saved");
    assert_eq!(saved.command, "true");

    let run = wait_stage(&client, id, SpecStage::Executing);
    let coder = run.session(SpecRole::Coder).expect("coder");
    let path = workspace_path(&client, run.coder_workspace_id.expect("fixture"));
    assert_ne!(
        run.coder_workspace_id,
        Some(workspace),
        "a new worktree by default"
    );
    fs::write(path.join("done.txt"), "slow\n").expect("fixture");
    coder_reports(&client, coder);
    let run = wait_stage(&client, id, SpecStage::Reviewing);

    // AC-14: AC-1's check passed and AC-2's failed, so only AC-1 may be met.
    let by = |c: &str| {
        run.checks
            .iter()
            .find(|r| r.criterion == c && r.round == 1)
            .cloned()
            .expect("fixture")
    };
    assert_eq!(by("AC-1").exit_code, Some(0));
    assert_eq!(by("AC-2").exit_code, Some(1));
    assert_eq!(by("gate").exit_code, Some(0));
    let log = fs::read_to_string(run_dir(&harness, id).join(by("AC-2").log)).expect("fixture");
    assert!(
        log.contains("command: /usr/bin/grep -q fast done.txt") && log.contains("exit: 1"),
        "{log}"
    );
    let claimed =
        submit(&client, id, planner, &report("met", &["done.txt:1"])).expect_err("refused");
    assert!(
        claimed
            .message
            .contains("AC-2 is met, but not every check of this round passed"),
        "{claimed:?}"
    );
    submit(&client, id, planner, &report("not_met", &["done.txt:1"])).expect("report");
    let run = wait_stage(&client, id, SpecStage::ResultGate);
    assert_eq!(
        run.outcome,
        Some(domain::spec_run::model::RunOutcome::Blocked)
    );
    gate(
        &client,
        id,
        GateKind::Result,
        GateDecision::Approve {
            options: None,
            save_gate_action: false,
            manual_checked: Vec::new(),
            next: Some(NextAction::Finish),
        },
    )
    .expect("G3");
    assert_eq!(detail(&client, id).run.stage, SpecStage::Done);

    // AC-6: the source with its SHA-256, the spec, the plan and the report outlive the daemon.
    let before = detail(&client, id);
    drop(client);
    daemon.crash();
    let restarted = harness.boot_with(keep_history);
    let client = connect(&restarted, "spec-run-after");
    let after = detail(&client, id);
    assert_eq!(after.run.source.sha256, before.run.source.sha256);
    assert!(after.run.source.sha256.is_some());
    assert_eq!(after.spec.as_deref(), Some(SPEC));
    assert!(after
        .plan
        .as_deref()
        .is_some_and(|p| p.contains("forge.spec_plan/v1")));
    assert!(after
        .report
        .as_deref()
        .is_some_and(|r| r.contains("| AC-2 | not_met |")));
    assert_eq!(after.run.stage, SpecStage::Done);
}

#[test]
fn without_a_worktree_the_review_leaves_out_what_was_already_modified() {
    let harness = common::Harness::new();
    install_claude(&harness);
    let repo = test_support::init_repo().expect("git repo");
    fs::write(repo.path().join("notes.md"), "mine, before the run\n").expect("fixture");
    let daemon = harness.boot();
    let client = connect(&daemon, "spec-run-checkout");
    let workspace = common::add_main_workspace(&client, repo.path());

    let (id, run) = drive_to_review(
        &client,
        workspace,
        options(Placement::Checkout),
        &plan(
            &check("test -f done.txt"),
            &check("/usr/bin/grep -q fast done.txt"),
            Some("true"),
        ),
        approve(),
        &[("done.txt", "fast\n")],
    );
    assert_eq!(run.coder_workspace_id, Some(workspace));
    assert_eq!(run.dirty_at_launch, ["notes.md"]);
    let planner = run.session(SpecRole::Planner).expect("fixture");
    let foreign =
        submit(&client, id, planner, &report("met", &["notes.md:1"])).expect_err("refused");
    assert!(
        foreign
            .message
            .contains("notes.md is not in the reviewed diff"),
        "{foreign:?}"
    );
    submit(&client, id, planner, &report("met", &["done.txt:1"]))
        .expect("report on the coder's own file");
    assert_eq!(
        wait_stage(&client, id, SpecStage::ResultGate).outcome,
        Some(domain::spec_run::model::RunOutcome::Done)
    );
}

#[test]
fn every_planner_stage_shares_one_session_unless_asked_for_one_each() {
    let harness = common::Harness::new();
    let args = install_claude(&harness);
    let repo = test_support::init_repo().expect("git repo");
    let daemon = harness.boot();
    let client = connect(&daemon, "spec-run-sessions");
    let workspace = common::add_main_workspace(&client, repo.path());
    let plan_text = plan(&check("true"), &check("true"), Some("true"));

    let planners = |run: &SpecRunView| {
        run.sessions
            .iter()
            .filter(|s| s.role == SpecRole::Planner)
            .map(|s| s.session_id)
            .collect::<Vec<_>>()
    };
    let (_, shared) = drive_to_review(
        &client,
        workspace,
        options(worktree("spec/one")),
        &plan_text,
        approve(),
        &[],
    );
    assert_eq!(planners(&shared).len(), 1, "{:#?}", shared.sessions);

    let mut each = options(worktree("spec/each"));
    each.session_per_stage = true;
    let (id, split) = drive_to_review(&client, workspace, each, &plan_text, approve(), &[]);
    let launched = planners(&split);
    assert!(launched.len() >= 2, "{:#?}", split.sessions);
    let second = args.join(launched[1].to_string());
    assert!(common::poll_until(Duration::from_secs(10), || second.exists()));
    let second = fs::read_to_string(second).expect("fixture");
    let spec = run_dir(&harness, id).join("spec.md");
    assert!(
        second.contains(&spec.to_string_lossy().into_owned()),
        "{second}"
    );
}

#[test]
fn a_check_past_its_timeout_is_killed_with_its_children() {
    let harness = common::Harness::new();
    install_claude(&harness);
    let repo = test_support::init_repo().expect("git repo");
    let daemon = harness.boot();
    let client = connect(&daemon, "spec-run-timeout");
    let workspace = common::add_main_workspace(&client, repo.path());
    let pid_file = harness.root().join("child.pid");
    let hang = serde_json::json!({
        "command": format!("/bin/sleep 300 & echo $! > {}; wait", pid_file.display()),
        "timeout_secs": 1
    })
    .to_string();

    let (_, run) = drive_to_review(
        &client,
        workspace,
        options(worktree("spec/hang")),
        &plan(&hang, &check("true"), Some("true")),
        approve(),
        &[],
    );
    let record = run
        .checks
        .iter()
        .find(|c| c.criterion == "AC-1")
        .expect("fixture");
    assert!(record.timed_out, "{record:?}");
    assert!(!record.passed());
    let pid = fs::read_to_string(&pid_file)
        .expect("fixture")
        .trim()
        .to_owned();
    let gone = common::poll_until(Duration::from_secs(10), || {
        !Command::new("kill")
            .args(["-0", &pid])
            .status()
            .expect("fixture")
            .success()
    });
    assert!(gone, "the check's child {pid} outlived its process group");
}

fn write_gh(path: &Path, log: &Path) {
    let script = format!(
        "#!/bin/sh\nfor arg in \"$@\"; do printf '%s\\n' \"$arg\"; done >> \"{}\"\n\
         printf '%s\\n' 'https://github.com/Acme/Cache/pull/7'\n",
        log.display()
    );
    fs::write(path, script).expect("fixture");
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("fixture");
}

#[test]
fn a_draft_pull_request_opens_only_after_the_result_gate() {
    let harness = common::Harness::new();
    install_claude(&harness);
    let gh = harness.root().join("gh");
    let gh_log = harness.root().join("gh.log");
    write_gh(&gh, &gh_log);
    let repo = test_support::init_repo().expect("git repo");
    let bare = harness.root().join("remote.git");
    assert!(Command::new("git")
        .args(["init", "--bare"])
        .arg(&bare)
        .status()
        .expect("fixture")
        .success());
    assert!(Command::new("git")
        .arg("-C")
        .arg(repo.path())
        .args(["remote", "add", "origin"])
        .arg(&bare)
        .status()
        .expect("fixture")
        .success());
    let daemon = harness.boot_with(|cfg| cfg.github.executable = gh.to_string_lossy().into_owned());
    let client = connect(&daemon, "spec-run-pr");
    let workspace = common::add_main_workspace(&client, repo.path());

    let mut launch = options(worktree("spec/pr"));
    launch.delivery = Delivery::DraftPr;
    let (id, run) = drive_to_review(
        &client,
        workspace,
        launch,
        &plan(
            &check("test -f done.txt"),
            &check("/usr/bin/grep -q fast done.txt"),
            Some("true"),
        ),
        approve(),
        &[("done.txt", "fast\n")],
    );
    let planner = run.session(SpecRole::Planner).expect("fixture");
    submit(&client, id, planner, &report("met", &["done.txt:1"])).expect("report");
    wait_stage(&client, id, SpecStage::ResultGate);
    assert!(!gh_log.exists(), "nothing opens before the result gate");
    let watcher = daemon.connect("spec-run-pr-events");
    let events = watcher.events();

    gate(
        &client,
        id,
        GateKind::Result,
        GateDecision::Approve {
            options: None,
            save_gate_action: false,
            manual_checked: Vec::new(),
            next: Some(NextAction::OpenDraftPr),
        },
    )
    .expect("G3");
    let agent = match common::wait_for(
        &events,
        common::DEADLINE,
        |event| matches!(event, DaemonEvent::SessionCreated(session) if session.background),
    ) {
        Some(DaemonEvent::SessionCreated(session)) => session.id,
        other => panic!("no pull-request agent: {other:?}"),
    };
    client
        .request(Request::SubmitPullRequestDraft {
            session_id: agent,
            draft: domain::PullRequestDraft {
                title: "Cache the loader".into(),
                body: "A body that forgot the table.".into(),
                branch: "spec/pr".into(),
                commit_message: "Cache the loader".into(),
            },
        })
        .expect("draft");
    match common::wait_for(&events, common::DEADLINE, |event| {
        matches!(event, DaemonEvent::PullRequestOpened { .. })
    }) {
        Some(DaemonEvent::PullRequestOpened { url, error, .. }) => {
            assert_eq!(error, None);
            assert_eq!(url.as_deref(), Some("https://github.com/Acme/Cache/pull/7"));
        }
        other => panic!("not opened: {other:?}"),
    }
    let args = fs::read_to_string(&gh_log).expect("fixture");
    assert!(args.lines().any(|l| l == "--draft"), "{args}");
    assert!(
        args.contains("## Criteria\n| ID | State |\n|---|---|\n| AC-1 | met |"),
        "{args}"
    );
}

#[test]
fn an_issue_becomes_the_drafters_source() {
    let harness = common::Harness::new();
    let args = install_claude(&harness);
    let gh = harness.root().join("gh");
    fs::write(
        &gh,
        "#!/bin/sh\nprintf '%s\\n' '{\"title\":\"Loader is slow\",\"body\":\"It reads twice.\",\"url\":\"https://github.com/Acme/Cache/issues/9\"}'\n",
    )
    .expect("fixture");
    fs::set_permissions(&gh, fs::Permissions::from_mode(0o755)).expect("fixture");
    let repo = test_support::init_repo().expect("git repo");
    let daemon = harness.boot_with(|cfg| cfg.github.executable = gh.to_string_lossy().into_owned());
    let client = connect(&daemon, "spec-run-issue");
    let workspace = common::add_main_workspace(&client, repo.path());
    let Response::SpecRunStarted {
        spec_run_id,
        session_id: None,
    } = client
        .request(Request::StartSpecRun {
            workspace_id: workspace,
            name: "slow loader".into(),
            source: SpecRunSource::Issue {
                reference: "#9".into(),
            },
            planner: pick(),
            coder: pick(),
            options: options(worktree("spec/issue")),
        })
        .expect("start")
    else {
        panic!()
    };
    let session_id = wait_drafter(&client, spec_run_id);
    let source =
        fs::read_to_string(run_dir(&harness, spec_run_id).join("source.md")).expect("fixture");
    assert!(
        source.starts_with("# Loader is slow\n\nIssue: https://github.com/Acme/Cache/issues/9"),
        "{source}"
    );
    let launched = args.join(session_id.to_string());
    assert!(common::poll_until(Duration::from_secs(10), || launched.exists()));
    let launch = fs::read_to_string(launched).expect("fixture");
    assert!(launch.contains("GitHub issue #9"), "{launch}");
    save(
        &client,
        spec_run_id,
        session_id,
        SpecArtifactKind::Exit,
        "intended_behavior\nThe loader reads twice by design: src/loader.rs:3",
    )
    .expect("early exit");
    assert_eq!(detail(&client, spec_run_id).run.stage, SpecStage::Exited);
}

fn round(
    client: &client::Client,
    id: SpecRunId,
    planner: SessionId,
    findings: Option<&str>,
) -> Result<Response, protocol::ProtocolError> {
    client
        .request(Request::SpecRound {
            spec_run_id: Some(id),
            caller_session_id: Some(planner),
            findings: findings.map(str::to_owned),
        })
        .map_err(|e| match e {
            client::ClientError::Protocol(error) => error,
            other => panic!("transport: {other:?}"),
        })
}

#[test]
fn rounds_are_ledger_attempts_and_the_full_route_runs_its_tasks_in_order() {
    let harness = common::Harness::new();
    install_claude(&harness);
    let repo = test_support::init_repo().expect("git repo");
    let daemon = harness.boot();
    let client = connect(&daemon, "spec-run-rounds");
    let workspace = common::add_main_workspace(&client, repo.path());
    let full = SPEC.replace("Route: standard", "Route: full");
    let plan_text = plan(&check("test -f done.txt"), &check("/usr/bin/grep -q fast done.txt"), Some("true"))
        .replace(
            "\"steps\"",
            "\"tasks\": [{\"id\": \"T1\", \"title\": \"write\", \"criteria\": [\"AC-1\"]}, \
{\"id\": \"T2\", \"title\": \"say fast\", \"criteria\": [\"AC-2\"], \"after\": [\"T1\"]}], \"steps\"",
        );
    let mut launch = options(worktree("spec/full"));
    launch.rounds = 2;

    let (id, drafter) = start(&client, workspace, launch);
    save(&client, id, drafter, SpecArtifactKind::Spec, &full).expect("spec");
    gate(&client, id, GateKind::Spec, approve()).expect("G1");
    let planner = wait_stage(&client, id, SpecStage::Planning)
        .session(SpecRole::Planner)
        .expect("fixture");
    save(&client, id, planner, SpecArtifactKind::Plan, &plan_text).expect("plan with tasks");
    gate(&client, id, GateKind::Plan, approve()).expect("G2");
    let run = wait_stage(&client, id, SpecStage::Executing);
    assert_eq!(run.tasks.len(), 2);
    let first = run.session(SpecRole::Coder).expect("fixture");
    coder_reports(&client, first);
    let run = wait_stage(&client, id, SpecStage::Reviewing);
    assert!(
        run.checks
            .iter()
            .all(|c| c.criterion != "AC-2" && c.criterion != "gate"),
        "task T1 checks only AC-1: {:?}",
        run.checks
    );
    let early = submit(&client, id, planner, &report("not_verified", &[])).expect_err("refused");
    assert_eq!(early.code, ErrorCode::PreconditionFailed);
    assert!(
        early
            .message
            .contains("start the next task before the report"),
        "{early:?}"
    );

    round(&client, id, planner, Some("1. done.txt is missing")).expect("round 2");
    let run = wait_stage(&client, id, SpecStage::Executing);
    let second = run.session(SpecRole::Coder).expect("fixture");
    assert_ne!(
        second, first,
        "a round is a new attempt with its own session"
    );
    assert_eq!(run.round, 2);
    coder_reports(&client, second);
    wait_stage(&client, id, SpecStage::Reviewing);
    let spent = round(&client, id, planner, Some("1. still missing")).expect_err("refused");
    assert_eq!(spent.code, ErrorCode::PreconditionFailed);
    assert!(spent.message.contains("round 2 of 2 is spent"), "{spent:?}");

    round(&client, id, planner, None).expect("accept T1");
    let run = wait_stage(&client, id, SpecStage::Executing);
    assert_eq!((run.current_task, run.round), (1, 1));
    coder_reports(&client, run.session(SpecRole::Coder).expect("fixture"));
    let run = wait_stage(&client, id, SpecStage::Reviewing);
    assert!(
        run.checks.iter().any(|c| c.criterion == "gate"),
        "the gate runs after the last task"
    );
    let last = round(&client, id, planner, None).expect_err("refused");
    assert!(last.message.contains("no further task"), "{last:?}");
}

/// An issue's drafter starts once `gh` has read the issue, after the request answered.
fn wait_drafter(client: &client::Client, id: SpecRunId) -> SessionId {
    let mut drafter = None;
    assert!(common::poll_until(Duration::from_secs(30), || {
        drafter = detail(client, id).run.session(SpecRole::Drafter);
        drafter.is_some()
    }));
    drafter.expect("drafter")
}

#[test]
fn an_approved_spec_edited_on_disk_stops_the_run_and_a_stuck_run_can_be_rejected() {
    let harness = common::Harness::new();
    install_claude(&harness);
    let repo = test_support::init_repo().expect("git repo");
    let daemon = harness.boot();
    let client = connect(&daemon, "spec-run-pinned");
    let workspace = common::add_main_workspace(&client, repo.path());
    let (id, run) = drive_to_review(
        &client,
        workspace,
        options(worktree("spec/pinned")),
        &plan(&check("true"), &check("true"), Some("true")),
        approve(),
        &[],
    );
    let planner = run.session(SpecRole::Planner).expect("planner");
    let spec = run_dir(&harness, id).join("spec.md");
    fs::write(&spec, SPEC.replace("AC-2", "AC-9")).expect("tamper");
    let tampered = submit(&client, id, planner, &report("met", &[])).expect_err("refused");
    assert!(
        tampered.message.contains("spec.md changed on disk"),
        "{tampered:?}"
    );

    let stuck = gate(
        &client,
        id,
        GateKind::Result,
        GateDecision::Reject {
            reason: "Stop: the spec was edited outside Forge.".into(),
        },
    );
    assert_eq!(stuck.expect("reject outside a gate"), Response::Ack);
    assert_eq!(detail(&client, id).run.stage, SpecStage::Rejected);
    assert!(prompts(&harness, id).contains("Stop: the spec was edited outside Forge."));
    let again = gate(
        &client,
        id,
        GateKind::Result,
        GateDecision::Reject { reason: "x".into() },
    )
    .expect_err("closed");
    assert_eq!(again.code, ErrorCode::Conflict);
}

#[test]
fn a_rejected_run_stops_its_coder_and_its_checks_and_stays_rejected() {
    let harness = common::Harness::new();
    install_claude(&harness);
    let repo = test_support::init_repo().expect("git repo");
    let daemon = harness.boot();
    let client = connect(&daemon, "spec-run-reject");
    let workspace = common::add_main_workspace(&client, repo.path());
    let reject = || GateDecision::Reject {
        reason: "Not this one.".into(),
    };
    let ended = |session: SessionId| {
        common::poll_until(Duration::from_secs(30), || {
            common::session(&client, session).is_none_or(|s| s.state.is_terminal())
        })
    };

    let (id, drafter) = start(&client, workspace, options(worktree("spec/reject-coder")));
    save(&client, id, drafter, SpecArtifactKind::Spec, SPEC).expect("spec saved");
    gate(&client, id, GateKind::Spec, approve()).expect("G1");
    let planner = wait_stage(&client, id, SpecStage::Planning)
        .session(SpecRole::Planner)
        .expect("planner");
    save(
        &client,
        id,
        planner,
        SpecArtifactKind::Plan,
        &plan(&check("true"), &check("true"), Some("true")),
    )
    .expect("plan saved");
    gate(&client, id, GateKind::Plan, approve()).expect("G2");
    let coder = wait_stage(&client, id, SpecStage::Executing)
        .session(SpecRole::Coder)
        .expect("coder");
    gate(&client, id, GateKind::Result, reject()).expect("reject while executing");
    assert!(ended(coder), "the coder kept running after the rejection");

    let (id, drafter) = start(&client, workspace, options(worktree("spec/reject-checks")));
    save(&client, id, drafter, SpecArtifactKind::Spec, SPEC).expect("spec saved");
    gate(&client, id, GateKind::Spec, approve()).expect("G1");
    let planner = wait_stage(&client, id, SpecStage::Planning)
        .session(SpecRole::Planner)
        .expect("planner");
    save(
        &client,
        id,
        planner,
        SpecArtifactKind::Plan,
        &plan(&check("/bin/sleep 300"), &check("true"), Some("true")),
    )
    .expect("plan saved");
    gate(&client, id, GateKind::Plan, approve()).expect("G2");
    let coder = wait_stage(&client, id, SpecStage::Executing)
        .session(SpecRole::Coder)
        .expect("coder");
    coder_reports(&client, coder);
    let mut checking = None;
    assert!(common::poll_until(Duration::from_secs(30), || {
        checking = detail(&client, id).run.session(SpecRole::Check);
        checking.is_some()
    }));
    gate(&client, id, GateKind::Result, reject()).expect("reject while verifying");
    assert!(
        ended(checking.expect("check")),
        "the check kept running after the rejection"
    );
    let log = run_dir(&harness, id).join("checks");
    assert!(common::poll_until(Duration::from_secs(30), || {
        fs::read_dir(&log).is_ok_and(|mut entries| {
            entries.any(|e| e.is_ok_and(|e| e.path().extension().is_some_and(|x| x == "log")))
        })
    }));
    // The checks' thread finishes after the log is written; a resurrected run would move on.
    let resurrected = common::poll_until(Duration::from_secs(2), || {
        detail(&client, id).run.stage != SpecStage::Rejected
    });
    let run = detail(&client, id).run;
    assert!(!resurrected, "{run:#?}");
    assert!(run.checks.is_empty(), "{:?}", run.checks);
    let handed: Vec<String> = fs::read_dir(run_dir(&harness, id).join("prompts"))
        .expect("fixture")
        .map(|e| {
            e.expect("fixture")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert!(
        !handed.iter().any(|name| name.contains("review-r")),
        "the planner was handed a review: {handed:?}"
    );
}
