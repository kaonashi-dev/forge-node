//! End-to-end daemon integration tests (§21): a real daemon on a temporary
//! socket driven by the real `client`, spawning real shell PTYs.
//!
//! Covers the §21 integration list — spawn, I/O, resize, kill (with a `sleep`
//! grandchild that must die too, §11.3), reconnect with grid verification,
//! 20 concurrent sessions, full queue → resync (§10.5), `StopDaemon` (§9.1) —
//! plus scenarios A, D and H of §19 and the worktree lifecycle of §14.
//!
//! Everything that depends on PTY or process timing polls with a generous
//! deadline instead of sleeping a fixed amount, so the suite stays honest on
//! slow CI machines. The one exception is the backpressure test, where being
//! slow to read *is* the condition under test.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use client::Client;
use daemon::core::Daemon;
use daemon::registry::CLIENT_QUEUE_CAPACITY;
use domain::{PtySize, SessionId, SessionState, TerminalId, WorkspaceId};
use protocol::{
    ClientKind, ClientMessage, DaemonEvent, DaemonMessage, Hello, RemoveProjectPolicy, Request,
    Response, PROTOCOL_VERSION,
};

/// A running daemon on a temp socket, torn down on drop.
struct TestDaemon {
    socket: PathBuf,
    _tmp: tempfile::TempDir,
    daemon: Arc<Daemon>,
    _thread: std::thread::JoinHandle<()>,
}

fn start_daemon() -> TestDaemon {
    start_daemon_with(test_config())
}

/// The config every test starts from.
///
/// Sessions run `/bin/sh` rather than `$SHELL` (§15.4 `[sessions].shell`): the
/// developer's login shell sources rc files that can hang or leave background
/// work behind, which has nothing to do with what these tests assert. POSIX `sh`
/// gives the same `set +m`, `$!`, `$$` and job semantics on macOS and Linux.
fn test_config() -> daemon::config::Config {
    let mut cfg = daemon::config::Config::default();
    cfg.sessions.shell = "/bin/sh".to_string();
    cfg
}

fn start_daemon_with(cfg: daemon::config::Config) -> TestDaemon {
    // Keep the socket path short by rooting it under /tmp (§ADR-004).
    let tmp = tempfile::tempdir_in("/tmp").expect("tempdir");
    let socket = tmp.path().join("d.sock");
    let db = persistence::Db::open_in_memory().expect("db");
    let worktrees_root = tmp.path().join("worktrees");

    let daemon = Daemon::start(
        db,
        cfg,
        worktrees_root,
        "test-instance".to_string(),
        "0.0.0-test".to_string(),
    )
    .expect("start daemon");

    let d = daemon.clone();
    let sock = socket.clone();
    let thread = std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async move {
            // Bind inside the runtime so tokio's UnixListener has a reactor.
            let listener = daemon::server::bind(&sock).expect("bind");
            daemon::server::serve(d, listener, sock.clone()).await;
        });
    });

    // Wait for the socket to exist before connecting.
    let deadline = Instant::now() + Duration::from_secs(5);
    while !socket.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }

    TestDaemon {
        socket,
        _tmp: tmp,
        daemon,
        _thread: thread,
    }
}

/// Drain events until `pred` matches or the timeout elapses.
fn wait_for(
    rx: &flume::Receiver<DaemonEvent>,
    timeout: Duration,
    mut pred: impl FnMut(&DaemonEvent) -> bool,
) -> Option<DaemonEvent> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(ev) => {
                if pred(&ev) {
                    return Some(ev);
                }
            }
            Err(flume::RecvTimeoutError::Timeout) => continue,
            Err(_) => break,
        }
    }
    None
}

fn row_text(row: &domain::Row) -> String {
    row.cells.iter().map(|c| c.text.as_str()).collect()
}

/// Drain terminal deltas until one carries a row whose text matches `pred`,
/// returning that row's text.
fn wait_for_row(
    rx: &flume::Receiver<DaemonEvent>,
    timeout: Duration,
    mut pred: impl FnMut(&str) -> bool,
) -> Option<String> {
    let mut found = None;
    wait_for(rx, timeout, |ev| match ev {
        DaemonEvent::TerminalDelta { delta, .. } => delta.rows.iter().any(|(_, row)| {
            let text = row_text(row);
            if pred(&text) {
                found = Some(text);
                true
            } else {
                false
            }
        }),
        _ => false,
    });
    found
}

/// The first run of digits following `key` in `text` (`"gc=1234"` → `1234`).
/// Skips occurrences with no digits after them, which is how the shell's echo of
/// the typed `gc=$!` is told apart from its output.
fn number_after(text: &str, key: &str) -> Option<i32> {
    let mut rest = text;
    while let Some(at) = rest.find(key) {
        let tail = &rest[at + key.len()..];
        let digits: String = tail.chars().take_while(char::is_ascii_digit).collect();
        if !digits.is_empty() {
            return digits.parse().ok();
        }
        rest = tail;
    }
    None
}

/// Whether `pid` still exists, via `kill(pid, 0)` (§11.3 verification).
fn pid_alive(pid: i32) -> bool {
    nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).is_ok()
}

