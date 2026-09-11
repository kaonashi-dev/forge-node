//! Phase 2 gate: key-to-delta latency, and what the canvas is actually sent.
//!
//! Types into a real session on a running daemon, times the round trip from the
//! write to the delta that carried its echo, and encodes each frame the way the
//! WebView receives it — so the byte cost printed here is the byte cost on the
//! IPC path, not an estimate of it.
//!
//! Exit 0 when the p95 is inside the §2 budget, 1 when it is not or when the
//! daemon cannot be reached. Never run automatically: it starts a shell session
//! and types into it.

use std::time::{Duration, Instant};

use client::{CellGrid, DaemonEvent};
use domain::PtySize;
use forge_tauri::cells::{self, Damage};
use forge_tauri::{connect_or_spawn, Locator};

/// The gate: `p95 ≤ 50 ms`.
const BUDGET: Duration = Duration::from_millis(50);
/// Keystrokes to time. 120 is the window the status bar's readout keeps.
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
    match run() {
        Ok(true) => std::process::exit(0),
        Ok(false) => std::process::exit(1),
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    }
}

fn run() -> Result<bool, String> {
    let client = connect_or_spawn(&Locator::from_env())?;
    let mut store = client.load_store().map_err(|error| error.to_string())?;
    let workspace = store
        .workspaces
        .first()
        .map(|workspace| workspace.id)
        .ok_or_else(|| "no workspace: add a project first".to_string())?;
    let (session, terminal) = client
        .create_shell_session(workspace)
        .map_err(|error| error.to_string())?;
    let snapshot = client
        .attach_terminal(terminal, SIZE)
        .map_err(|error| error.to_string())?;
    store.attach_terminal(terminal, &snapshot);
    let events = client.events();

    // Let the shell finish printing its prompt, so the first sample is not
    // really a measurement of `zsh` starting up.
    drain(&events, Duration::from_millis(750));

    let mut latencies = Vec::with_capacity(SAMPLES);
    let mut frame_bytes = Vec::with_capacity(SAMPLES);
    let mut lost = 0usize;

    for _ in 0..SAMPLES {
        let sent = Instant::now();
        client
            .write_terminal_input(terminal, b"x".to_vec())
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

    // Undo the line of `x`es so the session is left the way it was found.
    let _ = client.write_terminal_input(terminal, vec![0x15]);
    let _ = client.detach_terminal(terminal);
    let _ = client.kill_session(session);

    report(&latencies, &frame_bytes, lost, store.terminal(&terminal));
    let p95 = percentile(&latencies, 0.95);
    Ok(p95.is_some_and(|value| value <= BUDGET) && lost == 0)
}

fn report(latencies: &[Duration], frame_bytes: &[usize], lost: usize, grid: Option<&CellGrid>) {
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
        BUDGET.as_millis()
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
