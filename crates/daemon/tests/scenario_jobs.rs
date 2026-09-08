//! Headless agent runs end to end: a real child process, its real exit code.
//!
//! Everything here goes through the daemon with a fake provider CLI on the
//! test's own `PATH` (see `common`), so what is exercised is the actual spawn,
//! the actual stream reader and the actual reaping — not a mock of them. That
//! matters more for jobs than for most requests: the whole reason a job exists
//! is that "the process exited" is a fact somebody can act on, and a fact is
//! only worth having if it is really observed.

mod common;

use std::time::Duration;

use domain::{AgentProviderId, JobRequest, JobState, SessionRole};
use protocol::{DaemonEvent, ErrorCode};

/// The happy path, and the three things a caller actually needs from it: the
/// run reaches `Succeeded` on its own, the provider's own session id is read
/// out of the stream so a follow-up can resume it, and the stream is on disk.
#[test]
fn a_job_runs_to_completion_and_reports_how_it_ended() {
    let harness = common::Harness::new();
    let repo = test_support::init_repo().expect("git repo (git must be installed)");
    write_fake_headless_cli(harness.bin(), "claude", "Claude Code 2.1.0");

    let daemon = harness.boot();
    let client = daemon.connect("scenario-jobs");
    let events = client.events();
    let workspace_id = common::add_main_workspace(&client, repo.path());
    common::wait_for_detection(&client, |providers| {
        providers
            .iter()
            .any(|p| p.descriptor.id.as_str() == "claude" && p.detection.status.is_installed())
    });

    let job = client
        .start_job(JobRequest {
            provider_id: AgentProviderId::new("claude"),
            workspace_id,
            role: SessionRole::Reviewer,
            feature_id: Some(7),
            parent_session_id: None,
            prompt: "review the diff\nand say something".to_owned(),
            resume_from: None,
            schema: None,
        })
        .expect("StartJob");

    // Accepted, not necessarily started: the answer comes back before the
    // process does anything.
    assert!(matches!(job.state, JobState::Queued | JobState::Running));
    assert_eq!(job.summary, "review the diff", "the first line names it");
    assert_eq!(job.feature_id, Some(7));

    let finished = common::wait_for(
        &events,
        Duration::from_secs(20),
        |event| matches!(event, DaemonEvent::JobUpdated(j) if j.id == job.id && j.state.is_final()),
    )
    .expect("the job must reach a final state on its own");
    let DaemonEvent::JobUpdated(finished) = finished else {
        unreachable!("matched above")
    };

    assert_eq!(finished.state, JobState::Succeeded);
    assert_eq!(finished.exit_code, Some(0));
    assert!(finished.finished_at.is_some());
    // Read out of the event stream, which is what a follow-up job resumes.
    assert_eq!(finished.provider_session_id.as_deref(), Some("sess-abc"));

    // The file is the record: the provider's own words, untouched.
    let raw = std::fs::read_to_string(&finished.log_path).expect("the job log file");
    assert!(
        raw.contains("\"session_id\":\"sess-abc\""),
        "the file keeps what the provider wrote: {raw}"
    );
    // The CLI reports its own cwd, which must be the checkout it was asked for.
    let here = repo.path().canonicalize().expect("canonicalize the repo");
    assert!(
        raw.contains(&format!("\"cwd\":\"{}\"", here.display())),
        "the job runs in its workspace: {raw}"
    );

    // The request serves the readable form — one line per line of the stream,
    // which is what the Feature tab shows while a step runs.
    let (lines, done) = client.read_job_log(job.id, 0).expect("ReadJobLog");
    assert!(done, "a finished job says so");
    assert_eq!(lines.len(), raw.lines().count());
    assert!(
        lines.iter().any(|l| l == "result  done"),
        "a result line reads as its answer: {lines:?}"
    );
    // `from_line` skips what a follower already has.
    let (tail, _) = client.read_job_log(job.id, 1).expect("ReadJobLog from 1");
    assert_eq!(tail.len(), lines.len() - 1);

    // And it is in the list, and in a fresh client's snapshot.
    assert!(client
        .list_jobs()
        .expect("ListJobs")
        .iter()
        .any(|j| j.id == job.id));
}