/// Poll `cond` every 50 ms until it holds or `timeout` elapses.
fn poll_until(timeout: Duration, mut cond: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if cond() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Add `repo` as a project and return the id of its `Main` workspace.
fn add_main_workspace(client: &Client, repo: &Path) -> WorkspaceId {
    let resp = client
        .request(Request::AddProject {
            path: repo.to_path_buf(),
        })
        .expect("AddProject");
    assert_eq!(resp, Response::Ack);

    let Response::Snapshot { workspaces, .. } =
        client.request(Request::GetSnapshot).expect("GetSnapshot")
    else {
        panic!("expected Snapshot");
    };
    workspaces
        .iter()
        .find(|w| w.kind == domain::WorkspaceKind::Main)
        .expect("a Main workspace")
        .id
}

/// Create a shell session and wait until it reaches `Running`, returning its ids.
fn create_shell_session(
    client: &Client,
    events: &flume::Receiver<DaemonEvent>,
    workspace_id: WorkspaceId,
) -> (SessionId, TerminalId) {
    let resp = client
        .request(Request::CreateShellSession {
            workspace_id,
            parent: None,
            role: domain::SessionRole::Generic,
        })
        .expect("CreateShellSession");
    assert!(matches!(resp, Response::SessionCreated { .. }), "{resp:?}");

    let ev = wait_for(events, Duration::from_secs(30), |ev| {
        matches!(ev, DaemonEvent::SessionUpdated(s)
            if s.state == SessionState::Running && s.terminal_id.is_some())
    })
    .expect("session should reach Running");
    match ev {
        DaemonEvent::SessionUpdated(s) => (s.id, s.terminal_id.unwrap()),
        _ => unreachable!(),
    }
}

/// `AttachTerminal` at `cols`x`rows`, returning the `AttachAck` snapshot (§10.5).
fn attach(
    client: &Client,
    terminal_id: TerminalId,
    cols: u16,
    rows: u16,
) -> domain::TerminalSnapshot {
    let size = PtySize {
        cols,
        rows,
        pixel_width: 0,
        pixel_height: 0,
    };
    match client
        .request(Request::AttachTerminal { terminal_id, size })
        .expect("AttachTerminal")
    {
        Response::AttachAck { snapshot } => snapshot,
        other => panic!("expected AttachAck, got {other:?}"),
    }
}

/// Marker printed by [`wait_for_shell_prompt`] once the shell runs commands.
const READY_MARKER: &str = "forge_shell_ready";

/// Block until the shell inside `terminal_id` actually executes what it is sent.
///
/// `SessionState::Running` only means the PTY was spawned (§7.3). An interactive
/// shell is still sourcing rc files at that point, and its line editor discards
/// whatever was typed before it started — the characters are echoed by the tty
/// and then dropped, so the grid shows the command but it never runs. Typing
/// straight after `Running` therefore raced the developer's `.zshrc`, which is
/// why the PTY tests passed or timed out depending on the machine.
///
/// The sentinel is written with a quote in the middle, so the *echo* of the
/// typed line cannot match `READY_MARKER`: only real command output does.
fn wait_for_shell_prompt(
    client: &Client,
    events: &flume::Receiver<DaemonEvent>,
    terminal_id: TerminalId,
) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        write_input(client, terminal_id, "echo forge_shell\"\"_ready\n");
        if wait_for_row(events, Duration::from_secs(1), |t| t.contains(READY_MARKER)).is_some() {
            return;
        }
    }
    panic!("the shell never reached a prompt within 30s");
}

/// Type `text` into a terminal's PTY, as a real keyboard would.
///
/// Newlines are sent as `\r`, not `\n`. Pressing Enter on a terminal transmits
/// carriage return, which is what `terminal_core::input` encodes for
/// `Key::Enter` (§11.6); an interactive shell runs its own line editor in raw
/// mode and does not accept a bare line feed as "submit". Sending `\n` here left
/// the command echoed on the grid but never executed, so assertions that only
/// looked for the typed text passed while the ones that waited for its *output*
/// timed out.
fn write_input(client: &Client, terminal_id: TerminalId, text: &str) {
    let resp = client
        .request(Request::WriteTerminalInput {
            terminal_id,
            bytes: text.replace('\n', "\r").into_bytes(),
        })
        .expect("WriteTerminalInput");
    assert_eq!(resp, Response::Ack);
}

/// Kill a session and wait until it is reported `Exited` (§7.3).
fn kill_and_wait(
    client: &Client,
    events: &flume::Receiver<DaemonEvent>,
    session_id: SessionId,
) -> bool {
    client
        .request(Request::KillSession { session_id })
        .expect("KillSession");
    wait_for(events, Duration::from_secs(30), |ev| {
        matches!(ev, DaemonEvent::SessionUpdated(s)
            if s.id == session_id && matches!(s.state, SessionState::Exited { .. }))
    })
    .is_some()
}

/// Ask the daemon to stop. The response may never arrive: the connection handler
/// breaks out of its read loop as soon as the shutdown flag is set (§9.1), so a
/// dropped connection is a valid outcome here.
fn stop_daemon(client: &Client) {
    match client.request(Request::StopDaemon {
        kill_sessions: true,
    }) {
        Ok(Response::Ack) | Err(client::ClientError::Disconnected) => {}
        other => panic!("unexpected StopDaemon result: {other:?}"),
    }
}

#[test]
fn end_to_end_shell_session() {
    let td = start_daemon();
    let client = Client::connect(&td.socket, "itest").expect("connect");
    let events = client.events();

    // A real git repo so the project is Git-detected and gets a Main workspace.
    let repo = test_support::init_repo().expect("git repo (git must be installed)");

    // --- AddProject ---
    let resp = client
        .request(Request::AddProject {
            path: repo.path().to_path_buf(),
        })
        .expect("AddProject");
    assert_eq!(resp, Response::Ack);

    // Snapshot should now show the project and a Main workspace.
    let Response::Snapshot {
        projects,
        workspaces,
        ..
    } = client.request(Request::GetSnapshot).expect("GetSnapshot")
    else {
        panic!("expected Snapshot");
    };
    assert_eq!(projects.len(), 1, "one project");
    let main_ws = workspaces
        .iter()
        .find(|w| w.kind == domain::WorkspaceKind::Main)
        .expect("a Main workspace");

    // --- CreateShellSession ---
    let resp = client
        .request(Request::CreateShellSession {
            workspace_id: main_ws.id,
            parent: None,
            role: domain::SessionRole::Generic,
        })
        .expect("CreateShellSession");
    assert!(matches!(resp, Response::SessionCreated { .. }), "{resp:?}");

    // Wait for the session to reach Running with a terminal id.
    let ev = wait_for(&events, Duration::from_secs(10), |ev| {
        matches!(ev, DaemonEvent::SessionUpdated(s)
            if s.state == SessionState::Running && s.terminal_id.is_some())
    })
    .expect("session should reach Running");
    let (session_id, terminal_id) = match ev {
        DaemonEvent::SessionUpdated(s) => (s.id, s.terminal_id.unwrap()),
        _ => unreachable!(),
    };

    // --- AttachTerminal ---
    let resp = client
        .request(Request::AttachTerminal {
            terminal_id,
            size: PtySize {
                cols: 80,
                rows: 24,
                pixel_width: 0,
                pixel_height: 0,
            },
        })
        .expect("AttachTerminal");
    assert!(matches!(resp, Response::AttachAck { .. }), "AttachAck");

    // --- WriteTerminalInput: echo a unique marker ---
    let marker = "forge_marker_9137";
    let cmd = format!("echo {marker}\n");
    client
        .request(Request::WriteTerminalInput {
            terminal_id,
            bytes: cmd.into_bytes(),
        })
        .expect("WriteTerminalInput");

    // Expect a delta whose rows contain the marker (echoed and/or printed).
    let seen = wait_for(&events, Duration::from_secs(10), |ev| match ev {
        DaemonEvent::TerminalDelta { delta, .. } => {
            delta.rows.iter().any(|(_, r)| row_text(r).contains(marker))
        }
        _ => false,
    });
    assert!(
        seen.is_some(),
        "should observe terminal output with the marker"
    );

    // --- KillSession → session exits ---
    client
        .request(Request::KillSession { session_id })
        .expect("KillSession");
    let exited = wait_for(&events, Duration::from_secs(10), |ev| {
        matches!(ev, DaemonEvent::SessionUpdated(s)
            if s.id == session_id && matches!(s.state, SessionState::Exited { .. }))
    });
    assert!(exited.is_some(), "session should reach Exited after kill");

    // --- CloseSession removes it (now that it is terminal) ---
    let resp = client
        .request(Request::CloseSession { session_id })
        .expect("CloseSession");
    assert_eq!(resp, Response::Ack);

    // Cleanup: request the daemon to stop so the serve thread exits.
    let _ = client.request(Request::StopDaemon {
        kill_sessions: true,
    });
    assert!(td.daemon.is_shutting_down());
}

