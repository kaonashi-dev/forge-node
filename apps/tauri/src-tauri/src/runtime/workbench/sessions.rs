use client::Client;
use domain::{AgentProfileId, SessionId};
use serde::Serialize;
use tauri::{AppHandle, Emitter as _};

use super::commands::AgentLaunchRequest;
use super::{emit, fail_session};

#[derive(Debug, Serialize)]
struct AgentLaunchResult {
    request_id: String,
    session: Option<SessionId>,
    error: Option<String>,
    uncertain: bool,
}

impl AgentLaunchResult {
    fn new(
        request_id: String,
        result: Result<(SessionId, domain::TerminalId), client::ClientError>,
    ) -> Self {
        match result {
            Ok((session, _)) => Self {
                request_id,
                session: Some(session),
                error: None,
                uncertain: false,
            },
            Err(error) => Self {
                request_id,
                session: None,
                uncertain: !matches!(error, client::ClientError::Protocol(_)),
                error: Some(error.to_string()),
            },
        }
    }
}

pub(super) fn launch_agent(app: &AppHandle, client: &Client, request: AgentLaunchRequest) {
    let result = client.create_agent_session_with_role(
        request.workspace,
        request.provider,
        request.profile,
        request.parent,
        if request.read_only {
            domain::SessionRole::Reviewer
        } else {
            domain::SessionRole::Generic
        },
        None,
        Some(request.prompt),
        request.read_only,
    );
    emit(
        app,
        "workbench:agent_launched",
        &AgentLaunchResult::new(request.request_id, result),
    );
}

#[derive(Serialize)]
struct HandoffProgress {
    request_id: String,
    transcript: Option<domain::SessionTranscript>,
    error: Option<String>,
}

pub(super) fn load_handoff_progress(
    app: &AppHandle,
    client: &Client,
    request_id: String,
    session: SessionId,
) {
    let result = client.session_transcript(session, None, None);
    let (transcript, error) = match result {
        Ok(transcript) => (Some(transcript), None),
        Err(error) => (None, Some(error.to_string())),
    };
    emit(
        app,
        "workbench:handoff_progress",
        &HandoffProgress {
            request_id,
            transcript,
            error,
        },
    );
}

pub(super) fn load_session_transcript(
    app: &AppHandle,
    client: &Client,
    session: SessionId,
    max_lines: Option<u32>,
    max_bytes: Option<u32>,
) {
    match client.session_transcript(session, max_lines, max_bytes) {
        Ok(transcript) => emit(app, "workbench:session_transcript", &(session, transcript)),
        Err(error) => fail_session(app, "workbench:session_transcript_failed", session, &error),
    }
}

pub(super) fn load_external_transcript(
    app: &AppHandle,
    client: &Client,
    session: String,
    provider: String,
    profile: Option<AgentProfileId>,
    max_turns: Option<u32>,
    max_bytes: Option<u32>,
) {
    match client.external_transcript(session.clone(), provider, profile, max_turns, max_bytes) {
        Ok(transcript) => emit(app, "workbench:external_transcript", &(session, transcript)),
        Err(error) => fail_external(app, "workbench:external_transcript_failed", session, &error),
    }
}

pub(super) fn delete_external_session(
    app: &AppHandle,
    client: &Client,
    session: String,
    provider: String,
    profile: Option<AgentProfileId>,
) {
    match client.delete_external_session(session.clone(), provider, profile) {
        Ok(()) => emit(app, "workbench:external_session_deleted", &session),
        Err(error) => fail_external(
            app,
            "workbench:external_session_delete_failed",
            session,
            &error,
        ),
    }
}

/// A discovered run's id is the provider's own string, not a [`SessionId`], so
/// `SessionFailure` cannot carry it — and the History panel needs the failure
/// on the right card.
#[derive(Clone, Debug, Serialize)]
struct ExternalFailure {
    session: String,
    error: String,
}

/// The same, keyed by a discovered run's provider-side id.
fn fail_external(app: &AppHandle, event: &str, session: String, error: &client::ClientError) {
    tracing::warn!(%error, event, "external transcript read failed");
    let _ = app.emit(
        event,
        ExternalFailure {
            session,
            error: error.to_string(),
        },
    );
}

#[cfg(test)]
mod tests {
    use super::AgentLaunchResult;
    use client::{ClientError, ProtocolError};
    use domain::{SessionId, TerminalId};

    #[test]
    fn launch_result_keeps_the_created_id_and_request_identity() {
        let session = SessionId::new();
        let result = AgentLaunchResult::new("launch-42".into(), Ok((session, TerminalId::new())));
        assert_eq!(result.request_id, "launch-42");
        assert_eq!(result.session, Some(session));
        assert!(result.error.is_none());
        assert!(!result.uncertain);
    }

    #[test]
    fn only_a_structured_refusal_allows_retrying_a_failed_launch() {
        let refused = AgentLaunchResult::new(
            "refused".into(),
            Err(ClientError::Protocol(ProtocolError::not_found("profile"))),
        );
        assert!(refused.session.is_none());
        assert!(refused.error.is_some());
        assert!(!refused.uncertain);
        for error in [ClientError::Disconnected, ClientError::Timeout] {
            let lost = AgentLaunchResult::new("unknown".into(), Err(error));
            assert!(lost.session.is_none());
            assert!(lost.error.is_some());
            assert!(lost.uncertain);
        }
    }
}
