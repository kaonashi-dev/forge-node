//! Shared harness for the §19 acceptance-scenario integration tests.
//!
//! `tests/integration.rs` already covers the §21 integration list against a
//! daemon with an in-memory database and the machine's real environment. The
//! scenarios in this directory need two things that harness cannot give them,
//! so they get their own:
//!
//! * **A daemon that can be restarted.** Scenarios E and G are about what
//!   survives a daemon that went away, so the database has to be a file that
//!   both the old and the new daemon open, not `Db::open_in_memory`.
//! * **A hermetic `PATH`.** Scenario F is about what provider detection
//!   reports, which on a developer machine depends on which agent CLIs happen
//!   to be installed. Every daemon booted here runs with a generated login
//!   shell that exports a `PATH` containing exactly one directory the test
//!   owns, so detection (§13.1) sees only the fake binaries the test wrote and
//!   probes nothing else — no real provider, no `/usr/local/bin`.
//!
//! Everything that depends on PTY or process timing polls with a generous
//! deadline instead of sleeping a fixed amount, so the suite stays honest on
//! slow CI machines.

#![allow(dead_code)] // Each test binary uses a different subset of the harness.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use client::Client;
use daemon::core::Daemon;
use domain::{PtySize, Session, SessionId, SessionRole, SessionState, TerminalId, WorkspaceId};
use protocol::{DaemonEvent, ProviderInfo, Request, Response};

/// Upper bound for anything that waits on a real process, PTY or git command.
pub const DEADLINE: Duration = Duration::from_secs(30);

// =====================================================================
// Harness: a temp directory holding the database, socket, worktrees root
// and the fake `PATH` every daemon booted from it sees.
// =====================================================================

/// Everything a family of daemons started during one test shares.
pub struct Harness {
    tmp: tempfile::TempDir,
    socket: PathBuf,
    db: PathBuf,
    worktrees_root: PathBuf,
    bin: PathBuf,
    shell: PathBuf,
}

impl Harness {
    /// Create the temp tree and the fake login shell.
    ///
    /// The tree is rooted under `/tmp` so the socket path stays well under the
    /// `sun_path` limit (ADR-004); a `TMPDIR` on macOS is already ~50 bytes.
    #[must_use]
    pub fn new() -> Harness {
        let tmp = tempfile::tempdir_in("/tmp").expect("tempdir");
        let bin = tmp.path().join("bin");
        fs::create_dir_all(&bin).expect("create the fake PATH directory");
        // §12 resolves the environment by running `env -0` inside the login
        // shell. `env` is not a shell builtin, so with a `PATH` holding only
        // this directory the probe would print nothing between its sentinels,
        // the daemon would quietly take the process fallback — the machine's
        // real `PATH`, well-known bin directories and all — and detection would
        // find the developer's actual agent CLIs. Linking `env` in is what keeps
        // the single-entry `PATH` both hermetic and usable.
        std::os::unix::fs::symlink("/usr/bin/env", bin.join("env"))
            .expect("link /usr/bin/env into the fake PATH");
        let shell = write_executable(tmp.path(), "forge-test-shell", &login_shell_script(&bin));
        assert_hermetic_login_shell(&shell, &bin);
        Harness {
            socket: tmp.path().join("d.sock"),
            db: tmp.path().join("forge.db"),
            worktrees_root: tmp.path().join("worktrees"),
            bin,
            shell,
            tmp,
        }
    }

    /// The one directory on the `PATH` of every daemon booted from this
    /// harness. Write fake provider binaries here to make detection find them.
    #[must_use]
    pub fn bin(&self) -> &Path {
        &self.bin
    }

    /// The harness' temp root, for scratch files a test needs outside `PATH`.
    #[must_use]
    pub fn root(&self) -> &Path {
        self.tmp.path()
    }

    /// The database file the daemons share, so a restart sees the same rows.
    #[must_use]
    pub fn db_path(&self) -> &Path {
        &self.db
    }

    /// The transcript stores the usage scan reads (§16.2): `claude/projects/…`
    /// and `codex/sessions/…` live under here, and the harness' login shell
    /// points `CLAUDE_CONFIG_DIR`/`CODEX_HOME` at them.
    #[must_use]
    pub fn agent_home(&self) -> PathBuf {
        self.tmp.path().join("agent-home")
    }

