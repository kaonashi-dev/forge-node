//! The split column, as Tauri's `SplitShell` / `DetachSplit` / `ParkSplit`:
//! one extra terminal beside the active one, with its own replica, input gate
//! and measurement. There is one extra column, not a tree of them.

use std::sync::Arc;
use std::time::{Duration, Instant};

use domain::{SessionId, TerminalId};

use crate::app::{AppState, LinkStatus};
use crate::terminal::input::Outbox;
use crate::terminal::pane::TerminalUi;

/// Shared with Tauri's `CENTER_SPLIT_RATIO_KEY`: the primary column's fraction.
pub const RATIO_KEY: &str = "ui.center_split.ratio";
pub const RATIO_MIN: f32 = 0.2;
pub const RATIO_MAX: f32 = 0.8;
pub const RATIO_FALLBACK: f32 = 0.5;
/// How long a ⌘D waits for the worker before another press may ask again.
const PENDING: Duration = Duration::from_secs(5);

pub struct SplitPane {
    pub session: SessionId,
    pub terminal: TerminalId,
    pub ui: TerminalUi,
    /// The replica has listed the session with this terminal. Until then a
    /// missing row or terminal is lag: the worker publishes the split before
    /// the session's row says which terminal it has.
    pub listed: bool,
}

#[derive(Default)]
pub struct Split {
    pub pane: Option<SplitPane>,
    /// Keys go to the split column rather than the primary one.
    pub focused: bool,
    pending: Option<Instant>,
    /// The ratio while the divider is dragged, before it is stored.
    pub dragging: Option<f32>,
    /// Where the divider drag started: the pointer's travel and the ratio.
    pub drag_from: Option<(f32, f32)>,
    /// The two columns' laid-out width, which a divider drag divides by.
    pub width: f32,
}

#[must_use]
pub fn clamp_ratio(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(RATIO_MIN, RATIO_MAX)
    } else {
        RATIO_FALLBACK
    }
}

impl AppState {
    /// 0 is the primary column, 1 the split one.
    #[must_use]
    pub fn focused_pane(&self) -> usize {
        usize::from(self.split.focused && self.split.pane.is_some())
    }

    #[must_use]
    pub fn pane(&self, idx: usize) -> &TerminalUi {
        match (idx, &self.split.pane) {
            (1, Some(pane)) => &pane.ui,
            _ => &self.terminal,
        }
    }

    pub fn pane_mut(&mut self, idx: usize) -> &mut TerminalUi {
        match (idx, &mut self.split.pane) {
            (1, Some(pane)) => &mut pane.ui,
            _ => &mut self.terminal,
        }
    }

    pub(crate) fn pane_io(&mut self, idx: usize) -> (&mut TerminalUi, &mut Outbox) {
        match (idx, &mut self.split.pane) {
            (1, Some(pane)) => (&mut pane.ui, &mut self.out),
            _ => (&mut self.terminal, &mut self.out),
        }
    }

    /// The terminal a pane should show: the active session's, or the split's.
    #[must_use]
    pub fn pane_target(&self, idx: usize) -> Option<TerminalId> {
        match (idx, &self.split.pane) {
            (1, Some(pane)) => Some(pane.terminal),
            (1, None) => None,
            _ => self.active_row().and_then(|row| row.terminal_id),
        }
    }

    #[must_use]
    pub fn pane_session(&self, idx: usize) -> Option<SessionId> {
        match (idx, &self.split.pane) {
            (1, Some(pane)) => Some(pane.session),
            (1, None) => None,
            _ => self.active,
        }
    }

    /// The panes drawn now: the split column only while a session is shown.
    #[must_use]
    pub fn pane_count(&self) -> usize {
        if self.split.pane.is_some() && self.session_shown() {
            2
        } else {
            1
        }
    }

    #[must_use]
    pub fn split_ratio(&self) -> f32 {
        self.split.dragging.unwrap_or_else(|| {
            clamp_ratio(
                self.pref(RATIO_KEY)
                    .and_then(|value| value.trim().parse::<f32>().ok())
                    .unwrap_or(RATIO_FALLBACK),
            )
        })
    }

    pub fn commit_split_ratio(&mut self) {
        if let Some(ratio) = self.split.dragging.take() {
            let value = format!("{}", (clamp_ratio(ratio) * 100.0).round() / 100.0);
            self.write_pref(RATIO_KEY, &value);
        }
        self.split.drag_from = None;
    }