/// A provider that exits non-zero fails the job rather than looking finished:
/// the harness has to be able to tell "reviewed" from "crashed".
#[test]
fn a_job_that_exits_non_zero_is_failed() {
    let harness = common::Harness::new();
    let repo = test_support::init_repo().expect("git repo (git must be installed)");
    write_failing_headless_cli(harness.bin(), "claude", "Claude Code 2.1.0");

    let daemon = harness.boot();
    let client = daemon.connect("scenario-jobs-fail");
    let events = client.events();
    let workspace_id = common::add_main_workspace(&client, repo.path());
    common::wait_for_detection(&client, |providers| {
        providers
            .iter()
            .any(|p| p.descriptor.id.as_str() == "claude" && p.detection.status.is_installed())
    });

    let job = client
        .start_job(JobRequest {
            provider_id: AgentProviderId::new("claude"),
            workspace_id,
            role: SessionRole::Executor,
            feature_id: None,
            parent_session_id: None,
            prompt: "do the thing".to_owned(),
            resume_from: None,
            schema: None,
        })
        .expect("StartJob");

    let finished = common::wait_for(
        &events,
        Duration::from_secs(20),
        |event| matches!(event, DaemonEvent::JobUpdated(j) if j.id == job.id && j.state.is_final()),
    )
    .expect("the job must end");
    let DaemonEvent::JobUpdated(finished) = finished else {
        unreachable!("matched above")
    };
    assert_eq!(finished.state, JobState::Failed);
    assert_eq!(finished.exit_code, Some(3));
}

/// Cancelling kills the process group, and the job says `Cancelled` rather
/// than inventing an outcome from the exit code of something we killed.
#[test]
fn cancelling_a_job_stops_it_and_says_so() {
    let harness = common::Harness::new();
    let repo = test_support::init_repo().expect("git repo (git must be installed)");
    write_slow_headless_cli(harness.bin(), "claude", "Claude Code 2.1.0");

    let daemon = harness.boot();
    let client = daemon.connect("scenario-jobs-cancel");
    let events = client.events();
    let workspace_id = common::add_main_workspace(&client, repo.path());
    common::wait_for_detection(&client, |providers| {
        providers
            .iter()
            .any(|p| p.descriptor.id.as_str() == "claude" && p.detection.status.is_installed())
    });

    let job = client
        .start_job(JobRequest {
            provider_id: AgentProviderId::new("claude"),
            workspace_id,
            role: SessionRole::Generic,
            feature_id: None,
            parent_session_id: None,
            prompt: "take your time".to_owned(),
            resume_from: None,
            schema: None,
        })
        .expect("StartJob");
    common::wait_for(&events, Duration::from_secs(10), |event| {
        matches!(event, DaemonEvent::JobUpdated(j) if j.id == job.id && j.state == JobState::Running)
    })
    .expect("the job must start");

    client.cancel_job(job.id).expect("CancelJob");
    let cancelled = common::wait_for(
        &events,
        Duration::from_secs(10),
        |event| matches!(event, DaemonEvent::JobUpdated(j) if j.id == job.id && j.state.is_final()),
    )
    .expect("a cancel must end the job");
    let DaemonEvent::JobUpdated(cancelled) = cancelled else {
        unreachable!("matched above")
    };
    assert_eq!(cancelled.state, JobState::Cancelled);

    // Cancelling again is not an error: the job is simply already over.
    client
        .cancel_job(job.id)
        .expect("a second cancel is a no-op");
}

