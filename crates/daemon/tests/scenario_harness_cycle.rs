//! The harness cycle running itself: spec → human gate → implement → review.
//!
//! What is being tested is the property the whole design rests on — that a
//! step *ends observably*, so the next one can start without anybody watching
//! a terminal. The fake agents here are ordinary shell scripts that write the
//! artefacts a real agent would write and then exit; the daemon reads their
//! exit codes and moves the feature. Nothing in this file tells the daemon
//! what to do next, which is the point.

mod common;

use std::path::Path;
use std::time::Duration;

use domain::{HarnessAdvanceAction, HarnessStep};
use protocol::{Request, Response};

/// The full loop, end to end, with the gate in the middle.
#[test]
fn a_feature_walks_the_whole_cycle_and_stops_only_at_the_human_gate() {
    let harness = common::Harness::new();
    let repo = test_support::init_repo().expect("git repo (git must be installed)");
    init_harness_dir(repo.path());
    // One agent plays every role: which artefacts it writes depends on the
    // prompt it is given, exactly as a real one's behaviour does.
    write_cycle_agent(harness.bin(), "claude", "Claude Code 2.1.0");

    let daemon = harness.boot();
    let client = daemon.connect("scenario-cycle");
    let workspace_id = common::add_main_workspace(&client, repo.path());
    common::wait_for_detection(&client, |providers| {
        providers
            .iter()
            .any(|p| p.descriptor.id.as_str() == "claude" && p.detection.status.is_installed())
    });
    let project_id = project_id(&client);
    client
        .set_app_state("ui.harness.orchestrator", "provider:claude")
        .expect("pick the agent for every step");
    client
        .set_app_state("ui.harness.executor", "provider:claude")
        .expect("executor");
    client
        .set_app_state("ui.harness.reviewer", "provider:claude")
        .expect("reviewer");

    let feature = client
        .register_harness_feature(
            project_id,
            Some(workspace_id),
            "wire the stats command".to_owned(),
            Some("Wire stats".to_owned()),
        )
        .expect("RegisterHarnessFeature");
    assert_eq!(feature.status, "pending");

    // --- the spec step runs itself into the gate ---
    client
        .run_harness_step(project_id, feature.id, HarnessStep::Spec)
        .expect("RunHarnessStep(Spec)");
    wait_for_status(&client, project_id, feature.id, "spec_ready");

    // The gate really is a stop: nothing advances while nobody approves.
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(
        status(&client, project_id, feature.id),
        "spec_ready",
        "the cycle must wait for a person at the gate"
    );
    let timeline = client
        .get_harness_timeline(project_id, feature.id)
        .expect("GetHarnessTimeline");
    assert!(
        timeline.iter().any(|e| e.kind == "human_gate_opened"),
        "the gate is announced in the log: {timeline:?}"
    );

    // The gate is not a convention: asking for the implementation while the
    // spec is unapproved is refused by the transition table, not obeyed.
    let refused = client
        .run_harness_step(project_id, feature.id, HarnessStep::Implement)
        .expect_err("Implement before an approval must be refused");
    match &refused {
        client::ClientError::Protocol(error) => assert!(
            error.message.contains("spec_ready"),
            "the refusal names the status it refused from: {}",
            error.message
        ),
        other => panic!("expected a protocol error, got {other:?}"),
    }

    // --- approving starts the implementation, and the rest follows ---
    client
        .harness_advance(
            project_id,
            feature.id,
            None,
            HarnessAdvanceAction::ApproveSpec,
        )
        .expect("ApproveSpec");

    // Replaying the same approval against the revision the client first saw is
    // a no-op: it does not resolve a second gate or start a second implementer.
    let replay = client
        .harness_advance(
            project_id,
            feature.id,
            Some(0),
            HarnessAdvanceAction::ApproveSpec,
        )
        .expect("a stale approval is answered, not obeyed");
    assert_ne!(replay.status, "spec_ready");
    wait_for_status(&client, project_id, feature.id, "done");

    let timeline = client
        .get_harness_timeline(project_id, feature.id)
        .expect("GetHarnessTimeline");
    let kinds: Vec<&str> = timeline.iter().map(|e| e.kind.as_str()).collect();
    for expected in [
        "feature_registered",
        "spec_started",
        "human_gate_opened",
        "human_gate_resolved",
        "impl_started",
        "impl_done",
        "review_started",
        "review_verdict",
        "feature_done",
    ] {
        assert!(
            kinds.contains(&expected),
            "the log must record {expected}: {kinds:?}"
        );
    }
    // Three jobs ran, one per step, and each is on record with its exit code.
    let jobs = client.list_jobs().expect("ListJobs");
    assert_eq!(jobs.len(), 3, "one job per step: {jobs:?}");
    // Every attempt is written down, with the provider that ran it.
    let done = client
        .get_harness_feature(project_id, feature.id)
        .expect("GetHarnessFeature");
    let attempts = done.attempts.clone().unwrap_or_default();
    assert_eq!(attempts.len(), 3, "one attempt per step: {attempts:?}");
    assert!(
        attempts
            .iter()
            .all(|a| a.provider.as_deref() == Some("claude") && a.settled_at.is_some()),
        "each attempt names its provider and is settled: {attempts:?}"
    );
    assert!(done.revision.unwrap_or(0) > 0, "the row carries a revision");
    assert!(jobs.iter().all(|job| job.exit_code == Some(0)));
    assert!(jobs.iter().all(|job| job.feature_id == Some(feature.id)));
}

