use std::io::Write;
use std::sync::Arc;

use domain::{
    ActivityEvidence, ActivityState, ContextEnvelope, ContextId, SessionId, SessionKind,
    TerminalId, Timestamp,
};
use protocol::{ErrorCode, PromptDelivery, ProtocolError, Response};
use terminal_core::TerminalEngine;

use crate::{core::Inner, Daemon};

pub const MAX_PROMPT_BYTES: usize = 8_192;

#[derive(Clone)]
pub(crate) struct PendingPrompt {
    text: String,
    submit: bool,
    terminal: TerminalId,
    context: ContextId,
}

#[derive(Clone, Copy)]
enum Origin {
    Request(Option<SessionId>),
    Queue,
}

fn clean_text(text: String) -> Result<String, ProtocolError> {
    if text.len() > MAX_PROMPT_BYTES {
        return Err(ProtocolError::invalid_request("prompt exceeds 8 KiB"));
    }
    let cleaned: String = text
        .chars()
        .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
        .collect();
    if cleaned.trim().is_empty() {
        return Err(ProtocolError::invalid_request("prompt is empty"));
    }
    Ok(cleaned)
}

pub(crate) fn send(
    daemon: &Daemon,
    caller: Option<SessionId>,
    session_id: SessionId,
    text: String,
    submit: bool,
    when: PromptDelivery,
) -> Result<Response, ProtocolError> {
    let text = clean_text(text)?;
    let context = ContextId::new();
    let terminal = {
        let inner = daemon.lock();
        inner
            .sessions
            .get(&session_id)
            .ok_or_else(|| ProtocolError::not_found("session"))?
            .terminal_id
            .ok_or_else(|| ProtocolError::precondition_failed("target has no live terminal"))?
    };
    let pending = PendingPrompt {
        text,
        submit,
        terminal,
        context,
    };
    deliver(daemon, session_id, pending, when, Origin::Request(caller))
}

fn idle(session: &domain::Session) -> bool {
    session.activity.state == ActivityState::Idle
        && session.activity.evidence == Some(ActivityEvidence::Hook)
}

fn check_target(
    inner: &Inner,
    session_id: SessionId,
    when: PromptDelivery,
    pending: &PendingPrompt,
    new: bool,
) -> Result<TerminalId, ProtocolError> {
    let session = inner
        .sessions
        .get(&session_id)
        .ok_or_else(|| ProtocolError::not_found("session"))?;
    if session.kind != SessionKind::Agent || !session.state.is_active() {
        return Err(ProtocolError::precondition_failed(
            "target must be a live agent",
        ));
    }
    let terminal = session
        .terminal_id
        .ok_or_else(|| ProtocolError::precondition_failed("agent has no terminal"))?;
    if terminal != pending.terminal {
        return Err(ProtocolError::precondition_failed(
            "prompt belongs to an ended terminal",
        ));
    }
    if !idle(session) && when == PromptDelivery::IfIdle {
        return Err(ProtocolError::precondition_failed(
            "target is not hook-confirmed idle",
        ));
    }
    match inner.pending_prompts.get(&session_id) {
        Some(slot) if new || slot.context != pending.context => {
            return Err(ProtocolError::new(
                ErrorCode::Conflict,
                "target already has a queued prompt",
            ))
        }
        None if !new => {
            return Err(ProtocolError::precondition_failed(
                "queued prompt was already consumed",
            ))
        }
        _ => {}
    }
    Ok(terminal)
}

