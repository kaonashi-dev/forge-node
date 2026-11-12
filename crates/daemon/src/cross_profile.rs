//! `[orchestration] cross_profile`: what a session may read and steer in another
//! agent account. Policy is the pure [`allows`], like `idle.rs`; the rest
//! supplies it accounts and applies the answer.

use std::sync::Arc;

use domain::{AgentProfileId, ExternalSessionEntry, ProjectId, SessionId, SessionKind};
use protocol::{ErrorCode, ProtocolError, Request, Response};

use crate::Daemon;

/// An agent login: a profile, or `None` for a provider's default one.
pub type Account = Option<AgentProfileId>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Off,
    Read,
    Full,
}

impl Mode {
    /// An unknown value fails closed.
    #[must_use]
    pub fn parse(raw: &str) -> Self {
        match raw.trim() {
            "read" => Self::Read,
            "full" => Self::Full,
            _ => Self::Off,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Read => "read",
            Self::Full => "full",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    Read,
    Control,
}

/// `caller` is `None` for a client acting as no session, and `target` is `None`
/// for a session bound to no account (a shell, an editor); neither is restricted.
#[must_use]
pub fn allows(
    mode: Mode,
    caller: Option<Account>,
    target: Option<Account>,
    access: Access,
) -> bool {
    let (Some(caller), Some(target)) = (caller, target) else {
        return true;
    };
    caller == target
        || match mode {
            Mode::Full => true,
            Mode::Read => access == Access::Read,
            Mode::Off => false,
        }
}

fn refusal(mode: Mode, access: Access) -> ProtocolError {
    let verb = match access {
        Access::Read => "read",
        Access::Control => "control",
    };
    ProtocolError::with_details(
        ErrorCode::PreconditionFailed,
        format!(
            "[orchestration] cross_profile = \"{}\" does not let this session {verb} another account's sessions",
            mode.name()
        ),
        serde_json::json!({ "reason": "policy:cross_profile", "mode": mode.name(), "access": verb })
            .to_string(),
    )
}

/// The caller a request names itself wins over the connection's.
fn explicit_caller(request: &Request) -> Option<SessionId> {
    match request {
        Request::SendSessionPrompt {
            caller_session_id, ..
        }
        | Request::SendShellCommand {
            caller_session_id, ..
        } => *caller_session_id,
        _ => None,
    }
}

/// An unknown or non-agent caller counts as the default login.
fn caller_account(daemon: &Daemon, caller: SessionId) -> Account {
    daemon
        .lock()
        .sessions
        .get(&caller)
        .filter(|session| session.kind == SessionKind::Agent)
        .and_then(|session| session.agent_profile_id)
}

/// `None` for a session bound to no account, and for one that does not exist,
/// which the handler then reports as not found.
fn session_account(daemon: &Daemon, session_id: SessionId) -> Option<Account> {
    daemon
        .lock()
        .sessions
        .get(&session_id)
        .filter(|session| session.kind == SessionKind::Agent)
        .map(|session| session.agent_profile_id)
}

/// Every account a request reads or steers. Editor sessions belong to no
/// account, so the editor requests are not listed.
fn targets(daemon: &Daemon, request: &Request) -> Vec<(Option<Account>, Access)> {
    let session = |id: &SessionId, access| (session_account(daemon, *id), access);
    match request {
        Request::SendSessionPrompt { session_id, .. }
        | Request::SendShellCommand { session_id, .. }
        | Request::KillSession { session_id }
        | Request::RestartSession { session_id }
        | Request::CloseSession { session_id }
        | Request::SendSignal { session_id, .. }
        | Request::RenameSession { session_id, .. }
        | Request::SetSessionRole { session_id, .. }
        | Request::CreateChildSession {
            parent_session_id: session_id,
            ..
        } => vec![session(session_id, Access::Control)],
        Request::GetSessionTranscript { session_id, .. }
        | Request::GetSessionChanges { session_id } => vec![session(session_id, Access::Read)],
        Request::SendContext {
            source_session_id,
            target_session_id,
            spawn,
            include_transcript,
            ..
        } => {
            let mut checks = Vec::new();
            if *include_transcript {
                checks.push(session(source_session_id, Access::Read));
            }
            if spawn.is_some() {
                checks.push(session(source_session_id, Access::Control));
            }
            if let Some(target) = target_session_id {
                checks.push(session(target, Access::Control));
            }
            checks
        }
        Request::GetExternalTranscript { profile_id, .. } => {
            vec![(Some(*profile_id), Access::Read)]
        }
        Request::AdoptExternalSession { profile_id, .. }
        | Request::DeleteExternalSession { profile_id, .. } => {
            vec![(Some(*profile_id), Access::Control)]
        }
        _ => Vec::new(),
    }
}

/// [`Daemon::handle_request_from`] for a connection that declared the session it acts for.
pub(crate) fn handle(
    daemon: &Arc<Daemon>,
    client_kind: protocol::ClientKind,
    connection_caller: Option<SessionId>,
    request: Request,
) -> Result<Response, ProtocolError> {
    let Some(caller) = explicit_caller(&request).or(connection_caller) else {
        return daemon.handle_request_from(client_kind, request);
    };
    let targets = targets(daemon, &request);
    let filtered = matches!(
        request,
        Request::GetSnapshot | Request::ListExternalSessions { .. }
    );
    if targets.is_empty() && !filtered {
        return daemon.handle_request_from(client_kind, request);
    }
    let mode = Mode::parse(&daemon.orchestration_config().cross_profile);
    let account = caller_account(daemon, caller);
    for (target, access) in targets {
        if !allows(mode, Some(account), target, access) {
            return Err(refusal(mode, access));
        }
    }
    match request {
        Request::ListExternalSessions { project_id } => {
            list(daemon, project_id, Some((mode, account)))
        }
        Request::GetSnapshot => {
            let mut response = daemon.handle_request_from(client_kind, Request::GetSnapshot)?;
            if let Response::Snapshot {
                sessions,
                external_agents,
                ..
            } = &mut response
            {
                sessions.retain(|session| {
                    let target =
                        (session.kind == SessionKind::Agent).then_some(session.agent_profile_id);
                    allows(mode, Some(account), target, Access::Read)
                });
                external_agents
                    .retain(|run| allows(mode, Some(account), Some(run.profile_id), Access::Read));
            }
            Ok(response)
        }
        request => daemon.handle_request_from(client_kind, request),
    }
}

/// Discovery and the liveness probe are filesystem IO, taken off the core lock.
pub(crate) fn list(
    daemon: &Daemon,
    project_id: Option<ProjectId>,
    policy: Option<(Mode, Account)>,
) -> Result<Response, ProtocolError> {
    let (runs, profiles) = daemon.external_sessions();
    let liveness = agents::history::Liveness::scan(&profiles);
    let entries = runs
        .into_iter()
        .filter(|run| project_id.is_none_or(|id| run.project_id == id))
        .filter(|run| {
            policy.is_none_or(|(mode, account)| {
                allows(mode, Some(account), Some(run.profile_id), Access::Read)
            })
        })
        .map(|run| ExternalSessionEntry {
            live: liveness.of(&run),
            session: run,
        })
        .collect();
    Ok(Response::ExternalSessions(entries))
}

/// Resume a discovered run as an ordinary agent session, in the checkout and
/// account it was recorded in.
pub(crate) fn adopt(
    daemon: &Arc<Daemon>,
    session_id: &str,
    provider: &str,
    profile_id: Option<AgentProfileId>,
    title: Option<String>,
    force: bool,
) -> Result<Response, ProtocolError> {
    let run = daemon.find_external(session_id, provider, profile_id)?;
    let provider_id = domain::AgentProviderId::new(provider);
    let (supports_resume, workspace_id) = {
        let inner = daemon.lock();
        let supports_resume = inner
            .agents
            .descriptor(&provider_id)
            .map(|descriptor| descriptor.capabilities.supports_resume);
        let workspace_id = run
            .workspace_id
            .filter(|id| inner.workspaces.contains_key(id))
            .or_else(|| {
                inner
                    .workspaces
                    .values()
                    .find(|w| {
                        w.project_id == run.project_id && w.kind == domain::WorkspaceKind::Main
                    })
                    .map(|w| w.id)
            });
        (supports_resume, workspace_id)
    };
    match supports_resume {
        None => return Err(ProtocolError::invalid_request("unknown provider")),
        Some(false) => {
            return Err(ProtocolError::invalid_request(
                "provider cannot resume a conversation",
            ))
        }
        Some(true) => {}
    }
    let workspace_id = workspace_id
        .ok_or_else(|| ProtocolError::not_found("no checkout for this run's project"))?;
    let profiles = daemon.agent_profiles();
    if !force && agents::history::Liveness::scan(&profiles).of(&run) == Some(true) {
        return Err(ProtocolError::with_details(
            ErrorCode::PreconditionFailed,
            "a process still owns this conversation; end it first or pass force",
            serde_json::json!({ "reason": "live", "session_id": run.session_id,
                "provider": run.provider, "profile_id": run.profile_id,
                "transcript": run.transcript_path })
            .to_string(),
        ));
    }
    let title = title.or_else(|| Some(run.title.clone()));
    let response = daemon.create_session_with_options(
        workspace_id,
        SessionKind::Agent,
        Some(provider_id),
        profile_id,
        None,
        domain::SessionRole::Generic,
        Some(run.session_id),
        None,
        false,
        domain::AgentLaunchOptions::default(),
        title,
        None,
        false,
    )?;
    daemon.invalidate_external();
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_mode_bounds_reads_and_control_of_another_account() {
        let work = Some(AgentProfileId::new());
        let personal: Account = None;
        let cases = [
            (Mode::Off, Access::Read, false),
            (Mode::Off, Access::Control, false),
            (Mode::Read, Access::Read, true),
            (Mode::Read, Access::Control, false),
            (Mode::Full, Access::Read, true),
            (Mode::Full, Access::Control, true),
        ];
        for (mode, access, across) in cases {
            assert_eq!(
                allows(mode, Some(work), Some(personal), access),
                across,
                "{mode:?} {access:?}"
            );
            assert!(
                allows(mode, Some(work), Some(work), access),
                "own account under {mode:?}"
            );
            assert!(
                allows(mode, None, Some(work), access),
                "a human under {mode:?}"
            );
            assert!(
                allows(mode, Some(work), None, access),
                "an accountless session under {mode:?}"
            );
        }
    }

    #[test]
    fn an_unknown_mode_fails_closed() {
        assert_eq!(Mode::parse(" read "), Mode::Read);
        assert_eq!(Mode::parse("full"), Mode::Full);
        assert_eq!(Mode::parse("off"), Mode::Off);
        assert_eq!(Mode::parse("everything"), Mode::Off);
    }
}