/// A step whose agent fails blocks the feature instead of letting the cycle
/// walk past it — the difference between "reviewed" and "never ran".
#[test]
fn a_failing_step_blocks_the_feature_with_a_reason() {
    let harness = common::Harness::new();
    let repo = test_support::init_repo().expect("git repo (git must be installed)");
    init_harness_dir(repo.path());
    write_failing_agent(harness.bin(), "claude", "Claude Code 2.1.0");

    let daemon = harness.boot();
    let client = daemon.connect("scenario-cycle-fail");
    let workspace_id = common::add_main_workspace(&client, repo.path());
    common::wait_for_detection(&client, |providers| {
        providers
            .iter()
            .any(|p| p.descriptor.id.as_str() == "claude" && p.detection.status.is_installed())
    });
    let project_id = project_id(&client);
    client
        .set_app_state("ui.harness.orchestrator", "provider:claude")
        .expect("orchestrator");

    let feature = client
        .register_harness_feature(
            project_id,
            Some(workspace_id),
            "something that will fail".to_owned(),
            None,
        )
        .expect("RegisterHarnessFeature");
    client
        .run_harness_step(project_id, feature.id, HarnessStep::Spec)
        .expect("RunHarnessStep(Spec)");

    wait_for_status(&client, project_id, feature.id, "blocked");
    let timeline = client
        .get_harness_timeline(project_id, feature.id)
        .expect("GetHarnessTimeline");
    let blocked = timeline
        .iter()
        .find(|e| e.kind == "feature_blocked")
        .expect("the block is recorded");
    assert!(
        blocked.detail.contains("exited"),
        "the reason names the exit: {}",
        blocked.detail
    );
}

/// A spec step that exits 0 without writing its documents blocks instead of
/// opening an empty gate: exit 0 alone does not say the spec was written.
#[test]
fn a_spec_step_that_writes_nothing_blocks_instead_of_opening_the_gate() {
    let harness = common::Harness::new();
    let repo = test_support::init_repo().expect("git repo (git must be installed)");
    init_harness_dir(repo.path());
    write_silent_agent(harness.bin(), "claude", "Claude Code 2.1.0");

    let daemon = harness.boot();
    let client = daemon.connect("scenario-cycle-silent");
    let workspace_id = common::add_main_workspace(&client, repo.path());
    common::wait_for_detection(&client, |providers| {
        providers
            .iter()
            .any(|p| p.descriptor.id.as_str() == "claude" && p.detection.status.is_installed())
    });
    let project_id = project_id(&client);
    client
        .set_app_state("ui.harness.orchestrator", "provider:claude")
        .expect("orchestrator");

    let feature = client
        .register_harness_feature(
            project_id,
            Some(workspace_id),
            "silent work".to_owned(),
            None,
        )
        .expect("RegisterHarnessFeature");
    client
        .run_harness_step(project_id, feature.id, HarnessStep::Spec)
        .expect("RunHarnessStep(Spec)");

    wait_for_status(&client, project_id, feature.id, "blocked");
    let row = client
        .get_harness_feature(project_id, feature.id)
        .expect("GetHarnessFeature");
    let reason = row.blocked_reason.unwrap_or_default();
    assert!(
        reason.contains(&format!("gate_{}.md", feature.id)),
        "the reason names the missing gate document: {reason}"
    );
    let timeline = client
        .get_harness_timeline(project_id, feature.id)
        .expect("GetHarnessTimeline");
    assert!(
        !timeline.iter().any(|e| e.kind == "human_gate_opened"),
        "no gate opens with nothing to read: {timeline:?}"
    );
}

