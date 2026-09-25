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
        let mut inner = daemon.lock();
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
        let terminal = inner
            .sessions
            .get(&session_id)
            .and_then(|session| session.terminal_id);
        let writer = terminal.and_then(|id| {
            inner
                .terminals
                .get(&id)
                .map(|rt| std::sync::Arc::clone(&rt.writer))
        });
        let unread = inner
            .db
            .context()
            .list_for_session(session_id)
            .unwrap_or_default()
            .into_iter()
            .filter(|message| message.kind.is_some() && message.acked_at.is_none())
            .count();
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
        writer.map(|writer| (writer, unread.max(1)))
    };
    let Some((writer, unread)) = prepared else {
        return;
    };
    let line = pointer_line(unread);
    let mut bytes = line.into_bytes();
    let _ = crate::terminal::write_pty(&writer, &bytes);
    bytes.clear();
    bytes.push(b'\r');
    let _ = crate::terminal::write_pty(&writer, &bytes);
}
