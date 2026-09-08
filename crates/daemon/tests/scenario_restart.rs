//! §19 scenarios E and G: what survives a daemon that went away.
//!
//! Both scenarios hinge on the same thing the GUI cannot fake — a daemon
//! process that dies without telling anyone, and a second one that opens the
//! same database afterwards (§15.3). `common::TestDaemon::crash` models the
//! `kill -9` of scenario G: the accept loop stops and the socket is unlinked,
//! but no session is killed and no row is rewritten, so the reconciliation of
//! the next daemon has exactly the state a crash leaves behind.
//!
//! What is *not* covered here is the GUI half of both scenarios: the
//! "Disconnected" banner and the "Restart daemon" button of G, and the sidebar
//! nesting of E. Those are GUI concerns.

mod common;

use domain::{SessionId, SessionRole, SessionState};
use protocol::{Request, Response};

/// Both scenarios below are about rows that outlive the daemon that wrote them,
/// so they boot with `sessions.persist_history` on. The shipped default is off
/// — see `a_fresh_start_drops_the_session_history_by_default`.
fn keep_history(cfg: &mut daemon::config::Config) {
    cfg.sessions.persist_history = true;
}

/// §19 G: after a daemon restart every previously live session comes back
/// `Orphaned` and none comes back `Running` (§15.3 step 2, §3.3).
#[test]
fn a_daemon_restart_orphans_every_previously_running_session() {
    const COUNT: usize = 3;

    let harness = common::Harness::new();
    let repo = test_support::init_repo().expect("git repo (git must be installed)");

    let crashed = harness.boot_with(keep_history);
    let mut before: Vec<SessionId> = Vec::new();
    let workspace_id = {
        let client = crashed.connect("scenario-g-before");
        let events = client.events();
        let workspace_id = common::add_main_workspace(&client, repo.path());

        for _ in 0..COUNT {
            let (session_id, _terminal_id) =
                common::create_shell_session(&client, &events, workspace_id);
            before.push(session_id);
        }

        let live = common::sessions(&client);
        assert_eq!(live.len(), COUNT, "all {COUNT} sessions exist");
        assert!(
            live.iter().all(|s| s.state == SessionState::Running),
            "every session is Running before the crash: {:?}",
            live.iter().map(|s| &s.state).collect::<Vec<_>>()
        );
        assert!(live.iter().all(|s| s.terminal_id.is_some()));
        workspace_id
        // The client is dropped here; that alone must not disturb the sessions.
    };

    // `kill -9`: no `StopDaemon`, no kill, nothing written. The rows stay
    // `Running` in SQLite exactly as a crash would leave them.
    crashed.crash();

    let restarted = harness.boot_with(keep_history);
    let client = restarted.connect("scenario-g-after");
    let events = client.events();

    let Response::Snapshot {
        projects,
        workspaces,
        sessions,
        ..
    } = client.request(Request::GetSnapshot).expect("GetSnapshot")
    else {
        panic!("expected Snapshot");
    };

    assert_eq!(
        projects.len(),
        1,
        "the project survives the restart (§15.2)"
    );
    assert!(
        workspaces.iter().any(|w| w.id == workspace_id),
        "so does its Main workspace"
    );
    assert_eq!(sessions.len(), COUNT, "and every session row");

    for session in &sessions {
        assert!(
            before.contains(&session.id),
            "the restarted daemon reports the same session ids"
        );
        assert_eq!(
            session.state,
            SessionState::Orphaned,
            "a PTY never survives the daemon, so its session is Orphaned (§3.3, §15.3)"
        );
        assert!(
            session.terminal_id.is_none(),
            "terminal ids are runtime-only and are never persisted (§15.2)"
        );
        assert!(
            session.ended_at.is_some(),
            "Orphaned is a terminal state, so reconciliation stamps ended_at"
        );
    }
    assert!(
        !sessions.iter().any(|s| s.state.is_active()),
        "no session comes back Running or Starting (§19 G)"
    );

    // The PTYs of the crashed daemon are still out there; only the core that
    // spawned them can reap them. Do it before driving the new daemon further,
    // so its `Exited` writes cannot race the restart below on the shared file.
    crashed.kill_leftover_sessions();

    // An Orphaned session is not a dead end: `RestartSession` is the documented
    // way back (§7.3), which is what makes the restarted sidebar usable.
    let session_id = sessions[0].id;
    client
        .request(Request::RestartSession { session_id })
        .expect("RestartSession");
    common::wait_for_running(&events, |session| session.id == session_id);
    let restarted_session = common::session(&client, session_id).expect("the restarted session");
    assert_eq!(restarted_session.state, SessionState::Running);
    assert!(
        restarted_session.terminal_id.is_some(),
        "a restarted session gets a fresh terminal"
    );

    assert!(common::kill_and_wait(&client, &events, session_id));
    common::stop_daemon(&client);
}