/// A feature registered without a checkout cannot be run: there is nowhere to
/// run it. Refused up front rather than started in an arbitrary directory.
#[test]
fn a_feature_with_no_checkout_cannot_be_run() {
    let harness = common::Harness::new();
    let repo = test_support::init_repo().expect("git repo (git must be installed)");
    init_harness_dir(repo.path());
    write_cycle_agent(harness.bin(), "claude", "Claude Code 2.1.0");

    let daemon = harness.boot();
    let client = daemon.connect("scenario-cycle-unbound");
    common::add_main_workspace(&client, repo.path());
    let project_id = project_id(&client);

    // No workspace: the shape a `scripts/harness` registration leaves behind.
    let feature = client
        .register_harness_feature(project_id, None, "from the cli".to_owned(), None)
        .expect("RegisterHarnessFeature");
    let refused = client
        .run_harness_step(project_id, feature.id, HarnessStep::Spec)
        .expect_err("a feature bound to no checkout must be refused");
    match &refused {
        client::ClientError::Protocol(error) => assert!(
            error.message.contains("checkout"),
            "the refusal says what is missing: {}",
            error.message
        ),
        other => panic!("expected a protocol error, got {other:?}"),
    }
}

/// Two requests for one feature produce one job.
///
/// `DEFAULT_MAX_CONCURRENT_JOBS` is a global ceiling and says nothing about
/// *which* jobs: before the single-flight check, a click on *Run implement*
/// racing the automatic chain put two agents in one worktree, editing the same
/// files.
#[test]
fn two_requests_for_one_feature_produce_one_job() {
    let harness = common::Harness::new();
    let repo = test_support::init_repo().expect("git repo (git must be installed)");
    init_harness_dir(repo.path());
    write_slow_agent(harness.bin(), "claude", "Claude Code 2.1.0");

    let daemon = harness.boot();
    let client = daemon.connect("scenario-single-flight");
    let workspace_id = common::add_main_workspace(&client, repo.path());
    common::wait_for_detection(&client, |providers| {
        providers
            .iter()
            .any(|p| p.descriptor.id.as_str() == "claude" && p.detection.status.is_installed())
    });
    let project_id = project_id(&client);
    client
        .set_app_state("ui.harness.orchestrator", "provider:claude")
        .expect("orchestrator");

    let feature = client
        .register_harness_feature(project_id, Some(workspace_id), "slow work".to_owned(), None)
        .expect("RegisterHarnessFeature");
    client
        .run_harness_step(project_id, feature.id, HarnessStep::Spec)
        .expect("the first step starts");
    let refused = client
        .run_harness_step(project_id, feature.id, HarnessStep::Spec)
        .expect_err("the second must be refused while the first runs");
    match &refused {
        client::ClientError::Protocol(error) => assert!(
            error.message.contains("already has a step running"),
            "the refusal says why: {}",
            error.message
        ),
        other => panic!("expected a protocol error, got {other:?}"),
    }
    assert_eq!(client.list_jobs().expect("ListJobs").len(), 1);
}