#[test]
fn worktree_lifecycle() {
    let td = start_daemon();
    let client = Client::connect(&td.socket, "itest").expect("connect");
    let events = client.events();

    let repo = test_support::init_repo().expect("git repo");
    client
        .request(Request::AddProject {
            path: repo.path().to_path_buf(),
        })
        .expect("AddProject");

    let Response::Snapshot { projects, .. } =
        client.request(Request::GetSnapshot).expect("snapshot")
    else {
        panic!("snapshot");
    };
    let project_id = projects[0].id;

    // Create a managed worktree on a new branch.
    let resp = client
        .request(Request::CreateWorktree {
            project_id,
            branch: "feature/itest".to_string(),
            base: None,
            name: None,
        })
        .expect("CreateWorktree");
    assert_eq!(resp, Response::Ack);

    let created = wait_for(
        &events,
        Duration::from_secs(10),
        |ev| matches!(ev, DaemonEvent::WorkspaceCreated(w) if w.managed_by_app),
    )
    .expect("WorkspaceCreated for the managed worktree");
    let ws = match created {
        DaemonEvent::WorkspaceCreated(w) => w,
        _ => unreachable!(),
    };
    assert_eq!(ws.branch.as_deref(), Some("feature/itest"));
    assert!(ws.path.exists(), "worktree dir exists on disk");

    // Remove it (no dirty state, no sessions) — should succeed without force.
    let resp = client
        .request(Request::RemoveWorktree {
            workspace_id: ws.id,
            force: false,
        })
        .expect("RemoveWorktree");
    assert_eq!(resp, Response::Ack);
    assert!(!ws.path.exists(), "worktree dir removed from disk");

    let _ = client.request(Request::RemoveProject {
        project_id,
        policy: RemoveProjectPolicy::KeepEverything,
    });
    let _ = client.request(Request::StopDaemon {
        kill_sessions: true,
    });
}

#[test]
fn resize_terminal_reaches_the_grid_and_the_pty() {
    let td = start_daemon();
    let client = Client::connect(&td.socket, "itest").expect("connect");
    let events = client.events();
    let repo = test_support::init_repo().expect("git repo");
    let workspace_id = add_main_workspace(&client, repo.path());
    let (session_id, terminal_id) = create_shell_session(&client, &events, workspace_id);

    // Terminals spawn at 80x24 (§11.2); attaching at the same size must not
    // resize anything.
    let snapshot = attach(&client, terminal_id, 80, 24);
    assert_eq!(snapshot.size.cols, 80);
    assert_eq!(snapshot.visible.len(), 24);
    wait_for_shell_prompt(&client, &events, terminal_id);

    let size = PtySize {
        cols: 100,
        rows: 40,
        pixel_width: 0,
        pixel_height: 0,
    };
    let resp = client
        .request(Request::ResizeTerminal { terminal_id, size })
        .expect("ResizeTerminal");
    assert_eq!(resp, Response::Ack);

    // The program inside the PTY must observe the new geometry: `ResizeTerminal`
    // drives `TIOCSWINSZ` on the master, so `stty` reads back "rows cols". The
    // delta carrying that output must be shaped by the new size as well
    // (§10.5, §11.4).
    write_input(&client, terminal_id, "stty size\n");
    let mut width = 0usize;
    let seen = wait_for(&events, Duration::from_secs(30), |ev| match ev {
        DaemonEvent::TerminalDelta { delta, .. } => delta.rows.iter().any(|(_, row)| {
            if row_text(row).contains("40 100") {
                width = row.cells.len();
                true
            } else {
                false
            }
        }),
        _ => false,
    });
    assert!(
        seen.is_some(),
        "`stty size` inside the PTY should report the resized geometry"
    );
    assert_eq!(width, 100, "deltas carry rows of the new width");

    let snapshot = attach(&client, terminal_id, 100, 40);
    assert_eq!(snapshot.size, size, "snapshot reports the new PTY size");
    assert_eq!(snapshot.visible.len(), 40, "the grid grew to 40 rows");
    assert!(
        snapshot.visible.iter().all(|r| r.cells.len() == 100),
        "every visible row is 100 cells wide"
    );

    assert!(kill_and_wait(&client, &events, session_id));
    stop_daemon(&client);
}

/// §11.3, the invariant the plan spells out: the kill signal goes to `-pgid`,
/// never to the individual pid, "which would orphan grandchildren".
#[test]
fn kill_session_kills_grandchildren() {
    let td = start_daemon();
    let client = Client::connect(&td.socket, "itest").expect("connect");
    let events = client.events();
    let repo = test_support::init_repo().expect("git repo");
    let workspace_id = add_main_workspace(&client, repo.path());
    let (session_id, terminal_id) = create_shell_session(&client, &events, workspace_id);
    attach(&client, terminal_id, 80, 24);
    wait_for_shell_prompt(&client, &events, terminal_id);

    // `set +m` turns job control off so the background `sleep` stays in the
    // session's process group (the child is its leader after `setsid`, §11.3)
    // instead of being given one of its own. That is what makes this a test of
    // `kill(-pgid)`: with a plain `kill(pid)` the shell would die and the
    // grandchild would survive as an orphan.
    write_input(
        &client,
        terminal_id,
        "set +m; sleep 300 & echo gc=$! shell=$$\n",
    );
    let row = wait_for_row(&events, Duration::from_secs(30), |text| {
        number_after(text, "gc=").is_some() && number_after(text, "shell=").is_some()
    })
    .expect("the shell should print the background pid and its own pid");
    let grandchild = number_after(&row, "gc=").expect("grandchild pid");
    let shell = number_after(&row, "shell=").expect("shell pid");

    assert!(pid_alive(grandchild), "the `sleep` grandchild is running");
    assert_eq!(
        nix::unistd::getpgid(Some(nix::unistd::Pid::from_raw(grandchild)))
            .map(nix::unistd::Pid::as_raw),
        Ok(shell),
        "the grandchild must share the session's process group for this test to mean anything"
    );

    client
        .request(Request::KillSession { session_id })
        .expect("KillSession");

    // Poll instead of sleeping: SIGHUP reaches the whole group at once, but the
    // escalation to SIGKILL waits out the grace period (3 s by default, §11.3).
    assert!(
        poll_until(Duration::from_secs(30), || !pid_alive(grandchild)),
        "the `sleep` grandchild must die with its process group, not be orphaned"
    );
    assert!(
        wait_for(&events, Duration::from_secs(30), |ev| {
            matches!(ev, DaemonEvent::SessionUpdated(s)
                if s.id == session_id && matches!(s.state, SessionState::Exited { .. }))
        })
        .is_some(),
        "the session should reach Exited"
    );

    stop_daemon(&client);
}