    /// Start a daemon over this harness' database, socket and worktrees root,
    /// and wait until it is accepting connections.
    ///
    /// This is the production entry point ([`Daemon::start`], §15.3): it runs
    /// the startup reconciliation, the path validation, the worktree rescan and
    /// the background detection sweep, which is exactly what scenarios E, F and
    /// G are about.
    #[must_use]
    pub fn boot(&self) -> TestDaemon {
        self.boot_with(|_| {})
    }

    /// [`Harness::boot`] with the config tweaked first.
    ///
    /// The default config drops the session history on startup
    /// (`sessions.persist_history = false`), which is what the app ships with;
    /// the restart scenarios turn it on because the rows surviving a crash are
    /// the whole point of what they assert.
    #[must_use]
    pub fn boot_with(&self, configure: impl FnOnce(&mut daemon::config::Config)) -> TestDaemon {
        let db = persistence::Db::open(&self.db).expect("open the shared database");
        let mut cfg = daemon::config::Config::default();
        cfg.sessions.shell = self.shell.to_string_lossy().into_owned();
        configure(&mut cfg);

        let daemon = Daemon::start(
            db,
            cfg,
            self.worktrees_root.clone(),
            "test-instance".to_string(),
            "0.0.0-test".to_string(),
        )
        .expect("start daemon");

        let d = daemon.clone();
        let sock = self.socket.clone();
        let thread = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("tokio runtime");
            rt.block_on(async move {
                // Bind inside the runtime so tokio's UnixListener has a reactor.
                let listener = daemon::server::bind(&sock).expect("bind");
                daemon::server::serve(d, listener, sock.clone()).await;
            });
        });

        let socket = self.socket.clone();
        assert!(
            poll_until(Duration::from_secs(10), || socket.exists()),
            "the daemon should bind its socket"
        );
        TestDaemon {
            daemon,
            socket,
            _thread: thread,
        }
    }
}

impl Default for Harness {
    fn default() -> Self {
        Self::new()
    }
}

/// One running daemon of a [`Harness`].
pub struct TestDaemon {
    pub daemon: Arc<Daemon>,
    pub socket: PathBuf,
    _thread: std::thread::JoinHandle<()>,
}

impl TestDaemon {
    /// Connect a real `client::Client` to this daemon.
    #[must_use]
    pub fn connect(&self, client_version: &str) -> Client {
        Client::connect(&self.socket, client_version).expect("connect to the test daemon")
    }

    /// Simulate `kill -9` on the daemon (§19 G).
    ///
    /// The accept loop stops and unlinks the socket, and *nothing else
    /// happens*: no session is killed, no row is updated, the PTY children are
    /// simply abandoned. That is the state a crashed daemon leaves behind, and
    /// the reason `StopDaemon` cannot stand in for it — that path kills the
    /// sessions and records them `Exited`, so the next daemon would have
    /// nothing left to reconcile to `Orphaned`.
    pub fn crash(&self) {
        self.daemon.request_shutdown();
        assert!(
            poll_until(Duration::from_secs(10), || !self.socket.exists()),
            "the accept loop should unlink its socket on the way out"
        );
    }

    /// Kill the PTYs a crashed daemon still owns and block until they are gone.
    ///
    /// Only the abandoned core can reach them: the daemon that replaced it
    /// never inherited the runtimes. Call this *after* asserting on the new
    /// daemon and *before* driving it further — it writes `Exited` rows for
    /// those session ids into the database both daemons share.
    pub fn kill_leftover_sessions(&self) {
        for session in self.own_sessions().iter().filter(|s| s.state.is_active()) {
            let _ = self.daemon.handle_request(Request::KillSession {
                session_id: session.id,
            });
        }
        assert!(
            poll_until(DEADLINE, || self
                .own_sessions()
                .iter()
                .all(|s| !s.state.is_active())),
            "the abandoned PTYs should die when the crashed daemon kills them"
        );
    }

    /// The sessions this daemon core has in memory, asked directly rather than
    /// over the socket (a crashed daemon no longer has one).
    fn own_sessions(&self) -> Vec<Session> {
        match self.daemon.handle_request(Request::GetSnapshot) {
            Ok(Response::Snapshot { sessions, .. }) => sessions,
            _ => Vec::new(),
        }
    }
}

