//! §19 scenarios F and B (the halves that do not need the GUI).
//!
//! Every daemon here boots with a `PATH` containing exactly one directory the
//! test wrote (see `common`), so detection (§13.1) reports what the fixtures
//! say and nothing about the machine running the suite. That is what makes
//! "OpenCode is not found" and "`agent` is the Grok CLI, not Cursor" testable
//! at all: on the reference machine all four providers are installed.
//!
//! Not covered here: the picker rows and the "Set path…" dialog of §13.2, and
//! the `Cmd+Shift+]` switch time of scenario B — both are GUI.

mod common;

use domain::{AgentProviderId, DetectionStatus, SessionRole, SessionState};
use protocol::{DaemonEvent, ErrorCode, Request};

/// §19 F: a provider whose only candidate is a foreign binary is `Rejected`,
/// a missing one is `NotFound` and refuses to launch without taking anything
/// else down, and "Set path…" moves a provider to `Installed` — and actually
/// launches that binary — on the same running daemon.
#[test]
fn set_provider_executable_installs_a_provider_without_restarting_the_daemon() {
    let harness = common::Harness::new();
    let repo = test_support::init_repo().expect("git repo (git must be installed)");

    // `agent` is the Cursor descriptor's preferred candidate, but on many
    // machines it is the Grok CLI — the reason that descriptor carries
    // `expect_substring = "cursor"` (§7.5).
    common::write_fake_agent_cli(harness.bin(), "agent", "forge_grok", "grok-cli 1.2.0");
    // One provider that really is installed, so "the others keep working" is
    // observable rather than vacuous.
    common::write_fake_agent_cli(harness.bin(), "claude", "forge_claude", "Claude Code 1.0.0");
    // The binary the user picks with "Set path…" — deliberately not on PATH.
    let elsewhere = harness.root().join("elsewhere");
    std::fs::create_dir_all(&elsewhere).expect("create the off-PATH directory");
    let picked = common::write_fake_agent_cli(
        &elsewhere,
        "cursor-agent",
        "forge_cursor",
        "cursor-agent 2026.1",
    );

    let daemon = harness.boot();
    let client = daemon.connect("scenario-f");
    let events = client.events();
    let workspace_id = common::add_main_workspace(&client, repo.path());

    // --- startup detection (§13.1) ---
    common::wait_for_detection(&client, |providers| {
        providers.iter().all(|p| match p.descriptor.id.as_str() {
            "claude" => p.detection.status.is_installed(),
            "cursor" => matches!(p.detection.status, DetectionStatus::Rejected { .. }),
            _ => matches!(p.detection.status, DetectionStatus::NotFound),
        })
    });
    match common::provider(&client, "cursor").detection.status {
        DetectionStatus::Rejected { candidate, reason } => {
            assert_eq!(candidate, "agent", "the probed candidate is named");
            assert!(
                reason.to_lowercase().contains("cursor"),
                "the rejection says which marker was missing: {reason}"
            );
        }
        other => panic!("a foreign `agent` must be Rejected, got {other:?}"),
    }

    // A provider the picker shows as "Not found" refuses to launch...
    let refused = client
        .request(Request::CreateAgentSession {
            workspace_id,
            provider_id: AgentProviderId::new("opencode"),
            profile_id: None,
            parent: None,
            role: SessionRole::Generic,
            resume: None,
            initial_prompt: None,
            read_only: false,
        })
        .expect_err("launching a provider that is not installed must fail");
    match &refused {
        client::ClientError::Protocol(error) => {
            assert_eq!(error.code, ErrorCode::ProviderNotInstalled);
        }
        other => panic!("expected a ProviderNotInstalled protocol error, got {other:?}"),
    }

    // ...and nothing else is disturbed: the installed provider still launches.
    let (claude_session, claude_terminal) =
        common::create_agent_session(&client, &events, workspace_id, "claude");
    assert_eq!(
        common::attach_and_read_line(&client, &events, claude_terminal, "forge_claude_cwd_")
            .as_deref(),
        Some("forge_claude_cwd_ok"),
        "the installed provider launches in its workspace"
    );

    // --- "Set path…" (§13.2) on the running daemon ---
    client
        .request(Request::SetProviderExecutable {
            provider_id: AgentProviderId::new("cursor"),
            path: Some(picked.clone()),
        })
        .expect("SetProviderExecutable");
    assert!(
        common::wait_for(&events, common::DEADLINE, |event| matches!(
            event,
            DaemonEvent::AgentDetectionChanged { .. }
        ))
        .is_some(),
        "setting an override re-runs detection and broadcasts the result (§13.1)"
    );
    match common::provider(&client, "cursor").detection.status {
        DetectionStatus::Installed {
            executable,
            version,
        } => {
            assert_eq!(executable, picked, "the override is what got probed");
            assert_eq!(version.as_deref(), Some("cursor-agent 2026.1"));
        }
        other => panic!("the override should install the provider, got {other:?}"),
    }
    assert!(
        client.is_connected(),
        "no restart happened: this is the same connection to the same daemon"
    );
    assert_eq!(
        common::session(&client, claude_session)
            .expect("the earlier session")
            .state,
        SessionState::Running,
        "and the sessions started before the override are untouched (§19 F)"
    );

    // The override is what actually runs: the foreign `agent` is still first on
    // PATH, so a launch that ignored the override would announce itself as
    // `forge_grok_cwd_ok` instead.
    let (cursor_session, cursor_terminal) =
        common::create_agent_session(&client, &events, workspace_id, "cursor");
    assert_eq!(
        common::attach_and_read_line(&client, &events, cursor_terminal, "forge_").as_deref(),
        Some("forge_cursor_cwd_ok"),
        "the overridden binary is the one launched, not the candidate on PATH"
    );

    // Clearing the override falls back to the PATH candidate, which the probe
    // rejects again.
    client
        .request(Request::SetProviderExecutable {
            provider_id: AgentProviderId::new("cursor"),
            path: None,
        })
        .expect("clear the override");
    common::wait_for_detection(&client, |providers| {
        providers.iter().any(|p| {
            p.descriptor.id.as_str() == "cursor"
                && matches!(p.detection.status, DetectionStatus::Rejected { .. })
        })
    });

    assert!(common::kill_and_wait(&client, &events, claude_session));
    assert!(common::kill_and_wait(&client, &events, cursor_session));
    common::stop_daemon(&client);
}

