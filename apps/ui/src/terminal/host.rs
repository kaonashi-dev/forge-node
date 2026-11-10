//! The terminal's app-level gestures: zoom, ⌘-links, file drops, the split
//! chord and the bell's attention latch.

use rune::Tx;

use crate::app::AppState;
use crate::keymap::{Action, Zoom};
use crate::settings::prefs;
use crate::terminal::input::{CLICK_SLOP, Pointer, Press};
use crate::terminal::links::{self, Link, Target};
use crate::terminal::pane::Viewport;
use crate::terminal::renderer::CELL_PT;

/// The cell at `zoom`, in whole points so the drawn cell and the IME caret
/// never round apart at 1× or 2×.
#[must_use]
pub fn cell_at_zoom(zoom: f64) -> (f32, f32) {
    let zoom = zoom.clamp(prefs::TERMINAL_ZOOM.min, prefs::TERMINAL_ZOOM.max) as f32;
    (
        (CELL_PT.0 * zoom).round().max(1.0),
        (CELL_PT.1 * zoom).round().max(1.0),
    )
}

/// An agent's terminal rang and nobody has typed into it since, plus the
/// question latch: a shell's bell is a beep.
#[must_use]
pub fn asks(store: &client::Store, session: &domain::Session) -> bool {
    session.agent_provider_id.is_some()
        && session.state.is_active()
        && session
            .terminal_id
            .is_some_and(|terminal| store.wants_attention(&terminal))
}

impl AppState {
    #[must_use]
    pub fn terminal_zoom(&self) -> f64 {
        prefs::read_scale(self.pref(prefs::TERMINAL_ZOOM_KEY), prefs::TERMINAL_ZOOM)
    }

    /// The terminal cell in points at the stored zoom.
    #[must_use]
    pub fn cell_pt(&self) -> (f32, f32) {
        cell_at_zoom(self.terminal_zoom())
    }

    pub fn zoom_terminal(&mut self, zoom: Zoom) {
        let current = self.terminal_zoom();
        let next = match zoom {
            Zoom::In => prefs::step_value(current, true, prefs::TERMINAL_ZOOM),
            Zoom::Out => prefs::step_value(current, false, prefs::TERMINAL_ZOOM),
            Zoom::Reset => prefs::TERMINAL_ZOOM.fallback,
        };
        if (next - current).abs() > f64::EPSILON {
            self.write_pref(prefs::TERMINAL_ZOOM_KEY, &prefs::number_value(next));
            self.terminal.request_frame();
        }
    }

    /// A terminal chord: `Some(true)` taken, `Some(false)` not the terminal's
    /// to take here (an editor or another view is shown), `None` not a
    /// terminal chord at all.
    pub fn terminal_action(&mut self, action: Action) -> Option<bool> {
        let shown = self.session_shown()
            && self.active_line_editor().is_none()
            && self.active_row().and_then(|row| row.terminal_id).is_some();
        match action {
            Action::SplitPane if shown => self.split_pane(),
            Action::TerminalZoom(zoom) if shown => self.zoom_terminal(zoom),
            Action::SplitPane | Action::TerminalZoom(_) => return Some(false),
            _ => return None,
        }
        Some(true)
    }

    /// Checkout paths a reference may name, the pane's own first.
    fn link_roots(&self, idx: usize) -> Vec<String> {
        let Some(store) = &self.store else {
            return Vec::new();
        };
        let own = self.pane_session(idx).and_then(|session| {
            let session = store.sessions.iter().find(|row| row.id == session)?;
            store
                .workspaces
                .iter()
                .find(|workspace| workspace.id == session.workspace_id)
        });
        own.into_iter()
            .chain(store.workspaces.iter())
            .map(|workspace| workspace.path.display().to_string())
            .collect()
    }

