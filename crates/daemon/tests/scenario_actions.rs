//! Project actions end to end: a saved set survives the protocol, a run is a
//! `Task` session, and a single-instance action cannot be started twice in one
//! workspace however the requests race.
//!
//! A task that must stay alive waits in the shell builtin `read`: the
//! harness's login environment has no `sleep` on `PATH`.

mod common;

use std::path::Path;
use std::sync::{Arc, Barrier};

use client::Client;
use domain::{
    ActionKind, ActionReady, ActionSource, ProjectAction, ProjectActionId, ProjectId, SessionId,
    SessionKind, SessionState, Timestamp, WorkspaceId,
};
use protocol::{DaemonEvent, Request, Response};

fn project_id(client: &Client) -> ProjectId {
    let Response::Snapshot { projects, .. } =
        client.request(Request::GetSnapshot).expect("GetSnapshot")
    else {
        panic!("expected Snapshot");
    };
    projects[0].id
}

fn action(project_id: ProjectId, label: &str, command: &str) -> ProjectAction {
    ProjectAction {
        id: ProjectActionId::new(),
        project_id,
        label: label.to_owned(),
        command: command.to_owned(),
        kind: ActionKind::Custom,
        cwd: None,
        single_instance: true,
        pinned: false,
        ready: ActionReady::None,
        port: None,
        depends_on: Vec::new(),
        source: ActionSource::Manual,
        position: 0,
        created_at: Timestamp::now(),
    }
}

fn save(client: &Client, project_id: ProjectId, actions: &[ProjectAction]) {
    assert_eq!(
        client
            .request(Request::SetProjectActions {
                project_id,
                actions: actions.to_vec(),
            })
            .expect("SetProjectActions"),
        Response::Ack
    );
}

fn run(client: &Client, workspace_id: WorkspaceId, action_id: ProjectActionId) -> Response {
    client
        .request(Request::RunAction {
            workspace_id,
            action_id,
        })
        .expect("RunAction")
}

fn started(response: Response) -> (SessionId, domain::TerminalId) {
    match response {
        Response::SessionCreated {
            session_id,
            terminal_id,
        } => (session_id, terminal_id),
        other => panic!("expected SessionCreated, got {other:?}"),
    }
}

fn wait_for_exit(events: &flume::Receiver<DaemonEvent>, session_id: SessionId) {
    common::wait_for(events, common::DEADLINE, |event| {
        matches!(event, DaemonEvent::SessionUpdated(s)
            if s.id == session_id && matches!(s.state, SessionState::Exited { .. }))
    })
    .expect("the task should exit");
}

fn setup() -> (common::Harness, test_support::TempRepo) {
    let harness = common::Harness::new();
    let repo = test_support::init_repo().expect("git repo (git must be installed)");
    (harness, repo)
}

fn error_code(error: client::ClientError) -> protocol::ErrorCode {
    match error {
        client::ClientError::Protocol(p) => p.code,
        other => panic!("expected a protocol error, got {other:?}"),
    }
}