/// §19 B, daemon side: the four providers coexist as four live sessions of the
/// same workspace, each with its own PTY. (The `Cmd+Shift+]` switch budget of
/// the scenario is a GUI measurement, §20.)
#[test]
fn the_four_providers_coexist_as_live_sessions_of_one_workspace() {
    // (provider id, binary name detection looks for, marker prefix)
    const PROVIDERS: [(&str, &str); 4] = [
        ("claude", "claude"),
        ("codex", "codex"),
        ("opencode", "opencode"),
        // The Cursor descriptor accepts `cursor-agent` on the strength of its
        // name alone (§13.1 step 4).
        ("cursor", "cursor-agent"),
    ];

    let harness = common::Harness::new();
    let repo = test_support::init_repo().expect("git repo (git must be installed)");
    for (id, binary) in PROVIDERS {
        common::write_fake_agent_cli(
            harness.bin(),
            binary,
            &format!("forge_{id}"),
            &format!("{binary} 1.0.0"),
        );
    }

    let daemon = harness.boot();
    let client = daemon.connect("scenario-b");
    let events = client.events();
    let workspace_id = common::add_main_workspace(&client, repo.path());

    common::wait_for_detection(&client, |providers| {
        PROVIDERS.iter().all(|(id, _)| {
            providers
                .iter()
                .any(|p| p.descriptor.id.as_str() == *id && p.detection.status.is_installed())
        })
    });

    let mut live = Vec::new();
    for (id, _) in PROVIDERS {
        let (session_id, terminal_id) =
            common::create_agent_session(&client, &events, workspace_id, id);
        assert_eq!(
            common::attach_and_read_line(&client, &events, terminal_id, "forge_").as_deref(),
            Some(format!("forge_{id}_cwd_ok").as_str()),
            "{id} must reach its workspace"
        );
        live.push((id, session_id, terminal_id));
    }

    let sessions = common::sessions(&client);
    assert_eq!(sessions.len(), PROVIDERS.len());
    assert!(
        sessions.iter().all(|s| s.state == SessionState::Running),
        "all four sessions are live at the same time (§19 B)"
    );
    assert!(
        sessions.iter().all(|s| s.workspace_id == workspace_id),
        "in the same workspace"
    );
    let mut terminals: Vec<_> = live.iter().map(|(_, _, t)| *t).collect();
    terminals.sort_unstable();
    terminals.dedup();
    assert_eq!(terminals.len(), PROVIDERS.len(), "each has its own PTY");

    for (_, session_id, _) in &live {
        assert!(common::kill_and_wait(&client, &events, *session_id));
    }
    common::stop_daemon(&client);
}

/// §13.1 rejects the foreign `agent` binary for the `cursor` provider, so
/// launching that provider must be refused too (§13.3, DoD §29).
///
/// Regression test. The launch path used to resolve the program by walking the
/// same candidate list *without* running the probe, so `CreateAgentSession`
/// spawned the very binary detection had just refused: the picker said
/// "Rejected" while the session that opened was somebody else's CLI. Launching
/// now goes through the `Installed` executable or fails with
/// `ProviderNotInstalled`.
#[test]
fn launching_a_provider_whose_only_candidate_was_rejected_is_refused() {
    let harness = common::Harness::new();
    let repo = test_support::init_repo().expect("git repo (git must be installed)");
    common::write_fake_agent_cli(harness.bin(), "agent", "forge_grok", "grok-cli 1.2.0");

    let daemon = harness.boot();
    let client = daemon.connect("cursor-rejected");
    let events = client.events();
    let workspace_id = common::add_main_workspace(&client, repo.path());

    common::wait_for_detection(&client, |providers| {
        providers.iter().any(|p| {
            p.descriptor.id.as_str() == "cursor"
                && matches!(p.detection.status, DetectionStatus::Rejected { .. })
        })
    });

    let outcome = client.request(Request::CreateAgentSession {
        workspace_id,
        provider_id: AgentProviderId::new("cursor"),
        profile_id: None,
        parent: None,
        role: SessionRole::Generic,
        resume: None,
        initial_prompt: None,
        read_only: false,
    });

    // When the launch is (wrongly) accepted, show *which* binary answered, so
    // the failure names the problem instead of just reporting `Ok`.
    let launched = outcome.is_ok().then(|| {
        let (session_id, terminal_id) = common::wait_for_running(&events, |session| {
            session
                .agent_provider_id
                .as_ref()
                .map(AgentProviderId::as_str)
                == Some("cursor")
        });
        let announced = common::attach_and_read_line(&client, &events, terminal_id, "forge_");
        common::kill_and_wait(&client, &events, session_id);
        announced
    });

    common::stop_daemon(&client);
    assert!(
        outcome.is_err(),
        "the only `cursor` candidate on PATH failed its version probe (§13.1), \
         so launching the provider must fail instead of spawning it; \
         the foreign binary answered the session: {launched:?}"
    );
}
