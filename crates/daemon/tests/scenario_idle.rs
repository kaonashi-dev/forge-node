//! End to end: what happens to a session nobody is using.
//!
//! The policy itself is unit-tested in `daemon::idle`, and the daemon core's
//! own tests drive it over a fake PTY. What only a real daemon can show is the
//! whole chain: a real shell in a real PTY goes quiet, the sweeper notices, the
//! kill path runs, and a connected client learns about it as a `SessionUpdated`
//! carrying `Exited` — the same event any other stop produces, so the session
//! stays restartable and nothing special-cases an idle death.
//!
//! Each test drives one sweep explicitly (`Daemon::sweep_idle_sessions`) rather
//! than waiting out the sweeper's 30-second period. What the background thread
//! adds — that the pass happens at all — is a `std::thread::spawn` in
//! `Daemon::start`, not behaviour worth half a minute of test time.

mod common;

use std::time::Duration;

use domain::{SessionState, TerminalId};
use protocol::{DaemonEvent, Request, Response};

/// Long enough for a booted shell to print its prompt and fall silent.
const QUIET: Duration = Duration::from_millis(1500);

/// Warn and stop after one second, shells included: every session here is a
/// shell, and the shipped default exempts them (a prompt is a resting state,
/// not a leak).
fn stop_when_quiet(cfg: &mut daemon::config::Config) {
    cfg.sessions.idle_warn_after_secs = 1;
    cfg.sessions.idle_stop_after_secs = 1;
    cfg.sessions.idle_include_shells = true;
}

/// Stop watching a terminal, the way closing its tab does.
///
/// These tests have to attach to know the shell reached a prompt — delta events
/// only reach subscribers — but an attached terminal is in use by definition and
/// the policy spares it. Detaching first is what the scenario actually is: the
/// user walked away from a session they left running.
fn detach(client: &client::Client, terminal_id: TerminalId) {
    assert_eq!(
        client
            .request(Request::DetachTerminal { terminal_id })
            .expect("DetachTerminal"),
        Response::Ack
    );
}

#[test]
fn a_quiet_session_is_stopped_and_the_client_is_told() {
    let harness = common::Harness::new();
    let repo = test_support::init_repo().expect("git repo (git must be installed)");
    let running = harness.boot_with(stop_when_quiet);

    let client = running.connect("scenario-idle-stop");
    let events = client.events();
    let workspace_id = common::add_main_workspace(&client, repo.path());
    let (session_id, terminal_id) = common::create_shell_session(&client, &events, workspace_id);
    common::attach(&client, terminal_id, 80, 24);
    common::wait_for_shell_prompt(&client, &events, terminal_id);
    detach(&client, terminal_id);

    // Still warm: the shell just printed its prompt.
    assert_eq!(
        running.daemon.sweep_idle_sessions(),
        0,
        "a session that was busy a moment ago is in use"
    );

    std::thread::sleep(QUIET);
    assert_eq!(
        running.daemon.sweep_idle_sessions(),
        1,
        "a shell quiet for longer than the threshold is stopped"
    );

    assert!(
        common::wait_for(&events, Duration::from_secs(10), |event| {
            matches!(event, DaemonEvent::SessionUpdated(session)
                if session.id == session_id
                    && matches!(session.state, SessionState::Exited { .. }))
        })
        .is_some(),
        "the stop arrives as an ordinary SessionUpdated, not a special event"
    );

    // The row survives in a terminal state, so the session can be restarted.
    let stopped = common::session(&client, session_id).expect("the session row survives");
    assert!(stopped.state.is_terminal());
    assert!(stopped.terminal_id.is_none());

    common::stop_daemon(&client);
}

#[test]
fn typing_into_a_session_keeps_it_alive() {
    let harness = common::Harness::new();
    let repo = test_support::init_repo().expect("git repo (git must be installed)");
    let running = harness.boot_with(stop_when_quiet);

    let client = running.connect("scenario-idle-input");
    let events = client.events();
    let workspace_id = common::add_main_workspace(&client, repo.path());
    let (session_id, terminal_id) = common::create_shell_session(&client, &events, workspace_id);
    common::attach(&client, terminal_id, 80, 24);
    common::wait_for_shell_prompt(&client, &events, terminal_id);
    detach(&client, terminal_id);

    // Go quiet past the threshold, then use it: the very next sweep must leave
    // the session alone.
    std::thread::sleep(QUIET);
    common::write_input(&client, terminal_id, "echo still_here\n");
    assert_eq!(
        running.daemon.sweep_idle_sessions(),
        0,
        "a session the user is working in is in use"
    );
    assert!(common::session(&client, session_id)
        .expect("session")
        .state
        .is_active());

    // Quiet again afterwards: the reprieve is not permanent.
    std::thread::sleep(QUIET);
    assert_eq!(running.daemon.sweep_idle_sessions(), 1);

    common::stop_daemon(&client);
}

#[test]
fn a_terminal_the_user_is_watching_is_never_stopped() {
    let harness = common::Harness::new();
    let repo = test_support::init_repo().expect("git repo (git must be installed)");
    let running = harness.boot_with(stop_when_quiet);

    let client = running.connect("scenario-idle-attached");
    let events = client.events();
    let workspace_id = common::add_main_workspace(&client, repo.path());
    let (session_id, terminal_id) = common::create_shell_session(&client, &events, workspace_id);
    // Attached and stays attached: the tab is open on screen.
    common::attach(&client, terminal_id, 80, 24);
    common::wait_for_shell_prompt(&client, &events, terminal_id);

    std::thread::sleep(QUIET);
    assert_eq!(
        running.daemon.sweep_idle_sessions(),
        0,
        "idle_stop_attached is off: a terminal on screen is in use"
    );
    assert!(common::session(&client, session_id)
        .expect("session")
        .state
        .is_active());

    // Close the tab and it becomes a candidate like any other.
    detach(&client, terminal_id);
    std::thread::sleep(QUIET);
    assert_eq!(running.daemon.sweep_idle_sessions(), 1);

    common::stop_daemon(&client);
}

#[test]
fn the_shipped_defaults_never_stop_a_quiet_session() {
    let harness = common::Harness::new();
    let repo = test_support::init_repo().expect("git repo (git must be installed)");
    // Only `idle_include_shells` is changed, so a shell is subject to the rules
    // at all; every threshold stays exactly as the app ships it.
    let running = harness.boot_with(|cfg| cfg.sessions.idle_include_shells = true);

    let client = running.connect("scenario-idle-defaults");
    let events = client.events();
    let workspace_id = common::add_main_workspace(&client, repo.path());
    let (session_id, terminal_id) = common::create_shell_session(&client, &events, workspace_id);
    common::attach(&client, terminal_id, 80, 24);
    common::wait_for_shell_prompt(&client, &events, terminal_id);
    detach(&client, terminal_id);

    std::thread::sleep(QUIET);
    assert_eq!(
        running.daemon.sweep_idle_sessions(),
        0,
        "the defaults warn about an idle session and stop nothing"
    );
    assert!(common::session(&client, session_id)
        .expect("session")
        .state
        .is_active());

    common::stop_daemon(&client);
}
