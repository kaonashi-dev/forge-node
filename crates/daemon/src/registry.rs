//! Connected-client registry and event routing.
//!
//! Domain events broadcast to every client; terminal deltas go only to
//! subscribers. The outbound queue is bounded (256): when it fills the client
//! is marked behind for that terminal and, once drained, gets one fresh
//! `TerminalResync` instead of a backlog — the PTY loop never blocks.

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, MutexGuard};

use domain::{ClientId, SessionId, TerminalId};
use protocol::{ClientKind, DaemonEvent, DaemonMessage};

pub const CLIENT_QUEUE_CAPACITY: usize = 256;

/// One connected client's outbound channel and subscription state.
struct ClientHandle {
    tx: flume::Sender<DaemonMessage>,
    kind: ClientKind,
    inbox_session: Option<SessionId>,
    subscriptions: HashSet<TerminalId>,
    /// Terminals for which this client fell behind and needs a resync.
    behind: HashSet<TerminalId>,
    finished: HashMap<TerminalId, DaemonEvent>,
}

#[derive(Default)]
pub struct ClientRegistry {
    clients: Mutex<HashMap<ClientId, ClientHandle>>,
}

impl ClientRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Recover from poison the same way `Daemon::lock` does: a panicked client
    /// writer must not take down the PTY thread, which calls into here under
    /// the core lock.
    fn lock_clients(&self) -> MutexGuard<'_, HashMap<ClientId, ClientHandle>> {
        self.clients
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Register a client, returning the receiver its writer task drains.
    pub fn register(&self, id: ClientId) -> flume::Receiver<DaemonMessage> {
        self.register_kind(id, ClientKind::Gui)
    }

    pub fn register_kind(&self, id: ClientId, kind: ClientKind) -> flume::Receiver<DaemonMessage> {
        let (tx, rx) = flume::bounded(CLIENT_QUEUE_CAPACITY);
        self.lock_clients().insert(
            id,
            ClientHandle {
                tx,
                kind,
                inbox_session: None,
                subscriptions: HashSet::new(),
                behind: HashSet::new(),
                finished: HashMap::new(),
            },
        );
        rx
    }

    /// A `forgectl inbox --wait` or `ask --wait` on this connection. Dropped
    /// with the client, the same way a directory watch is.
    pub fn set_inbox_watch(&self, id: ClientId, session_id: Option<SessionId>) {
        if let Some(client) = self.lock_clients().get_mut(&id) {
            client.inbox_session = session_id;
        }
    }

    #[must_use]
    pub fn session_inbox_watched(&self, session_id: SessionId) -> bool {
        self.lock_clients()
            .values()
            .any(|client| client.inbox_session == Some(session_id))
    }

    /// Removes all subscriptions as well as the client.
    pub fn unregister(&self, id: ClientId) {
        self.lock_clients().remove(&id);
    }

    /// Number of currently connected clients. Used by `stats` and to
    /// decide whether a `DaemonNotice` can be delivered now or must be queued
    /// for the first client (startup runs before the accept loop).
    pub fn client_count(&self) -> usize {
        self.lock_clients().len()
    }

    /// Send a single message to one client (e.g. a request response). Returns
    /// `false` if the client is gone or its queue is full.
    ///
    /// Callers must not ignore `false` for a *response*: the client is blocked
    /// on that `request_id` and has no timeout of its own. See
    /// [`crate::server`], which closes the connection instead so the client
    /// observes a disconnect rather than hanging forever.
    #[must_use]
    pub fn send_to(&self, id: ClientId, msg: DaemonMessage) -> bool {
        let clients = self.lock_clients();
        clients.get(&id).is_some_and(|c| c.tx.try_send(msg).is_ok())
    }

    /// Broadcast to every connected client.
    pub fn broadcast_domain(&self, event: DaemonEvent) {
        let clients = self.lock_clients();
        for c in clients.values() {
            if cli_skips(c.kind, &event) {
                continue;
            }
            // Domain events are low-volume; a momentarily full queue drops this
            // one rather than blocking. The client resyncs domain state on
            // reconnect / next GetSnapshot.
            let _ = c.tx.try_send(DaemonMessage::Event(event.clone()));
        }
    }

    pub fn subscribe(&self, id: ClientId, terminal_id: TerminalId) {
        if let Some(c) = self.lock_clients().get_mut(&id) {
            c.subscriptions.insert(terminal_id);
            c.behind.remove(&terminal_id);
        }
    }

    /// Unsubscribe a client from a terminal (`DetachTerminal`).
    pub fn unsubscribe(&self, id: ClientId, terminal_id: TerminalId) {
        if let Some(c) = self.lock_clients().get_mut(&id) {
            c.subscriptions.remove(&terminal_id);
            c.behind.remove(&terminal_id);
            c.finished.remove(&terminal_id);
        }
    }

    /// How many clients are subscribed to a terminal. Used by `AttachTerminal`
    /// to decide whether the attacher may impose its size on a shared grid.
    pub fn subscriber_count(&self, terminal_id: TerminalId) -> usize {
        self.lock_clients()
            .values()
            .filter(|c| c.subscriptions.contains(&terminal_id))
            .count()
    }

    /// Whether any client is subscribed to a terminal (drives whether the PTY
    /// loop bothers building deltas).
    pub fn has_subscribers(&self, terminal_id: TerminalId) -> bool {
        self.lock_clients()
            .values()
            .any(|c| c.subscriptions.contains(&terminal_id))
    }

    /// Every terminal at least one client is watching, in one pass.
    ///
    /// A caller that has to classify many terminals — the idle sweeper walking
    /// every live session — would otherwise take this lock once per terminal,
    /// under the core lock, on every sweep.
    #[must_use]
    pub fn subscribed_terminals(&self) -> HashSet<TerminalId> {
        self.lock_clients()
            .values()
            .flat_map(|c| c.subscriptions.iter().copied())
            .collect()
    }

    /// Route a terminal delta to its subscribers, applying the backpressure
    /// policy. `resync` is called lazily to build a fresh snapshot event only for
    /// clients that need one (behind clients whose queue has drained).
    pub fn route_terminal_delta(
        &self,
        terminal_id: TerminalId,
        delta: &DaemonEvent,
        mut resync: impl FnMut() -> DaemonEvent,
    ) {
        let mut clients = self.lock_clients();
        // Build the resync event at most once per call.
        let mut resync_event: Option<DaemonEvent> = None;
        for c in clients.values_mut() {
            if !c.subscriptions.contains(&terminal_id) {
                continue;
            }
            if c.behind.contains(&terminal_id) {
                if c.tx.is_full() {
                    continue;
                }
                // Try to recover: only send a fresh snapshot once the queue has room.
                let ev = resync_event.get_or_insert_with(&mut resync);
                if c.tx.try_send(DaemonMessage::Event(ev.clone())).is_ok() {
                    c.behind.remove(&terminal_id);
                }
                continue;
            }
            if c.tx.try_send(DaemonMessage::Event(delta.clone())).is_err() {
                // Queue full: drop the backlog conceptually and mark for resync.
                c.behind.insert(terminal_id);
            }
        }
    }

    #[must_use]
    pub fn needs_resync(&self, id: ClientId) -> bool {
        self.lock_clients()
            .get(&id)
            .is_some_and(|client| !client.behind.is_empty())
    }

    /// Called after a socket write frees capacity, including when the PTY has gone quiet.
    pub fn recover(
        &self,
        id: ClientId,
        mut snapshot: impl FnMut(TerminalId) -> Option<DaemonEvent>,
    ) {
        let mut clients = self.lock_clients();
        let Some(client) = clients.get_mut(&id) else {
            return;
        };
        let terminals: Vec<_> = client.behind.iter().copied().collect();
        for terminal in terminals {
            if client.tx.is_full() {
                break;
            }
            let event = client
                .finished
                .remove(&terminal)
                .or_else(|| snapshot(terminal));
            if let Some(event) = event {
                if let Err(error) = client.tx.try_send(DaemonMessage::Event(event)) {
                    if let DaemonMessage::Event(event) = error.into_inner() {
                        client.finished.insert(terminal, event);
                    }
                    break;
                }
            }
            client.behind.remove(&terminal);
        }
    }

    /// Preserve the final grid for lagging clients before its engine is reaped.
    pub fn finish_terminal(&self, terminal: TerminalId, snapshot: impl FnOnce() -> DaemonEvent) {
        let mut clients = self.lock_clients();
        let mut snapshot = Some(snapshot);
        let mut event = None;
        for client in clients
            .values_mut()
            .filter(|client| client.behind.contains(&terminal))
        {
            if event.is_none() {
                event = snapshot.take().map(|build| build());
            }
            if let Some(event) = &event {
                client.finished.insert(terminal, event.clone());
            }
        }
    }

    /// Send a coalesced activity/bell event to clients NOT subscribed to the
    /// terminal, for unread badges.
    pub fn notify_non_subscribers(&self, terminal_id: TerminalId, event: DaemonEvent) {
        let clients = self.lock_clients();
        for c in clients.values() {
            if cli_skips(c.kind, &event) {
                continue;
            }
            if !c.subscriptions.contains(&terminal_id) {
                let _ = c.tx.try_send(DaemonMessage::Event(event.clone()));
            }
        }
    }
}

