//! Phase 2 gate: key-to-delta latency, and what the canvas is actually sent.
//!
//! Types into a real session on a running daemon, times the round trip from the
//! write to the delta that carried its echo, and encodes each frame the way the
//! WebView receives it — so the byte cost printed here is the byte cost on the
//! IPC path, not an estimate of it.
//!
//! Exit 0 when the p95 is inside the §2 budget, 1 when it is not or when the
//! daemon cannot be reached. Never run automatically: it starts a session and
//! types into it.
//!
//! Two modes:
//!
//! ```sh
//! forge-tauri-latency                      # a shell session (the §2 gate)
//! forge-tauri-latency --editor <path>      # forge-editor on that file (R33)
//! ```
//!
//! The editor mode is the feature-19 measurement: it opens the file through
//! `CreateEditorSession`, so the round trip it times is the real route —
//! PTY → daemon VT → IPC → the frame the canvas paints — and it also reports
//! what one editor session costs in processes and resident memory, and how
//! often its state reaches the shell under a burst of typing.

use std::time::{Duration, Instant};

use client::{CellGrid, Client, DaemonEvent};
use domain::{PtySize, SessionId, TerminalId, WorkspaceId};
use forge_tauri::cells::{self, Damage};
use forge_tauri::{connect_or_spawn, Locator};

/// The gate: `p95 ≤ 50 ms`.
const BUDGET: Duration = Duration::from_millis(50);
/// The editor route's own budget (R33), tighter because it is a TUI redrawing
/// one row rather than a shell echoing a character.
const EDITOR_BUDGET: Duration = Duration::from_millis(16);
/// Keystrokes to time. 120 is the rolling p95 window.
const SAMPLES: usize = 120;
/// How long one echo may take before it is counted as lost.
const ECHO_TIMEOUT: Duration = Duration::from_secs(2);

const SIZE: PtySize = PtySize {
    cols: 100,
    rows: 32,
    pixel_width: 800,
    pixel_height: 576,
};

fn main() {
    let mut args = std::env::args().skip(1);
    let mode = match args.next().as_deref() {
        Some("--editor") => match args.next() {
            Some(path) => Mode::Editor(path),
            None => {
                eprintln!("--editor needs a workspace-relative path");
                std::process::exit(1);
            }
        },
        Some(other) => {
            eprintln!("unknown argument {other:?}; try --editor <path>");
            std::process::exit(1);
        }
        None => Mode::Shell,
    };
    match run(&mode) {
        Ok(true) => std::process::exit(0),
        Ok(false) => std::process::exit(1),
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    }
}

enum Mode {
    Shell,
    /// A workspace-relative path to open in `forge-editor`.
    Editor(String),
}

impl Mode {
    /// Start the session this mode measures.
    fn open(
        &self,
        client: &Client,
        workspace: WorkspaceId,
    ) -> Result<(SessionId, TerminalId), String> {
        match self {
            Mode::Shell => client
                .create_shell_session(workspace)
                .map_err(|error| error.to_string()),
            // Read-only because H1 is (design D6); the route being timed is the
            // same one either way.
            Mode::Editor(path) => client
                .create_editor_session(workspace, path, None, true, false)
                .map_err(|error| error.to_string()),
        }
    }

    fn budget(&self) -> Duration {
        match self {
            Mode::Shell => BUDGET,
            Mode::Editor(_) => EDITOR_BUDGET,
        }
    }

    /// What to type. A shell echoes a character; the editor is read-only in H1,
    /// so it is driven with a caret move, which is what redraws a row.
    fn keystroke(&self) -> Vec<u8> {
        match self {
            Mode::Shell => b"x".to_vec(),
            // A right arrow. It is a caret move, which is what redraws a row
            // without needing a writable buffer, and it walks forward through
            // the file rather than repainting the same cell 120 times.
            Mode::Editor(_) => b"\x1b[C".to_vec(),
        }
    }

    /// How long to wait before the first sample, for the session to settle.
    fn warmup(&self) -> Duration {
        match self {
            // Long enough for a login shell to print its prompt, so the first
            // sample is not really a measurement of `zsh` starting up.
            Mode::Shell => Duration::from_millis(750),
            // The editor has to handshake, receive the buffer and paint it.
            Mode::Editor(_) => Duration::from_millis(1_500),
        }
    }
}

