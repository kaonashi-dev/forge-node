use std::collections::HashMap;

use client::{DaemonInfo, ProviderInfo, Store};
use domain::{
    AgentProfile, ExternalAgentSession, Project, ProjectGroup, ProviderUsage, PullRequestState,
    Session, SessionId, TerminalId, Workspace,
};
use serde::Serialize;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LaunchKind {
    Shell,
    Agent,
}

/// Launchables for the tab-strip + menu.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Launchable {
    pub kind: LaunchKind,
    pub label: String,
    pub detail: Option<String>,
    pub provider: Option<String>,
    pub profile: Option<String>,
    pub enabled: bool,
    pub key: String,
    /// Whether this row can be launched with a prompt already in hand.
    ///
    /// The prompt is one trailing argv entry, so a provider that declares no
    /// prompt style refuses the launch outright (`AgentError::PromptUnsupported`).
    /// The handoff dialog needs to say that up front rather than fail on
    /// confirm. Always `true` for a shell, which never sees this field.
    pub supports_initial_prompt: bool,
}

fn launchables(store: &Store) -> Vec<Launchable> {
    let mut out = vec![Launchable {
        kind: LaunchKind::Shell,
        label: "New Terminal".to_string(),
        detail: None,
        provider: None,
        profile: None,
        enabled: true,
        key: "shell".to_string(),
        supports_initial_prompt: false,
    }];
    for provider in &store.providers {
        let id = &provider.descriptor.id;
        let installed = provider.detection.status.is_installed();
        out.push(Launchable {
            kind: LaunchKind::Agent,
            label: provider.descriptor.display_name.clone(),
            detail: if installed {
                None
            } else {
                Some("not installed".to_string())
            },
            provider: Some(id.to_string()),
            profile: None,
            enabled: installed,
            key: id.to_string(),
            supports_initial_prompt: provider.descriptor.capabilities.supports_initial_prompt,
        });
        for profile in store
            .agent_profiles
            .iter()
            .filter(|profile| profile.provider_id == *id)
        {
            out.push(Launchable {
                kind: LaunchKind::Agent,
                label: profile.name.clone(),
                detail: None,
                provider: Some(id.to_string()),
                profile: Some(profile.id.to_string()),
                enabled: installed || profile.executable.is_some(),
                key: format!("profile:{}", profile.id),
                supports_initial_prompt: provider.descriptor.capabilities.supports_initial_prompt,
            });
        }
    }
    out
}

#[derive(Clone, Debug, Serialize)]
pub struct DaemonInfoDto {
    pub protocol_version: u32,
    pub daemon_version: String,
    pub instance_id: String,
    pub started_at: domain::Timestamp,
}

impl From<&DaemonInfo> for DaemonInfoDto {
    fn from(info: &DaemonInfo) -> Self {
        Self {
            protocol_version: info.protocol_version,
            daemon_version: info.daemon_version.clone(),
            instance_id: info.instance_id.clone(),
            started_at: info.started_at,
        }
    }
}

/// Per-session attention flags for tab washes (§16.3). Mirrors
/// `apps/tauri on the runtime thread.
#[derive(Clone, Debug, Serialize)]
pub struct SessionAttentionFlags {
    pub wants_you: bool,
    pub unread: bool,
}

fn session_wants_you(store: &Store, session: &Session) -> bool {
    session.agent_provider_id.is_some()
        && session.state.is_active()
        && session
            .terminal_id
            .is_some_and(|terminal_id| store.wants_attention(&terminal_id))
}

fn session_unread(store: &Store, session: &Session) -> bool {
    session
        .terminal_id
        .is_some_and(|terminal_id| store.has_unread(&terminal_id))
}

