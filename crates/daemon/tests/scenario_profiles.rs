//! Launch profiles end to end (§13.4).
//!
//! A profile is the saved form of what a hand-written shell wrapper did:
//! `CLAUDE_CONFIG_DIR=~/.claude-work claude --model opus`. What matters is
//! that the *child process* really receives that environment and those
//! arguments, that the directory exists before the agent starts, and that a
//! restart reproduces all of it — none of which a unit test over the builder
//! can show.

mod common;

use std::path::Path;

use domain::{AgentProfile, AgentProfileId, AgentProviderId, SessionRole, Timestamp};
use protocol::{DaemonEvent, ErrorCode, Request, Response};

const MARKER: &str = "forge_profile";

fn profile(name: &str, config_dir: &Path, args: &[&str]) -> AgentProfile {
    AgentProfile {
        id: AgentProfileId::new(),
        provider_id: AgentProviderId::new("claude"),
        name: name.to_owned(),
        executable: None,
        args: args.iter().map(|arg| (*arg).to_owned()).collect(),
        env: vec![(
            "CLAUDE_CONFIG_DIR".to_owned(),
            config_dir.to_string_lossy().into_owned(),
        )],
        created_at: Timestamp::now(),
    }
}

/// The whole point of §13.4: the agent runs with the profile's environment and
/// arguments, its config directory is created first, and a restart repeats it.
#[test]
fn a_profile_launches_the_agent_with_its_own_environment_and_arguments() {
    let harness = common::Harness::new();
    let repo = test_support::init_repo().expect("git repo (git must be installed)");
    common::write_env_reporting_cli(harness.bin(), "claude", MARKER, "Claude Code 1.0.0");

    let daemon = harness.boot();
    let client = daemon.connect("profiles");
    let events = client.events();
    let workspace_id = common::add_main_workspace(&client, repo.path());
    common::wait_for_detection(&client, |providers| {
        providers
            .iter()
            .any(|p| p.descriptor.id.as_str() == "claude" && p.detection.status.is_installed())
    });

    // Deliberately not created by the test: the daemon must create it, the way
    // the shell wrapper's `mkdir -p` did.
    let config_dir = harness.root().join("claude-work");
    let work = profile("Work", &config_dir, &["--model", "opus"]);

    assert_eq!(
        client
            .request(Request::SaveAgentProfile {
                profile: work.clone()
            })
            .expect("SaveAgentProfile"),
        Response::Ack
    );
    let broadcast = common::wait_for(&events, std::time::Duration::from_secs(5), |event| {
        matches!(event, DaemonEvent::AgentProfilesChanged { profiles }
            if profiles.iter().any(|p| p.id == work.id))
    });
    assert!(
        broadcast.is_some(),
        "saving a profile is broadcast to every client"
    );

    // --- launch through the profile ---
    let response = client
        .request(Request::CreateAgentSession {
            workspace_id,
            provider_id: AgentProviderId::new("claude"),
            profile_id: Some(work.id),
            parent: None,
            role: SessionRole::Generic,
            resume: None,
            initial_prompt: None,
            read_only: false,
        })
        .expect("CreateAgentSession with a profile");
    assert!(
        matches!(response, Response::SessionCreated { .. }),
        "{response:?}"
    );

    let (session_id, terminal_id) =
        common::wait_for_running(&events, |session| session.agent_profile_id == Some(work.id));
    assert_eq!(
        common::attach_and_read_line(&client, &events, terminal_id, MARKER).as_deref(),
        Some("forge_profile:claude-work:--model opus"),
        "the child gets the profile's environment and arguments"
    );
    assert!(
        config_dir.is_dir(),
        "the profile's config directory is created before the agent starts"
    );

    // --- restart reproduces the profile ---
    client.kill_session(session_id).expect("kill the session");
    common::wait_for(&events, std::time::Duration::from_secs(10), |event| {
        matches!(event, DaemonEvent::SessionUpdated(session)
            if session.id == session_id && session.state.is_terminal())
    })
    .expect("the killed session reaches a terminal state");

    client
        .request(Request::RestartSession { session_id })
        .expect("RestartSession");
    let (_, restarted_terminal) =
        common::wait_for_running(&events, |session| session.id == session_id);
    assert_eq!(
        common::attach_and_read_line(&client, &events, restarted_terminal, MARKER).as_deref(),
        Some("forge_profile:claude-work:--model opus"),
        "a restart re-applies the profile rather than the bare provider"
    );

    // --- deleting the profile leaves the running session alone ---
    assert_eq!(
        client
            .request(Request::RemoveAgentProfile {
                profile_id: work.id
            })
            .expect("RemoveAgentProfile"),
        Response::Ack
    );
    let Response::Snapshot {
        agent_profiles,
        sessions,
        ..
    } = client.request(Request::GetSnapshot).expect("GetSnapshot")
    else {
        panic!("GetSnapshot must answer with a snapshot");
    };
    assert!(agent_profiles.is_empty());
    let session = sessions
        .iter()
        .find(|session| session.id == session_id)
        .expect("the session outlives its profile");
    assert!(session.state.is_active(), "the process keeps running");
    assert_eq!(
        session.agent_profile_id,
        Some(work.id),
        "history keeps the pointer even though the profile is gone"
    );
}

