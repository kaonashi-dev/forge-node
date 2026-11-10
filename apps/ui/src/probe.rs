//! Synchronous live-daemon probe. It runs on the process main thread and has
//! no UI, so it cannot violate the UI-thread rule; the app always goes through
//! the worker.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use client::{Client, ClientError, Request, Store};

#[derive(serde::Serialize)]
pub struct ProbeReport {
    pub socket: PathBuf,
    pub daemon_version: String,
    pub snapshot_ms: f64,
    pub projects: usize,
    pub workspaces: usize,
    pub sessions: usize,
    pub added_project: bool,
    pub created_shell: bool,
    pub attach_ms: Option<f64>,
    pub terminal_seq: Option<u64>,
    pub terminal_size: Option<(u16, u16)>,
    pub input_ms: Option<f64>,
    pub session_id: Option<String>,
}

pub fn run(socket: &Path, client_version: &str) -> Result<ProbeReport, String> {
    let client = connect(socket, client_version)?;
    let daemon_version = client.daemon_info().daemon_version.clone();

    let started = Instant::now();
    let mut store = load(&client)?;
    let snapshot_ms = started.elapsed().as_secs_f64() * 1000.0;
    let mut added_project = false;
    if store.workspaces.is_empty() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"));
        client
            .add_project(path)
            .map_err(|error| format!("AddProject {}: {error}", path.display()))?;
        added_project = true;
        store = load(&client)?;
    }
    let workspace = store
        .workspaces
        .first()
        .map(|workspace| workspace.id)
        .ok_or_else(|| "daemon snapshot still has no workspace after AddProject".to_string())?;

    let mut created_shell = false;
    let (session_id, terminal_id) = match store
        .sessions
        .iter()
        .find_map(|session| session.terminal_id.map(|terminal| (session.id, terminal)))
    {
        Some(found) => found,
        None => {
            let (session_id, terminal_id) = client
                .create_shell_session(workspace)
                .map_err(|error| format!("CreateShellSession: {error}"))?;
            created_shell = true;
            store = load(&client)?;
            (session_id, terminal_id)
        }
    };

    let started = Instant::now();
    let size = domain::PtySize {
        cols: 100,
        rows: 30,
        pixel_width: 0,
        pixel_height: 0,
    };
    let snapshot = client
        .attach_terminal(terminal_id, size)
        .map_err(|error| format!("AttachTerminal: {error}"))?;
    let attach_ms = Some(started.elapsed().as_secs_f64() * 1000.0);
    let terminal_seq = Some(snapshot.seq);
    let terminal_size = Some((snapshot.size.cols, snapshot.size.rows));

    let started = Instant::now();
    client
        .write_terminal_input(terminal_id, b"echo forge-ui\r".to_vec())
        .map_err(|error| format!("WriteTerminalInput: {error}"))?;
    let input_ms = Some(started.elapsed().as_secs_f64() * 1000.0);

    let _ = client.detach_terminal(terminal_id);

    Ok(ProbeReport {
        socket: socket.to_path_buf(),
        daemon_version,
        snapshot_ms,
        projects: store.projects.len(),
        workspaces: store.workspaces.len(),
        sessions: store.sessions.len(),
        added_project,
        created_shell,
        attach_ms,
        terminal_seq,
        terminal_size,
        input_ms,
        session_id: Some(session_id.to_string()),
    })
}

fn connect(socket: &Path, client_version: &str) -> Result<Client, String> {
    let mut last = String::new();
    for attempt in 1..=20 {
        match Client::connect(socket, client_version) {
            Ok(client) => return Ok(client),
            Err(error) => {
                last = error.to_string();
                if attempt < 20 {
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
        }
    }
    Err(format!("connect {}: {last}", socket.display()))
}

fn load(client: &Client) -> Result<Store, String> {
    let response = client
        .request(Request::GetSnapshot)
        .map_err(|error: ClientError| error.to_string())?;
    let mut store = Store::new();
    store.apply_snapshot(response);
    Ok(store)
}