    pub fn focus_pane(&mut self, idx: usize) {
        self.split.focused = idx == 1 && self.split.pane.is_some();
        self.terminal.focus_terminal = true;
    }

    /// ⌘D: a new shell beside the active terminal. A second press is a no-op.
    pub fn split_pane(&mut self) {
        if self.split.pane.is_some()
            || self.split.pending.is_some_and(|at| at.elapsed() < PENDING)
            || !self.session_shown()
            || self.active_line_editor().is_some()
        {
            return;
        }
        let Some(workspace) = self.active_row().map(|row| row.workspace_id) else {
            return;
        };
        let size = self.terminal.shared.measured().unwrap_or((80, 24));
        let Some(connection) = &self.connection else {
            return;
        };
        if connection
            .split_shell(self.generation, workspace, size)
            .is_ok()
        {
            self.split.pending = Some(Instant::now());
        }
    }

    /// Closes the split column; its session keeps running in the strip.
    pub fn join_panes(&mut self) {
        self.split.pending = None;
        self.split.focused = false;
        let Some(pane) = self.split.pane.take() else {
            return;
        };
        if let Some(connection) = &self.connection {
            let _ = connection.detach_split(self.generation, pane.terminal);
        }
        self.terminal.request_frame();
    }

    /// A tab for the session the split column shows focuses that column
    /// instead of moving it into the primary one, as Tauri's `focusSession`.
    pub fn select_split(&mut self, id: SessionId) -> bool {
        if self
            .split
            .pane
            .as_ref()
            .is_some_and(|pane| pane.session == id)
        {
            self.focus_pane(1);
            return true;
        }
        false
    }

