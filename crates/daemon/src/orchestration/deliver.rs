//! One pointer line, and only while the session is idle and nobody is waiting
//! on its inbox. The body of a message never goes into the PTY.

use domain::orchestration::{pointer_decision, pointer_line, PointerDecision};
use domain::ActivityState;
use domain::SessionId;

use super::Daemon;

pub fn consider_pointer(daemon: &Daemon, session_id: SessionId) {
    let decision = {
        let inner = daemon.lock();
        let state = inner
            .sessions
            .get(&session_id)
            .map(|session| session.activity.state)
            .unwrap_or(ActivityState::Unknown);
        let typed = inner
            .ledger
            .pointer
            .get(&session_id)
            .is_some_and(|slot| slot.typed_this_idle);
        let watched = daemon.registry().session_inbox_watched(session_id);
        pointer_decision(state, watched, typed)
    };
    match decision {
        PointerDecision::Type => deliver_pointer(daemon, session_id),
        PointerDecision::Queue => {
            daemon
                .lock()
                .ledger
                .pointer
                .entry(session_id)
                .or_default()
                .queued = true;
        }
        PointerDecision::Suppress => {}
    }
}

pub fn deliver_pointer(daemon: &Daemon, session_id: SessionId) {
    if daemon.registry().session_inbox_watched(session_id) {
        return;
    }
    let prepared = {
        let inner = daemon.lock();
        inner
            .sessions
            .get(&session_id)
            .and_then(|session| session.terminal_id)
            .and_then(|id| {
                inner
                    .terminals
                    .get(&id)
                    .map(|rt| (id, std::sync::Arc::clone(&rt.writer)))
            })
    };
    let Some((terminal, writer)) = prepared else {
        return;
    };
    let unread = daemon
        .context_writer
        .unread_count(session_id)
        .unwrap_or(0)
        .max(1);
    // A pointer's paste and CR share the prompt/input/hook ordering, not two separate writes.
    let mut writer = writer
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let update = {
        let mut inner = daemon.lock();
        if daemon.registry().session_inbox_watched(session_id) {
            return;
        }
        if inner
            .sessions
            .get(&session_id)
            .is_none_or(|s| s.terminal_id != Some(terminal))
        {
            return;
        }
        let state = inner
            .sessions
            .get(&session_id)
            .map(|session| session.activity.state);
        if state != Some(ActivityState::Idle) {
            if let Some(slot) = inner.ledger.pointer.get_mut(&session_id) {
                slot.queued = true;
            } else {
                inner.ledger.pointer.insert(
                    session_id,
                    super::PointerSlot {
                        queued: true,
                        typed_this_idle: false,
                    },
                );
            }
            return;
        }
        if inner
            .ledger
            .pointer
            .get(&session_id)
            .is_some_and(|slot| slot.typed_this_idle)
        {
            return;
        }
        if let Some(slot) = inner.ledger.pointer.get_mut(&session_id) {
            slot.typed_this_idle = true;
            slot.queued = false;
        } else {
            inner.ledger.pointer.insert(
                session_id,
                super::PointerSlot {
                    queued: false,
                    typed_this_idle: true,
                },
            );
        }
        inner.sessions.get_mut(&session_id).map(|session| {
            session.activity = domain::orchestration::apply_activity(
                &session.activity,
                domain::orchestration::ActivityTrigger::Input,
                domain::Timestamp::now(),
            );
            session.clone()
        })
    };
    if let Some(session) = update {
        daemon
            .registry()
            .broadcast_domain(protocol::DaemonEvent::SessionUpdated(session));
    }
    let line = pointer_line(unread);
    use std::io::Write;
    if writer
        .write_all(line.as_bytes())
        .and_then(|_| writer.flush())
        .is_ok()
    {
        let _ = writer.write_all(b"\r").and_then(|_| writer.flush());
    }
}