fn cli_skips(kind: ClientKind, event: &DaemonEvent) -> bool {
    kind == ClientKind::Cli
        && matches!(
            event,
            DaemonEvent::TerminalActivity { .. }
                | DaemonEvent::TerminalBell { .. }
                | DaemonEvent::ClipboardStore { .. }
                | DaemonEvent::EditorFrame { .. }
                | DaemonEvent::ProviderUsageChanged { .. }
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::{Cursor, PtySize, TermModes, TerminalDelta, TerminalSnapshot};

    fn delta_event(terminal_id: TerminalId, seq: u64) -> DaemonEvent {
        DaemonEvent::TerminalDelta {
            terminal_id,
            delta: TerminalDelta {
                patches: Vec::new(),
                scrollback_len: 0,
                scrollback_generation: 0,
                seq,
                rows: vec![],
                scrolled_lines: 0,
                cursor: Cursor::default(),
                modes: TermModes::default(),
            },
        }
    }

    fn resync_event(terminal_id: TerminalId) -> DaemonEvent {
        DaemonEvent::TerminalResync {
            terminal_id,
            snapshot: TerminalSnapshot {
                scrollback_generation: 0,
                seq: 999,
                size: PtySize::default(),
                visible: vec![],
                scrollback_tail: vec![],
                scrollback_len: 0,
                cursor: Cursor::default(),
                modes: TermModes::default(),
                title: None,
            },
        }
    }

    #[test]
    fn only_subscribers_receive_deltas() {
        let reg = ClientRegistry::new();
        let a = ClientId::new();
        let b = ClientId::new();
        let rx_a = reg.register(a);
        let _rx_b = reg.register(b);
        let term = TerminalId::new();
        reg.subscribe(a, term);

        reg.route_terminal_delta(term, &delta_event(term, 1), || resync_event(term));
        assert_eq!(rx_a.len(), 1, "subscriber a gets the delta");
        // b is not subscribed → nothing routed to it.
    }

    #[test]
    fn full_queue_marks_behind_then_recovers_with_resync() {
        let reg = ClientRegistry::new();
        let a = ClientId::new();
        let rx = reg.register(a);
        let term = TerminalId::new();
        reg.subscribe(a, term);

        // Fill the queue past capacity to force "behind".
        for seq in 0..(CLIENT_QUEUE_CAPACITY as u64 + 10) {
            reg.route_terminal_delta(term, &delta_event(term, seq), || resync_event(term));
        }
        // Drain everything the client buffered.
        while rx.try_recv().is_ok() {}

        // Next routed delta should deliver a single resync (recovery path).
        reg.route_terminal_delta(term, &delta_event(term, 9999), || resync_event(term));
        let msg = rx.try_recv().expect("a recovery message");
        match msg {
            DaemonMessage::Event(DaemonEvent::TerminalResync { .. }) => {}
            other => panic!("expected resync, got {other:?}"),
        }
    }

    #[test]
    fn a_full_queue_does_not_build_a_resync_snapshot() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let reg = ClientRegistry::new();
        let a = ClientId::new();
        let _rx = reg.register(a);
        let term = TerminalId::new();
        reg.subscribe(a, term);
        for seq in 0..(CLIENT_QUEUE_CAPACITY as u64 + 1) {
            reg.route_terminal_delta(term, &delta_event(term, seq), || resync_event(term));
        }

        let builds = AtomicUsize::new(0);
        for seq in 0..8 {
            reg.route_terminal_delta(term, &delta_event(term, seq), || {
                builds.fetch_add(1, Ordering::SeqCst);
                resync_event(term)
            });
        }
        assert_eq!(
            builds.load(Ordering::SeqCst),
            0,
            "a full queue must not construct snapshots it cannot send"
        );
    }

    #[test]
    fn recover_sends_a_resync_once_the_queue_has_room() {
        let reg = ClientRegistry::new();
        let a = ClientId::new();
        let rx = reg.register(a);
        let term = TerminalId::new();
        reg.subscribe(a, term);
        for seq in 0..(CLIENT_QUEUE_CAPACITY as u64 + 1) {
            reg.route_terminal_delta(term, &delta_event(term, seq), || resync_event(term));
        }
        while rx.try_recv().is_ok() {}

        reg.recover(a, |_| Some(resync_event(term)));
        let msg = rx.try_recv().expect("a recovery message after drain");
        match msg {
            DaemonMessage::Event(DaemonEvent::TerminalResync { .. }) => {}
            other => panic!("expected resync, got {other:?}"),
        }
    }

    #[test]
    fn domain_events_reach_all_clients() {
        let reg = ClientRegistry::new();
        let a = ClientId::new();
        let b = ClientId::new();
        let rx_a = reg.register(a);
        let rx_b = reg.register(b);
        reg.broadcast_domain(DaemonEvent::DaemonShuttingDown {
            reason: "test".into(),
        });
        assert_eq!(rx_a.len(), 1);
        assert_eq!(rx_b.len(), 1);
    }
}