/// A step whose agent fails once is retried, and the cycle carries on.
///
/// The distinction `max_step_attempts` buys: a rate limit and "I could not do
/// this" used to be the same outcome, and both stopped the feature dead.
#[test]
fn a_step_that_fails_once_is_retried_rather_than_blocking() {
    let harness = common::Harness::new();
    let repo = test_support::init_repo().expect("git repo (git must be installed)");
    init_harness_dir(repo.path());
    write_flaky_agent(harness.bin(), "claude", "Claude Code 2.1.0");

    let daemon = harness.boot();
    let client = daemon.connect("scenario-retry");
    let workspace_id = common::add_main_workspace(&client, repo.path());
    common::wait_for_detection(&client, |providers| {
        providers
            .iter()
            .any(|p| p.descriptor.id.as_str() == "claude" && p.detection.status.is_installed())
    });
    let project_id = project_id(&client);
    client
        .set_app_state("ui.harness.orchestrator", "provider:claude")
        .expect("orchestrator");

    let feature = client
        .register_harness_feature(
            project_id,
            Some(workspace_id),
            "flaky work".to_owned(),
            None,
        )
        .expect("RegisterHarnessFeature");
    client
        .run_harness_step(project_id, feature.id, HarnessStep::Spec)
        .expect("RunHarnessStep(Spec)");

    wait_for_status(&client, project_id, feature.id, "spec_ready");
    let timeline = client
        .get_harness_timeline(project_id, feature.id)
        .expect("GetHarnessTimeline");
    assert!(
        timeline.iter().any(|e| e.kind == "step_retried"),
        "the retry is on the record, not silent: {timeline:#?}"
    );
    let row = client
        .get_harness_feature(project_id, feature.id)
        .expect("GetHarnessFeature");
    let attempts = row.attempts.unwrap_or_default();
    assert_eq!(attempts.len(), 2, "a retry is a new attempt: {attempts:?}");
    assert_eq!(attempts[0].outcome.as_deref(), Some("failed"));
    assert_eq!(attempts[1].outcome.as_deref(), Some("succeeded"));
}

// =====================================================================
// Helpers
// =====================================================================

fn project_id(client: &client::Client) -> domain::ProjectId {
    let Response::Snapshot { projects, .. } =
        client.request(Request::GetSnapshot).expect("GetSnapshot")
    else {
        panic!("expected Snapshot");
    };
    projects.first().expect("one project").id
}

fn status(client: &client::Client, project_id: domain::ProjectId, feature_id: u32) -> String {
    client
        .get_harness_feature(project_id, feature_id)
        .expect("GetHarnessFeature")
        .status
}

/// Poll until the feature reaches `want`, or fail saying where it stopped.
///
/// Polling rather than waiting on an event because what is being asserted is
/// the state *on disk*: the daemon writes it, and a client that reads it back
/// is the same thing the GUI does.
fn wait_for_status(
    client: &client::Client,
    project_id: domain::ProjectId,
    feature_id: u32,
    want: &str,
) {
    let reached = common::poll_until(Duration::from_secs(30), || {
        status(client, project_id, feature_id) == want
    });
    // The timeline goes into the failure: "it is blocked" alone never says
    // which step gave up, and that is always the next question.
    assert!(
        reached,
        "feature {feature_id} never reached '{want}'; it is '{}'\n{:#?}",
        status(client, project_id, feature_id),
        client
            .get_harness_timeline(project_id, feature_id)
            .unwrap_or_default()
    );
}

/// The minimum `harness/` a repository needs to be initialised.
fn init_harness_dir(repo: &Path) {
    std::fs::create_dir_all(repo.join("harness/progress")).expect("harness/progress");
    std::fs::create_dir_all(repo.join("harness/specs")).expect("harness/specs");
    std::fs::write(
        repo.join("harness/features.json"),
        r#"{"project":"t","rules":{"max_review_rounds":2},"features":[]}"#,
    )
    .expect("features.json");
}

/// A fake agent that plays every role, deciding from its prompt which
/// artefacts to write — the spec files, the impl note, or a review that
/// approves. It writes into `$FORGE_HARNESS_ROOT`, which is how a real agent
/// in a worktree reaches the repository's shared harness state.
fn write_cycle_agent(dir: &Path, name: &str, version: &str) -> std::path::PathBuf {
    // Parameter expansion only, and absolute paths for the one external
    // command: the harness gives a daemon a `PATH` with nothing on it but the
    // fixtures, so a fake that reached for `sed` would quietly produce empty
    // strings and write its artefacts under the wrong name.
    let script = format!(
        "#!/bin/sh\n\
         if [ \"$1\" = --version ]; then printf '%s\\n' {version}; exit 0; fi\n\
         prompt=\"$*\"\n\
         root=${{FORGE_HARNESS_ROOT:-$(pwd)}}\n\
         rest=${{prompt#*harness feature }}\n\
         id=${{rest%% *}}\n\
         rest=${{prompt#*harness/specs/}}\n\
         spec_dir=${{rest%%/*}}\n\
         printf '{{\"type\":\"system\",\"session_id\":\"sess-%s\"}}\\n' \"$id\"\n\
         case \"$prompt\" in\n\
         \x20 *'/feature skill'*)\n\
         \x20   /bin/mkdir -p \"$root/harness/specs/$spec_dir\"\n\
         \x20   for f in requirements.md design.md tasks.md; do\n\
         \x20     printf 'spec\\n' > \"$root/harness/specs/$spec_dir/$f\"\n\
         \x20   done\n\
         \x20   printf 'gate\\n' > \"$root/harness/progress/gate_$id.md\"\n\
         \x20   ;;\n\
         \x20 *'/feature-go'*)\n\
         \x20   printf 'context\\n' > \"$root/harness/progress/context_$id.md\"\n\
         \x20   printf 'implemented\\n' > \"$root/harness/progress/impl_$id.md\"\n\
         \x20   ;;\n\
         \x20 *Review*)\n\
         \x20   printf 'looks right\\nAPPROVED\\n' > \"$root/harness/progress/review_$id.md\"\n\
         \x20   ;;\n\
         esac\n\
         printf '{{\"type\":\"result\",\"session_id\":\"sess-%s\"}}\\n' \"$id\"\n\
         exit 0\n",
        version = sh_quote(version),
    );
    write_executable(dir, name, &script)
}