/// §11.3 ends with "reap; estado `Exited { signal }`": after the kill the child
/// must be waited for — no zombie left in the process table for the lifetime of
/// the daemon — and the resulting `Exited` must carry the *numeric* signal that
/// terminated it, not an empty status.
///
/// `kill(pid, 0)` succeeds for a zombie, so the liveness poll below is itself
/// the reap assertion: the pid only disappears once it has been waited for.
#[test]
fn a_killed_shell_is_reaped_and_reports_its_signal() {
    let td = start_daemon();
    let client = Client::connect(&td.socket, "itest").expect("connect");
    let events = client.events();
    let repo = test_support::init_repo().expect("git repo");
    let workspace_id = add_main_workspace(&client, repo.path());
    let (session_id, terminal_id) = create_shell_session(&client, &events, workspace_id);
    attach(&client, terminal_id, 80, 24);
    wait_for_shell_prompt(&client, &events, terminal_id);

    write_input(&client, terminal_id, "echo shell=$$\n");
    let row = wait_for_row(&events, Duration::from_secs(30), |t| {
        number_after(t, "shell=").is_some()
    })
    .expect("the shell should print its pid");
    let shell = number_after(&row, "shell=").expect("shell pid");

    client
        .request(Request::KillSession { session_id })
        .expect("KillSession");

    let ev = wait_for(&events, Duration::from_secs(30), |ev| {
        matches!(ev, DaemonEvent::SessionUpdated(s)
            if s.id == session_id && matches!(s.state, SessionState::Exited { .. }))
    })
    .expect("the session should reach Exited");
    let DaemonEvent::SessionUpdated(session) = ev else {
        unreachable!()
    };
    let SessionState::Exited { code, signal } = session.state else {
        unreachable!()
    };
    assert_eq!(
        signal,
        Some(nix::sys::signal::Signal::SIGHUP as i32),
        "a Shell session is killed with SIGHUP (§11.3), so the exit must report \
         that signal; got code={code:?} signal={signal:?}"
    );

    assert!(
        poll_until(Duration::from_secs(30), || !pid_alive(shell)),
        "the killed shell must be reaped, not left as a zombie"
    );

    stop_daemon(&client);
}

/// A `tracing` writer that keeps everything the daemon logs in memory, so a test
/// can assert on what did — and did not — reach the log (§22).
#[derive(Clone, Default)]
struct LogCapture(Arc<std::sync::Mutex<Vec<u8>>>);