    /// The split column's half of `sync_terminal`: adopt what the worker
    /// published after ⌘D, reattach after a reconnect, gate input until both a
    /// snapshot and a measurement exist, and size its PTY to its own column.
    pub fn sync_split(&mut self) {
        let Some(beside) = self.terminal.shared.beside().cloned() else {
            return;
        };
        let generation = self.generation;
        if self.split.pane.is_none() {
            let Some(at) = self.split.pending else {
                return;
            };
            let meta = beside.meta();
            if meta.generation == generation
                && meta.has_grid
                && let (Some(session), Some(terminal)) = (meta.session_id, meta.terminal_id)
            {
                let mut ui = TerminalUi::new(Arc::clone(&beside), false);
                ui.begin_attach(generation, terminal);
                ui.attach_generation = generation;
                ui.focus_terminal = true;
                self.split.pane = Some(SplitPane {
                    session,
                    terminal,
                    ui,
                    listed: false,
                });
                self.split.pending = None;
                self.split.focused = true;
            } else if at.elapsed() >= PENDING {
                self.split.pending = None;
            }
            return;
        }
        if self.status == LinkStatus::Reconnecting {
            return;
        }
        let (session, terminal) = match &self.split.pane {
            Some(pane) => (pane.session, pane.terminal),
            None => return,
        };
        let listed = self
            .store
            .as_ref()
            .and_then(|store| store.sessions.iter().find(|row| row.id == session))
            .map(|row| row.terminal_id);
        let primary = self.active_row().and_then(|row| row.terminal_id);
        let seen = listed == Some(Some(terminal))
            || self.split.pane.as_ref().is_some_and(|pane| pane.listed);
        if let Some(pane) = self.split.pane.as_mut() {
            pane.listed = seen;
        }
        let gone = match listed {
            Some(Some(shown)) => shown != terminal,
            _ => seen,
        };
        if gone || primary == Some(terminal) {
            self.join_panes();
            return;
        }
        let agent = self.rows.iter().any(|row| row.id == session && row.agent);
        let measured = beside.measured();
        let meta = beside.meta();
        let ui = self.pane_mut(1);
        ui.agent = agent;
        ui.observe_resync(beside.resync_epoch(), beside.measure_epoch());
        if ui.attach.terminal() != Some(terminal) || ui.attach.generation() != generation {
            ui.begin_attach(generation, terminal);
        }
        let stale = meta.generation != generation || meta.terminal_id != Some(terminal);
        if stale && ui.attach_generation != generation {
            ui.attach_generation = generation;
            ui.requested = None;
            if let Some(connection) = &self.connection {
                let size = measured.unwrap_or((80, 24));
                let _ = connection.attach_split(generation, session, terminal, size);
            }
            return;
        }
        self.refresh_attach_state(1);
        let ui = self.pane_mut(1);
        if !ui.is_live() || stale {
            return;
        }
        let Some(measured) = measured else {
            return;
        };
        if meta
            .size
            .is_some_and(|size| size.cols == measured.0 && size.rows == measured.1)
        {
            ui.requested = None;
            return;
        }
        if ui
            .requested
            .is_some_and(|(size, at)| size != measured || at.elapsed() > Duration::from_secs(2))
        {
            ui.requested = None;
        }
        if ui.requested.is_some() {
            return;
        }
        let pty = domain::PtySize {
            cols: measured.0,
            rows: measured.1,
            pixel_width: 0,
            pixel_height: 0,
        };
        let sent = self.connection.as_ref().is_some_and(|connection| {
            connection
                .resize_and_attach(generation, terminal, pty)
                .is_ok()
        });
        if sent {
            self.pane_mut(1).requested = Some((measured, Instant::now()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::{Command, ConnectionHandle, Metrics, fixture_store};
    use crate::terminal::SharedTerminal;

    fn state() -> (AppState, flume::Receiver<Command>, Arc<SharedTerminal>) {
        let store = fixture_store(&crate::bench::fixture_path()).expect("fixture");
        let metrics = Arc::new(Metrics::default());
        let shared = SharedTerminal::new();
        let mut state = AppState::for_bench(store, Arc::clone(&metrics), Arc::clone(&shared));
        let (handle, rx) = ConnectionHandle::test_handle(metrics);
        state.connection = Some(handle);
        (state, rx, shared)
    }

    #[test]
    fn a_split_is_adopted_from_the_worker_and_takes_the_keys() {
        let (mut state, rx, shared) = state();
        state.split_pane();
        let Ok(Command::SplitShell { size, .. }) = rx.try_recv() else {
            panic!("⌘D asks the worker for a shell");
        };
        assert_eq!(size, (80, 24));
        state.split_pane();
        assert!(rx.try_recv().is_err(), "a second press waits for the first");

        let other = state
            .rows
            .iter()
            .find(|row| Some(row.id) != state.active && row.terminal_id.is_some())
            .cloned()
            .expect("another terminal session");
        let terminal = other.terminal_id.unwrap();
        let split = shared.beside().unwrap();
        let grid = crate::terminal::fixture::titled_grid(40, 10, "split");
        let size = grid.size;
        split.publish_grid(state.generation, other.id, terminal, grid, size);
        state.sync_split();
        assert_eq!(state.focused_pane(), 1, "the new column takes the keys");
        assert_eq!(state.pane_target(1), Some(terminal));

        split.set_measured(40, 10);
        state.sync_split();
        assert!(
            state.pane(1).is_live(),
            "snapshot and measurement both arrived"
        );
        while rx.try_recv().is_ok() {}
        let mut tx = rune::Tx::default();
        let key = rune::KeyEvent {
            key: rune::Key::Text("z".into()),
            mods: rune::Mods::default(),
        };
        assert!(state.terminal_key(&key, &mut tx));
        assert!(
            matches!(rx.try_recv(), Ok(Command::Input { terminal: t, .. }) if t == terminal),
            "the focused column's terminal gets the key"
        );

        assert!(state.select_split(other.id), "its tab focuses the column");
        state.join_panes();
        assert!(
            matches!(rx.try_recv(), Ok(Command::DetachSplit { terminal: t, .. }) if t == terminal)
        );
        assert_eq!(state.focused_pane(), 0);
    }

    #[test]
    fn a_split_outrunning_its_session_created_event_stays_open() {
        let (mut state, _rx, shared) = state();
        state.split_pane();
        let (session, terminal) = (SessionId::new(), TerminalId::new());
        let grid = crate::terminal::fixture::titled_grid(40, 10, "split");
        let size = grid.size;
        shared
            .beside()
            .unwrap()
            .publish_grid(state.generation, session, terminal, grid, size);
        state.sync_split();
        state.sync_split();
        assert_eq!(
            state.pane_target(1),
            Some(terminal),
            "not yet listed is not gone"
        );
    }

    #[test]
    fn the_ratio_is_clamped_as_tauri_clamps_it() {
        assert_eq!(clamp_ratio(0.05), RATIO_MIN);
        assert_eq!(clamp_ratio(0.95), RATIO_MAX);
        assert_eq!(clamp_ratio(f32::NAN), RATIO_FALLBACK);
    }
}