// =====================================================================
// Generated executables
// =====================================================================

/// Quote `s` as one POSIX single-quoted literal, safe to embed in a script.
fn sh_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for ch in s.chars() {
        if ch == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}

/// Write `script` to `dir/name`, mark it `0755` and return its path.
fn write_executable(dir: &Path, name: &str, script: &str) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, script).expect("write generated script");
    let mut perms = fs::metadata(&path).expect("stat script").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&path, perms).expect("chmod script");
    path
}

/// The generated login shell (§12) that pins `PATH` to `bin`.
///
/// The daemon resolves the environment by running `<shell> -l -c '... env -0
/// ...'`. The `-l` is dropped here on purpose: a real login shell sources
/// `/etc/profile`, which on macOS runs `path_helper` and rebuilds `PATH` from
/// `/etc/paths` — the test would then detect whichever agent CLIs this machine
/// has installed instead of the fakes it wrote. `ENV`/`BASH_ENV` are cleared
/// for the same reason on the interactive path.
fn login_shell_script(bin: &Path) -> String {
    let agent_home = bin.parent().expect("bin has a parent").join("agent-home");
    format!(
        "#!/bin/sh\n\
         # Generated by crates/daemon/tests/common: a login shell with a PATH\n\
         # the test owns, so agent detection (§13.1) is hermetic.\n\
         PATH={path}\n\
         export PATH\n\
         # The agent transcript stores the usage scan reads (§16.2), pointed at\n\
         # the harness tree: without this a token count would be read off the\n\
         # developer's own ~/.claude, which is neither hermetic nor cheap.\n\
         CLAUDE_CONFIG_DIR={claude}\n\
         CODEX_HOME={codex}\n\
         export CLAUDE_CONFIG_DIR CODEX_HOME\n\
         unset ENV\n\
         unset BASH_ENV\n\
         [ \"$1\" = -l ] && shift\n\
         exec /bin/sh \"$@\"\n",
        path = sh_quote(&bin.to_string_lossy()),
        claude = sh_quote(&agent_home.join("claude").to_string_lossy()),
        codex = sh_quote(&agent_home.join("codex").to_string_lossy())
    )
}

/// Check the generated shell the way §12 will, and fail the test setup if the
/// answer is not the single directory the harness owns.
///
/// This guard exists because the failure it catches is silent: if the login
/// shell yields no variables the daemon falls back to the process environment
/// with a widened `PATH`, detection then finds whichever agent CLIs the
/// developer has installed, and a test that meant to probe its own fixtures
/// quietly probes the machine instead.
fn assert_hermetic_login_shell(shell: &Path, bin: &Path) {
    let output = Command::new(shell)
        .args(["-l", "-c", "env -0"])
        .output()
        .expect("run the generated login shell");
    let vars = String::from_utf8_lossy(&output.stdout);
    let path = vars
        .split('\0')
        .find_map(|entry| entry.strip_prefix("PATH="))
        .unwrap_or_default();
    assert_eq!(
        path,
        bin.to_string_lossy(),
        "the generated login shell must resolve to exactly the harness PATH, \
         or the daemon takes the §12 process fallback and detection stops being hermetic"
    );
}

/// Write a fake agent CLI into `dir` and return its path.
///
/// * `<name> --version` prints `version_output`, which is what the §13.1 probe
///   reads to accept or reject the binary.
/// * Launched as an agent it prints `<marker>_cwd_ok` when the daemon really
///   started it inside the workspace it advertises through `FORGE_WORKSPACE`
///   (§13.3), `<marker>_cwd_bad` otherwise, and then behaves like a TUI that
///   stays up, so the session stays `Running` until something kills it.
///
/// Both sides of the cwd comparison are resolved with `pwd -P`, so the macOS
/// `/tmp` → `/private/tmp` symlink cannot fake a mismatch: a plain `pwd` after
/// `cd` prints the *logical* path it was given, while the process' own
/// directory resolves physically.
pub fn write_fake_agent_cli(dir: &Path, name: &str, marker: &str, version_output: &str) -> PathBuf {
    let script = format!(
        "#!/bin/sh\n\
         if [ \"$1\" = --version ]; then\n\
         \x20 printf '%s\\n' {version}\n\
         \x20 exit 0\n\
         fi\n\
         here=$(/bin/pwd -P)\n\
         want=$(cd \"$FORGE_WORKSPACE\" 2>/dev/null && /bin/pwd -P)\n\
         if [ \"$here\" = \"$want\" ]; then\n\
         \x20 printf '%s\\n' {marker}_cwd_ok\n\
         else\n\
         \x20 printf '%s\\n' {marker}_cwd_bad\n\
         fi\n\
         exec /bin/cat\n",
        version = sh_quote(version_output),
        marker = sh_quote(marker),
    );
    write_executable(dir, name, &script)
}