/// The shipped default (`sessions.persist_history = false`): a restart opens on
/// a clean session list. Projects and workspaces still survive — only the dead
/// session rows go.
#[test]
fn a_fresh_start_drops_the_session_history_by_default() {
    let harness = common::Harness::new();
    let repo = test_support::init_repo().expect("git repo (git must be installed)");

    let crashed = harness.boot();
    let workspace_id = {
        let client = crashed.connect("fresh-start-before");
        let events = client.events();
        let workspace_id = common::add_main_workspace(&client, repo.path());
        common::create_shell_session(&client, &events, workspace_id);
        common::create_shell_session(&client, &events, workspace_id);
        assert_eq!(common::sessions(&client).len(), 2);
        workspace_id
    };

    crashed.crash();

    let restarted = harness.boot();
    let client = restarted.connect("fresh-start-after");

    let Response::Snapshot {
        projects,
        workspaces,
        sessions,
        ..
    } = client.request(Request::GetSnapshot).expect("GetSnapshot")
    else {
        panic!("expected Snapshot");
    };

    assert!(
        sessions.is_empty(),
        "the session history is dropped on startup, not kept as Orphaned rows: {:?}",
        sessions.iter().map(|s| &s.state).collect::<Vec<_>>()
    );
    assert_eq!(projects.len(), 1, "the project still survives (§15.2)");
    assert!(
        workspaces.iter().any(|w| w.id == workspace_id),
        "and so does its Main workspace, so the tree reopens where it was"
    );

    // The crashed daemon still owns the PTYs it spawned; reap them before this
    // one goes away. Their rows are gone, so nothing is written back.
    crashed.kill_leftover_sessions();
    common::stop_daemon(&client);
}

/// §19 E: a child session created with a role stays nested under its parent
/// across a daemon restart, both come back `Orphaned`, and restarting the child
/// keeps the parent (ADR-010).
#[test]
fn a_child_session_stays_nested_across_a_daemon_restart_and_keeps_its_parent() {
    let harness = common::Harness::new();
    let repo = test_support::init_repo().expect("git repo (git must be installed)");

    let crashed = harness.boot_with(keep_history);
    let (parent_id, child_id, workspace_id) = {
        let client = crashed.connect("scenario-e-before");
        let events = client.events();
        let workspace_id = common::add_main_workspace(&client, repo.path());

        // A: a root session.
        let (parent_id, _) = common::create_shell_session(&client, &events, workspace_id);
        // B: "New child session" with the Planner role (§8.1, §8.2).
        let (child_id, _) =
            common::create_child_session(&client, &events, parent_id, SessionRole::Planner);

        let parent = common::session(&client, parent_id).expect("A");
        let child = common::session(&client, child_id).expect("B");
        assert!(parent.is_root(), "A is a graph root (ADR-010)");
        assert_eq!(
            child.parent_session_id,
            Some(parent_id),
            "B is nested under A"
        );
        assert_eq!(
            child.root_session_id, parent_id,
            "B inherits A's root (ADR-010)"
        );
        assert_eq!(child.role, SessionRole::Planner);
        assert_eq!(
            child.workspace_id, workspace_id,
            "the SameWorkspace policy keeps B where A runs (§8.2)"
        );
        (parent_id, child_id, workspace_id)
    };

    crashed.crash();

    let restarted = harness.boot_with(keep_history);
    let client = restarted.connect("scenario-e-after");
    let events = client.events();

    let parent = common::session(&client, parent_id).expect("A survived the restart");
    let child = common::session(&client, child_id).expect("B survived the restart");
    assert_eq!(
        parent.state,
        SessionState::Orphaned,
        "A is Orphaned (§19 E)"
    );
    assert_eq!(child.state, SessionState::Orphaned, "B is Orphaned (§19 E)");
    assert_eq!(
        child.parent_session_id,
        Some(parent_id),
        "B is still nested under A after the restart"
    );
    assert_eq!(
        child.root_session_id, parent_id,
        "and still shares A's root"
    );
    assert_eq!(
        child.role,
        SessionRole::Planner,
        "the role tag is persisted, not runtime state (§15.2)"
    );
    assert_eq!(child.workspace_id, workspace_id);

    crashed.kill_leftover_sessions();

    // `RestartSession` on B: Orphaned → Starting → Running (§7.3), and the
    // graph edge must survive it.
    client
        .request(Request::RestartSession {
            session_id: child_id,
        })
        .expect("RestartSession");
    common::wait_for_running(&events, |session| session.id == child_id);

    let child = common::session(&client, child_id).expect("B is back");
    assert_eq!(child.state, SessionState::Running);
    assert!(child.terminal_id.is_some());
    assert_eq!(
        child.parent_session_id,
        Some(parent_id),
        "RestartSession preserves the parent (§19 E)"
    );
    assert_eq!(child.root_session_id, parent_id);
    assert_eq!(child.role, SessionRole::Planner, "and the role");
    assert_eq!(
        common::session(&client, parent_id).expect("A").state,
        SessionState::Orphaned,
        "restarting a child does not resurrect its parent"
    );

    assert!(common::kill_and_wait(&client, &events, child_id));
    common::stop_daemon(&client);
}
