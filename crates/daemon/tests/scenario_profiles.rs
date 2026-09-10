//! Launch profiles end to end (§13.4).
//!
//! A profile is the saved form of what a hand-written shell wrapper did:
//! `CLAUDE_CONFIG_DIR=~/.claude-personal claude --model opus`. What matters is
//! that the *child process* really receives that directory and those
//! arguments, that the directory exists before the agent starts, and that a
//! restart reproduces all of it — none of which a unit test over the builder
//! can show.

mod common;

use std::path::PathBuf;

use domain::{AgentProfile, AgentProfileId, AgentProviderId, SessionRole, Timestamp};
use protocol::{DaemonEvent, ErrorCode, Request, Response};

const MARKER: &str = "forge_profile";

fn profile(name: &str, config_dir: Option<&str>, args: &[&str]) -> AgentProfile {
    AgentProfile {
        id: AgentProfileId::new(),
        provider_id: AgentProviderId::new("claude"),
        name: name.to_owned(),
        executable: None,
        config_dir: config_dir.map(PathBuf::from),
        args: args.iter().map(|arg| (*arg).to_owned()).collect(),
        created_at: Timestamp::now(),
    }
}

/// The whole point of §13.4: the agent runs with the profile's directory and
/// arguments, the directory is created first, and a restart repeats it.
///
/// The directory here is the one the form suggests — a bare `.claude-personal`
/// — because that is the case that used to fail: resolved against the daemon's
/// working directory it is `/.claude-personal`, which under launchd is a
/// read-only file system.
#[test]
fn a_relative_config_directory_lands_in_the_home_directory() {
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
    let expected = harness.home().join(".claude-personal");
    let personal = profile("Personal", Some(".claude-personal"), &["--model", "opus"]);

    assert_eq!(
        client
            .request(Request::SaveAgentProfile {
                profile: personal.clone()
            })
            .expect("SaveAgentProfile"),
        Response::Ack
    );
    let broadcast = common::wait_for(&events, std::time::Duration::from_secs(5), |event| {
        matches!(event, DaemonEvent::AgentProfilesChanged { profiles }
            if profiles.iter().any(|p| p.id == personal.id))
    });
    assert!(
        broadcast.is_some(),
        "saving a profile is broadcast to every client"
    );

    // --- launch through the profile ---
    let session_id = launch(&client, workspace_id, "claude", Some(personal.id));
    let (session_id, terminal_id) = common::wait_for_running(&events, |session| {
        session.id == session_id && session.agent_profile_id == Some(personal.id)
    });
    assert_eq!(
        common::attach_and_read_line(&client, &events, terminal_id, MARKER).as_deref(),
        Some("forge_profile:.claude-personal:--model opus"),
        "the child gets the profile's directory and arguments"
    );
    assert!(
        expected.is_dir(),
        "the profile's config directory is created under $HOME before the agent starts"
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
        Some("forge_profile:.claude-personal:--model opus"),
        "a restart re-applies the profile rather than the bare provider"
    );

    // --- deleting the profile leaves the running session alone ---
    assert_eq!(
        client
            .request(Request::RemoveAgentProfile {
                profile_id: personal.id
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
        Some(personal.id),
        "history keeps the pointer even though the profile is gone"
    );
}

/// The other two spellings the form accepts: an absolute path on another
/// volume, and the `~/…` a user types out of shell habit. Both name a
/// directory, so both are created and exported the same way.
#[test]
fn a_config_directory_can_be_absolute_or_tilde_relative() {
    let harness = common::Harness::new();
    let repo = test_support::init_repo().expect("git repo (git must be installed)");
    common::write_env_reporting_cli(harness.bin(), "claude", MARKER, "Claude Code 1.0.0");

    let daemon = harness.boot();
    let client = daemon.connect("profiles-paths");
    let events = client.events();
    let workspace_id = common::add_main_workspace(&client, repo.path());
    common::wait_for_detection(&client, |providers| {
        providers
            .iter()
            .any(|p| p.descriptor.id.as_str() == "claude" && p.detection.status.is_installed())
    });

    // Somewhere else entirely: an accounts directory outside the home tree.
    let elsewhere = harness.root().join("accounts/claude-elsewhere");
    let outside = profile("Elsewhere", Some(&elsewhere.to_string_lossy()), &[]);
    client
        .request(Request::SaveAgentProfile {
            profile: outside.clone(),
        })
        .expect("an absolute config directory saves");

    let session_id = launch(&client, workspace_id, "claude", Some(outside.id));
    let (_, terminal_id) = common::wait_for_running(&events, |session| session.id == session_id);
    assert_eq!(
        common::attach_and_read_line(&client, &events, terminal_id, MARKER).as_deref(),
        Some("forge_profile:claude-elsewhere:"),
        "an absolute directory is used exactly as typed"
    );
    assert!(elsewhere.is_dir(), "including the parents it needed");

    // `~/…`: the same directory a bare relative path would name, spelled the
    // way a shell user spells it. No shell ever sees this string.
    let tilde = profile("Tilde", Some("~/.claude-tilde"), &[]);
    client
        .request(Request::SaveAgentProfile {
            profile: tilde.clone(),
        })
        .expect("a ~-relative config directory saves");

    let session_id = launch(&client, workspace_id, "claude", Some(tilde.id));
    let (_, terminal_id) = common::wait_for_running(&events, |session| session.id == session_id);
    assert_eq!(
        common::attach_and_read_line(&client, &events, terminal_id, MARKER).as_deref(),
        Some("forge_profile:.claude-tilde:"),
        "a ~ is expanded to the launching user's home"
    );
    assert!(harness.home().join(".claude-tilde").is_dir());
}

/// The second shape of a personal profile: no directory at all, because the
/// separation is already baked into a `claude-personal` wrapper on `PATH` —
/// the thing a shell alias stands for. The name is not a path anyone typed
/// out, so it has to be found the way the shell would find it.
#[test]
fn a_profile_can_run_a_wrapper_named_like_a_shell_alias() {
    let harness = common::Harness::new();
    let repo = test_support::init_repo().expect("git repo (git must be installed)");
    common::write_env_reporting_cli(harness.bin(), "claude", MARKER, "Claude Code 1.0.0");
    // The alias' wrapper: on `PATH`, named for the account, and reporting a
    // marker of its own so the assertion cannot pass on the plain binary.
    common::write_env_reporting_cli(
        harness.bin(),
        "claude-personal",
        "forge_alias",
        "Claude Code 1.0.0",
    );

    let daemon = harness.boot();
    let client = daemon.connect("profiles-alias");
    let events = client.events();
    let workspace_id = common::add_main_workspace(&client, repo.path());
    common::wait_for_detection(&client, |providers| {
        providers
            .iter()
            .any(|p| p.descriptor.id.as_str() == "claude" && p.detection.status.is_installed())
    });

    let mut personal = profile("Personal", None, &["--model", "opus"]);
    personal.executable = Some(PathBuf::from("claude-personal"));
    client
        .request(Request::SaveAgentProfile {
            profile: personal.clone(),
        })
        .expect("a bare name on PATH is probed and accepted");

    let session_id = launch(&client, workspace_id, "claude", Some(personal.id));
    let (_, terminal_id) = common::wait_for_running(&events, |session| session.id == session_id);
    let line = common::attach_and_read_line(&client, &events, terminal_id, "forge_alias");
    assert_eq!(
        line.as_deref(),
        // The inherited `CLAUDE_CONFIG_DIR` is untouched: this profile switches
        // accounts inside the wrapper, not through the environment.
        Some("forge_alias:claude:--model opus"),
        "the wrapper on PATH is what runs"
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

    let save = |profile: AgentProfile| client.request(Request::SaveAgentProfile { profile });

    save(profile("Personal", Some(".claude-personal"), &[])).expect("the first profile saves");

    // A second profile of the same agent cannot share its name, whatever the
    // case: the two would be indistinguishable in a launch menu.
    let clash = save(profile("personal", Some(".claude-other"), &[]))
        .expect_err("a duplicate name is refused");
    assert_eq!(protocol_code(&clash), ErrorCode::Conflict);

    let unnamed = save(profile("   ", None, &[])).expect_err("a nameless profile is refused");
    assert_eq!(protocol_code(&unnamed), ErrorCode::InvalidRequest);

    // Cursor CLI documents no directory of its own, so a directory saved for it
    // would look applied and change nothing.
    let mut cursor = profile("Cursor personal", Some(".cursor-personal"), &[]);
    cursor.provider_id = AgentProviderId::new("cursor");
    let refused = save(cursor).expect_err("a directory no provider variable carries is refused");
    assert_eq!(protocol_code(&refused), ErrorCode::InvalidRequest);

    // A profile may bring its own binary, but only one the version probe
    // accepts — the same rule `SetProviderExecutable` follows (§13.1 step 4).
    let mut foreign = profile("Foreign", None, &[]);
    foreign.executable = Some(harness.root().join("nothing-here"));
    let refused = save(foreign).expect_err("an executable that is not there is refused");
    assert_eq!(protocol_code(&refused), ErrorCode::ProviderNotInstalled);

    let mut off_path = profile("Not on PATH", None, &[]);
    off_path.executable = Some(PathBuf::from("claude-personal"));
    let refused = save(off_path).expect_err("a bare name nothing on PATH answers to is refused");
    assert_eq!(protocol_code(&refused), ErrorCode::ProviderNotInstalled);

    // ...and a profile pointing at a real, probed binary is accepted.
    let elsewhere = harness.root().join("elsewhere");
    std::fs::create_dir_all(&elsewhere).expect("create the off-PATH directory");
    let picked = common::write_env_reporting_cli(&elsewhere, "claude", MARKER, "Claude Code 2.0.0");
    let mut own_binary = profile("Own binary", None, &[]);
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

    let work = profile("Work", Some(".claude-work"), &[]);
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

fn launch(
    client: &client::Client,
    workspace_id: domain::WorkspaceId,
    provider: &str,
    profile_id: Option<AgentProfileId>,
) -> domain::SessionId {
    let response = client
        .request(Request::CreateAgentSession {
            workspace_id,
            provider_id: AgentProviderId::new(provider),
            profile_id,
            parent: None,
            role: SessionRole::Generic,
            resume: None,
            initial_prompt: None,
            read_only: false,
        })
        .expect("CreateAgentSession with a profile");
    match response {
        Response::SessionCreated { session_id, .. } => session_id,
        other => panic!("{other:?}"),
    }
}

fn protocol_code(error: &client::ClientError) -> ErrorCode {
    match error {
        client::ClientError::Protocol(error) => error.code,
        other => panic!("expected a protocol error, got {other:?}"),
    }
}