/// A fake agent CLI that reports the environment and arguments it was given.
///
/// Prints `<marker>:<basename of $CLAUDE_CONFIG_DIR or "none">:<args>` on one
/// line — short enough to survive an 80-column grid — then blocks on `cat` so
/// the session stays `Running` like a real TUI would.
pub fn write_env_reporting_cli(dir: &Path, name: &str, marker: &str, version: &str) -> PathBuf {
    let script = format!(
        "#!/bin/sh\n\
         if [ \"$1\" = --version ]; then\n\
         \x20 printf '%s\\n' {version}\n\
         \x20 exit 0\n\
         fi\n\
         dir=${{CLAUDE_CONFIG_DIR:-none}}\n\
         printf '%s:%s:%s\\n' {marker} \"${{dir##*/}}\" \"$*\"\n\
         exec /bin/cat\n",
        version = sh_quote(version),
        marker = sh_quote(marker),
    );
    write_executable(dir, name, &script)
}

// =====================================================================
// Event and polling helpers
// =====================================================================

/// Drain events until `pred` matches or the timeout elapses.
pub fn wait_for(
    rx: &flume::Receiver<DaemonEvent>,
    timeout: Duration,
    mut pred: impl FnMut(&DaemonEvent) -> bool,
) -> Option<DaemonEvent> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(event) => {
                if pred(&event) {
                    return Some(event);
                }
            }
            Err(flume::RecvTimeoutError::Timeout) => continue,
            Err(flume::RecvTimeoutError::Disconnected) => break,
        }
    }
    None
}