/// Everything a profile can get wrong is refused where the user can still see
/// the form, not later when a session fails to start.
#[test]
fn saving_a_profile_rejects_what_a_launch_could_not_honor() {
    let harness = common::Harness::new();
    common::write_env_reporting_cli(harness.bin(), "claude", MARKER, "Claude Code 1.0.0");
    let daemon = harness.boot();
    let client = daemon.connect("profile-validation");
    common::wait_for_detection(&client, |providers| {
        providers
            .iter()
            .any(|p| p.descriptor.id.as_str() == "claude" && p.detection.status.is_installed())
    });

    let dir = harness.root().join("cfg");
    let save = |profile: AgentProfile| client.request(Request::SaveAgentProfile { profile });

    save(profile("Work", &dir, &[])).expect("the first profile saves");

    // A second profile of the same agent cannot share its name, whatever the
    // case: the two would be indistinguishable in a launch menu.
    let clash = save(profile("work", &dir, &[])).expect_err("a duplicate name is refused");
    assert_eq!(protocol_code(&clash), ErrorCode::Conflict);

    // Forge owns the terminal contract; a profile that redefined it would break
    // the emulator instead of configuring the agent.
    let mut reserved = profile("Reserved", &dir, &[]);
    reserved.env = vec![("TERM".to_owned(), "dumb".to_owned())];
    let refused = save(reserved).expect_err("a reserved variable is refused");
    assert_eq!(protocol_code(&refused), ErrorCode::InvalidRequest);

    let mut malformed = profile("Malformed", &dir, &[]);
    malformed.env = vec![("NOT A NAME".to_owned(), "x".to_owned())];
    let refused = save(malformed).expect_err("a malformed variable name is refused");
    assert_eq!(protocol_code(&refused), ErrorCode::InvalidRequest);

    let mut unnamed = profile("   ", &dir, &[]);
    unnamed.env = vec![];
    let refused = save(unnamed).expect_err("a nameless profile is refused");
    assert_eq!(protocol_code(&refused), ErrorCode::InvalidRequest);

    // A profile may bring its own binary, but only one the version probe
    // accepts — the same rule `SetProviderExecutable` follows (§13.1 step 4).
    let mut foreign = profile("Foreign", &dir, &[]);
    foreign.executable = Some(harness.root().join("nothing-here"));
    let refused = save(foreign).expect_err("an executable that is not there is refused");
    assert_eq!(protocol_code(&refused), ErrorCode::ProviderNotInstalled);

    // ...and a profile pointing at a real, probed binary is accepted.
    let elsewhere = harness.root().join("elsewhere");
    std::fs::create_dir_all(&elsewhere).expect("create the off-PATH directory");
    let picked = common::write_env_reporting_cli(&elsewhere, "claude", MARKER, "Claude Code 2.0.0");
    let mut own_binary = profile("Own binary", &dir, &[]);
    own_binary.executable = Some(picked);
    save(own_binary).expect("a probed executable is accepted");
}

/// A profile belongs to one provider: its arguments mean nothing to another.
#[test]
fn a_profile_cannot_be_applied_to_a_different_provider() {
    let harness = common::Harness::new();
    let repo = test_support::init_repo().expect("git repo (git must be installed)");
    common::write_env_reporting_cli(harness.bin(), "claude", MARKER, "Claude Code 1.0.0");
    common::write_fake_agent_cli(harness.bin(), "codex", "forge_codex", "codex 1.0.0");

    let daemon = harness.boot();
    let client = daemon.connect("profile-mismatch");
    let workspace_id = common::add_main_workspace(&client, repo.path());
    common::wait_for_detection(&client, |providers| {
        providers.iter().all(|p| match p.descriptor.id.as_str() {
            "claude" | "codex" => p.detection.status.is_installed(),
            _ => true,
        })
    });

    let work = profile("Work", &harness.root().join("cfg"), &[]);
    client
        .request(Request::SaveAgentProfile {
            profile: work.clone(),
        })
        .expect("SaveAgentProfile");

    let refused = client
        .request(Request::CreateAgentSession {
            workspace_id,
            provider_id: AgentProviderId::new("codex"),
            profile_id: Some(work.id),
            parent: None,
            role: SessionRole::Generic,
            resume: None,
            initial_prompt: None,
            read_only: false,
        })
        .expect_err("a Claude profile cannot launch Codex");
    assert_eq!(protocol_code(&refused), ErrorCode::InvalidRequest);

    let unknown = client
        .request(Request::CreateAgentSession {
            workspace_id,
            provider_id: AgentProviderId::new("claude"),
            profile_id: Some(AgentProfileId::new()),
            parent: None,
            role: SessionRole::Generic,
            resume: None,
            initial_prompt: None,
            read_only: false,
        })
        .expect_err("an unknown profile id is not silently ignored");
    assert_eq!(protocol_code(&unknown), ErrorCode::NotFound);
}

fn protocol_code(error: &client::ClientError) -> ErrorCode {
    match error {
        client::ClientError::Protocol(error) => error.code,
        other => panic!("expected a protocol error, got {other:?}"),
    }
}