fn run(mode: &Mode) -> Result<bool, String> {
    let client = connect_or_spawn(&Locator::from_env())?;
    let mut store = client.load_store().map_err(|error| error.to_string())?;
    let workspace = store
        .workspaces
        .first()
        .map(|workspace| workspace.id)
        .ok_or_else(|| "no workspace: add a project first".to_string())?;
    let (session, terminal) = mode.open(&client, workspace)?;
    let snapshot = client
        .attach_terminal(terminal, SIZE)
        .map_err(|error| error.to_string())?;
    store.attach_terminal(terminal, &snapshot);
    let events = client.events();

    drain(&events, mode.warmup());

    let mut latencies = Vec::with_capacity(SAMPLES);
    let mut frame_bytes = Vec::with_capacity(SAMPLES);
    let mut lost = 0usize;
    // R33: how often the editor's state reaches the shell while it is being
    // typed into. Counted over the whole burst, because the claim is that it
    // coalesces rather than riding every keystroke.
    let mut state_updates = 0usize;
    let burst_began = Instant::now();

    for _ in 0..SAMPLES {
        let sent = Instant::now();
        client
            .write_terminal_input(terminal, mode.keystroke())
            .map_err(|error| error.to_string())?;

        let deadline = sent + ECHO_TIMEOUT;
        let mut echoed = false;
        while Instant::now() < deadline {
            let Ok(event) = events.recv_timeout(deadline.saturating_duration_since(Instant::now()))
            else {
                break;
            };
            let damage = match &event {
                DaemonEvent::TerminalDelta {
                    terminal_id, delta, ..
                } if *terminal_id == terminal => {
                    if delta.scrolled_lines > 0 {
                        Damage::Full
                    } else {
                        Damage::Rows(delta.changed_rows().map(|(index, _)| index).collect())
                    }
                }
                _ => {
                    if let DaemonEvent::SessionUpdated(updated) = &event {
                        if updated.id == session && updated.editor.is_some() {
                            state_updates += 1;
                        }
                    }
                    let _ = store.apply_event(&event);
                    continue;
                }
            };
            let _ = store.apply_event(&event);
            latencies.push(sent.elapsed());
            echoed = true;

            if let Some(grid) = store.terminal(&terminal) {
                let payload = cells::frame(terminal, grid, 0, &damage, false, 1);
                let encoded = serde_json::to_vec(&payload).map_err(|error| error.to_string())?;
                frame_bytes.push(encoded.len());
            }
            break;
        }
        if !echoed {
            lost += 1;
        }
    }

    let burst = burst_began.elapsed();
    if let Mode::Editor(path) = mode {
        report_editor_cost(path, &client, session, state_updates, burst);
    } else {
        // Undo the line of `x`es so the session is left the way it was found.
        let _ = client.write_terminal_input(terminal, vec![0x15]);
    }
    let _ = client.detach_terminal(terminal);
    let _ = client.kill_session(session);

    report(
        &latencies,
        &frame_bytes,
        lost,
        store.terminal(&terminal),
        mode.budget(),
    );
    let p95 = percentile(&latencies, 0.95);
    Ok(p95.is_some_and(|value| value <= mode.budget()) && lost == 0)
}

/// What one editor session costs, read from the live process rather than
/// inferred: `ps` over the process group the daemon spawned.
fn report_editor_cost(
    path: &str,
    client: &Client,
    session: SessionId,
    state_updates: usize,
    burst: Duration,
) {
    println!("editor buffer: {path}");
    let state = client
        .load_store()
        .ok()
        .and_then(|store| store.sessions.iter().find(|s| s.id == session).cloned())
        .and_then(|s| s.editor);
    match state {
        Some(state) => println!(
            "editor state: {}:{} dirty={} read_only={} version={}",
            state.line, state.column, state.dirty, state.read_only, state.document_version
        ),
        None => println!("editor state: none reported"),
    }
    println!(
        "state broadcasts: {state_updates} in {:.1} s of typing ({:.2}/s)",
        burst.as_secs_f64(),
        state_updates as f64 / burst.as_secs_f64().max(f64::EPSILON)
    );

    // `ps` rather than a crate: this binary is a measurement tool run by hand,
    // and one `ps` costs nothing next to another dependency in the workspace.
    match std::process::Command::new("ps")
        .args(["-Ao", "pid=,rss=,comm="])
        .output()
    {
        Ok(out) => {
            let text = String::from_utf8_lossy(&out.stdout);
            let mut count = 0usize;
            let mut rss_kib = 0u64;
            for line in text.lines() {
                if !line.contains("forge-editor") {
                    continue;
                }
                count += 1;
                rss_kib += line
                    .split_whitespace()
                    .nth(1)
                    .and_then(|value| value.parse::<u64>().ok())
                    .unwrap_or(0);
            }
            println!(
                "forge-editor processes: {count} · rss {:.1} MiB",
                rss_kib as f64 / 1024.0
            );
        }
        Err(error) => println!("forge-editor processes: could not run ps ({error})"),
    }
}

fn report(
    latencies: &[Duration],
    frame_bytes: &[usize],
    lost: usize,
    grid: Option<&CellGrid>,
    budget: Duration,
) {
    let ms = |value: Option<Duration>| {
        value.map_or("—".to_string(), |d| {
            format!("{:.1}", d.as_secs_f64() * 1000.)
        })
    };
    println!("samples: {} (lost {lost})", latencies.len());
    println!(
        "key-to-delta: p50 {} ms · p95 {} ms · p99 {} ms · budget {} ms",
        ms(percentile(latencies, 0.5)),
        ms(percentile(latencies, 0.95)),
        ms(percentile(latencies, 0.99)),
        budget.as_millis()
    );

    if !frame_bytes.is_empty() {
        let total: usize = frame_bytes.iter().sum();
        let max = frame_bytes.iter().copied().max().unwrap_or(0);
        println!(
            "frame on the wire: mean {} B · max {max} B",
            total / frame_bytes.len()
        );
    }

    // What a full-screen repaint costs, which is the frame the coalesce floor
    // is sized for and the one a per-cell encoding would have blown up.
    if let Some(grid) = grid {
        if let Ok(full) = serde_json::to_vec(&cells::frame(
            domain::TerminalId::new(),
            grid,
            0,
            &Damage::Full,
            false,
            0,
        )) {
            println!(
                "full repaint: {} B for {}×{} cells",
                full.len(),
                grid.size.cols,
                grid.visible.len()
            );
        }
    }
}

fn percentile(samples: &[Duration], fraction: f64) -> Option<Duration> {
    if samples.is_empty() {
        return None;
    }
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    let index = ((sorted.len() as f64 * fraction).ceil() as usize)
        .saturating_sub(1)
        .min(sorted.len() - 1);
    Some(sorted[index])
}

fn drain(events: &flume::Receiver<DaemonEvent>, window: Duration) {
    let deadline = Instant::now() + window;
    while Instant::now() < deadline {
        if events
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .is_err()
        {
            break;
        }
    }
}
