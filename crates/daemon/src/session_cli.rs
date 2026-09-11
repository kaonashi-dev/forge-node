//! Agent-facing CLI over a running daemon: session list/read/spawn-child and
//! context send/list. Uses `FORGE_SESSION_ID` when `--from` is omitted so a
//! Forge-launched agent can mediate without peer-to-peer provider APIs.

use std::str::FromStr;

use clap::{Parser, Subcommand, ValueEnum};
use domain::{
    AgentProfileId, AgentProviderId, ChildWorkspacePolicy, SessionId, SessionKind, SessionRole,
};
use protocol::SendContextSpawn;

/// Connect to the resolved socket (honoring `FORGE_SOCKET`).
fn connect() -> anyhow::Result<client::Client> {
    let socket = crate::paths::socket_path()?;
    client::Client::connect(&socket, env!("CARGO_PKG_VERSION"))
        .map_err(|error| anyhow::anyhow!("daemon not running ({error})"))
}

fn parse_session_id(raw: &str) -> anyhow::Result<SessionId> {
    SessionId::from_str(raw).map_err(|e| anyhow::anyhow!("bad session id: {e}"))
}

fn session_from_env_or(flag: Option<String>) -> anyhow::Result<SessionId> {
    if let Some(raw) = flag {
        return parse_session_id(&raw);
    }
    let raw = std::env::var("FORGE_SESSION_ID").map_err(|_| {
        anyhow::anyhow!("pass --from <session-id> or set FORGE_SESSION_ID (Forge injects it)")
    })?;
    parse_session_id(&raw)
}

#[derive(Parser, Debug)]
pub struct SessionCli {
    #[command(subcommand)]
    pub command: SessionCommand,
}

#[derive(Subcommand, Debug)]
pub enum SessionCommand {
    /// List Forge sessions (id, state, kind, provider, title).
    List,
    /// Print a session's terminal transcript (plain text).
    Read {
        session_id: String,
        #[arg(long)]
        max_bytes: Option<u32>,
    },
    /// Spawn a child under the source session with any installed provider.
    SpawnChild {
        #[arg(long)]
        from: Option<String>,
        #[arg(long)]
        provider: String,
        #[arg(long)]
        profile: Option<String>,
        #[arg(long)]
        prompt: Option<String>,
        #[arg(long, default_value = "generic")]
        role: RoleArg,
        #[arg(long, default_value = "same")]
        workspace: WorkspaceArg,
        #[arg(long)]
        branch_hint: Option<String>,
    },
}

#[derive(Parser, Debug)]
pub struct ContextCli {
    #[command(subcommand)]
    pub command: ContextCommand,
}

