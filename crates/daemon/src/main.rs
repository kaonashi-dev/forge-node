//! forge-daemon — the local runtime binary (ADR-003). A thin CLI over the
//! `daemon` library crate; see [`daemon`] for the runtime itself.

use clap::{Parser, Subcommand};
use daemon::session_cli::{self, ContextCli, SessionCli};

#[derive(Parser)]
#[command(name = "forge-daemon", version, about = "Forge runtime daemon")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Run the daemon (default when no subcommand is given).
    Run,
    /// Print the resolved paths and effective config, then exit.
    Info,
    /// Stream protocol messages as JSON for debugging. Not yet wired.
    Dump {
        #[arg(long)]
        json: bool,
    },
    /// Print runtime statistics from a running daemon.
    Stats,
    /// List, read, or spawn child sessions against a running daemon.
    Session(SessionCli),
    /// Send or list context envelopes against a running daemon.
    Context(ContextCli),
    /// Copy a legacy install into the Forge home.
    MigrateHome {
        /// Report what would happen without writing anything.
        #[arg(long)]
        dry_run: bool,
        /// Also move managed worktrees with `git worktree move`.
        #[arg(long)]
        move_worktrees: bool,
        /// Machine-readable report.
        #[arg(long)]
        json: bool,
        /// Seconds to wait for a running daemon to release its locks.
        #[arg(long, default_value_t = 0)]
        wait: u64,
        /// Delete the old database and agent plugins after a migration.
        #[arg(long)]
        remove_legacy: bool,
    },
}

fn main() -> anyhow::Result<()> {
    match Cli::parse().command.unwrap_or(Command::Run) {
        Command::Run => daemon::run(),
        Command::Info => daemon::info(),
        Command::Dump { .. } => {
            eprintln!("forge-daemon dump: not wired yet.");
            Ok(())
        }
        Command::Stats => daemon::stats(),
        Command::Session(cli) => session_cli::run_session(cli),
        Command::Context(cli) => session_cli::run_context(cli),
        Command::MigrateHome {
            dry_run,
            move_worktrees,
            json,
            wait,
            remove_legacy,
        } => daemon::migrate_home::run(&daemon::migrate_home::Options {
            dry_run,
            move_worktrees,
            json,
            wait_secs: wait,
            remove_legacy,
        }),
    }
}
