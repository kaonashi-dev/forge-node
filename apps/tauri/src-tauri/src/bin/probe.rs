//! Phase 1 wire gate: connect, print session count, attach, print first delta.
//!
//! Exit 0 on success (an idle terminal with no delta is still success).
//! Exit 1 if the socket cannot be reached and the daemon cannot be spawned.

use std::time::{Duration, Instant};

use client::DaemonEvent;
use domain::PtySize;
use forge_tauri::{connect_or_spawn, Locator, CLIENT_VERSION};

fn main() {
    let code = match run() {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("{error}");
            1
        }
    };
    std::process::exit(code);
}

fn run() -> Result<(), String> {
    let locator = Locator::from_env();
    let socket = locator.socket_path()?;
    eprintln!("connecting to {} ({CLIENT_VERSION})", socket.display());
    let client = connect_or_spawn(&locator)?;
    let info = client.daemon_info();
    println!(
        "connected: daemon {}, protocol {}",
        info.daemon_version, info.protocol_version
    );

    let store = client.load_store().map_err(|error| error.to_string())?;
    println!("sessions: {}", store.sessions.len());

    let Some(session) = store
        .sessions
        .iter()
        .find(|session| session.terminal_id.is_some())
    else {
        println!("no session with a terminal to attach");
        return Ok(());
    };
    let terminal_id = session.terminal_id.expect("checked above");
    let snapshot = client
        .attach_terminal(
            terminal_id,
            PtySize {
                cols: 80,
                rows: 24,
                pixel_width: 0,
                pixel_height: 0,
            },
        )
        .map_err(|error| error.to_string())?;
    println!(
        "attached {terminal_id}: seq={}, rows={}",
        snapshot.seq,
        snapshot.visible.len()
    );

    if std::env::var_os("FORGE_PROBE_NUDGE").is_some() {
        eprintln!("nudging {terminal_id} with a carriage return");
        client
            .write_terminal_input(terminal_id, b"\r".to_vec())
            .map_err(|error| error.to_string())?;
    }

    let events = client.events();
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut seen = 0usize;
    let mut first_delta = None;
    while Instant::now() < deadline {
        match events.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(event) => {
                seen += 1;
                if let DaemonEvent::TerminalDelta {
                    terminal_id: id,
                    delta,
                } = event
                {
                    if id == terminal_id && first_delta.is_none() {
                        first_delta = Some(delta.seq);
                    }
                }
            }
            Err(_) => break,
        }
        if first_delta.is_some() {
            break;
        }
    }

    match first_delta {
        Some(seq) => println!("first delta: seq={seq}"),
        None => println!("no delta within 3s (an idle terminal is not a failure)"),
    }
    println!("events seen: {seen}");
    let _ = client.detach_terminal(terminal_id);
    Ok(())
}