#[derive(Subcommand, Debug)]
pub enum ContextCommand {
    /// List envelopes for a session (inbox and outbox).
    List {
        #[arg(long)]
        session: Option<String>,
    },
    /// Persist an envelope and deliver it (PTY paste) or spawn a child with it.
    Send {
        #[arg(long)]
        from: Option<String>,
        /// Existing session to receive the context.
        #[arg(long, conflicts_with = "spawn_provider")]
        to: Option<String>,
        /// Spawn a child with this provider instead of targeting `--to`.
        #[arg(long, conflicts_with = "to")]
        spawn_provider: Option<String>,
        #[arg(long)]
        profile: Option<String>,
        #[arg(long)]
        summary: Option<String>,
        #[arg(long)]
        instructions: Option<String>,
        #[arg(long, default_value_t = false)]
        include_transcript: bool,
        #[arg(long, default_value = "generic")]
        role: RoleArg,
        #[arg(long, default_value = "same")]
        workspace: WorkspaceArg,
        #[arg(long)]
        branch_hint: Option<String>,
    },
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum RoleArg {
    Generic,
    Orchestrator,
    Planner,
    Researcher,
    Executor,
    Reviewer,
    Tester,
}

impl From<RoleArg> for SessionRole {
    fn from(value: RoleArg) -> Self {
        match value {
            RoleArg::Generic => SessionRole::Generic,
            RoleArg::Orchestrator => SessionRole::Orchestrator,
            RoleArg::Planner => SessionRole::Planner,
            RoleArg::Researcher => SessionRole::Researcher,
            RoleArg::Executor => SessionRole::Executor,
            RoleArg::Reviewer => SessionRole::Reviewer,
            RoleArg::Tester => SessionRole::Tester,
        }
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum WorkspaceArg {
    Same,
    Worktree,
}

impl WorkspaceArg {
    fn policy(self, branch_hint: Option<String>) -> ChildWorkspacePolicy {
        match self {
            WorkspaceArg::Same => ChildWorkspacePolicy::SameWorkspace,
            WorkspaceArg::Worktree => ChildWorkspacePolicy::NewManagedWorktree {
                branch_hint,
                base: None,
            },
        }
    }
}

/// Run `forge-daemon session …`.
pub fn run_session(cli: SessionCli) -> anyhow::Result<()> {
    let client = connect()?;
    match cli.command {
        SessionCommand::List => {
            let snap = client.load_store()?;
            for session in snap.sessions {
                let provider = session
                    .agent_provider_id
                    .as_ref()
                    .map(|p| p.as_str())
                    .unwrap_or("-");
                let title = session.title.resolve("session");
                println!(
                    "{}\t{:?}\t{:?}\t{}\t{}",
                    session.id, session.state, session.kind, provider, title
                );
            }
            Ok(())
        }
        SessionCommand::Read {
            session_id,
            max_bytes,
        } => {
            let id = parse_session_id(&session_id)?;
            let transcript = client.session_transcript(id, None, max_bytes)?;
            print!("{}", transcript.text);
            if !transcript.text.ends_with('\n') {
                println!();
            }
            if transcript.truncated {
                eprintln!("# truncated: {} lines kept within budget", transcript.lines);
            }
            Ok(())
        }
        SessionCommand::SpawnChild {
            from,
            provider,
            profile,
            prompt,
            role,
            workspace,
            branch_hint,
        } => {
            let parent = session_from_env_or(from)?;
            let profile_id = profile
                .as_deref()
                .map(AgentProfileId::from_str)
                .transpose()
                .map_err(|e| anyhow::anyhow!("bad profile id: {e}"))?;
            let (session_id, terminal_id) = client.create_child_session(
                parent,
                SessionKind::Agent,
                Some(AgentProviderId::new(provider)),
                profile_id,
                role.into(),
                workspace.policy(branch_hint),
                prompt,
            )?;
            println!("session_id={session_id}");
            println!("terminal_id={terminal_id}");
            Ok(())
        }
    }
}

/// Run `forge-daemon context …`.
pub fn run_context(cli: ContextCli) -> anyhow::Result<()> {
    let client = connect()?;
    match cli.command {
        ContextCommand::List { session } => {
            let id = session_from_env_or(session)?;
            let envelopes = client.list_context_envelopes(id)?;
            for envelope in envelopes {
                let target = envelope
                    .target_session_id
                    .map(|t| t.to_string())
                    .unwrap_or_else(|| "-".into());
                let summary = envelope.summary.as_deref().unwrap_or("");
                println!(
                    "{}\t{}\t{}\t{}",
                    envelope.id, envelope.source_session_id, target, summary
                );
            }
            Ok(())
        }
        ContextCommand::Send {
            from,
            to,
            spawn_provider,
            profile,
            summary,
            instructions,
            include_transcript,
            role,
            workspace,
            branch_hint,
        } => {
            let source = session_from_env_or(from)?;
            let target = to.as_deref().map(parse_session_id).transpose()?;
            let spawn = match spawn_provider {
                Some(provider) => {
                    let profile_id = profile
                        .as_deref()
                        .map(AgentProfileId::from_str)
                        .transpose()
                        .map_err(|e| anyhow::anyhow!("bad profile id: {e}"))?;
                    Some(SendContextSpawn {
                        kind: SessionKind::Agent,
                        provider_id: Some(AgentProviderId::new(provider)),
                        profile_id,
                        role: role.into(),
                        workspace_policy: workspace.policy(branch_hint),
                    })
                }
                None => None,
            };
            if target.is_none() && spawn.is_none() {
                anyhow::bail!("pass --to <session-id> or --spawn-provider <id>");
            }
            match client.send_context(
                source,
                target,
                spawn,
                summary,
                instructions,
                include_transcript,
                None,
            )? {
                client::SendContextResult::Delivered => {
                    println!("ok=delivered");
                }
                client::SendContextResult::Spawned {
                    session_id,
                    terminal_id,
                } => {
                    println!("session_id={session_id}");
                    println!("terminal_id={terminal_id}");
                }
            }
            Ok(())
        }
    }
}