    /// The link under a pane-local point, read from that one line.
    fn link_under(&self, idx: usize, x: f32, y: f32) -> Option<Link> {
        let pane = self.pane(idx);
        let slot = pane.shared.lock();
        let grid = slot.grid.as_ref()?;
        let viewport = Viewport {
            grid,
            scroll: pane.scroll.min(grid.scrollback_len as usize),
        };
        let (col, abs) = viewport.cell_at(x, y, self.cell_pt());
        links::link_at(&viewport, abs, col, &self.link_roots(idx))
    }

    fn set_link(&mut self, idx: usize, link: Option<Link>) -> bool {
        let pane = self.pane_mut(idx);
        if pane.link == link {
            return false;
        }
        pane.link = link;
        pane.request_frame();
        true
    }

    /// The pointer moved over a pane: with ⌘ held the link under it is
    /// underlined. Without ⌘ nothing is read, so a plain hover costs nothing.
    pub fn terminal_hover(&mut self, idx: usize, at: Option<(f32, f32)>, tx: &mut Tx) {
        self.pane_mut(idx).hover = at;
        let link = match at {
            Some((x, y)) if tx.mods().cmd => self.link_under(idx, x, y),
            _ => None,
        };
        if !self.set_link(idx, link) {
            tx.live_only();
        }
    }

    /// ⌘ went down or up with the pointer still: underline or drop the link.
    pub fn terminal_modifiers(&mut self, mods: rune::Mods, tx: &mut Tx) {
        if !mods.ctrl && self.tabs.gesture.is_some() {
            self.commit_tab_switcher();
            tx.instant();
        }
        let mut changed = false;
        for idx in 0..self.pane_count() {
            let link = match self.pane(idx).hover {
                Some((x, y)) if mods.cmd => self.link_under(idx, x, y),
                _ => None,
            };
            changed |= self.set_link(idx, link);
        }
        if !changed {
            tx.live_only();
        }
    }

    /// A ⌘-press on a link opens it on release; it
    /// never starts a selection. Returns whether the gesture was the link's.
    pub fn link_press(&mut self, idx: usize, pointer: &Pointer, tx: &mut Tx) -> bool {
        match pointer.phase {
            Press::Down => {
                if !tx.mods().cmd {
                    return false;
                }
                let Some(link) = self.link_under(idx, pointer.x, pointer.y) else {
                    return false;
                };
                let pane = self.pane_mut(idx);
                pane.link = Some(link);
                pane.link_press = Some((pointer.x, pointer.y));
                true
            }
            Press::Move => {
                let pane = self.pane_mut(idx);
                let Some((x, y)) = pane.link_press else {
                    return false;
                };
                if (pointer.x - x).hypot(pointer.y - y) > CLICK_SLOP {
                    pane.link_press = None;
                    pane.link = None;
                    pane.request_frame();
                }
                true
            }
            Press::Up => {
                let pane = self.pane_mut(idx);
                let Some((x, y)) = pane.link_press.take() else {
                    return false;
                };
                let link = pane.link.clone();
                if !pointer.cancelled
                    && (pointer.x - x).hypot(pointer.y - y) <= CLICK_SLOP
                    && let Some(link) = link
                {
                    self.open_link(link.target, tx);
                }
                true
            }
        }
    }

    fn open_link(&mut self, target: Target, tx: &mut Tx) {
        match target {
            Target::Url(url) => {
                let _ = tx.open_url(&url);
            }
            Target::Path { root, path, line } => {
                let workspace = self.store.as_ref().and_then(|store| {
                    store
                        .workspaces
                        .iter()
                        .find(|workspace| workspace.path.display().to_string() == root)
                        .map(|workspace| workspace.id)
                });
                let (Some(workspace), Some(connection)) = (workspace, &self.connection) else {
                    return;
                };
                let _ = connection.open_reference(workspace, path, line);
            }
        }
    }