impl LogCapture {
    fn contents(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
}

impl Write for LogCapture {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl tracing_subscriber::fmt::MakeWriter<'_> for LogCapture {
    type Writer = LogCapture;
    fn make_writer(&self) -> Self::Writer {
        self.clone()
    }
}

/// §22/§23: the log carries the *shape* of the traffic — one `ipc.request` span
/// per request, with its id and variant — and never its content. Neither the
/// bytes a client writes to a PTY nor the bytes the child writes back may appear
/// anywhere in it.
///
/// The subscriber is global, so it also sees whatever the rest of this binary's
/// tests log: the markers below are unique to this test, which makes "absent
/// from the whole capture" the strongest form of the assertion.
#[test]
fn terminal_content_never_reaches_the_log() {
    const TYPED: &str = "forge_secret_keystrokes_zzz1";
    const PRINTED: &str = "forge_secret_output_zzz2";

    let capture = LogCapture::default();
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_max_level(tracing::Level::TRACE)
        .with_span_events(tracing_subscriber::fmt::format::FmtSpan::NEW)
        .with_writer(capture.clone())
        .finish();
    // Another test in this binary may have installed one first; either way the
    // assertions below only need *a* capturing subscriber to be active.
    let _ = tracing::subscriber::set_global_default(subscriber);

    let td = start_daemon();
    let client = Client::connect(&td.socket, "itest").expect("connect");
    let events = client.events();
    let repo = test_support::init_repo().expect("git repo");
    let workspace_id = add_main_workspace(&client, repo.path());
    let (_session_id, terminal_id) = create_shell_session(&client, &events, workspace_id);
    attach(&client, terminal_id, 80, 24);
    wait_for_shell_prompt(&client, &events, terminal_id);

    // Type a secret and make the child echo a different one back, so both
    // directions of the PTY have carried content by the time we look.
    write_input(&client, terminal_id, &format!("echo {TYPED} {PRINTED}\n"));
    assert!(
        wait_for_row(&events, Duration::from_secs(30), |t| t.contains(PRINTED)).is_some(),
        "the shell should echo the marker back"
    );

    let logged = capture.contents();
    assert!(
        logged.contains("ipc.request"),
        "§22 requires an ipc.request span per request; log was:\n{logged}"
    );
    assert!(
        logged.contains("WriteTerminalInput"),
        "the span must record the request variant; log was:\n{logged}"
    );
    assert!(
        !logged.contains(TYPED),
        "keystrokes leaked into the log (§22/§23)"
    );
    assert!(
        !logged.contains(PRINTED),
        "PTY output leaked into the log (§22/§23)"
    );

    stop_daemon(&client);
}

/// §9.1: on the way out the daemon kills every live session with the configured
/// grace — including one that *ignores* the first signal, which is what the
/// escalation to SIGKILL exists for.
///
/// The shell traps SIGHUP, so nothing but the escalation can end it. Before this
/// was synchronous the escalation lived on a detached thread that the exiting
/// process took with it, and the session survived the daemon holding its PTY.
#[test]
fn shutdown_kills_a_session_that_ignores_the_first_signal() {
    let mut cfg = test_config();
    cfg.sessions.kill_grace_ms = 300;
    let td = start_daemon_with(cfg);
    let client = Client::connect(&td.socket, "itest").expect("connect");
    let events = client.events();
    let repo = test_support::init_repo().expect("git repo");
    let workspace_id = add_main_workspace(&client, repo.path());
    let (_session_id, terminal_id) = create_shell_session(&client, &events, workspace_id);
    attach(&client, terminal_id, 80, 24);
    wait_for_shell_prompt(&client, &events, terminal_id);

    write_input(&client, terminal_id, "trap '' HUP; echo shell=$$\n");
    let row = wait_for_row(&events, Duration::from_secs(30), |t| {
        number_after(t, "shell=").is_some()
    })
    .expect("the shell should print its pid");
    let shell = number_after(&row, "shell=").expect("shell pid");
    assert!(pid_alive(shell));

    td.daemon.shutdown_sessions();

    assert!(
        poll_until(Duration::from_secs(30), || !pid_alive(shell)),
        "a session ignoring SIGHUP must still be SIGKILLed on shutdown (§9.1)"
    );
}

/// Scenario D of §19: closing the GUI does not kill the PTY, and a brand-new
/// client that re-attaches sees the same grid — no lost or duplicated lines.
#[test]
fn reconnect_with_a_new_client_preserves_the_grid() {
    const FIRST: &str = "forge_reattach_alpha";
    const SECOND: &str = "forge_reattach_beta";

    let td = start_daemon();
    let repo = test_support::init_repo().expect("git repo");

    let (session_id, terminal_id) = {
        let client = Client::connect(&td.socket, "itest-a").expect("connect");
        let events = client.events();
        let workspace_id = add_main_workspace(&client, repo.path());
        let (session_id, terminal_id) = create_shell_session(&client, &events, workspace_id);
        attach(&client, terminal_id, 80, 24);
        wait_for_shell_prompt(&client, &events, terminal_id);

        // FIRST is emitted now; SECOND is emitted by a background child after
        // this client has disconnected. This exercises output produced while
        // no GUI is attached, not just reconstruction of old output.
        write_input(
            &client,
            terminal_id,
            "stty -echo; echo forge_noecho\"\"_ready\n",
        );
        assert!(
            wait_for_row(&events, Duration::from_secs(30), |text| {
                text.contains("forge_noecho_ready")
            })
            .is_some(),
            "the shell should disable tty echo before the marker script"
        );
        write_input(
            &client,
            terminal_id,
            &format!("printf '{FIRST}\\n'; /bin/sh -c \"sleep 1; printf '\\r\\n{SECOND}\\n'\" &\n"),
        );
        assert!(
            wait_for_row(&events, Duration::from_secs(30), |text| text
                .contains(FIRST))
            .is_some(),
            "the terminal should emit the first marker before disconnect"
        );
        (session_id, terminal_id)
        // Dropping the client disconnects it; the daemon drops its
        // subscriptions and keeps the PTY running (§9.1).
    };

    // Keep the GUI absent long enough for the background child to emit SECOND.
    std::thread::sleep(Duration::from_millis(1_500));

    let client = Client::connect(&td.socket, "itest-b").expect("reconnect");
    let events = client.events();
    let Response::Snapshot { sessions, .. } =
        client.request(Request::GetSnapshot).expect("GetSnapshot")
    else {
        panic!("expected Snapshot");
    };
    let session = sessions
        .iter()
        .find(|s| s.id == session_id)
        .expect("the session survived the disconnect");
    assert_eq!(
        session.state,
        SessionState::Running,
        "the PTY outlives the client (§9.1)"
    );
    assert_eq!(session.terminal_id, Some(terminal_id));

    // Re-attach at the same size so nothing reflows, then verify the grid.
    let snapshot = attach(&client, terminal_id, 80, 24);
    let rows: Vec<String> = snapshot
        .scrollback_tail
        .iter()
        .chain(snapshot.visible.iter())
        .map(row_text)
        .collect();
    for marker in [FIRST, SECOND] {
        assert_eq!(
            rows.iter().filter(|text| text.contains(marker)).count(),
            1,
            "`{marker}` appears exactly once: no lost or duplicated lines"
        );
    }
    let first = rows.iter().position(|text| text.contains(FIRST)).unwrap();
    let second = rows.iter().position(|text| text.contains(SECOND)).unwrap();
    assert!(first < second, "output order survives the reconnect");

    // The reattached client is a live subscriber again.
    write_input(
        &client,
        terminal_id,
        "stty echo; echo forge_reattach_gamma\n",
    );
    assert!(
        wait_for_row(&events, Duration::from_secs(30), |t| {
            t.trim() == "forge_reattach_gamma"
        })
        .is_some(),
        "the new client receives deltas after re-attaching"
    );

    assert!(kill_and_wait(&client, &events, session_id));
    stop_daemon(&client);
}

/// Phase 0.3 reference-machine smoke. This is ignored in CI because it needs
/// the four real provider binaries and their user-level authentication state.
#[test]
#[ignore = "requires installed claude, codex, opencode, and cursor-agent TUIs"]
fn installed_agent_tuis_render_and_accept_input() {
    const PROVIDERS: [&str; 4] = ["claude", "codex", "opencode", "cursor"];

    let td = start_daemon();
    let client = Client::connect(&td.socket, "agent-tui-smoke").expect("connect");
    let events = client.events();
    let repo = test_support::init_repo().expect("git repo");
    let workspace_id = add_main_workspace(&client, repo.path());

    assert!(
        poll_until(Duration::from_secs(30), || {
            let Ok(store) = client.load_store() else {
                return false;
            };
            PROVIDERS.iter().all(|id| {
                store.providers.iter().any(|provider| {
                    provider.descriptor.id.as_str() == *id
                        && provider.detection.status.is_installed()
                })
            })
        }),
        "all four provider binaries must be detected on the reference machine"
    );

    for provider in PROVIDERS {
        client
            .create_agent_session(
                workspace_id,
                domain::AgentProviderId::new(provider),
                None,
                None,
                None,
            )
            .unwrap_or_else(|error| panic!("failed to launch {provider}: {error}"));
        let event = wait_for(&events, Duration::from_secs(30), |event| {
            matches!(event, DaemonEvent::SessionUpdated(session)
                if session.agent_provider_id.as_ref().is_some_and(|id| id.as_str() == provider)
                    && session.state == SessionState::Running
                    && session.terminal_id.is_some())
        })
        .unwrap_or_else(|| panic!("{provider} did not reach Running"));
        let DaemonEvent::SessionUpdated(session) = event else {
            unreachable!()
        };
        let terminal_id = session.terminal_id.expect("agent terminal");
        let snapshot = attach(&client, terminal_id, 100, 32);
        let initial_content = snapshot
            .visible
            .iter()
            .any(|row| row.cells.iter().any(|cell| !cell.text.trim().is_empty()));
        let rendered = initial_content
            || wait_for(&events, Duration::from_secs(30), |event| {
                matches!(event, DaemonEvent::TerminalDelta { terminal_id: id, delta }
                    if *id == terminal_id && delta.rows.iter().any(|(_, row)|
                        row.cells.iter().any(|cell| !cell.text.trim().is_empty())))
            })
            .is_some();
        assert!(rendered, "{provider} must render non-blank terminal cells");

        // Arrow-down plus Escape is non-destructive but exercises the complete
        // GUI input byte path expected by interactive provider TUIs.
        client
            .write_terminal_input(terminal_id, b"\x1b[B\x1b".to_vec())
            .unwrap_or_else(|error| panic!("failed to send input to {provider}: {error}"));
        assert!(kill_and_wait(&client, &events, session.id));
    }

    stop_daemon(&client);
}

/// Phase 0.5 reference-machine measurement for the daemon-owned part of the
/// input path. The GUI records the remaining delta-to-render interval itself.
#[test]
#[ignore = "reference-machine latency measurement"]
fn key_byte_to_grid_delta_p95_is_under_33ms() {
    const SAMPLES: usize = 40;

    let td = start_daemon();
    let client = Client::connect(&td.socket, "latency-smoke").expect("connect");
    let events = client.events();
    let repo = test_support::init_repo().expect("git repo");
    let workspace_id = add_main_workspace(&client, repo.path());
    let (session_id, terminal_id) = create_shell_session(&client, &events, workspace_id);
    attach(&client, terminal_id, 100, 32);
    wait_for_shell_prompt(&client, &events, terminal_id);

    let mut samples = Vec::with_capacity(SAMPLES);
    for _ in 0..SAMPLES {
        while events.try_recv().is_ok() {}
        let started = Instant::now();
        client
            .write_terminal_input(terminal_id, b"x".to_vec())
            .expect("write one key byte");
        assert!(
            wait_for(&events, Duration::from_secs(5), |event| {
                matches!(event, DaemonEvent::TerminalDelta { terminal_id: id, .. }
                    if *id == terminal_id)
            })
            .is_some(),
            "each key byte should produce a terminal delta"
        );
        samples.push(started.elapsed());
    }
    samples.sort_unstable();
    let p95 = samples[((SAMPLES as f64 * 0.95).ceil() as usize) - 1];
    eprintln!(
        "key byte -> authoritative grid delta: p95={:.2}ms",
        p95.as_secs_f64() * 1000.
    );
    assert!(
        p95 <= Duration::from_millis(33),
        "key-to-grid p95 {:.2}ms exceeded 33ms",
        p95.as_secs_f64() * 1000.
    );

    assert!(kill_and_wait(&client, &events, session_id));
    stop_daemon(&client);
}

/// Phase 0.2 macOS reference smoke for real full-screen terminal programs.
#[test]
#[ignore = "requires local vim and htop binaries"]
fn macos_vim_htop_color_and_alt_screen_smoke() {
    let td = start_daemon();
    let client = Client::connect(&td.socket, "terminal-tui-smoke").expect("connect");
    let events = client.events();
    let repo = test_support::init_repo().expect("git repo");
    let workspace_id = add_main_workspace(&client, repo.path());
    let (session_id, terminal_id) = create_shell_session(&client, &events, workspace_id);
    attach(&client, terminal_id, 100, 32);
    wait_for_shell_prompt(&client, &events, terminal_id);

    write_input(
        &client,
        terminal_id,
        "printf '\\033[31mforge_color_ready\\033[0m\\n'\n",
    );
    assert!(
        wait_for(&events, Duration::from_secs(30), |event| {
            matches!(event, DaemonEvent::TerminalDelta { delta, .. }
            if delta.rows.iter().any(|(_, row)| {
                row_text(row).contains("forge_color_ready")
                    && row.cells.iter().any(|cell| cell.fg != domain::Color::Default)
            }))
        })
        .is_some(),
        "ANSI color output should reach styled cells"
    );

    write_input(
        &client,
        terminal_id,
        "vim -Nu NONE -n +'call setline(1,\"forge_vim_ready\")' +'redraw'\n",
    );
    assert!(
        wait_for(&events, Duration::from_secs(30), |event| {
            matches!(event, DaemonEvent::TerminalDelta { delta, .. }
                if delta.modes.alt_screen
                    && delta.rows.iter().any(|(_, row)| row_text(row).contains("forge_vim_ready")))
        })
        .is_some(),
        "vim should render in the alternate screen"
    );
    client
        .write_terminal_input(terminal_id, b"\x1b:qa!\r".to_vec())
        .expect("exit vim");
    std::thread::sleep(Duration::from_millis(200));

    write_input(&client, terminal_id, "htop\n");
    assert!(
        wait_for(&events, Duration::from_secs(30), |event| {
            matches!(event, DaemonEvent::TerminalDelta { delta, .. }
                if delta.modes.alt_screen
                    && delta.rows.iter().any(|(_, row)|
                        row.cells.iter().any(|cell| !cell.text.trim().is_empty())))
        })
        .is_some(),
        "htop should render non-blank alternate-screen cells"
    );
    client
        .write_terminal_input(terminal_id, b"q".to_vec())
        .expect("exit htop");

    assert!(kill_and_wait(&client, &events, session_id));
    stop_daemon(&client);
}

/// §21: 20 concurrent sessions in one workspace, all reaching `Running`,
/// all writable, all closable.
#[test]
fn twenty_concurrent_sessions() {
    const COUNT: usize = 20;

    let td = start_daemon();
    let client = Client::connect(&td.socket, "itest").expect("connect");
    let events = client.events();
    let repo = test_support::init_repo().expect("git repo");
    let workspace_id = add_main_workspace(&client, repo.path());

    for _ in 0..COUNT {
        let resp = client
            .request(Request::CreateShellSession {
                workspace_id,
                parent: None,
                role: domain::SessionRole::Generic,
            })
            .expect("CreateShellSession");
        assert!(matches!(resp, Response::SessionCreated { .. }), "{resp:?}");
    }

    let mut running: HashMap<SessionId, TerminalId> = HashMap::new();
    wait_for(&events, Duration::from_secs(60), |ev| {
        if let DaemonEvent::SessionUpdated(s) = ev {
            if s.state == SessionState::Running {
                if let Some(terminal_id) = s.terminal_id {
                    running.insert(s.id, terminal_id);
                }
            }
        }
        running.len() == COUNT
    });
    assert_eq!(running.len(), COUNT, "all {COUNT} sessions reach Running");

    // Every terminal accepts input.
    for terminal_id in running.values() {
        write_input(&client, *terminal_id, "echo forge_many\n");
    }

    // Kill them all, then close them once they are terminal (§7.3).
    for session_id in running.keys() {
        client
            .request(Request::KillSession {
                session_id: *session_id,
            })
            .expect("KillSession");
    }
    let mut exited: HashMap<SessionId, ()> = HashMap::new();
    wait_for(&events, Duration::from_secs(60), |ev| {
        if let DaemonEvent::SessionUpdated(s) = ev {
            if matches!(s.state, SessionState::Exited { .. }) && running.contains_key(&s.id) {
                exited.insert(s.id, ());
            }
        }
        exited.len() == COUNT
    });
    assert_eq!(exited.len(), COUNT, "all {COUNT} sessions reach Exited");

    for session_id in running.keys() {
        let resp = client
            .request(Request::CloseSession {
                session_id: *session_id,
            })
            .expect("CloseSession");
        assert_eq!(resp, Response::Ack);
    }
    let Response::Snapshot { sessions, .. } =
        client.request(Request::GetSnapshot).expect("GetSnapshot")
    else {
        panic!("expected Snapshot");
    };
    assert!(sessions.is_empty(), "every session was closed");

    stop_daemon(&client);
}

/// §9.1: `StopDaemon { kill_sessions: true }` kills live sessions, flips the
/// shutdown flag and lets the accept loop unlink the socket on its way out.
#[test]
fn stop_daemon_kills_sessions_and_frees_the_socket() {
    let td = start_daemon();
    let client = Client::connect(&td.socket, "itest").expect("connect");
    let events = client.events();
    let repo = test_support::init_repo().expect("git repo");
    let workspace_id = add_main_workspace(&client, repo.path());
    let (_session_id, terminal_id) = create_shell_session(&client, &events, workspace_id);
    attach(&client, terminal_id, 80, 24);
    wait_for_shell_prompt(&client, &events, terminal_id);

    // Learn the shell's pid so "killed the sessions" is verifiable from outside.
    write_input(&client, terminal_id, "echo shell=$$\n");
    let row = wait_for_row(&events, Duration::from_secs(30), |t| {
        number_after(t, "shell=").is_some()
    })
    .expect("the shell should print its pid");
    let shell = number_after(&row, "shell=").expect("shell pid");
    assert!(pid_alive(shell));

    // Refusing to stop with live sessions is the documented precondition.
    assert!(
        client
            .request(Request::StopDaemon {
                kill_sessions: false
            })
            .is_err(),
        "StopDaemon without kill_sessions is refused while a session is live"
    );
    assert!(!td.daemon.is_shutting_down());

    stop_daemon(&client);
    assert!(
        poll_until(Duration::from_secs(10), || td.daemon.is_shutting_down()),
        "the shutdown flag is set"
    );
    assert!(
        poll_until(Duration::from_secs(30), || !pid_alive(shell)),
        "the session's shell is killed"
    );
    assert!(
        poll_until(Duration::from_secs(30), || !td.socket.exists()),
        "the accept loop unlinks the socket on exit, freeing the path"
    );
    assert!(
        Client::connect(&td.socket, "itest").is_err(),
        "nothing is listening any more"
    );
}

/// A raw protocol client that completes the handshake and then, deliberately,
/// stops reading — the only way to hold the daemon's bounded per-client queue
/// full from an integration test (`client::Client` always drains eagerly).
struct SlowClient {
    stream: UnixStream,
    decoder: protocol::FrameDecoder,
    buf: Vec<u8>,
}

impl SlowClient {
    /// Connect and perform the §9.2 handshake.
    fn connect(socket: &Path) -> SlowClient {
        let stream = UnixStream::connect(socket).expect("connect");
        let mut slow = SlowClient {
            stream,
            decoder: protocol::FrameDecoder::new(),
            buf: vec![0u8; 64 * 1024],
        };
        slow.send(&ClientMessage::Hello(Hello {
            protocol_version: PROTOCOL_VERSION,
            client_version: "itest-slow".to_string(),
            client_kind: ClientKind::Gui,
        }));
        match slow.recv(Duration::from_secs(10)) {
            Some(DaemonMessage::HelloAck(_)) => {}
            other => panic!("expected HelloAck, got {other:?}"),
        }
        slow
    }