/// Poll `cond` every 50 ms until it holds or `timeout` elapses.
pub fn poll_until(timeout: Duration, mut cond: impl FnMut() -> bool) -> bool {
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

/// The text of one grid row.
pub fn row_text(row: &domain::Row) -> String {
    row.cells.iter().map(|c| c.text.as_str()).collect()
}

/// Drain terminal deltas until one carries a row whose text matches `pred`.
pub fn wait_for_row(
    rx: &flume::Receiver<DaemonEvent>,
    timeout: Duration,
    mut pred: impl FnMut(&str) -> bool,
) -> Option<String> {
    let mut found = None;
    wait_for(rx, timeout, |event| match event {
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

// =====================================================================
// Request helpers
// =====================================================================

/// Add `repo` as a project and return the id of its `Main` workspace.
pub fn add_main_workspace(client: &Client, repo: &Path) -> WorkspaceId {
    let response = client
        .request(Request::AddProject {
            path: repo.to_path_buf(),
        })
        .expect("AddProject");
    assert_eq!(response, Response::Ack);
    main_workspace(client)
}

/// The `Main` workspace of the only project.
pub fn main_workspace(client: &Client) -> WorkspaceId {
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

/// Every workspace the daemon knows about.
pub fn workspaces(client: &Client) -> Vec<domain::Workspace> {
    let Response::Snapshot { workspaces, .. } =
        client.request(Request::GetSnapshot).expect("GetSnapshot")
    else {
        panic!("expected Snapshot");
    };
    workspaces
}

/// Every session the daemon knows about.
pub fn sessions(client: &Client) -> Vec<Session> {
    let Response::Snapshot { sessions, .. } =
        client.request(Request::GetSnapshot).expect("GetSnapshot")
    else {
        panic!("expected Snapshot");
    };
    sessions
}

/// One session by id, or `None` if the daemon dropped it.
pub fn session(client: &Client, session_id: SessionId) -> Option<Session> {
    sessions(client).into_iter().find(|s| s.id == session_id)
}

/// Every provider with its current detection result (§13.1).
pub fn providers(client: &Client) -> Vec<ProviderInfo> {
    match client
        .request(Request::ListAgentProviders)
        .expect("ListAgentProviders")
    {
        Response::Providers(providers) => providers,
        other => panic!("expected Providers, got {other:?}"),
    }
}

/// The detection result of one provider.
pub fn provider(client: &Client, id: &str) -> ProviderInfo {
    providers(client)
        .into_iter()
        .find(|p| p.descriptor.id.as_str() == id)
        .unwrap_or_else(|| panic!("no provider `{id}` is registered"))
}

/// Wait until every provider's detection result satisfies `pred`.
///
/// Startup detection runs on its own thread (§13.1), so the first snapshot a
/// client takes can still carry the default `NotFound` placeholders.
pub fn wait_for_detection(client: &Client, mut pred: impl FnMut(&[ProviderInfo]) -> bool) {
    assert!(
        poll_until(DEADLINE, || pred(&providers(client))),
        "provider detection never reached the expected state: {:?}",
        providers(client)
            .iter()
            .map(|p| (p.descriptor.id.to_string(), p.detection.status.clone()))
            .collect::<Vec<_>>()
    );
}

/// Create a shell session and wait until it reaches `Running`.
pub fn create_shell_session(
    client: &Client,
    events: &flume::Receiver<DaemonEvent>,
    workspace_id: WorkspaceId,
) -> (SessionId, TerminalId) {
    let response = client
        .request(Request::CreateShellSession {
            workspace_id,
            parent: None,
            role: SessionRole::Generic,
        })
        .expect("CreateShellSession");
    let Response::SessionCreated {
        session_id,
        terminal_id,
    } = response
    else {
        panic!("expected SessionCreated, got {response:?}");
    };
    let running = wait_for_running(events, |session| session.parent_session_id.is_none());
    assert_eq!(
        running,
        (session_id, terminal_id),
        "the response ids name the running session (L3)"
    );
    running
}

/// Create a child session under `parent` and wait until it reaches `Running`.
pub fn create_child_session(
    client: &Client,
    events: &flume::Receiver<DaemonEvent>,
    parent: SessionId,
    role: SessionRole,
) -> (SessionId, TerminalId) {
    let response = client
        .request(Request::CreateChildSession {
            parent_session_id: parent,
            kind: domain::SessionKind::Shell,
            provider_id: None,
            profile_id: None,
            role,
            workspace_policy: domain::ChildWorkspacePolicy::SameWorkspace,
            initial_prompt: None,
        })
        .expect("CreateChildSession");
    assert!(
        matches!(response, Response::SessionCreated { .. }),
        "expected SessionCreated, got {response:?}"
    );
    wait_for_running(events, move |session| {
        session.parent_session_id == Some(parent)
    })
}

/// Create an agent session and wait until it reaches `Running`.
pub fn create_agent_session(
    client: &Client,
    events: &flume::Receiver<DaemonEvent>,
    workspace_id: WorkspaceId,
    provider_id: &str,
) -> (SessionId, TerminalId) {
    let id = domain::AgentProviderId::new(provider_id);
    let response = client
        .request(Request::CreateAgentSession {
            workspace_id,
            provider_id: id.clone(),
            profile_id: None,
            parent: None,
            role: SessionRole::Generic,
            resume: None,
            initial_prompt: None,
            read_only: false,
        })
        .unwrap_or_else(|e| panic!("CreateAgentSession({provider_id}) failed: {e}"));
    assert!(
        matches!(response, Response::SessionCreated { .. }),
        "expected SessionCreated, got {response:?}"
    );
    wait_for_running(events, move |session| {
        session.agent_provider_id.as_ref() == Some(&id)
    })
}

/// Wait for a `SessionUpdated` that reaches `Running` with a terminal and
/// satisfies `pred`.
pub fn wait_for_running(
    events: &flume::Receiver<DaemonEvent>,
    mut pred: impl FnMut(&Session) -> bool,
) -> (SessionId, TerminalId) {
    let event = wait_for(events, DEADLINE, |event| {
        matches!(event, DaemonEvent::SessionUpdated(session)
            if session.state == SessionState::Running
                && session.terminal_id.is_some()
                && pred(session))
    })
    .expect("a session should reach Running");
    match event {
        DaemonEvent::SessionUpdated(session) => {
            (session.id, session.terminal_id.expect("terminal"))
        }
        _ => unreachable!(),
    }
}

/// `AttachTerminal` at `cols`x`rows`, returning the `AttachAck` snapshot (§10.5).
pub fn attach(
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

/// Attach to a terminal and return the first line that starts with `prefix`,
/// whether it was already on the grid when we attached or arrives as a delta.
///
/// Reading the whole verdict rather than matching one expected string is what
/// makes a failure legible: `Some("…_cwd_bad")` says the program ran in the
/// wrong directory, `None` says it never ran at all.
pub fn attach_and_read_line(
    client: &Client,
    events: &flume::Receiver<DaemonEvent>,
    terminal_id: TerminalId,
    prefix: &str,
) -> Option<String> {
    let snapshot = attach(client, terminal_id, 80, 24);
    if let Some(line) = snapshot_line_starting_with(&snapshot, prefix) {
        return Some(line);
    }
    let mut found = None;
    wait_for_row(events, DEADLINE, |text| {
        let text = text.trim();
        if text.starts_with(prefix) {
            found = Some(text.to_string());
            true
        } else {
            false
        }
    });
    found
}

/// The first line of `snapshot` that starts with `prefix`.
fn snapshot_line_starting_with(
    snapshot: &domain::TerminalSnapshot,
    prefix: &str,
) -> Option<String> {
    snapshot
        .scrollback_tail
        .iter()
        .chain(snapshot.visible.iter())
        .map(|row| row_text(row).trim().to_string())
        .find(|text| text.starts_with(prefix))
}

/// Type `text` into a terminal's PTY, as a real keyboard would: Enter is a
/// carriage return, not a line feed (§11.6).
pub fn write_input(client: &Client, terminal_id: TerminalId, text: &str) {
    let response = client
        .request(Request::WriteTerminalInput {
            terminal_id,
            bytes: text.replace('\n', "\r").into_bytes(),
        })
        .expect("WriteTerminalInput");
    assert_eq!(response, Response::Ack);
}

/// Marker printed by [`wait_for_shell_prompt`] once the shell runs commands.
const READY_MARKER: &str = "forge_shell_ready";

/// Block until the shell inside `terminal_id` actually executes what it is sent.
///
/// `SessionState::Running` only means the PTY was spawned (§7.3); the shell's
/// line editor discards whatever was typed before it started. The sentinel is
/// written with a quote in the middle so the *echo* of the typed line cannot
/// match it: only real command output does.
pub fn wait_for_shell_prompt(
    client: &Client,
    events: &flume::Receiver<DaemonEvent>,
    terminal_id: TerminalId,
) {
    let deadline = Instant::now() + DEADLINE;
    while Instant::now() < deadline {
        write_input(client, terminal_id, "echo forge_shell\"\"_ready\n");
        if wait_for_row(events, Duration::from_secs(1), |t| t.contains(READY_MARKER)).is_some() {
            return;
        }
    }
    panic!("the shell never reached a prompt within {DEADLINE:?}");
}

/// Kill a session and wait until it is reported `Exited` (§7.3).
pub fn kill_and_wait(
    client: &Client,
    events: &flume::Receiver<DaemonEvent>,
    session_id: SessionId,
) -> bool {
    client
        .request(Request::KillSession { session_id })
        .expect("KillSession");
    wait_for(events, DEADLINE, |event| {
        matches!(event, DaemonEvent::SessionUpdated(session)
            if session.id == session_id && matches!(session.state, SessionState::Exited { .. }))
    })
    .is_some()
}

/// Ask the daemon to stop. The response may never arrive: the connection
/// handler breaks out of its read loop as soon as the shutdown flag is set
/// (§9.1), so a dropped connection is a valid outcome.
pub fn stop_daemon(client: &Client) {
    match client.request(Request::StopDaemon {
        kill_sessions: true,
    }) {
        Ok(Response::Ack) | Err(client::ClientError::Disconnected) => {}
        other => panic!("unexpected StopDaemon result: {other:?}"),
    }
}

// =====================================================================
// Git assertions (test fixtures may call git directly, unlike the daemon)
// =====================================================================

/// `git -C <repo> rev-parse --abbrev-ref HEAD`.
#[must_use]
pub fn checked_out_branch(repo: &Path) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .expect("run git rev-parse");
    assert!(
        out.status.success(),
        "git rev-parse failed in {}: {}",
        repo.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}