/// A provider with no headless spelling is refused up front, with a reason,
/// instead of being launched with arguments guessed for it.
#[test]
fn a_provider_without_a_headless_mode_is_refused() {
    let harness = common::Harness::new();
    let repo = test_support::init_repo().expect("git repo (git must be installed)");
    // Cursor declares no headless spec; its candidate must pass the version
    // probe so the refusal is about the mode, not about detection.
    common::write_fake_agent_cli(
        harness.bin(),
        "agent",
        "forge_cursor",
        "cursor-agent 2026.1",
    );

    let daemon = harness.boot();
    let client = daemon.connect("scenario-jobs-refuse");
    let workspace_id = common::add_main_workspace(&client, repo.path());
    common::wait_for_detection(&client, |providers| {
        providers
            .iter()
            .any(|p| p.descriptor.id.as_str() == "cursor" && p.detection.status.is_installed())
    });

    let refused = client
        .start_job(JobRequest {
            provider_id: AgentProviderId::new("cursor"),
            workspace_id,
            role: SessionRole::Generic,
            feature_id: None,
            parent_session_id: None,
            prompt: "do the thing".to_owned(),
            resume_from: None,
            schema: None,
        })
        .expect_err("a provider with no headless mode must be refused");
    match &refused {
        client::ClientError::Protocol(error) => {
            assert_eq!(error.code, ErrorCode::InvalidRequest);
            assert!(
                error.message.contains("headless"),
                "the refusal says what is missing: {}",
                error.message
            );
        }
        other => panic!("expected a protocol error, got {other:?}"),
    }
}

// =====================================================================
// Fake provider CLIs
// =====================================================================

/// A fake `claude` that behaves like `claude -p --output-format stream-json`:
/// it emits a few JSON lines, one of them carrying a `session_id`, and exits 0.
///
/// It also reports its own working directory, which is how the test checks the
/// job really ran in the checkout it was asked for.
fn write_fake_headless_cli(dir: &std::path::Path, name: &str, version: &str) -> std::path::PathBuf {
    let script = format!(
        "#!/bin/sh\n\
         if [ \"$1\" = --version ]; then printf '%s\\n' {version}; exit 0; fi\n\
         here=$(/bin/pwd -P)\n\
         printf '{{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"sess-abc\",\"cwd\":\"%s\"}}\\n' \"$here\"\n\
         printf '{{\"type\":\"assistant\",\"session_id\":\"sess-abc\"}}\\n'\n\
         printf '{{\"type\":\"result\",\"session_id\":\"sess-abc\",\"result\":\"done\"}}\\n'\n\
         exit 0\n",
        version = sh_quote(version),
    );
    write_executable(dir, name, &script)
}

/// A fake `claude` that fails the way a real one does: it says something on
/// stdout first, then exits non-zero.
fn write_failing_headless_cli(
    dir: &std::path::Path,
    name: &str,
    version: &str,
) -> std::path::PathBuf {
    let script = format!(
        "#!/bin/sh\n\
         if [ \"$1\" = --version ]; then printf '%s\\n' {version}; exit 0; fi\n\
         printf '{{\"type\":\"result\",\"is_error\":true}}\\n'\n\
         printf 'boom\\n' >&2\n\
         exit 3\n",
        version = sh_quote(version),
    );
    write_executable(dir, name, &script)
}

/// A fake `claude` that never finishes on its own, so a cancel has something
/// to cancel.
///
/// `/bin/sleep`, absolute: the job's `PATH` is the one hermetic `bin` the
/// harness wrote, which holds `env` and the fakes and nothing else. A bare
/// `sleep` exits 127 straight away and the test then races a job that already
/// failed against the cancel it is supposed to be testing.
fn write_slow_headless_cli(dir: &std::path::Path, name: &str, version: &str) -> std::path::PathBuf {
    let script = format!(
        "#!/bin/sh\n\
         if [ \"$1\" = --version ]; then printf '%s\\n' {version}; exit 0; fi\n\
         printf '{{\"type\":\"system\",\"session_id\":\"sess-slow\"}}\\n'\n\
         /bin/sleep 300\n",
        version = sh_quote(version),
    );
    write_executable(dir, name, &script)
}

fn sh_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

fn write_executable(dir: &std::path::Path, name: &str, script: &str) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join(name);
    std::fs::write(&path, script).expect("write the fake CLI");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
        .expect("make the fake CLI executable");
    path
}