    fn send(&mut self, msg: &ClientMessage) {
        let frame = protocol::encode_frame(msg).expect("encode");
        self.stream.write_all(&frame).expect("write");
        self.stream.flush().expect("flush");
    }

    /// Read the next message, or `None` on timeout/EOF.
    fn recv(&mut self, timeout: Duration) -> Option<DaemonMessage> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Ok(Some(payload)) = self.decoder.next_frame() {
                return Some(protocol::decode_payload(&payload).expect("decode"));
            }
            let left = deadline.checked_duration_since(Instant::now())?;
            self.stream
                .set_read_timeout(Some(left.max(Duration::from_millis(1))))
                .ok()?;
            match self.stream.read(&mut self.buf) {
                Ok(0) => return None,
                Ok(n) => self.decoder.push(&self.buf[..n]),
                Err(_) => return None, // timeout or socket error
            }
        }
    }

    /// Send a request and read messages until its response arrives.
    fn request(&mut self, request_id: u64, body: Request) -> Response {
        self.send(&ClientMessage::Request { request_id, body });
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let left = deadline
                .checked_duration_since(Instant::now())
                .expect("response before the deadline");
            match self.recv(left) {
                Some(DaemonMessage::Response {
                    request_id: id,
                    body,
                }) if id == request_id => return body.expect("request succeeded"),
                Some(_) => continue,
                None => panic!("no response to request {request_id}"),
            }
        }
    }
}