/// A fake agent that stays alive long enough for a second request to race it.
fn write_slow_agent(dir: &Path, name: &str, version: &str) -> std::path::PathBuf {
    let script = format!(
        "#!/bin/sh\n\
         if [ \"$1\" = --version ]; then printf '%s\\n' {version}; exit 0; fi\n\
         printf '{{\"type\":\"system\"}}\\n'\n\
         /bin/sleep 5\n\
         exit 0\n",
        version = sh_quote(version),
    );
    write_executable(dir, name, &script)
}

/// A fake agent that fails its first run and writes the spec on its second.
///
/// The counter is a file in the harness root, which both runs share.
fn write_flaky_agent(dir: &Path, name: &str, version: &str) -> std::path::PathBuf {
    let script = format!(
        "#!/bin/sh\n\
         if [ \"$1\" = --version ]; then printf '%s\\n' {version}; exit 0; fi\n\
         prompt=\"$*\"\n\
         root=${{FORGE_HARNESS_ROOT:-$(pwd)}}\n\
         rest=${{prompt#*harness feature }}\n\
         id=${{rest%% *}}\n\
         rest=${{prompt#*harness/specs/}}\n\
         spec_dir=${{rest%%/*}}\n\
         marker=\"$root/harness/progress/.tried_$id\"\n\
         if [ ! -f \"$marker\" ]; then\n\
         \x20 printf 'x' > \"$marker\"\n\
         \x20 printf '{{\"type\":\"result\",\"is_error\":true}}\\n'\n\
         \x20 exit 2\n\
         fi\n\
         /bin/mkdir -p \"$root/harness/specs/$spec_dir\"\n\
         for f in requirements.md design.md tasks.md; do\n\
         \x20 printf 'spec\\n' > \"$root/harness/specs/$spec_dir/$f\"\n\
         done\n\
         printf 'gate\\n' > \"$root/harness/progress/gate_$id.md\"\n\
         printf '{{\"type\":\"result\"}}\\n'\n\
         exit 0\n",
        version = sh_quote(version),
    );
    write_executable(dir, name, &script)
}

/// A fake agent that always fails, for the blocked path.
fn write_failing_agent(dir: &Path, name: &str, version: &str) -> std::path::PathBuf {
    let script = format!(
        "#!/bin/sh\n\
         if [ \"$1\" = --version ]; then printf '%s\\n' {version}; exit 0; fi\n\
         printf '{{\"type\":\"result\",\"is_error\":true}}\\n'\n\
         exit 2\n",
        version = sh_quote(version),
    );
    write_executable(dir, name, &script)
}

/// A fake agent that exits 0 while writing nothing: the sandbox case, where
/// the step "succeeded" and left no gate documents behind.
fn write_silent_agent(dir: &Path, name: &str, version: &str) -> std::path::PathBuf {
    let script = format!(
        "#!/bin/sh\n\
         if [ \"$1\" = --version ]; then printf '%s\\n' {version}; exit 0; fi\n\
         printf '{{\"type\":\"result\"}}\\n'\n\
         exit 0\n",
        version = sh_quote(version),
    );
    write_executable(dir, name, &script)
}

fn sh_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

fn write_executable(dir: &Path, name: &str, script: &str) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join(name);
    std::fs::write(&path, script).expect("write the fake CLI");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
        .expect("make the fake CLI executable");
    path
}