/// Domain lists without the attached [`client::CellGrid`]. The grid stays on
/// the runtime thread; cloning it onto the IPC path is the defect
/// `docs/performance.md` forbids. Terminal output reaches the canvas as the
/// merged runs of `runtime::cells`, on its own event.
#[derive(Clone, Debug, Serialize)]
pub struct ShellSnapshot {
    pub project_groups: Vec<ProjectGroup>,
    pub projects: Vec<Project>,
    pub workspaces: Vec<Workspace>,
    pub sessions: Vec<Session>,
    pub providers: Vec<ProviderInfo>,
    /// Subscription meters read from provider endpoints (§16.2).
    pub usage: Vec<ProviderUsage>,
    /// Agent runs discovered on disk that the daemon did not launch (§13.5).
    /// Read-only: the History panel resumes them by asking the provider's own
    /// CLI to re-enter its session, never by replaying the transcript.
    pub external_agents: Vec<ExternalAgentSession>,
    /// The daemon's cached pull-request read. Never fetched by `GetSnapshot`:
    /// only `RefreshPullRequests` may touch the network.
    pub pull_requests: PullRequestState,
    pub launchables: Vec<Launchable>,
    /// Launch profiles (§13.4), for the settings section that edits them.
    ///
    /// `launchables` already flattens these into rows the `+` menu can start,
    /// but a profile is also a thing with an executable, arguments and an
    /// environment, and none of that survives the flattening.
    pub agent_profiles: Vec<AgentProfile>,
    /// Every project's file-sharing rules (§14.2), in application order, for
    /// the settings section that edits them.
    pub worktree_shares: Vec<domain::ShareRule>,
    pub session_attention: HashMap<SessionId, SessionAttentionFlags>,
    /// Persisted GUI preferences (§15.2). The daemon stays authoritative:
    /// writes go through `SetAppState`, and this is the last read of them.
    pub app_state: HashMap<String, String>,
    pub live_sessions: usize,
    pub installed_agents: usize,
    pub provider_count: usize,
}

impl ShellSnapshot {
    pub fn from_store(store: &Store) -> Self {
        let live_sessions = store
            .sessions
            .iter()
            .filter(|session| session.state.is_active())
            .count();
        let installed_agents = store
            .providers
            .iter()
            .filter(|provider| provider.detection.status.is_installed())
            .count();
        let session_attention = store
            .sessions
            .iter()
            .map(|session| {
                (
                    session.id,
                    SessionAttentionFlags {
                        wants_you: session_wants_you(store, session),
                        unread: session_unread(store, session),
                    },
                )
            })
            .collect();
        Self {
            project_groups: store.project_groups.clone(),
            projects: store.projects.clone(),
            workspaces: store.workspaces.clone(),
            sessions: store.sessions.clone(),
            providers: store.providers.clone(),
            usage: store.usage.clone(),
            external_agents: store.external_agents.clone(),
            pull_requests: store.pull_requests.clone(),
            launchables: launchables(store),
            agent_profiles: store.agent_profiles.clone(),
            worktree_shares: store.worktree_shares.clone(),
            session_attention,
            app_state: store.app_state.iter().cloned().collect(),
            live_sessions,
            installed_agents,
            provider_count: store.providers.len(),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct ConnectedPayload {
    pub daemon: DaemonInfoDto,
    pub session_count: usize,
    pub store: ShellSnapshot,
    pub active_session: Option<SessionId>,
    pub active_terminal: Option<TerminalId>,
}

/// The shell lists, published whenever one of them changes.
///
/// Terminal output does *not* travel on this payload: it has its own
/// `runtime:cells` event, so a frame of PTY output never drags the session
/// tree through `serde_json` with it.
///
/// Borrowed rather than owned so publishing costs one [`ShellSnapshot`] and no
/// copies of it. Owning it meant building the snapshot, cloning it into the
/// [`ConnectedPayload`] the `connect` command answers from, and cloning *that*
/// again inside `remember_connected` — three of the largest allocation in the
/// bridge for one publish.
#[derive(Clone, Debug, Serialize)]
pub struct StatePayload<'a> {
    pub store: &'a ShellSnapshot,
    pub active_session: SessionId,
    pub active_terminal: TerminalId,
}

#[derive(Clone, Debug, Serialize)]
pub struct DisconnectedPayload {
    pub reason: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct HostStatus {
    pub client_ready: bool,
    pub connected: bool,
    pub session_count: usize,
    pub daemon_version: Option<String>,
    pub instance_id: Option<String>,
    pub reason: Option<String>,
}

impl Default for HostStatus {
    fn default() -> Self {
        Self {
            client_ready: true,
            connected: false,
            session_count: 0,
            daemon_version: None,
            instance_id: None,
            reason: None,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Latest {
    pub status: HostStatus,
    pub payload: Option<ConnectedPayload>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use client::Store;

    #[test]
    fn empty_store_still_offers_a_terminal() {
        let snapshot = ShellSnapshot::from_store(&Store::new());
        assert_eq!(snapshot.launchables.len(), 1);
        assert_eq!(snapshot.launchables[0].kind, LaunchKind::Shell);
        assert!(snapshot.launchables[0].enabled);
    }
}