/// §10.5 / scenario H of §19: a subscriber that stops draining must not grow the
/// daemon's memory without bound. Once its 256-deep queue fills, the backlog is
/// dropped and it gets one fresh `TerminalResync` instead.
#[test]
fn slow_subscriber_gets_a_resync_instead_of_an_unbounded_backlog() {
    // Wide lines so each delta is big enough to fill the kernel socket buffer
    // quickly; the queue behind it then fills at the ~125 deltas/s of §10.5.
    const FLOOD: &str = "forge_backpressure_flood_forge_backpressure_flood_forge_bp_xx";

    let td = start_daemon();
    let client = Client::connect(&td.socket, "itest").expect("connect");
    let events = client.events();
    let repo = test_support::init_repo().expect("git repo");
    let workspace_id = add_main_workspace(&client, repo.path());
    let (session_id, terminal_id) = create_shell_session(&client, &events, workspace_id);

    // Get the shell to a prompt before the slow client subscribes, then detach:
    // input typed while the shell is still sourcing rc files is echoed and then
    // dropped, so the flood would never start.
    attach(&client, terminal_id, 80, 24);
    wait_for_shell_prompt(&client, &events, terminal_id);
    client
        .request(Request::DetachTerminal { terminal_id })
        .expect("DetachTerminal");

    // The slow client subscribes, then never reads again until we say so.
    let mut slow = SlowClient::connect(&td.socket);
    let resp = slow.request(
        1,
        Request::AttachTerminal {
            terminal_id,
            size: PtySize {
                cols: 80,
                rows: 24,
                pixel_width: 0,
                pixel_height: 0,
            },
        },
    );
    assert!(matches!(resp, Response::AttachAck { .. }));

    // `set +m` keeps `yes` in the session's process group so the kill at the end
    // takes it down with the shell (§11.3).
    write_input(&client, terminal_id, &format!("set +m; yes {FLOOD}\n"));

    // Let the flood run while the slow client ignores its socket. This is the
    // one place a fixed pause is inherent: the test *is* the paused GUI of
    // scenario H. Filling the 256-deep queue at the ~8 ms delta floor of §10.5
    // takes ~2 s, so this leaves a wide margin on a slow CI machine.
    std::thread::sleep(Duration::from_secs(8));

    // Now drain. The daemon must hand us one fresh snapshot, not the backlog:
    // reading more than a couple of queues' worth of messages without a resync
    // means the daemon buffered without bound.
    let cap = CLIENT_QUEUE_CAPACITY * 2;
    let mut messages = 0usize;
    let mut last_delta_seq = 0u64;
    let mut resync_seq = None;
    let deadline = Instant::now() + Duration::from_secs(60);
    while resync_seq.is_none() && messages < cap && Instant::now() < deadline {
        match slow.recv(Duration::from_secs(5)) {
            Some(DaemonMessage::Event(DaemonEvent::TerminalDelta { delta, .. })) => {
                messages += 1;
                last_delta_seq = delta.seq;
            }
            Some(DaemonMessage::Event(DaemonEvent::TerminalResync { snapshot, .. })) => {
                resync_seq = Some(snapshot.seq);
            }
            Some(_) => messages += 1,
            None => break,
        }
    }

    let Some(resync_seq) = resync_seq else {
        panic!(
            "a slow subscriber must receive a TerminalResync (§10.5); \
             read {messages} message(s) without one"
        );
    };
    assert!(
        resync_seq > last_delta_seq + 1,
        "the resync skips the deltas that were dropped while the queue was full \
         ({last_delta_seq} -> {resync_seq}); the backlog is discarded, not replayed"
    );

    drop(slow);
    assert!(kill_and_wait(&client, &events, session_id));
    stop_daemon(&client);
}

