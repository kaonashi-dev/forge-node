//! forge-daemon library surface (ADR-003).
//!
//! The runtime is exposed as a library so integration tests can start a daemon
//! on a temporary socket and drive it with the real `client`. The `forge-daemon`
//! binary (`main.rs`) is a thin CLI over [`run`].

mod acp;
pub mod config;
mod context_xfer;
pub mod core;
pub mod environment;
pub mod external_agents;
pub mod harness_io;
pub mod harness_runner;
pub mod idle;
pub mod jobs;
pub mod juva;
pub mod lockfile;
pub mod logging;
mod opencode_db;
pub mod paths;
pub mod pull_requests;
pub mod registry;
pub mod server;
pub mod session_cli;
pub mod shares;
pub mod terminal;
pub mod terminfo;
pub mod usage_stats;

pub use core::Daemon;

use std::path::PathBuf;

/// Resolve paths + config and print them — the `info` subcommand.
pub fn info() -> anyhow::Result<()> {
    let cfg = config::Config::load(&paths::config_file()?)?;
    println!("socket:      {}", paths::socket_path()?.display());
    println!("lockfile:    {}", paths::lock_path()?.display());
    println!("db:          {}", paths::db_path()?.display());
    println!("worktrees:   {}", paths::worktrees_root()?.display());
    println!("logs:        {}", paths::logs_dir()?.display());
    println!("scrollback:  {}", cfg.effective_scrollback());
    println!("kill grace:  {:?}", cfg.kill_grace());
    println!(
        "history:     {}",
        if cfg.sessions.persist_history {
            "kept across restarts (sessions.persist_history = true)"
        } else {
            "dropped on start (sessions.persist_history = false)"
        }
    );
    Ok(())
}

/// Ask a running daemon for runtime statistics and print them (§22).
///
/// Connects to the resolved socket (honoring `FORGE_SOCKET`), sends
/// `GetStats`, and writes stable `key=value` lines to stdout. Does **not**
/// acquire the singleton lock or start a daemon.
pub fn stats() -> anyhow::Result<()> {
    let socket = paths::socket_path()?;
    let client = client::Client::connect(&socket, env!("CARGO_PKG_VERSION"))
        .map_err(|error| anyhow::anyhow!("forge-daemon stats: daemon not running ({error})"))?;
    let stats = client
        .get_stats()
        .map_err(|error| anyhow::anyhow!("forge-daemon stats: failed to read stats ({error})"))?;
    println!("uptime_secs={}", stats.uptime_secs);
    println!("sessions.starting={}", stats.sessions_by_state.starting);
    println!("sessions.running={}", stats.sessions_by_state.running);
    println!("sessions.exited={}", stats.sessions_by_state.exited);
    println!("sessions.failed={}", stats.sessions_by_state.failed);
    println!("sessions.orphaned={}", stats.sessions_by_state.orphaned);
    println!("open_terminals={}", stats.open_terminals);
    println!("connected_clients={}", stats.connected_clients);
    Ok(())
}

/// Run the daemon: acquire the singleton lock, open the DB, bind the socket, and
/// serve until a shutdown signal or `StopDaemon` (§9).
pub fn run() -> anyhow::Result<()> {
    let socket_path = paths::socket_path()?;
    let lock_path = paths::lock_path()?;
    let cfg = config::Config::load(&paths::config_file()?)?;
    let version = env!("CARGO_PKG_VERSION").to_string();
    let instance_id = uuid::Uuid::now_v7().to_string();
    let started_at = domain::Timestamp::now();

    let _lock = match lockfile::acquire(&lock_path, &instance_id, &version, started_at) {
        Ok(lock) => lock,
        Err(lockfile::LockError::AlreadyHeld) => {
            eprintln!("forge-daemon: another instance already running; exiting.");
            return Ok(());
        }
        Err(e) => return Err(e.into()),
    };

    let _guard = logging::init(&paths::logs_dir()?, &cfg.daemon.log_level).ok();
    tracing::info!(%instance_id, version, "forge-daemon starting");

    paths::ensure_private_dir(&paths::data_dir()?)?;
    let db =
        persistence::Db::open(&paths::db_path()?).map_err(|e| anyhow::anyhow!("open db: {e}"))?;

    let worktrees_root = if cfg.worktrees.root.is_empty() {
        paths::worktrees_root()?
    } else {
        PathBuf::from(&cfg.worktrees.root)
    };

    let daemon = core::Daemon::start(db, cfg, worktrees_root, instance_id, version)
        .map_err(|e| anyhow::anyhow!("start daemon: {e}"))?;

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    // Bind inside the runtime context (tokio's UnixListener needs a reactor).
    let _enter = rt.enter();
    let listener = server::bind(&socket_path)?;
    tracing::info!(socket = %socket_path.display(), "listening");

    let exiting = daemon.clone();
    rt.block_on(async move {
        let mut server = tokio::spawn(server::serve(daemon.clone(), listener, socket_path.clone()));
        tokio::select! {
            _ = &mut server => {}
            _ = shutdown_signal() => {
                tracing::info!("received shutdown signal");
                daemon.registry().broadcast_domain(
                    protocol::DaemonEvent::DaemonShuttingDown { reason: "signal".into() },
                );
                // Ask the accept loop to stop and give it a moment to unlink the
                // socket. On the signal path the `select!` used to complete and
                // drop the `JoinHandle`, aborting `serve` before its cleanup ran
                // and leaving a stale socket file behind.
                daemon.request_shutdown();
                let _ = tokio::time::timeout(SHUTDOWN_GRACE, server).await;
                // Belt and braces: unlink it here too, in case the accept loop
                // was blocked elsewhere when the timeout expired.
                let _ = std::fs::remove_file(&socket_path);
            }
        }
    });

    // §9.1: whichever path got us here — `StopDaemon` or SIGTERM/SIGINT — every
    // live session is killed with the configured grace *before* the process
    // leaves. The per-session escalation of `KillSession` runs on a detached
    // thread and would die with us, leaving anything that ignored the first
    // signal orphaned with its PTY.
    exiting.shutdown_sessions();

    tracing::info!("forge-daemon stopped");
    Ok(())
}

/// How long the accept loop gets to wind down and unlink its socket after a
/// shutdown signal (§9.1).
const SHUTDOWN_GRACE: std::time::Duration = std::time::Duration::from_secs(2);

/// Resolve when the process receives SIGTERM or SIGINT (§9.1).
async fn shutdown_signal() {
    use tokio::signal::unix::{signal, SignalKind};
    let mut term = signal(SignalKind::terminate()).expect("install SIGTERM handler");
    let mut int = signal(SignalKind::interrupt()).expect("install SIGINT handler");
    tokio::select! {
        _ = term.recv() => {}
        _ = int.recv() => {}
    }
}
