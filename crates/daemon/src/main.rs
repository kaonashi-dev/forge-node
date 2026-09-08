//! forge-daemon — the local runtime binary (ADR-003). A thin CLI over the
//! `daemon` library crate; see [`daemon`] for the runtime itself.

use clap::{Parser, Subcommand};

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
    /// Stream protocol messages as JSON for debugging (§10.4). Not yet wired.
    Dump {
        #[arg(long)]
        json: bool,
    },
    /// Print runtime statistics from a running daemon (§22).
    Stats,
}

fn main() -> anyhow::Result<()> {
    match Cli::parse().command.unwrap_or(Command::Run) {
        Command::Run => daemon::run(),
        Command::Info => daemon::info(),
        Command::Dump { .. } => {
            eprintln!("forge-daemon dump: not wired yet. See execution.md.");
            Ok(())
        }
        Command::Stats => daemon::stats(),
    }
}