fn git(repo: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .expect("run git");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn two_racing_runs_start_one_task_and_name_it_to_the_other() {
    let (harness, repo) = setup();
    let daemon = harness.boot();
    let client = daemon.connect("actions");
    let workspace_id = common::add_main_workspace(&client, repo.path());
    let project_id = project_id(&client);
    let dev = action(project_id, "dev", "read -r line");
    save(&client, project_id, std::slice::from_ref(&dev));

    let gate = Arc::new(Barrier::new(2));
    let racers: Vec<_> = (0..2)
        .map(|n| {
            let gate = Arc::clone(&gate);
            let other = daemon.connect(&format!("racer-{n}"));
            let action_id = dev.id;
            std::thread::spawn(move || {
                gate.wait();
                run(&other, workspace_id, action_id)
            })
        })
        .collect();
    let answers: Vec<Response> = racers.into_iter().map(|t| t.join().unwrap()).collect();

    let created: Vec<SessionId> = answers
        .iter()
        .filter_map(|r| match r {
            Response::SessionCreated { session_id, .. } => Some(*session_id),
            _ => None,
        })
        .collect();
    let named: Vec<SessionId> = answers
        .iter()
        .filter_map(|r| match r {
            Response::ActionAlreadyRunning { session_id } => Some(*session_id),
            _ => None,
        })
        .collect();
    assert_eq!(created.len(), 1, "{answers:?}");
    assert_eq!(
        named, created,
        "the loser is pointed at the winner's session"
    );

    let session = common::session(&client, created[0]).expect("the task session");
    assert_eq!(session.kind, SessionKind::Task);
    assert_eq!(session.action.expect("tagged").action_id, dev.id);
    daemon.kill_leftover_sessions();
}

#[test]
fn an_exited_task_frees_its_action_and_a_restart_runs_the_current_command() {
    let (harness, repo) = setup();
    let daemon = harness.boot();
    let client = daemon.connect("actions");
    let events = client.events();
    let workspace_id = common::add_main_workspace(&client, repo.path());
    let project_id = project_id(&client);
    let mut test = action(project_id, "test", "echo first-run");
    save(&client, project_id, std::slice::from_ref(&test));

    let (first, _) = started(run(&client, workspace_id, test.id));
    wait_for_exit(&events, first);

    // An edit while it is not running applies to the next run of the same session.
    test.command = "echo second-run; read -r line".to_owned();
    save(&client, project_id, std::slice::from_ref(&test));
    assert_eq!(
        client
            .request(Request::RestartSession { session_id: first })
            .expect("RestartSession"),
        Response::Ack
    );
    let (_, terminal_id) = common::wait_for_running(&events, |s| s.id == first);
    let line = common::attach_and_read_line(&client, &events, terminal_id, "second-run");
    assert_eq!(line.as_deref(), Some("second-run"));

    // Live again, so a run names it rather than starting another.
    assert_eq!(
        run(&client, workspace_id, test.id),
        Response::ActionAlreadyRunning { session_id: first }
    );
    daemon.kill_leftover_sessions();
}

#[test]
fn one_action_runs_once_per_workspace_and_freely_when_not_single_instance() {
    let (harness, repo) = setup();
    let daemon = harness.boot();
    let client = daemon.connect("actions");
    let events = client.events();
    let main = common::add_main_workspace(&client, repo.path());
    let project_id = project_id(&client);
    git(repo.path(), &["branch", "feature/actions"]);
    client
        .request(Request::CreateWorktree {
            project_id,
            branch: "feature/actions".to_owned(),
            base: None,
            name: None,
        })
        .expect("CreateWorktree");
    let worktree = common::wait_for(
        &events,
        common::DEADLINE,
        |event| matches!(event, DaemonEvent::WorkspaceCreated(w) if w.managed_by_app),
    )
    .expect("WorkspaceCreated");
    let DaemonEvent::WorkspaceCreated(worktree) = worktree else {
        unreachable!()
    };

    let dev = action(project_id, "dev", "read -r line");
    let mut watch = action(project_id, "watch", "read -r line");
    watch.single_instance = false;
    save(&client, project_id, &[dev.clone(), watch.clone()]);

    let (in_main, _) = started(run(&client, main, dev.id));
    let (in_worktree, _) = started(run(&client, worktree.id, dev.id));
    assert_ne!(in_main, in_worktree);

    let (a, _) = started(run(&client, main, watch.id));
    let (b, _) = started(run(&client, main, watch.id));
    assert_ne!(a, b);
    daemon.kill_leftover_sessions();
}

#[test]
fn removing_a_running_action_leaves_its_session_untagged() {
    let (harness, repo) = setup();
    let daemon = harness.boot();
    let client = daemon.connect("actions");
    let events = client.events();
    let workspace_id = common::add_main_workspace(&client, repo.path());
    let project_id = project_id(&client);
    let dev = action(project_id, "dev", "read -r line");
    save(&client, project_id, std::slice::from_ref(&dev));
    let (session_id, _) = started(run(&client, workspace_id, dev.id));

    save(&client, project_id, &[]);
    common::wait_for(&events, common::DEADLINE, |event| {
        matches!(event, DaemonEvent::SessionUpdated(s) if s.id == session_id && s.action.is_none())
    })
    .expect("the session loses its tag");
    let session = common::session(&client, session_id).expect("still there");
    assert!(
        session.state.is_active(),
        "removing the action does not stop it"
    );
    assert_eq!(
        error_code(
            client
                .request(Request::RunAction {
                    workspace_id,
                    action_id: dev.id
                })
                .unwrap_err()
        ),
        protocol::ErrorCode::NotFound
    );
    daemon.kill_leftover_sessions();
}

#[test]
fn a_saved_set_comes_back_in_the_snapshot_and_bad_sets_are_refused() {
    let (harness, repo) = setup();
    let daemon = harness.boot();
    let client = daemon.connect("actions");
    common::add_main_workspace(&client, repo.path());
    let project_id = project_id(&client);
    let lint = action(project_id, "lint", "true");
    let test = action(project_id, "test", "true");
    save(&client, project_id, &[lint, test]);

    let Response::Snapshot {
        project_actions, ..
    } = client.request(Request::GetSnapshot).expect("GetSnapshot")
    else {
        panic!("expected Snapshot");
    };
    let labels: Vec<&str> = project_actions.iter().map(|a| a.label.as_str()).collect();
    assert_eq!(labels, ["lint", "test"]);
    assert_eq!(project_actions[1].position, 1);

    let error = client
        .request(Request::SetProjectActions {
            project_id,
            actions: vec![
                action(project_id, "Dev", "x"),
                action(project_id, "dev", "y"),
            ],
        })
        .unwrap_err();
    assert_eq!(error_code(error), protocol::ErrorCode::InvalidRequest);
}

fn readiness_of(
    events: &flume::Receiver<DaemonEvent>,
    session_id: SessionId,
) -> domain::ActionReadiness {
    let event = common::wait_for(events, common::DEADLINE, |event| {
        matches!(event, DaemonEvent::SessionUpdated(s)
            if s.id == session_id
                && s.action.as_ref().is_some_and(|run| run.readiness != domain::ActionReadiness::Probing))
    })
    .expect("readiness should settle");
    let DaemonEvent::SessionUpdated(session) = event else {
        unreachable!()
    };
    session.action.expect("tagged").readiness
}

fn free_port() -> u16 {
    std::net::TcpListener::bind(("127.0.0.1", 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

#[test]
fn an_output_pattern_makes_a_task_ready_with_the_url_it_printed() {
    let (harness, repo) = setup();
    let daemon = harness.boot();
    let client = daemon.connect("actions");
    let events = client.events();
    let workspace_id = common::add_main_workspace(&client, repo.path());
    let project_id = project_id(&client);
    let mut dev = action(
        project_id,
        "dev",
        "printf 'booting\\n\\033[32mready\\033[0m at http://127.0.0.1:1234/\\n'; read -r line",
    );
    dev.ready = ActionReady::Output {
        pattern: "READY at".to_owned(),
        timeout_secs: 20,
    };
    let mut quiet = action(project_id, "quiet", "read -r line");
    quiet.ready = ActionReady::Output {
        pattern: "never".to_owned(),
        timeout_secs: 1,
    };
    save(&client, project_id, &[dev.clone(), quiet.clone()]);

    let (session_id, _) = started(run(&client, workspace_id, dev.id));
    assert_eq!(
        readiness_of(&events, session_id),
        domain::ActionReadiness::Ready {
            url: Some("http://127.0.0.1:1234/".to_owned())
        }
    );
    let (quiet_id, _) = started(run(&client, workspace_id, quiet.id));
    assert_eq!(
        readiness_of(&events, quiet_id),
        domain::ActionReadiness::TimedOut
    );
    daemon.kill_leftover_sessions();
}

#[test]
fn a_port_probe_waits_for_the_server_to_listen() {
    let (harness, repo) = setup();
    let daemon = harness.boot();
    let client = daemon.connect("actions");
    let events = client.events();
    let workspace_id = common::add_main_workspace(&client, repo.path());
    let project_id = project_id(&client);
    let port = free_port();
    let mut api = action(project_id, "api", "read -r line");
    api.port = Some(domain::ActionPort {
        base: port,
        per_worktree: false,
    });
    api.ready = ActionReady::Port { timeout_secs: 20 };
    save(&client, project_id, std::slice::from_ref(&api));

    let (session_id, _) = started(run(&client, workspace_id, api.id));
    let session = common::session(&client, session_id).unwrap();
    assert_eq!(
        session.action.unwrap().readiness,
        domain::ActionReadiness::Probing
    );
    let _server = std::net::TcpListener::bind(("127.0.0.1", port)).unwrap();
    assert_eq!(
        readiness_of(&events, session_id),
        domain::ActionReadiness::Ready {
            url: Some(format!("http://localhost:{port}"))
        }
    );
    daemon.kill_leftover_sessions();
}

#[test]
fn ports_are_injected_and_a_taken_one_is_reported_or_moved() {
    let (harness, repo) = setup();
    let daemon = harness.boot();
    let client = daemon.connect("actions");
    let events = client.events();
    let workspace_id = common::add_main_workspace(&client, repo.path());
    let project_id = project_id(&client);
    let held = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = held.local_addr().unwrap().port();

    let mut fixed = action(project_id, "fixed", "echo \"port=$PORT\"; read -r line");
    fixed.port = Some(domain::ActionPort {
        base: port,
        per_worktree: false,
    });
    let mut moving = action(
        project_id,
        "moving",
        "echo \"at={port} env=$FORGE_ACTION_PORT\"; read -r line",
    );
    moving.port = Some(domain::ActionPort {
        base: port,
        per_worktree: true,
    });
    save(&client, project_id, &[fixed.clone(), moving.clone()]);

    let (fixed_id, fixed_terminal) = started(run(&client, workspace_id, fixed.id));
    let session = common::session(&client, fixed_id).unwrap();
    assert_eq!(
        session.action.unwrap().readiness,
        domain::ActionReadiness::PortInUse { port }
    );
    assert_eq!(
        common::attach_and_read_line(&client, &events, fixed_terminal, "port="),
        Some(format!("port={port}"))
    );

    let (moving_id, moving_terminal) = started(run(&client, workspace_id, moving.id));
    let moved = common::session(&client, moving_id)
        .unwrap()
        .action
        .unwrap()
        .port
        .unwrap();
    assert_ne!(moved, port);
    assert_eq!(
        common::attach_and_read_line(&client, &events, moving_terminal, "at="),
        Some(format!("at={moved} env={moved}"))
    );
    drop(held);
    daemon.kill_leftover_sessions();
}

fn wait_for_session(
    events: &flume::Receiver<DaemonEvent>,
    session_id: SessionId,
    mut pred: impl FnMut(&domain::Session) -> bool,
) -> domain::Session {
    let event = common::wait_for(
        events,
        common::DEADLINE,
        |event| matches!(event, DaemonEvent::SessionUpdated(s) if s.id == session_id && pred(s)),
    )
    .expect("the session should reach the expected state");
    let DaemonEvent::SessionUpdated(session) = event else {
        unreachable!()
    };
    session
}

fn queued(response: Response) -> SessionId {
    match response {
        Response::ActionQueued { session_id } => session_id,
        other => panic!("expected ActionQueued, got {other:?}"),
    }
}

#[test]
fn a_dependent_spawns_only_once_its_dependency_is_ready() {
    let (harness, repo) = setup();
    let daemon = harness.boot();
    let client = daemon.connect("actions");
    let events = client.events();
    let workspace_id = common::add_main_workspace(&client, repo.path());
    let project_id = project_id(&client);
    let mut db = action(
        project_id,
        "db",
        "read -r -t 1 line; echo 'db accepting'; read -r line",
    );
    db.ready = ActionReady::Output {
        pattern: "accepting".to_owned(),
        timeout_secs: 20,
    };
    let mut dev = action(project_id, "dev", "echo dev-up; read -r line");
    dev.depends_on = vec![db.id];
    save(&client, project_id, &[db.clone(), dev.clone()]);

    let dev_session = queued(run(&client, workspace_id, dev.id));
    let waiting = common::session(&client, dev_session).unwrap();
    assert_eq!(waiting.state, SessionState::Starting);
    assert_eq!(
        waiting.terminal_id, None,
        "nothing is spawned while it waits"
    );
    assert_eq!(
        waiting.action.unwrap().readiness,
        domain::ActionReadiness::Waiting { on: vec![db.id] }
    );

    let running = wait_for_session(&events, dev_session, |s| s.state == SessionState::Running);
    let db_session = common::sessions(&client)
        .into_iter()
        .find(|s| s.action.as_ref().is_some_and(|run| run.action_id == db.id))
        .expect("the dependency was started");
    assert!(
        matches!(
            db_session.action.unwrap().readiness,
            domain::ActionReadiness::Ready { .. }
        ),
        "the dependent ran before its dependency was ready"
    );
    let line =
        common::attach_and_read_line(&client, &events, running.terminal_id.unwrap(), "dev-up");
    assert_eq!(line.as_deref(), Some("dev-up"));
    daemon.kill_leftover_sessions();
}

#[test]
fn a_failed_dependency_fails_its_dependent_and_a_waiting_one_can_be_stopped() {
    let (harness, repo) = setup();
    let daemon = harness.boot();
    let client = daemon.connect("actions");
    let events = client.events();
    let workspace_id = common::add_main_workspace(&client, repo.path());
    let project_id = project_id(&client);
    let migrate = action(project_id, "migrate", "exit 3");
    let mut api = action(project_id, "api", "echo api; read -r line");
    api.depends_on = vec![migrate.id];
    let mut slow = action(project_id, "slow", "read -r line");
    slow.ready = ActionReady::Output {
        pattern: "never".to_owned(),
        timeout_secs: 60,
    };
    let mut web = action(project_id, "web", "echo web; read -r line");
    web.depends_on = vec![slow.id];
    save(
        &client,
        project_id,
        &[migrate.clone(), api.clone(), slow.clone(), web.clone()],
    );

    let api_session = queued(run(&client, workspace_id, api.id));
    let failed = wait_for_session(&events, api_session, |s| {
        matches!(s.state, SessionState::Failed { .. })
    });
    let SessionState::Failed { reason } = failed.state else {
        unreachable!()
    };
    assert!(reason.contains("migrate exited with 3"), "{reason}");

    let web_session = queued(run(&client, workspace_id, web.id));
    client
        .request(Request::KillSession {
            session_id: web_session,
        })
        .expect("KillSession");
    wait_for_session(
        &events,
        web_session,
        |s| matches!(&s.state, SessionState::Failed { reason } if reason.contains("before it started")),
    );

    let mut cycle_a = action(project_id, "a", "true");
    let mut cycle_b = action(project_id, "b", "true");
    cycle_a.depends_on = vec![cycle_b.id];
    cycle_b.depends_on = vec![cycle_a.id];
    let error = client
        .request(Request::SetProjectActions {
            project_id,
            actions: vec![cycle_a, cycle_b],
        })
        .unwrap_err();
    assert_eq!(error_code(error), protocol::ErrorCode::InvalidRequest);
    daemon.kill_leftover_sessions();
}
