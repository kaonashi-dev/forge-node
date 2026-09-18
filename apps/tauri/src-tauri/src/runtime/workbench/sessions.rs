use client::Client;
use domain::{AgentProfileId, SessionId};
use serde::Serialize;
use tauri::{AppHandle, Emitter as _};

use super::{emit, fail_session};

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