fn deliver(
    daemon: &Daemon,
    session_id: SessionId,
    pending: PendingPrompt,
    when: PromptDelivery,
    origin: Origin,
) -> Result<Response, ProtocolError> {
    let new = matches!(origin, Origin::Request(_));
    if !matches!(
        when,
        PromptDelivery::Now | PromptDelivery::IfIdle | PromptDelivery::Queue
    ) {
        return Err(ProtocolError::invalid_request(
            "unknown prompt delivery policy",
        ));
    }
    let gate = {
        let mut inner = daemon.lock();
        check_target(&inner, session_id, when, &pending, new)?;
        Arc::clone(inner.prompt_gates.entry(session_id).or_default())
    };
    // Only prompt requests share this gate; SQLite and a blocked PTY never hold the core lock.
    let _gate = gate
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (terminal, writer) = {
        let inner = daemon.lock();
        if matches!(origin, Origin::Request(Some(id)) if !inner.sessions.contains_key(&id)) {
            return Err(ProtocolError::not_found("caller session"));
        }
        let terminal = check_target(&inner, session_id, when, &pending, new)?;
        let writer = inner
            .terminals
            .get(&terminal)
            .map(|rt| Arc::clone(&rt.writer))
            .ok_or_else(|| ProtocolError::not_found("terminal"))?;
        (terminal, writer)
    };
    if let Origin::Request(caller) = origin {
        let envelope = ContextEnvelope {
            id: pending.context,
            source_session_id: caller,
            target_session_id: Some(session_id),
            summary: Some("Session prompt (accepted, not proof of delivery)".into()),
            instructions: Some(pending.text.clone()),
            artifacts: vec![],
            git_context: None,
            created_at: Timestamp::now(),
            run_id: None,
            task_id: None,
            kind: None,
            in_reply_to: None,
            acked_at: None,
        };
        daemon
            .context_writer
            .insert(&envelope)
            .map_err(|e| ProtocolError::new(ErrorCode::Internal, e.to_string()))?;
    }
    if when == PromptDelivery::Queue {
        let mut inner = daemon.lock();
        let current = check_target(&inner, session_id, when, &pending, new)?;
        if current != terminal {
            return Err(ProtocolError::precondition_failed(
                "target terminal changed",
            ));
        }
        if !inner.sessions.get(&session_id).is_some_and(idle) {
            let context = pending.context;
            inner.pending_prompts.insert(session_id, pending);
            return Ok(result(context, false, false));
        }
    }
    // Paste and CR are indivisible with respect to input and hook state updates.
    let mut writer = writer
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (modes, submitted, update) = {
        let mut inner = daemon.lock();
        let current = check_target(&inner, session_id, when, &pending, new)?;
        if current != terminal {
            return Err(ProtocolError::precondition_failed(
                "target terminal changed",
            ));
        }
        let idle = inner.sessions.get(&session_id).is_some_and(idle);
        if !idle && when == PromptDelivery::Queue {
            let context = pending.context;
            inner.pending_prompts.insert(session_id, pending);
            return Ok(result(context, false, false));
        }
        let modes = inner
            .terminals
            .get(&terminal)
            .ok_or_else(|| ProtocolError::not_found("terminal"))?
            .engine
            .modes();
        inner.pending_prompts.remove(&session_id);
        let update = if idle {
            inner.sessions.get_mut(&session_id).map(|s| {
                s.activity = domain::orchestration::apply_activity(
                    &s.activity,
                    domain::orchestration::ActivityTrigger::Input,
                    Timestamp::now(),
                );
                s.last_activity_at = Timestamp::now();
                s.clone()
            })
        } else {
            None
        };
        (modes, pending.submit && idle, update)
    };
    if let Some(session) = update {
        daemon
            .registry()
            .broadcast_domain(protocol::DaemonEvent::SessionUpdated(session));
    }
    // LF and TAB are keys in an unbracketed permission dialog, so flatten them too.
    let plain;
    let paste = if modes.bracketed_paste {
        pending.text.as_str()
    } else {
        plain = pending.text.replace(['\n', '\t'], " ");
        &plain
    };
    let bytes = terminal_input::paste(paste, &modes);
    writer
        .write_all(&bytes)
        .and_then(|_| writer.flush())
        .map_err(|e| ProtocolError::new(ErrorCode::IoError, e.to_string()))?;
    if submitted {
        writer
            .write_all(b"\r")
            .and_then(|_| writer.flush())
            .map_err(|e| ProtocolError::new(ErrorCode::IoError, e.to_string()))?;
    }
    Ok(result(pending.context, true, submitted))
}

fn result(context_id: ContextId, delivered: bool, submitted: bool) -> Response {
    Response::PromptDelivery {
        context_id,
        delivered,
        queued: !delivered,
        submitted,
    }
}

pub(crate) fn deliver_pending(daemon: &Daemon, session_id: SessionId) {
    let pending = {
        let inner = daemon.lock();
        if !inner.sessions.get(&session_id).is_some_and(idle) {
            return;
        }
        inner.pending_prompts.get(&session_id).cloned()
    };
    if let Some(pending) = pending {
        if let Err(error) = deliver(
            daemon,
            session_id,
            pending,
            PromptDelivery::Queue,
            Origin::Queue,
        ) {
            tracing::warn!(%session_id, %error, "queued prompt delivery failed; not replaying an uncertain write");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn controls_are_removed_and_size_is_refused_not_truncated() {
        assert_eq!(
            clean_text("hello\x1b[Z\r\x07\n\tworld".into()).unwrap(),
            "hello[Z\n\tworld"
        );
        assert!(clean_text("x".repeat(MAX_PROMPT_BYTES + 1)).is_err());
        assert!(clean_text("\x07\r".into()).is_err());
    }
}