    /// Files dropped from Finder become their quoted paths at the prompt, as
    /// a paste, so a bracketed-paste shell does not run them.
    pub fn terminal_drop(&mut self, idx: usize, files: &[std::path::PathBuf], tx: &mut Tx) {
        let words: Option<Vec<String>> = files
            .iter()
            .map(|file| links::shell_quote(&file.display().to_string()))
            .collect();
        let Some(words) = words.filter(|words| !words.is_empty()) else {
            eprintln!("[drop] refused: a path with a control character");
            return;
        };
        self.focus_pane(idx);
        let mut text = words.join(" ");
        text.push(' ');
        self.terminal_paste(&text, tx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_zoomed_cell_is_whole_points_and_clamped() {
        assert_eq!(cell_at_zoom(1.0), CELL_PT);
        assert_eq!(cell_at_zoom(1.2), (10.0, 22.0));
        assert_eq!(cell_at_zoom(0.5), (4.0, 9.0));
        assert_eq!(cell_at_zoom(9.0), (16.0, 36.0), "past 200 % is 200 %");
    }

    fn state() -> AppState {
        let store =
            crate::connection::fixture_store(&crate::bench::fixture_path()).expect("fixture");
        AppState::for_bench(
            store,
            std::sync::Arc::new(crate::connection::Metrics::default()),
            crate::terminal::SharedTerminal::new(),
        )
    }

    #[test]
    fn the_zoom_chords_step_the_shared_preference() {
        let mut state = state();
        let row = state.active_row().cloned().expect("active row");
        assert!(row.terminal_id.is_some());
        assert_eq!(
            state.terminal_action(Action::TerminalZoom(Zoom::In)),
            Some(true)
        );
        assert_eq!(state.pref(prefs::TERMINAL_ZOOM_KEY), Some("1.1"));
        assert_eq!(state.cell_pt(), (9.0, 20.0));
        state.terminal_action(Action::TerminalZoom(Zoom::Reset));
        assert_eq!(state.cell_pt(), CELL_PT);
        state.workbench.mode = crate::workbench::CenterMode::Changes;
        assert_eq!(
            state.terminal_action(Action::TerminalZoom(Zoom::In)),
            Some(false),
            "not the terminal's chord while it is not shown"
        );
        assert_eq!(state.terminal_action(Action::Settings), None);
    }

    #[test]
    fn an_agents_bell_marks_its_row_until_it_is_answered() {
        use crate::app::SessionStatus;
        use crate::connection::UiEvent;
        let mut state = state();
        let (id, terminal) = state
            .store
            .as_ref()
            .and_then(|store| {
                store
                    .sessions
                    .iter()
                    .find(|s| {
                        s.agent_provider_id.is_some()
                            && s.state.is_active()
                            && s.terminal_id.is_some()
                            && s.activity.state != domain::ActivityState::Waiting
                    })
                    .map(|s| (s.id, s.terminal_id.unwrap()))
            })
            .expect("a live agent");
        let status = |state: &AppState| state.rows.iter().find(|r| r.id == id).unwrap().status;
        assert_ne!(status(&state), Some(SessionStatus::Attention));
        state.apply_events(
            vec![UiEvent::Daemon {
                generation: state.generation,
                event: Box::new(client::DaemonEvent::TerminalBell {
                    terminal_id: terminal,
                }),
            }],
            &mut Tx::default(),
        );
        assert_eq!(status(&state), Some(SessionStatus::Attention));
        state.select(id);
        state.terminal.shared.publish_grid(
            state.generation,
            id,
            terminal,
            crate::terminal::fixture::grid(80, 24, crate::terminal::fixture::ModeSet::Plain, true),
            domain::PtySize {
                cols: 80,
                rows: 24,
                pixel_width: 0,
                pixel_height: 0,
            },
        );
        let (handle, _rx) = crate::connection::ConnectionHandle::test_handle(std::sync::Arc::new(
            crate::connection::Metrics::default(),
        ));
        state.connection = Some(handle);
        state.out.entries.push(crate::terminal::input::Entry {
            pane: 0,
            bytes: b"y".to_vec(),
            label: "answer".into(),
        });
        state.flush_outbox();
        assert_ne!(
            status(&state),
            Some(SessionStatus::Attention),
            "typing answers it"
        );
    }
}
