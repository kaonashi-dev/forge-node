//! forge-daemon library surface (ADR-003).
//!
//! The runtime is exposed as a library so integration tests can start a daemon
//! on a temporary socket and drive it with the real `client`. The `forge-daemon`
//! binary (`main.rs`) is a thin CLI over [`run`].

pub mod actions;
pub mod config;
mod context_xfer;
pub mod core;
mod definitions;
mod diagnostics;
pub mod editor;
pub mod editor_wire;
pub mod environment;
mod file_index;
mod file_watch;
pub mod idle;
pub mod juva;
pub mod lockfile;
pub mod logging;
mod lsp;
pub mod migrate_home;
pub mod orchestration;
pub mod paths;
mod prompts;
pub mod pull_requests;
pub mod registry;
mod review;
pub mod server;
pub mod session_cli;
pub mod shares;
mod shutdown;
pub mod terminal;
pub mod terminfo;
pub mod usage_stats;
pub mod worktrees;

pub use agents::history as external_agents;
pub use core::Daemon;

use std::path::Path;

/// Resolve paths + config and print them — the `info` subcommand.
pub fn info() -> anyhow::Result<()> {
    let inputs = client::paths::PathInputs::from_env();
    let paths = client::paths::resolve(&inputs)?;
    let cfg = config::Config::load(&paths.config_file)?;
    let worktrees_root = effective_worktrees_root(&cfg, &inputs, &paths);
    let show = |path: &Path| shorten(path, inputs.home.as_deref());

    println!(
        "layout:        {}",
        match paths.layout {
            client::Layout::Legacy => "legacy (forge-daemon migrate-home)",
            _ => "home",
        }
    );
    println!("home:          {}", show(&paths.home));
    println!("config:        {}", show(&paths.config_file));
    println!("db:            {}", show(&paths.db));
    println!("worktrees:     {}", show(&worktrees_root));
    println!("runs:          {}", show(&paths.runs));
    println!("agent-plugins: {}", show(&paths.agent_plugins));
    println!("logs:          {}", show(&paths.logs));
    println!("socket:        {}", paths::socket_path()?.display());
    println!("lockfile:      {}", paths::lock_path()?.display());
    if let Some(legacy) = &paths.legacy {
        if paths.db.exists() {
            let old_root = legacy.join("worktrees");
            let count = persistence::managed_worktree_paths(&paths.db)?
                .into_iter()
                .filter(|path| path.starts_with(&old_root))
                .count();
            println!("legacy worktrees: {count}");
        }
    }
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

/// The worktrees root the daemon will actually use: `[worktrees] root` when
/// set, the resolved default otherwise.
fn effective_worktrees_root(
    cfg: &config::Config,
    inputs: &client::paths::PathInputs,
    paths: &client::ForgePaths,
) -> std::path::PathBuf {
    client::paths::worktrees_root(
        &cfg.worktrees.root,
        inputs.home.as_deref(),
        &paths.default_worktrees_root,
    )
    .unwrap_or_else(|_| paths.default_worktrees_root.clone())
}

/// Abbreviate a path under the user's home as `~`.
fn shorten(path: &Path, home: Option<&Path>) -> String {
    let Some(home) = home else {
        return path.display().to_string();
    };
    let Ok(rest) = path.strip_prefix(home) else {
        return path.display().to_string();
    };
    if rest.as_os_str().is_empty() {
        "~".to_string()
    } else {
        format!("~/{}", rest.display())
    }
}

/// Keep the state root private, following a symlinked root to its target.
fn ensure_private_home(home: &Path) -> Result<(), paths::PathError> {
    match std::fs::symlink_metadata(home) {
        Ok(meta) if meta.file_type().is_symlink() => {
            paths::ensure_private_dir(&home.canonicalize()?)
        }
        _ => paths::ensure_private_dir(home),
    }
}

/// Ask a running daemon for runtime statistics and print them.
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
/// serve until a shutdown signal or `StopDaemon`.
pub fn run() -> anyhow::Result<()> {
    let socket_path = paths::socket_path()?;
    let lock_path = paths::lock_path()?;
    let inputs = client::paths::PathInputs::from_env();
    let forge = client::paths::resolve(&inputs)?;
    let cfg = config::Config::load(&forge.config_file)?;
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

    ensure_private_home(&forge.home)?;
    let _guard = logging::init(&forge.logs, &cfg.daemon.log_level).ok();
    tracing::info!(%instance_id, version, "forge-daemon starting");

    // Socket isolation does not grant a second daemon ownership of the same metadata.
    let _data_lock = lockfile::acquire(
        &forge.home.join("app.db.lock"),
        &instance_id,
        &version,
        started_at,
    )?;
    let db = persistence::Db::open(&forge.db).map_err(|e| anyhow::anyhow!("open db: {e}"))?;

    let mut notices = Vec::new();
    let worktrees_root = match client::paths::worktrees_root(
        &cfg.worktrees.root,
        inputs.home.as_deref(),
        &forge.default_worktrees_root,
    ) {
        Ok(root) => root,
        Err(error) => {
            notices.push(format!(
                "[worktrees] root {:?} is not an absolute path; using {}",
                error.0,
                forge.default_worktrees_root.display()
            ));
            forge.default_worktrees_root.clone()
        }
    };
    if let Some(home) = inputs.home.as_deref() {
        for (what, path) in [("data", &forge.home), ("worktrees", &worktrees_root)] {
            if let Some(folder) = paths::risky_location(path, home) {
                notices.push(format!(
                    "the {what} directory {} is inside {folder}, which macOS may block or a \
                     file-sync service may corrupt; consider moving it",
                    path.display()
                ));
            }
        }
    }

    let daemon = core::Daemon::start(
        db,
        cfg,
        core::DaemonPaths {
            forge,
            worktrees_root,
        },
        instance_id,
        version,
    )
    .map_err(|e| anyhow::anyhow!("start daemon: {e}"))?;
    for message in notices {
        daemon.notice(protocol::NoticeLevel::Warning, message);
    }

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

    // Whether triggered by `StopDaemon` or SIGTERM/SIGINT, every
    // live session is killed with the configured grace *before* the process
    // leaves. The per-session escalation of `KillSession` runs on a detached
    // thread and would die with us, leaving anything that ignored the first
    // signal orphaned with its PTY.
    exiting.shutdown_sessions();

    tracing::info!("forge-daemon stopped");
    Ok(())
}

/// How long the accept loop gets to wind down and unlink its socket after a
/// shutdown signal.
const SHUTDOWN_GRACE: std::time::Duration = std::time::Duration::from_secs(2);

/// Resolve when the process receives SIGTERM or SIGINT.
async fn shutdown_signal() {
    use tokio::signal::unix::{signal, SignalKind};
    let mut term = signal(SignalKind::terminate()).expect("install SIGTERM handler");
    let mut int = signal(SignalKind::interrupt()).expect("install SIGINT handler");
    tokio::select! {
        _ = term.recv() => {}
        _ = int.recv() => {}
    }
}