/// Spawn the `forge-daemon` binary with `FORGE_SOCKET` pointed at `socket`.
fn run_stats_cli(socket: &Path) -> std::process::Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_forge-daemon"))
        .arg("stats")
        .env("FORGE_SOCKET", socket)
        .env_remove("RUST_BACKTRACE")
        .output()
        .expect("spawn forge-daemon stats")
}

#[test]
fn stats_against_running_daemon() {
    let td = start_daemon();
    let output = run_stats_cli(&td.socket);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "stats against a live daemon must exit 0; stderr={stderr}"
    );
    assert!(
        stdout.contains("uptime_secs="),
        "stdout must report uptime: {stdout}"
    );
    assert!(
        stdout.contains("sessions.starting="),
        "stdout must report sessions by state: {stdout}"
    );
    assert!(
        stdout.contains("sessions.running="),
        "stdout must report sessions by state: {stdout}"
    );
    assert!(
        stdout.contains("sessions.exited="),
        "stdout must report sessions by state: {stdout}"
    );
    assert!(
        stdout.contains("sessions.failed="),
        "stdout must report sessions by state: {stdout}"
    );
    assert!(
        stdout.contains("sessions.orphaned="),
        "stdout must report sessions by state: {stdout}"
    );
    assert!(
        stdout.contains("open_terminals="),
        "stdout must report open terminals: {stdout}"
    );
    assert!(
        stdout.contains("connected_clients="),
        "stdout must report connected clients: {stdout}"
    );
    // Fresh daemon: no sessions yet; the stats client itself counts as one
    // connected client; uptime is non-negative.
    assert!(stdout.contains("sessions.starting=0"));
    assert!(stdout.contains("sessions.running=0"));
    assert!(stdout.contains("open_terminals=0"));
    assert!(
        stdout.contains("connected_clients=1") || stdout.contains("connected_clients=2"),
        "at least the stats client is connected: {stdout}"
    );

    let client = Client::connect(&td.socket, "itest").expect("connect");
    stop_daemon(&client);
}

#[test]
fn stats_without_daemon_exits_nonzero() {
    let tmp = tempfile::tempdir_in("/tmp").expect("tempdir");
    let missing = tmp.path().join("no-daemon.sock");
    let output = run_stats_cli(&missing);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "stats with no daemon must exit non-zero"
    );
    assert!(!stderr.trim().is_empty(), "stderr must explain the failure");
    assert!(
        !stderr.to_lowercase().contains("stack backtrace"),
        "must not dump a stack backtrace by default: {stderr}"
    );
    assert!(
        !stderr.contains("not wired yet"),
        "must not be the old stub: {stderr}"
    );
    assert!(
        stderr.contains("daemon not running") || stderr.contains("forge-daemon stats"),
        "stderr must be a clear stats failure: {stderr}"
    );
    assert!(
        stdout.trim().is_empty(),
        "stdout must stay empty on failure"
    );
}

#[test]
fn stats_does_not_touch_singleton_lock() {
    let lock = daemon::paths::lock_path().expect("lock path");
    let before_exists = lock.exists();
    let before_meta = std::fs::metadata(&lock).ok();
    let before_mtime = before_meta.as_ref().and_then(|m| m.modified().ok());
    let before_bytes = before_exists.then(|| std::fs::read(&lock).ok()).flatten();

    let tmp = tempfile::tempdir_in("/tmp").expect("tempdir");
    let missing = tmp.path().join("stats-no-touch.sock");
    let output = run_stats_cli(&missing);
    assert!(
        !output.status.success(),
        "expected a failed stats against a missing socket"
    );

    let after_exists = lock.exists();
    assert_eq!(
        before_exists, after_exists,
        "stats must not create or remove the singleton lock"
    );
    if before_exists {
        let after_bytes = std::fs::read(&lock).ok();
        assert_eq!(
            before_bytes, after_bytes,
            "stats must not rewrite the singleton lock"
        );
        let after_mtime = std::fs::metadata(&lock)
            .ok()
            .and_then(|m| m.modified().ok());
        assert_eq!(
            before_mtime, after_mtime,
            "stats must not touch the singleton lock mtime"
        );
    }

    let side_lock = tmp.path().join("daemon.lock");
    assert!(
        !side_lock.exists(),
        "stats must not create a daemon.lock next to the test socket"
    );
}
