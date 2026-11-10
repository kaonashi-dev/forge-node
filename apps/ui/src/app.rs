//! The UI-thread replica: shell state plus the terminal's local view state.
//! Terminal deltas arrive through `SharedTerminal`; only shell events use
//! `AppHandle::update`, so a frame of output never rebuilds the sidebar.

use std::cell::Cell;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use client::Store;
use domain::{ActivityState, SessionId, SessionKind, TerminalId, WorkspaceId};
use rune::{Ime, KeyEvent, Tx};
use rune_kit::shell::PaneState;

use crate::connection::{ConnectionHandle, Metrics, UiEvent};
use crate::editor::EditorUi;
use crate::keymap::{self, Action, Keymap};
use crate::terminal::SharedTerminal;
use crate::terminal::input::{self, Effects, Outbox, Pointer, Press};
use crate::terminal::pane::TerminalUi;
use crate::workbench::{CenterMode, Panel, Workbench};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkStatus {
    Connecting,
    Connected,
    Reconnecting,
}

impl LinkStatus {
    pub fn label(self) -> &'static str {
        match self {
            LinkStatus::Connecting => "Connecting",
            LinkStatus::Connected => "Connected",
            LinkStatus::Reconnecting => "Reconnecting",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionStatus {
    Busy,
    Attention,
}

#[derive(Clone, Debug)]
pub struct SessionRow {
    pub id: SessionId,
    pub kind: SessionKind,
    /// Started by an agent provider; the strip groups these after shells.
    pub agent: bool,
    /// The provider's id, for its mark on the tab and the row.
    pub provider: Option<String>,
    /// `None` for an editor the daemon presents as lines, whose PTY carries
    /// no screen this side should paint.
    pub terminal_id: Option<TerminalId>,
    pub id_text: String,
    pub title: String,
    pub workspace_id: WorkspaceId,
    pub status: Option<SessionStatus>,
    pub running: bool,
    /// How a session that stopped ended, in words.
    pub ended: Option<String>,
    /// An editor's file mark, resolved when the store changes.
    pub file: Option<crate::file_icons::FileIcon>,
}

#[must_use]
pub fn strip_workspace(
    chosen: Option<WorkspaceId>,
    active: Option<WorkspaceId>,
    first: Option<WorkspaceId>,
) -> Option<WorkspaceId> {
    chosen.or(active).or(first)
}

/// A checkout's tabs: its sessions with a terminal, and its editors. No
/// checkout is no tabs, never every session. Shells lead and the rest
/// follow, each group in creation order; an editor is not a shell.
#[must_use]
pub fn rows_in_strip(rows: &[SessionRow], workspace: Option<WorkspaceId>) -> Vec<&SessionRow> {
    let Some(workspace) = workspace else {
        return Vec::new();
    };
    let (mut shells, rest): (Vec<&SessionRow>, Vec<&SessionRow>) = rows
        .iter()
        .filter(|row| {
            row.workspace_id == workspace
                && (row.terminal_id.is_some() || row.kind == SessionKind::Editor)
        })
        .partition(|row| !row.agent && row.kind != SessionKind::Editor);
    shells.extend(rest);
    shells
}

/// ⌘W closes a session only while its terminal is what the centre shows: a
/// session is not recoverable, so a view over it never reaches for it.
#[must_use]
pub fn closes_session(mode: CenterMode) -> bool {
    mode == CenterMode::Session
}

#[derive(Clone, Debug)]
pub struct WorkspaceChoice {
    pub id: WorkspaceId,
}

#[derive(Clone, Debug)]
pub struct CommandStatus {
    pub label: &'static str,
    pub ok: bool,
    pub message: String,
    pub took_us: u64,
}

pub struct AppState {
    pub sidebar: PaneState,
    pub store: Option<Store>,
    pub rows: Vec<SessionRow>,
    pub workspaces: Vec<WorkspaceChoice>,
    pub active: Option<SessionId>,
    pub active_text: Option<String>,
    pub generation: u64,
    pub status: LinkStatus,
    pub status_detail: String,
    pub last_snapshot_us: Option<u64>,
    pub last_command: Option<CommandStatus>,
    pub metrics: Arc<Metrics>,
    pub connection: Option<ConnectionHandle>,
    pub terminal: TerminalUi,
    pub editor_surface: client::EditorSurface,
    pub editor: EditorUi,
    pub workbench: Workbench,
    pub palette: crate::palette::Palette,
    pub projects: crate::projects::Projects,
    pub commit: crate::commit::CommitBox,
    pub review: Option<crate::pr_review::ReviewDialog>,
    pub compose: crate::pr_compose::Compose,
    /// The pull-request filter field, shared by the panel across switches.
    pub pr_query: rune::TextEdit,
    /// `ui.diff.split` as this window last read or wrote it.
    pub diff_split: bool,
    pub usage: crate::usage::Usage,
    pub storage: crate::storage::Storage,
    pub history: crate::history::History,
    /// Action runs grouped for the Processes panel, rebuilt with the store.
    pub processes: Vec<crate::processes::ProcessGroup>,
    /// Each checkout's last status mark, so Git re-reads when it moves.
    pub checkout_marks: HashMap<WorkspaceId, crate::workbench::CheckoutMark>,
    pub theme_editor: crate::appearance::ThemeEditor,
    pub settings: crate::settings::Settings,
    pub sync: crate::sync::Sync,
    pub menus: crate::menus::Menus,
    pub updates: crate::updates::Updates,
    pub about: bool,
    pub native_menu: Option<crate::native_menus::MenuSignature>,
    pub split: crate::terminal::split::Split,
    pub agent_ops: crate::agent_ops::AgentOps,
    pub session_changes: crate::session_changes::SessionChangesView,
    pub code_review: crate::code_review::CodeReview,
    pub tabs: crate::tabs::Tabs,
    pub keymap: Keymap,
    pub toasts: crate::toasts::Toasts,
    /// What a text field edits while its dialog is closing; never shown.
    pub scratch_edit: rune::TextEdit,
    /// A live session whose close waits for a person to confirm it.
    pub confirm_close: Option<SessionId>,
    /// Sessions killed on the way to closing; each is closed once it is
    /// terminal, because the daemon refuses to close a live one.
    pub pending_close: std::collections::HashSet<SessionId>,
    /// The session being renamed and the name being typed.
    pub rename: Option<(SessionId, rune::TextEdit)>,
    /// Shows the client's probes (socket counts, Refresh, Reconnect): on for
    /// a fixture, or with `FORGE_UI_DEBUG` set.
    pub debug: bool,
    /// A workspace picked in Projects; Changes and Files show it instead of
    /// the active session's until a session is selected.
    pub chosen_workspace: Option<WorkspaceId>,
    /// `ui.theme_base` from the daemon: a built-in
    /// base, `"system"`, or unset.
    pub theme_pref: Option<String>,
    /// A scroll offset that brings a moved caret into view, applied once.
    pub editor_reveal: Option<f32>,
    /// A file to open in the first workspace once the replica is loaded.
    pub open_path: Option<String>,
    /// An opened editor whose session has not reached the replica yet.
    pub select_when_listed: Option<SessionId>,
    /// Text a shown terminal asked to put on the clipboard, written by the
    /// next `apply_events`, which holds the `Tx`.
    pub pending_copy: Option<String>,
    pub fixture_titles: HashMap<TerminalId, String>,
    pub out: Outbox,
    pub last_input: Option<(usize, String)>,
    pub last_focus: Option<String>,
    pub mirror_at: f64,
    pub switch_started: Cell<Option<Instant>>,
    pub switch_ms: Cell<Option<f64>>,
    pub view_ms: Cell<Option<f64>>,
    /// Decodes preview images on its own thread; `None` without a window.
    pub images: Option<rune::ImageCache>,
}

impl AppState {
    pub fn new(
        metrics: Arc<Metrics>,
        shared: Arc<SharedTerminal>,
        fixture: bool,
        initial: Option<Store>,
    ) -> Self {
        metrics.note_ui_thread();
        let mut state = Self {
            sidebar: PaneState::new(300.0).range(220.0, 520.0),
            store: None,
            rows: Vec::new(),
            workspaces: Vec::new(),
            active: None,
            active_text: None,
            generation: 0,
            status: LinkStatus::Connecting,
            status_detail: "starting client worker".to_string(),
            last_snapshot_us: None,
            last_command: None,
            metrics,
            connection: None,
            terminal: TerminalUi::new(shared, fixture),
            editor_surface: client::EditorSurface::Cells,
            editor: EditorUi::default(),
            workbench: Workbench::default(),
            palette: crate::palette::Palette::default(),
            projects: crate::projects::Projects::default(),
            commit: crate::commit::CommitBox::default(),
            review: None,
            compose: crate::pr_compose::Compose::default(),
            pr_query: rune::TextEdit::new(""),
            diff_split: false,
            usage: crate::usage::Usage::default(),
            storage: crate::storage::Storage::default(),
            history: crate::history::History::default(),
            processes: Vec::new(),
            checkout_marks: HashMap::new(),
            theme_editor: crate::appearance::ThemeEditor::default(),
            settings: crate::settings::Settings::default(),
            sync: crate::sync::Sync::default(),
            menus: crate::menus::Menus::default(),
            updates: crate::updates::Updates::default(),
            about: false,
            native_menu: None,
            split: crate::terminal::split::Split::default(),
            agent_ops: crate::agent_ops::AgentOps::default(),
            session_changes: crate::session_changes::SessionChangesView::default(),
            code_review: crate::code_review::CodeReview::default(),
            tabs: crate::tabs::Tabs::default(),
            keymap: Keymap::default(),
            toasts: crate::toasts::Toasts::default(),
            scratch_edit: rune::TextEdit::new(""),
            confirm_close: None,
            pending_close: std::collections::HashSet::new(),
            rename: None,
            debug: fixture || std::env::var_os("FORGE_UI_DEBUG").is_some(),
            chosen_workspace: None,
            theme_pref: None,
            editor_reveal: None,
            open_path: None,
            select_when_listed: None,
            pending_copy: None,
            fixture_titles: HashMap::new(),
            out: Outbox::default(),
            last_input: None,
            last_focus: None,
            mirror_at: -1.0,
            switch_started: Cell::new(None),
            switch_ms: Cell::new(None),
            view_ms: Cell::new(None),
            images: None,
        };
        if let Some(store) = initial {
            state.set_store(store);
        }
        state
    }

    pub fn for_bench(store: Store, metrics: Arc<Metrics>, shared: Arc<SharedTerminal>) -> Self {
        let mut state = Self::new(metrics, shared, true, Some(store));
        state.generation = 1;
        state.status = LinkStatus::Connected;
        state
    }

    pub fn active_row(&self) -> Option<&SessionRow> {
        let active = self.active?;
        self.rows.iter().find(|row| row.id == active)
    }

    pub fn select(&mut self, id: SessionId) {
        self.chosen_workspace = None;
        if self.select_split(id) {
            return;
        }
        self.split.focused = false;
        if self.active != Some(id) && self.workbench.mode == CenterMode::SessionChanges {
            self.workbench.show(CenterMode::Session);
        }
        if self.active == Some(id) {
            self.tabs.touch(id);
            self.terminal.focus_terminal = true;
            return;
        }
        self.active = Some(id);
        self.active_text = self
            .rows
            .iter()
            .find(|row| row.id == id)
            .map(|row| row.id_text.clone());
        self.tabs.touch(id);
        self.terminal.focus_terminal = true;
        match self.active_row().and_then(|row| row.terminal_id) {
            Some(target) => self.terminal.begin_attach(self.generation, target),
            None => self.terminal.on_disconnect(),
        }
        self.switch_started.set(Some(Instant::now()));
    }

    fn set_store(&mut self, store: Store) {
        let mut sessions: Vec<&domain::Session> = store.sessions.iter().collect();
        // Creation order; the sort is stable.
        sessions.sort_by_key(|session| session.created_at);
        self.rows = sessions
            .into_iter()
            .map(|session| {
                let lines = session.kind == SessionKind::Editor
                    && self.editor_surface == client::EditorSurface::Dom;
                SessionRow {
                    id: session.id,
                    kind: session.kind,
                    agent: session.agent_provider_id.is_some(),
                    provider: session.agent_provider_id.as_ref().map(ToString::to_string),
                    terminal_id: session.terminal_id.filter(|_| !lines),
                    id_text: session.id.to_string(),
                    workspace_id: session.workspace_id,
                    title: crate::session_label::session_title(session),
                    status: if crate::terminal::host::asks(&store, session) {
                        Some(SessionStatus::Attention)
                    } else {
                        row_status(session)
                    },
                    running: session.state.is_active(),
                    ended: (!session.state.is_active())
                        .then(|| crate::session_label::state_label(&session.state)),
                    file: session
                        .editor
                        .as_ref()
                        .map(|editor| crate::file_icons::for_path(&editor.path)),
                }
            })
            .collect();
        self.workspaces = store
            .workspaces
            .iter()
            .map(|workspace| WorkspaceChoice { id: workspace.id })
            .collect();
        self.fixture_titles
            .extend(store.sessions.iter().filter_map(|session| {
                session
                    .terminal_id
                    .map(|terminal| (terminal, session.title.resolve("untitled").to_string()))
            }));
        let keep = self
            .active
            .filter(|id| self.rows.iter().any(|row| row.id == *id));
        let next = keep
            .or_else(|| self.rows.iter().find(|row| row.running).map(|row| row.id))
            .or_else(|| self.rows.first().map(|row| row.id));
        self.active = next;
        self.active_text = next.and_then(|id| {
            self.rows
                .iter()
                .find(|row| row.id == id)
                .map(|row| row.id_text.clone())
        });
        self.theme_pref = store
            .app_state_value(crate::theme::THEME_KEY)
            .map(str::to_string);
        self.keymap
            .seed_from(store.app_state_value(crate::keymap::KEYMAP_KEY));
        self.tabs
            .seed(store.app_state_value(crate::tabs::TAB_ORDER_KEY));
        let live_workspaces: std::collections::HashSet<String> = store
            .workspaces
            .iter()
            .map(|workspace| workspace.id.to_string())
            .collect();
        self.tabs.retain(&self.rows, &live_workspaces);
        if let Some(active) = self.active {
            self.tabs.touch(active);
        }
        self.history.index = crate::history::index(&store.external_agents);
        self.processes =
            crate::processes::group(&store.sessions, &store.workspaces, &store.projects);
        self.diff_split =
            store.app_state_value(crate::workbench::patch::DIFF_SPLIT_KEY) == Some("true");
        self.store = Some(store);
        self.workbench.set_split(self.diff_split);
        self.terminal.set_fixture_titles(
            self.fixture_titles
                .iter()
                .map(|(terminal, title)| (*terminal, title.clone())),
        );
        if self.palette.open {
            self.refresh_palette_context();
        }
        self.editor_store_changed();
    }

    pub fn apply_events(&mut self, events: Vec<UiEvent>, tx: &mut Tx) {
        self.metrics.note_ui_thread();
        let mut changed = false;
        for event in events {
            changed |= self.apply_event(event);
        }
        if changed {
            tx.instant();
            self.sync_terminal();
            self.sync_split();
        }
        if let Some(text) = self.pending_copy.take() {
            tx.copy(text);
        }
        if let Some(offset) = self.editor_reveal.take() {
            let [_, viewport] = self.editor.scroll.get();
            self.editor.scroll.set([offset, viewport]);
            tx.scroll_to_instant(crate::editor::SCROLL_KEY, 0.0, offset);
        }
        if self.editor.reveal.is_some() {
            self.advance_reveal(tx);
        }
    }

    /// The active session, when it is an editor this side paints as lines.
    #[must_use]
    pub fn active_line_editor(&self) -> Option<SessionId> {
        self.active_row()
            .filter(|row| {
                row.kind == SessionKind::Editor && self.editor_surface == client::EditorSurface::Dom
            })
            .map(|row| row.id)
    }

    /// Whether `terminal` is the active session's, including the PTY under a
    /// line editor, which the session rows deliberately leave out.
    #[must_use]
    pub fn shows_terminal(&self, terminal: TerminalId) -> bool {
        let Some(active) = self.active else {
            return false;
        };
        self.store
            .as_ref()
            .and_then(|store| store.sessions.iter().find(|session| session.id == active))
            .and_then(|session| session.terminal_id)
            == Some(terminal)
    }

    pub fn editor_ime(&mut self, ev: &Ime) -> bool {
        if self.active_line_editor().is_none() || !self.session_shown() {
            return false;
        }
        self.editor.ime(ev);
        self.flush_editor();
        true
    }

    #[must_use]
    pub fn active_editor_state(&self) -> Option<&domain::EditorState> {
        let active = self.active?;
        self.store
            .as_ref()?
            .sessions
            .iter()
            .find(|session| session.id == active)?
            .editor
            .as_ref()
    }

    pub fn editor_key(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool {
        if self.editor_chord(ev, tx) || self.global_key(ev) {
            return true;
        }
        if crate::menus::tree_menu_key(ev, tx) {
            return false;
        }
        if self.active_line_editor().is_none() || !self.session_shown() || !self.editor.key(ev) {
            return false;
        }
        // Sent now rather than on the next poll: a key waits for no frame.
        self.flush_editor();
        true
    }

    /// Keys, pastes and compositions go to the session only while it is what
    /// the centre shows; under Changes or Files they belong to those views.
    #[must_use]
    pub fn session_shown(&self) -> bool {
        self.workbench.mode == CenterMode::Session
    }

    /// Opens a file from the Files view in an editor in its workspace, and
    /// shows that session; the daemon answers with the session that holds it.
    pub fn open_in_editor(&mut self, path: &str) {
        if self
            .workbench
            .files
            .tree
            .get(path)
            .is_none_or(|node| node.data.is_none())
        {
            return;
        }
        self.open_file(path, None);
    }

    /// An app chord, taken before any surface sees the key.
    pub fn global_key(&mut self, ev: &KeyEvent) -> bool {
        if self.tabs.gesture.is_some() && ev.key == rune::Key::Escape {
            self.cancel_tab_switcher();
            return true;
        }
        if self.settings_escape(ev, Instant::now()) {
            return true;
        }
        let Some(action) = self.keymap.app_action(ev) else {
            return false;
        };
        if let Some(taken) = self.terminal_action(action) {
            return taken;
        }
        match action {
            Action::CommandPalette => self.open_palette(crate::palette::Scope::Everything),
            Action::CommandPaletteCommands => {
                self.open_palette(crate::palette::Scope::Commands);
            }
            Action::GoToPalette => self.open_palette(crate::palette::Scope::Places),
            Action::FilePalette => self.open_palette(crate::palette::Scope::Files),
            Action::NewShell => self.request_create(),
            Action::CloseSession => {
                if matches!(
                    self.workbench.mode,
                    CenterMode::Compare | CenterMode::PrDetail | CenterMode::PrCompose
                ) {
                    self.close_center();
                } else if let Some(active) = self.active
                    && closes_session(self.workbench.mode)
                {
                    self.request_close(active);
                }
            }
            Action::NewAgent => self.launch_default(),
            Action::NewWorktree => self.new_worktree_here(),
            Action::ToggleSidebar => self.sidebar.toggle(),
            Action::TogglePanel(panel) => self.toggle_panel(panel),
            Action::CycleViews => self.cycle_views(),
            Action::FocusTab(n) => self.focus_tab(n),
            Action::Settings => {
                if self.workbench.mode == CenterMode::Settings {
                    self.close_settings();
                } else {
                    self.open_settings(None);
                }
            }
            Action::NextSession => self.step_session(1),
            Action::PreviousSession => self.step_session(-1),
            Action::SwitchTabNext => self.step_tab_switcher(1),
            Action::SwitchTabPrevious => self.step_tab_switcher(-1),
            Action::FindInFiles => self.find_in_files(),
            Action::SplitPane => {}
            Action::TerminalZoom(_) => {}
            Action::ReopenClosedTab => self.reopen_closed_editor(),
            Action::PreviousCodeView => self.step_code_view(-1),
            Action::NextCodeView => self.step_code_view(1),
            Action::CloseOtherViews => crate::menus::close_other_views(self),
            Action::CloseViewsToRight => crate::menus::close_views_to_right(self),
            Action::MoveTabLeft => self.move_tab(-1),
            Action::MoveTabRight => self.move_tab(1),
            Action::GoToDefinition
            | Action::GoBack
            | Action::EditorZoomIn
            | Action::EditorZoomOut
            | Action::EditorZoomReset => return self.editor_action(action),
        }
        true
    }

    /// A view's chord: it shows the view, or
    /// hides the sidebar when that view is already what it shows.
    pub fn toggle_panel(&mut self, panel: Panel) {
        if self.sidebar.is_open() && self.workbench.panel == panel {
            self.sidebar.set_open(false);
        } else {
            self.show_panel(panel);
        }
    }

    /// The next sidebar view, in the switcher's order.
    pub fn cycle_views(&mut self) {
        let views = crate::chrome::VIEWS;
        let at = views
            .iter()
            .position(|(panel, ..)| *panel == self.workbench.panel)
            .unwrap_or(0);
        let next = if self.sidebar.is_open() {
            views[(at + 1) % views.len()].0
        } else {
            self.workbench.panel
        };
        self.show_panel(next);
    }

    /// The strip's Nth tab (1-based), as Option-N.
    pub fn focus_tab(&mut self, n: usize) {
        let Some(id) = self.tab_rows().get(n.wrapping_sub(1)).map(|row| row.id) else {
            return;
        };
        self.select(id);
        self.show_mode(CenterMode::Session);
    }

    /// Asks for a branch to check out beside the window's project.
    pub fn new_worktree_here(&mut self) {
        self.sync_workbench_workspace();
        let project = self.store.as_ref().and_then(|store| {
            let workspace = self.workbench.workspace?;
            store
                .workspaces
                .iter()
                .find(|candidate| candidate.id == workspace)
                .map(|found| found.project_id)
        });
        if let Some(project) = project {
            crate::branch_picker::open(self, project);
        }
    }

    /// The active workspace's approved actions, for the palette: (id, label).
    fn palette_actions(&mut self) -> Vec<(domain::ProjectActionId, String)> {
        self.sync_workbench_workspace();
        let Some(workspace) = self.workbench.workspace else {
            return Vec::new();
        };
        self.actions_for(workspace)
            .into_iter()
            .map(|action| (action.id, action.label.clone()))
            .collect()
    }

    /// Opens the palette on `scope`, with the context the scope will list.
    pub fn open_palette(&mut self, scope: crate::palette::Scope) {
        self.sync_workbench_workspace();
        let agents = self.installed_agents();
        let actions = self.palette_actions();
        let recent = self.recent_files();
        let places = self.place_entries();
        let hints = self.palette_hints();
        self.palette.show(scope);
        self.palette.set_agents(agents);
        self.palette.set_actions(actions);
        self.palette.set_recent(recent);
        self.palette.set_places(places);
        self.palette.set_hints(hints);
    }

    /// Keeps a palette that is already open in step with the replica.
    fn refresh_palette_context(&mut self) {
        let agents = self.installed_agents();
        let actions = self.palette_actions();
        let places = self.place_entries();
        let hints = self.palette_hints();
        self.palette.set_agents(agents);
        self.palette.set_actions(actions);
        self.palette.set_places(places);
        self.palette.set_hints(hints);
    }

    /// Sessions and checkouts `@` lists. A session already on screen is not
    /// somewhere to go; waiting sessions lead, then running ones.
    fn place_entries(&self) -> Vec<crate::palette::Entry> {
        use crate::palette::Entry;
        let mut waiting = Vec::new();
        let mut running = Vec::new();
        let mut stopped = Vec::new();
        for row in self
            .rows
            .iter()
            .filter(|row| row.terminal_id.is_some() || row.kind == SessionKind::Editor)
            .filter(|row| Some(row.id) != self.active)
        {
            let note = self
                .store
                .as_ref()
                .and_then(|store| {
                    let workspace = store
                        .workspaces
                        .iter()
                        .find(|workspace| workspace.id == row.workspace_id)?;
                    let project = store
                        .projects
                        .iter()
                        .find(|project| project.id == workspace.project_id)
                        .map(|project| project.name.clone());
                    project
                        .or_else(|| workspace.branch.clone())
                        .or_else(|| workspace.display_name.clone())
                        .or_else(|| Some(workspace.path.display().to_string()))
                })
                .unwrap_or_default();
            let note = match (&row.ended, note.is_empty()) {
                (Some(ended), true) => ended.clone(),
                (Some(ended), false) => format!("{note} · {ended}"),
                (None, _) => note,
            };
            let entry = Entry::Session {
                id: row.id,
                label: row.title.clone(),
                note,
                agent: row.provider.clone(),
                kind: row.kind,
                waiting: row.status == Some(SessionStatus::Attention),
            };
            if row.status == Some(SessionStatus::Attention) {
                waiting.push(entry);
            } else if row.running {
                running.push(entry);
            } else {
                stopped.push(entry);
            }
        }
        let mut out = waiting;
        out.extend(running);
        out.extend(stopped);
        if let Some(store) = &self.store {
            for workspace in &store.workspaces {
                let project = store
                    .projects
                    .iter()
                    .find(|project| project.id == workspace.project_id)
                    .map(|project| project.name.clone());
                out.push(Entry::Checkout {
                    id: workspace.id,
                    label: workspace
                        .branch
                        .clone()
                        .or_else(|| workspace.display_name.clone())
                        .unwrap_or_else(|| workspace.path.display().to_string()),
                    note: project.unwrap_or_default(),
                });
            }
        }
        out
    }

    fn palette_hints(&self) -> Vec<(crate::palette::Command, String)> {
        use crate::palette::Command;
        let mut hints = Vec::new();
        let mut add = |command: Command, action: Action| {
            if let Some(label) = self.keymap.chord_label(action) {
                hints.push((command, label));
            }
        };
        add(Command::NewShell, Action::NewShell);
        add(Command::NewWorktree, Action::NewWorktree);
        for row in self.installed_agents() {
            add(
                Command::NewAgent(row.provider, row.profile),
                Action::NewAgent,
            );
        }
        add(
            Command::Panel(Panel::Projects),
            Action::TogglePanel(Panel::Projects),
        );
        add(
            Command::Panel(Panel::Files),
            Action::TogglePanel(Panel::Files),
        );
        add(
            Command::Panel(Panel::History),
            Action::TogglePanel(Panel::History),
        );
        add(
            Command::Panel(Panel::PullRequests),
            Action::TogglePanel(Panel::PullRequests),
        );
        add(Command::Panel(Panel::Git), Action::TogglePanel(Panel::Git));
        add(
            Command::Panel(Panel::Processes),
            Action::TogglePanel(Panel::Processes),
        );
        add(Command::ToggleSidebar, Action::ToggleSidebar);
        add(Command::Show(CenterMode::Settings), Action::Settings);
        add(Command::FindInFiles, Action::FindInFiles);
        add(Command::SplitPane, Action::SplitPane);
        hints
    }

    /// Ctrl-Tab: start the MRU ring, or step the one already open. A quick tap
    /// never reveals the card; releasing Control commits it either way.
    fn step_tab_switcher(&mut self, delta: isize) {
        if self.tabs.gesture.is_some() {
            self.tabs.step_gesture(delta);
            return;
        }
        let local: Vec<SessionId> = self.tab_rows().iter().map(|row| row.id).collect();
        let live: Vec<SessionId> = self
            .rows
            .iter()
            .filter(|row| row.terminal_id.is_some() && row.kind != SessionKind::Editor)
            .map(|row| row.id)
            .collect();
        let (targets, foreign_at) =
            crate::tabs::switcher_ids(self.tabs.mru(), &local, self.active, &live);
        if targets.len() < 2 {
            return;
        }
        let index = crate::tabs::initial_index(targets.len(), delta);
        self.tabs.start_gesture(targets, foreign_at, index);
    }

    /// Control came up: show the highlighted tab. Escape and a window blur
    /// cancel instead, which leaves the active session exactly where it was.
    pub fn commit_tab_switcher(&mut self) {
        let Some(id) = self.tabs.commit_gesture() else {
            return;
        };
        if self.rows.iter().any(|row| row.id == id) {
            self.select(id);
            self.show_mode(CenterMode::Session);
        }
        self.switch_started.set(Some(Instant::now()));
    }

    pub fn cancel_tab_switcher(&mut self) {
        self.tabs.cancel_gesture();
    }

    /// The active tab one place left or right, never across the shell/agent
    /// split. Reordering does not change which tab is active.
    pub fn move_tab(&mut self, delta: isize) {
        if let Some(active) = self.active {
            self.move_tab_by(active, delta);
        }
    }

    pub fn move_tab_by(&mut self, id: SessionId, delta: isize) {
        let tabs = self.tab_rows();
        let ids: Vec<SessionId> = tabs.iter().map(|row| row.id).collect();
        let Some(from) = ids.iter().position(|candidate| *candidate == id) else {
            return;
        };
        let Some(to) = from.checked_add_signed(delta) else {
            return;
        };
        let (Some(left), Some(right)) = (tabs.get(from), tabs.get(to)) else {
            return;
        };
        if crate::tabs::is_shell(left) != crate::tabs::is_shell(right) {
            return;
        }
        let mut order = ids;
        order.swap(from, to);
        let workspace = left.workspace_id;
        self.tabs.set_order(workspace, order);
        self.persist_tab_order(workspace);
        if let Some(active) = self.active {
            self.tabs.touch(active);
        }
    }

    fn persist_tab_order(&mut self, _workspace: WorkspaceId) {
        let live: std::collections::HashSet<String> = self
            .workspaces
            .iter()
            .map(|choice| choice.id.to_string())
            .collect();
        let value = self.tabs.serialized(&live);
        if let Err(error) = self.write_pref_checked(crate::tabs::TAB_ORDER_KEY, &value) {
            self.toasts.danger("Tab order was not saved", error);
        }
    }

    /// Drops a dragged tab into the gap the pointer chose and stores that order.
    pub fn drop_tab(&mut self) {
        let Some(order) = self.tabs.dropped() else {
            self.tabs.cancel_drag();
            return;
        };
        let workspace = strip_workspace(
            self.chosen_workspace,
            self.active_row().map(|row| row.workspace_id),
            self.workspaces.first().map(|choice| choice.id),
        );
        let Some(workspace) = workspace else {
            self.tabs.cancel_drag();
            return;
        };
        self.tabs.set_order(workspace, order);
        self.tabs.cancel_drag();
        self.persist_tab_order(workspace);
        if let Some(active) = self.active {
            self.tabs.touch(active);
        }
    }

    /// A tab press that may become a drag: remember the strip it came from.
    pub fn begin_tab_drag(&mut self, id: SessionId) {
        let ids: Vec<SessionId> = self.tab_rows().iter().map(|row| row.id).collect();
        let Some(gap) = ids.iter().position(|candidate| *candidate == id) else {
            return;
        };
        self.tabs.begin_drag(id, gap, ids);
    }

    /// The pointer moved during a tab drag: the gap under it, never past the
    /// shell/agent split.
    pub fn drag_tab_to(&mut self, x: f32) {
        let Some(id) = self.tabs.drag.as_ref().map(|drag| drag.id) else {
            return;
        };
        let base = self.tab_rows();
        let ids: Vec<SessionId> = base.iter().map(|row| row.id).collect();
        let gap = crate::tabs::gap_at(&self.tabs.rects, &ids, x);
        let gap = crate::tabs::clamp_gap(&base, id, gap);
        if let Some(drag) = &mut self.tabs.drag {
            drag.gap = gap;
        }
    }

    /// Rebind one action, or unbind it with `None`, and store the result. A
    /// conflict is reported but not refused: the newer binding wins, and
    /// the report says which one it shadowed.
    pub fn rebind_key(&mut self, action: Action, chord: Option<rune::Chord>) {
        let label = chord.map(|chord| chord.label());
        self.keymap.set(action, chord);
        self.save_keymap();
        let mut conflicted = false;
        for conflict in self.keymap.conflicts() {
            if conflict.actions.contains(&action) {
                conflicted = true;
                self.toasts.neutral(
                    format!("{} is already in use", conflict.label()),
                    Some(format!(
                        "{} · {}",
                        conflict.context,
                        conflict.action_labels()
                    )),
                );
            }
        }
        if !conflicted && let Some(label) = label {
            self.toasts.success(
                "Shortcut saved",
                Some(format!("{} · {label}", action.label())),
            );
        }
    }

    pub fn reset_key(&mut self, action: Action) {
        self.settings.keyboard.error = None;
        self.keymap.reset(action);
        self.save_keymap();
    }

    fn save_keymap(&mut self) {
        let value = self.keymap.to_json();
        if let Err(error) = self.write_pref_checked(keymap::KEYMAP_KEY, &value) {
            self.toasts.danger("Keymap was not saved", error);
        }
    }

    /// Providers the daemon found installed, for the palette's agent entries.
    /// Installed agents the person has not hidden in Settings › Agents.
    pub(crate) fn installed_agents(&self) -> Vec<crate::agent_ops::Choice> {
        let Some(store) = &self.store else {
            return Vec::new();
        };
        crate::agent_ops::choices(store, false)
    }

    /// The checkout's strip: the checkout the
    /// window was pointed at, else the active session's, else the first. A
    /// stored order is applied within the shell/agent grouping.
    pub fn tab_rows(&self) -> Vec<&SessionRow> {
        let workspace = strip_workspace(
            self.chosen_workspace,
            self.active_row().map(|row| row.workspace_id),
            self.workspaces.first().map(|choice| choice.id),
        );
        let base = rows_in_strip(&self.rows, workspace);
        match workspace.and_then(|workspace| self.tabs.order_for(workspace)) {
            Some(order) => crate::tabs::applied(&base, order),
            None => base,
        }
    }

    fn step_session(&mut self, delta: isize) {
        let tabs: Vec<SessionId> = self.tab_rows().iter().map(|row| row.id).collect();
        let count = tabs.len();
        if count == 0 {
            return;
        }
        let at = self
            .active
            .and_then(|id| tabs.iter().position(|tab| *tab == id))
            .unwrap_or(0);
        let next = (at as isize + delta).rem_euclid(count as isize) as usize;
        let id = tabs[next];
        self.workbench.show(CenterMode::Session);
        self.select(id);
    }

    /// Keys inside the palette's field: move, run or close.
    pub fn palette_key(&mut self, ev: &KeyEvent) -> bool {
        match ev.key {
            rune::Key::Up => self.palette.move_selection(-1),
            rune::Key::Down => self.palette.move_selection(1),
            rune::Key::Escape => self.palette.close(),
            rune::Key::Enter => {
                if let Some(entry) = self.palette.chosen() {
                    self.run_palette(entry);
                }
            }
            _ => return false,
        }
        true
    }

    pub fn run_palette(&mut self, entry: crate::palette::Entry) {
        use crate::palette::{Command, Entry};
        self.palette.close();
        match entry {
            Entry::Command { command, .. } => match command {
                Command::NewShell => self.request_create(),
                Command::Show(CenterMode::Settings) => self.open_settings(None),
                Command::Show(mode) => self.show_mode(mode),
                Command::Settings(section) => self.open_settings(Some(section)),
                Command::Panel(panel) => self.show_panel(panel),
                Command::ToggleSidebar => self.sidebar.toggle(),
                Command::NewWorktree => self.new_worktree_here(),
                Command::Theme(base) => self.choose_theme(base),
                Command::FindInFiles => self.find_in_files(),
                Command::SplitPane => {
                    self.show_mode(CenterMode::Session);
                    self.split_pane();
                }
                Command::JoinPanes => self.join_panes(),
                Command::RunAction(action) => {
                    self.sync_workbench_workspace();
                    if let Some(workspace) = self.workbench.workspace {
                        self.run_action(workspace, action);
                    }
                }
                Command::NewAgent(provider, profile) => {
                    self.sync_workbench_workspace();
                    if let (Some(connection), Some(workspace)) =
                        (&self.connection, self.workbench.workspace)
                    {
                        let _ = connection.create_agent(workspace, provider, profile, None, false);
                        self.workbench.show(CenterMode::Session);
                    }
                }
            },
            Entry::File(path) => self.open_file(&path, None),
            Entry::Session { id, .. } => self.focus_session(id),
            Entry::Checkout { id, .. } => {
                self.focus_checkout(id);
                self.inspect(id, CenterMode::Changes);
            }
            Entry::Symbol(hit) => self.open_file(&hit.path, Some(hit.line.max(1))),
        }
    }

    /// Opens the commit dialog for the workspace Changes shows, and asks Juva
    /// for a draft.
    /// Asks Juva for a message, only when the person asks.
    pub fn draft_commit(&mut self) {
        let Some(workspace) = self.workbench.workspace else {
            return;
        };
        self.commit.follow(Some(workspace));
        self.commit.drafting = true;
        self.commit.error = None;
        match &self.connection {
            Some(connection) => {
                let _ = connection.draft_commit(workspace);
            }
            None => self.worker_not_ready("DraftWithJuva"),
        }
    }

    pub fn create_commit(&mut self) {
        self.commit.follow(self.workbench.workspace);
        let (Some(workspace), Some(message)) = (self.commit.workspace, self.commit.ready_message())
        else {
            return;
        };
        self.commit.committing = true;
        self.commit.error = None;
        match &self.connection {
            Some(connection) => {
                let _ = connection.commit(workspace, message);
            }
            None => self.worker_not_ready("CreateCommit"),
        }
    }

    /// The project the workspace Changes shows belongs to.
    fn workbench_project(&self) -> Option<(domain::ProjectId, WorkspaceId, Option<String>)> {
        let workspace = self.workbench.workspace?;
        self.store
            .as_ref()?
            .workspaces
            .iter()
            .find(|candidate| candidate.id == workspace)
            .map(|found| (found.project_id, found.id, found.branch.clone()))
    }

    pub fn fetch(&mut self) {
        let Some((project, _, _)) = self.workbench_project() else {
            return;
        };
        if let Some(connection) = &self.connection
            && connection.fetch(project).is_ok()
        {
            self.sync.fetch_started(project);
        }
    }

    pub fn open_pr_dialog(&mut self) {
        let dirty = !self.workbench.changes.files.is_empty();
        if let Some((_, workspace, branch)) = self.workbench_project() {
            self.sync.open_pr(workspace, branch.as_deref(), dirty);
        }
    }

    pub fn create_pull_request(&mut self) {
        let Some(pr) = self.sync.pr.as_mut() else {
            return;
        };
        let Some((title, body, commit)) = pr.ready() else {
            return;
        };
        pr.opening = true;
        pr.outcome = None;
        let workspace = pr.workspace;
        match &self.connection {
            Some(connection) => {
                let _ = connection.open_pull_request(workspace, title, body, commit);
            }
            None => self.worker_not_ready("CreatePullRequest"),
        }
    }

    pub fn refresh_pull_requests(&mut self) {
        if let Some(connection) = &self.connection {
            let _ = connection.refresh_pull_requests();
        }
    }

    pub fn run_action(&mut self, workspace: WorkspaceId, action: domain::ProjectActionId) {
        match &self.connection {
            Some(connection) => {
                if connection.run_action(workspace, action).is_ok() {
                    self.workbench.show(CenterMode::Session);
                }
            }
            None => self.worker_not_ready("RunAction"),
        }
    }

    /// The approved actions of the project `workspace` belongs to.
    #[must_use]
    pub fn actions_for(&self, workspace: WorkspaceId) -> Vec<&domain::ProjectAction> {
        let Some(store) = &self.store else {
            return Vec::new();
        };
        let Some(project) = store
            .workspaces
            .iter()
            .find(|candidate| candidate.id == workspace)
            .map(|found| found.project_id)
        else {
            return Vec::new();
        };
        store
            .project_actions
            .iter()
            .filter(|action| action.project_id == project)
            .collect()
    }

    /// Whether `action` has a live session in `workspace`.
    #[must_use]
    pub fn action_running(&self, workspace: WorkspaceId, action: &domain::ProjectActionId) -> bool {
        self.store.as_ref().is_some_and(|store| {
            store.sessions.iter().any(|session| {
                session.workspace_id == workspace
                    && session.state.is_active()
                    && session
                        .action
                        .as_ref()
                        .is_some_and(|run| &run.action_id == action)
            })
        })
    }

    /// Hands the stopped operation to the agent `prompt_agent` picks, with
    /// the shared prompt; the daemon launches it like any other agent.
    pub fn resolve_with_agent(&mut self) {
        let Some(state) = self.workbench.changes.rebase.as_ref() else {
            return;
        };
        let prompt = domain::conflict_prompt::conflict_prompt(state);
        let (provider, profile) = match self.prompt_agent() {
            Ok(agent) => agent,
            Err(reason) => {
                self.workbench.changes.rebase_error = Some(reason);
                return;
            }
        };
        let (Some(connection), Some(workspace)) = (&self.connection, self.workbench.workspace)
        else {
            return;
        };
        if connection
            .create_agent(workspace, provider, profile, Some(prompt), false)
            .is_ok()
        {
            self.workbench.show(CenterMode::Session);
        }
    }

    pub fn rebase_step(&mut self, step: crate::workbench::RebaseStep) {
        let (Some(connection), Some(scope)) = (&self.connection, self.workbench.scope()) else {
            return;
        };
        let _ = connection.rebase_step(scope, step);
    }

    pub fn add_project(&mut self, path: std::path::PathBuf) {
        match &self.connection {
            Some(connection) => {
                let _ = connection.add_project(path);
            }
            None => self.worker_not_ready("AddProject"),
        }
    }

    pub fn shell_in(&mut self, workspace: WorkspaceId) {
        match &self.connection {
            Some(connection) => {
                let _ = connection.create_shell_session(workspace);
                self.workbench.show(CenterMode::Session);
            }
            None => self.worker_not_ready("CreateShellSession"),
        }
    }

    /// Shows a session from a list outside the tab strip (Processes), with
    /// the window pointed at its checkout.
    pub fn focus_session(&mut self, id: SessionId) {
        let workspace = self
            .rows
            .iter()
            .find(|row| row.id == id)
            .map(|row| row.workspace_id);
        self.select(id);
        if workspace.is_some() {
            self.chosen_workspace = workspace;
        }
        self.show_mode(CenterMode::Session);
    }

    pub fn restart_session(&mut self, id: SessionId) {
        if let Some(connection) = &self.connection {
            let _ = connection.restart_session(id);
        }
    }

    pub fn kill_session(&mut self, id: SessionId) {
        if let Some(connection) = &self.connection {
            let _ = connection.kill_session(id);
        }
    }

    /// Points the window at a checkout, as a rail card does: its
    /// tabs, Git and Files follow, and its first session is selected.
    pub fn focus_checkout(&mut self, workspace: WorkspaceId) {
        if let Some(id) = self
            .rows
            .iter()
            .find(|row| row.workspace_id == workspace && row.terminal_id.is_some())
            .map(|row| row.id)
        {
            self.select(id);
        }
        // After `select`, which forgets a chosen checkout for the session's.
        self.chosen_workspace = Some(workspace);
        self.sync_workbench_workspace();
    }

    /// Shows Changes or Files for `workspace` rather than the active session's.
    pub fn inspect(&mut self, workspace: WorkspaceId, mode: CenterMode) {
        self.chosen_workspace = Some(workspace);
        self.show_mode(mode);
    }

    fn pull_request(&self, repository: &str, number: u32) -> Option<&domain::PullRequest> {
        self.store
            .as_ref()?
            .pull_requests
            .pull_requests
            .iter()
            .find(|pr| pr.repository == repository && pr.number == number)
    }

    pub fn open_review(&mut self, repository: &str, number: u32) {
        let agents = self
            .store
            .as_ref()
            .map(|store| crate::pr_review::review_agents(&store.providers))
            .unwrap_or_default();
        if let Some(pr) = self.pull_request(repository, number) {
            self.review = Some(crate::pr_review::ReviewDialog::new(pr, &agents));
        }
    }

    /// Opens the branch compare for `workspace`, reading its branches for the
    /// base picker on the way.
    pub fn open_compare(&mut self, workspace: WorkspaceId) {
        self.chosen_workspace = Some(workspace);
        self.sync_workbench_workspace();
        self.show_mode(CenterMode::Compare);
        self.request_compare_branches();
    }

    pub fn open_compare_current(&mut self) {
        if let Some(workspace) = self.workbench.workspace {
            self.open_compare(workspace);
        }
    }

    fn request_compare_branches(&mut self) {
        let project = self.workbench.workspace.and_then(|workspace| {
            self.store
                .as_ref()?
                .workspaces
                .iter()
                .find(|row| row.id == workspace)
                .map(|row| row.project_id)
        });
        if let Some(project) = project
            && self.workbench.request_branches(project)
        {
            crate::project_ops::send(self, crate::project_ops::Op::ListBranches(project));
        }
    }

    pub fn choose_compare_base(&mut self, base: &str) {
        self.workbench.compare.request(Some(base.to_string()));
        self.workbench.mode = CenterMode::Compare;
    }

    /// One preference for both diffs; changing it is a redraw, never a read.
    pub fn set_diff_split(&mut self, split: bool) {
        if self.diff_split == split {
            return;
        }
        self.diff_split = split;
        self.write_pref(
            crate::workbench::patch::DIFF_SPLIT_KEY,
            if split { "true" } else { "false" },
        );
        self.workbench.set_split(split);
    }

    pub fn open_pr_detail(&mut self, key: String) {
        self.workbench.open_pr_detail(key);
        self.sidebar.set_open(true);
    }

    pub fn close_center(&mut self) {
        if self.workbench.mode == CenterMode::PrDetail {
            self.workbench.pr_detail = None;
        }
        self.workbench.show(CenterMode::Session);
    }

    /// Launches the review: read-only, in the checkout `review_workspace`
    /// picks, with the recipe's prompt.
    pub fn start_review(&mut self) {
        let Some(review) = &self.review else {
            return;
        };
        let Some(pr) = self
            .pull_request(&review.repository, review.number)
            .cloned()
        else {
            return;
        };
        let problem = if review.agent.is_none() {
            Some("Pick an agent with a review mode.")
        } else if review.prompt(&pr).is_none() {
            Some("Write what the review should look for.")
        } else {
            None
        };
        if let Some(problem) = problem {
            if let Some(review) = self.review.as_mut() {
                review.error = Some(problem.to_string());
            }
            return;
        }
        let workspace = self.store.as_ref().and_then(|store| {
            crate::pr_review::review_workspace(
                &store.workspaces,
                pr.project_id,
                self.workbench.workspace,
            )
        });
        let (Some(workspace), Some(connection)) = (workspace, &self.connection) else {
            if let Some(review) = self.review.as_mut() {
                review.error = Some("No checkout of this repository is open here.".into());
            }
            return;
        };
        let review = self.review.take().expect("checked above");
        let prompt = review.prompt(&pr);
        if let Some(agent) = review.agent
            && connection
                .create_agent(workspace, agent, None, prompt, true)
                .is_ok()
        {
            self.workbench.show(CenterMode::Session);
        }
    }

    /// The base to paint with: the saved one, the system's pair for
    /// `"system"`, and calm-dark when nothing is saved.
    #[must_use]
    pub fn theme_base(&self, system_dark: bool) -> &str {
        match self.theme_pref.as_deref() {
            Some(crate::theme::SYSTEM) => crate::theme::default_base(system_dark),
            Some(base) => base,
            None => crate::theme::DEFAULT_BASE,
        }
    }

    pub fn show_mode(&mut self, mode: CenterMode) {
        self.sync_workbench_workspace();
        self.workbench.show(mode);
        if !matches!(mode, CenterMode::Session | CenterMode::Settings) {
            self.sidebar.set_open(true);
        }
    }

    /// Shows a sidebar view, opening the sidebar if it was hidden.
    pub fn show_panel(&mut self, panel: Panel) {
        self.sync_workbench_workspace();
        self.workbench.show_panel(panel);
        self.sidebar.set_open(true);
    }

    /// The workspace the active session runs in, or the first one.
    pub(crate) fn sync_workbench_workspace(&mut self) {
        let store = self.store.as_ref();
        let from_session = self.active.and_then(|active| {
            store?
                .sessions
                .iter()
                .find(|session| session.id == active)
                .map(|session| session.workspace_id)
        });
        let workspace = self
            .chosen_workspace
            .or(from_session)
            .or_else(|| self.workspaces.first().map(|choice| choice.id));
        self.workbench.set_workspace(workspace);
    }

    /// Reads the analytics while Settings shows them, once per window; the
    /// daemon keeps them for a minute, so asking again is cheap but pointless.
    fn sync_usage(&mut self) {
        let usage = &mut self.usage;
        if self.workbench.mode != CenterMode::Settings
            || self.settings.section != crate::settings::Section::Stats
            || usage.analytics.is_some()
            || usage.pending == Some(usage.window_days)
        {
            return;
        }
        if let Some(connection) = &self.connection
            && connection.usage(self.generation, usage.window_days).is_ok()
        {
            usage.pending = Some(usage.window_days);
        }
    }

    /// Re-reads Git when the window's checkout moved. Marks are kept per
    /// checkout, so switching away and back is not a move, while a change made
    /// meanwhile is.
    fn watch_checkout(&mut self) {
        let Some(workspace) = self.workbench.workspace else {
            return;
        };
        let Some(status) = self.store.as_ref().and_then(|store| {
            store
                .workspaces
                .iter()
                .find(|candidate| candidate.id == workspace)
                .map(|found| &found.status)
        }) else {
            return;
        };
        // Compared in place: this runs on every poll, and a mark is built
        // only when the checkout's status changed.
        let same = self.checkout_marks.get(&workspace).is_some_and(|mark| {
            mark.dirty == status.dirty && mark.head.as_deref() == status.head.as_deref()
        });
        if same {
            return;
        }
        let after = crate::workbench::CheckoutMark {
            head: status.head.clone(),
            dirty: status.dirty,
        };
        let before = self.checkout_marks.insert(workspace, after.clone());
        if crate::workbench::checkout_moved(before.as_ref(), Some(&after)) {
            self.workbench.checkout_moved();
        }
    }

    fn sync_storage(&mut self) {
        let storage = &mut self.storage;
        let shows_paths = matches!(
            self.settings.section,
            crate::settings::Section::General | crate::settings::Section::Storage
        );
        if self.workbench.mode != CenterMode::Settings
            || !shows_paths
            || storage.asked == Some(self.generation)
        {
            return;
        }
        if let Some(connection) = &self.connection
            && connection.paths(self.generation).is_ok()
        {
            storage.asked = Some(self.generation);
        }
    }

    fn sync_workbench(&mut self) {
        // The sidebar's panels follow the window's checkout as the strip does.
        self.sync_workbench_workspace();
        self.watch_checkout();
        if self.palette.asked_in.is_some() && self.palette.asked_in != self.workbench.scope() {
            self.palette.forget_files();
        }
        self.sync_usage();
        self.sync_storage();
        self.sync_review();
        self.commit.follow(self.workbench.workspace);
        self.compose.follow(self.workbench.workspace);
        if let Some(store) = &self.store {
            self.workbench.pr_index.sync(&store.pull_requests);
        }
        if let Some(search) = self.palette.take_search() {
            self.sync_workbench_workspace();
            if let (Some(connection), Some(scope)) = (&self.connection, self.workbench.scope()) {
                self.palette.asked_in = Some(scope);
                let read = match search {
                    crate::palette::Search::Files(query) => crate::workbench::Read::Search(query),
                    crate::palette::Search::Symbols(query) => {
                        crate::workbench::Read::Definition(query)
                    }
                };
                let _ = connection.read(self.generation, scope, read);
            }
        }
        // The sidebar's panels read while a session holds the centre.
        self.send_file_reads(Instant::now());
    }

    fn sync_editor(&mut self) {
        let shown = self.active_line_editor();
        let moved = self.editor.session != shown;
        if let Some((session, events)) = self.editor.set_session(shown)
            && let Some(connection) = &self.connection
        {
            let _ = connection.editor_input(self.generation, session, events);
        }
        if moved {
            self.follow_find();
        }
        if let Some(path) = self.open_path.take() {
            match (&self.connection, self.workspaces.first()) {
                (Some(connection), Some(workspace)) if self.status == LinkStatus::Connected => {
                    let _ = connection.open_editor(workspace.id, path.clone());
                }
                _ => self.open_path = Some(path),
            }
        }
        self.flush_editor();
    }

    fn flush_editor(&mut self) {
        let out = self.editor.take_requests();
        let Some(connection) = &self.connection else {
            return;
        };
        if let Some((session, first_line, line_count)) = out.view {
            let _ = connection.editor_view(self.generation, session, first_line, line_count);
        }
        if let Some((session, events)) = out.input {
            let _ = connection.editor_input(self.generation, session, events);
        }
    }

    fn accept_generation(&mut self, generation: u64) -> bool {
        if generation == self.generation {
            true
        } else {
            self.metrics
                .stale_events
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            false
        }
    }

    fn apply_event(&mut self, event: UiEvent) -> bool {
        match event {
            UiEvent::Connecting { generation, detail } => {
                if generation < self.generation {
                    self.metrics
                        .stale_events
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    return false;
                }
                let first = self.generation == 0;
                self.generation = generation;
                self.status = if first {
                    LinkStatus::Connecting
                } else {
                    LinkStatus::Reconnecting
                };
                self.status_detail = detail;
                self.terminal.on_generation(generation);
                true
            }
            UiEvent::Connected {
                generation,
                detail,
                editor_surface,
            } => {
                if !self.accept_generation(generation) {
                    return false;
                }
                self.status = LinkStatus::Connected;
                self.status_detail = detail;
                self.editor_surface = editor_surface;
                self.editor.reset_requests();
                self.editor.autosave.reset();
                true
            }
            UiEvent::Disconnected { generation, reason } => {
                if !self.accept_generation(generation) {
                    return false;
                }
                self.status = LinkStatus::Reconnecting;
                self.status_detail = reason;
                self.tabs.cancel_gesture();
                self.terminal.on_disconnect();
                true
            }
            UiEvent::Snapshot {
                generation,
                store,
                took_us,
            } => {
                if !self.accept_generation(generation) {
                    return false;
                }
                self.last_snapshot_us = Some(took_us);
                self.set_store(*store);
                crate::projects::seed(self);
                self.status = LinkStatus::Connected;
                true
            }
            UiEvent::Daemon { generation, event } => {
                if !self.accept_generation(generation) {
                    return false;
                }
                if let client::DaemonEvent::ReviewUpdated { run } = event.as_ref() {
                    self.apply_review_run(run);
                    return true;
                }
                crate::project_ops::observe(self, event.as_ref());
                if let client::DaemonEvent::ClipboardStore { terminal_id, text } = event.as_ref() {
                    // Only a terminal this window shows may set the clipboard;
                    // anything else running in a PTY is not the person's choice.
                    if self.shows_terminal(*terminal_id) {
                        eprintln!("[clipboard] {} bytes from the active session", text.len());
                        self.pending_copy = Some(text.clone());
                    } else {
                        eprintln!(
                            "[clipboard] refused {} bytes from a session not shown",
                            text.len()
                        );
                    }
                    return false;
                }
                if let client::DaemonEvent::FileChanged { workspace_id, path } = event.as_ref() {
                    self.workbench
                        .file_changed(*workspace_id, path, Instant::now());
                    return self.workbench.mode == CenterMode::Search;
                }
                if let client::DaemonEvent::EditorFrame { session_id, frame } = *event {
                    if let Some(offset) = self.editor.apply_frame(session_id, frame) {
                        self.editor_reveal = Some(offset);
                    }
                    return true;
                }
                match event.as_ref() {
                    client::DaemonEvent::RemoteRefsUpdated {
                        project_id,
                        remote,
                        updated,
                        error,
                    } => {
                        self.sync
                            .fetch_done(*project_id, remote, *updated, error.as_deref());
                        self.workbench.refresh();
                        return true;
                    }
                    client::DaemonEvent::PullRequestOpened {
                        workspace_id,
                        url,
                        error,
                    } => {
                        self.sync
                            .pr_opened(*workspace_id, url.clone(), error.clone());
                        self.compose
                            .pr_opened(*workspace_id, url.clone(), error.clone());
                        return true;
                    }
                    client::DaemonEvent::PullRequestsUpdated { .. } => return true,
                    client::DaemonEvent::EditorFocusRequested { session_id, .. } => {
                        self.editor_focus_requested(*session_id);
                        return true;
                    }
                    _ => {}
                }
                if let client::DaemonEvent::JuvaDraftReady {
                    workspace_id,
                    draft,
                    fell_back,
                } = event.as_ref()
                {
                    self.commit.apply_draft(*workspace_id, draft, *fell_back);
                    return true;
                }
                if let client::DaemonEvent::PullRequestDraftReady {
                    workspace_id,
                    session_id,
                    draft,
                } = event.as_ref()
                {
                    self.chosen_workspace = Some(*workspace_id);
                    self.sync_workbench_workspace();
                    self.compose.receive(crate::pr_compose::ReceivedDraft {
                        workspace: *workspace_id,
                        session: *session_id,
                        draft: draft.clone(),
                    });
                    self.workbench.show(CenterMode::PrCompose);
                    return true;
                }
                let reset = matches!(event.as_ref(), client::DaemonEvent::FactoryReset);
                let agents = matches!(
                    event.as_ref(),
                    client::DaemonEvent::AgentProfilesChanged { .. }
                        | client::DaemonEvent::AgentDetectionChanged { .. }
                );
                if let Some(store) = &mut self.store {
                    let _ = store.apply_event(&event);
                }
                if reset {
                    self.store = None;
                    self.rows.clear();
                    self.workspaces.clear();
                    self.active = None;
                    self.active_text = None;
                    self.terminal.detach(self.generation, None);
                    self.request_refresh();
                    return true;
                }
                let rebuild = matches!(
                    event.as_ref(),
                    client::DaemonEvent::SessionCreated(_)
                        | client::DaemonEvent::SessionUpdated(_)
                        | client::DaemonEvent::SessionRemoved { .. }
                        | client::DaemonEvent::ProjectAdded(_)
                        | client::DaemonEvent::ProjectUpdated(_)
                        | client::DaemonEvent::ProjectRemoved { .. }
                        | client::DaemonEvent::WorkspaceCreated(_)
                        | client::DaemonEvent::WorkspaceUpdated(_)
                        | client::DaemonEvent::WorkspaceRemoved { .. }
                        | client::DaemonEvent::TerminalBell { .. }
                );
                if rebuild && let Some(store) = self.store.take() {
                    self.set_store(store);
                }
                if let Some(id) = self.select_when_listed
                    && self.rows.iter().any(|row| row.id == id)
                {
                    self.select_when_listed = None;
                    self.select(id);
                }
                rebuild || agents
            }
            UiEvent::Diff {
                generation,
                scope,
                result,
            } => {
                if !self.accept_generation(generation) {
                    return false;
                }
                self.workbench.apply_diff(scope, result.map(|diff| *diff));
                true
            }
            UiEvent::Compare {
                generation,
                scope,
                requested,
                result,
            } => {
                if !self.accept_generation(generation) {
                    return false;
                }
                self.workbench
                    .apply_compare(scope, requested, result.map(|data| *data));
                true
            }
            UiEvent::Directory {
                generation,
                scope,
                path,
                result,
            } => {
                if !self.accept_generation(generation) {
                    return false;
                }
                self.workbench.apply_directory(scope, &path, result);
                true
            }
            UiEvent::File {
                generation,
                scope,
                result,
            } => {
                if !self.accept_generation(generation) {
                    return false;
                }
                self.workbench.apply_file(scope, result.map(|file| *file));
                true
            }
            UiEvent::Image {
                generation,
                scope,
                path,
                result,
            } => {
                if !self.accept_generation(generation) {
                    return false;
                }
                self.apply_image(scope, &path, result);
                true
            }
            UiEvent::ContentSearch {
                generation,
                scope,
                query,
                result,
            } => {
                if !self.accept_generation(generation) {
                    return false;
                }
                self.workbench
                    .search
                    .apply(scope, &query, result.map(|results| *results));
                true
            }
            UiEvent::Watched {
                generation,
                ticket,
                result,
            } => {
                if !self.accept_generation(generation) {
                    return false;
                }
                self.workbench.apply_watch(ticket, result);
                true
            }
            UiEvent::Paths { generation, result } => {
                if !self.accept_generation(generation) {
                    return false;
                }
                match result {
                    Ok(paths) => {
                        self.storage.error = None;
                        self.storage.paths = Some(*paths);
                        self.maybe_preview_migration();
                    }
                    Err(error) => self.storage.error = Some(error),
                }
                true
            }
            UiEvent::StorageMigration {
                generation,
                action,
                result,
            } => {
                if !self.accept_generation(generation) {
                    return false;
                }
                self.apply_migration(action, result);
                true
            }
            UiEvent::PreferenceFailed {
                generation,
                key,
                error,
            } => {
                if !self.accept_generation(generation) {
                    return false;
                }
                let title = if key == keymap::KEYMAP_KEY {
                    "Keymap not saved"
                } else {
                    "Preference not saved"
                };
                self.toasts.danger(title, error);
                true
            }
            UiEvent::Usage {
                generation,
                window_days,
                result,
            } => {
                if !self.accept_generation(generation) {
                    return false;
                }
                if self.usage.pending == Some(window_days) {
                    self.usage.pending = None;
                }
                if window_days == self.usage.window_days {
                    match result {
                        Ok(analytics) => {
                            self.usage.error = None;
                            self.usage.analytics = Some(*analytics);
                        }
                        Err(error) => self.usage.error = Some(error),
                    }
                }
                true
            }
            UiEvent::Search {
                generation,
                scope,
                query,
                result,
            } => {
                if !self.accept_generation(generation) {
                    return false;
                }
                if self.workbench.scope() != Some(scope) {
                    return false;
                }
                match result {
                    Ok(paths) => {
                        let palette_scope = self.palette.scope();
                        if matches!(
                            palette_scope,
                            crate::palette::Scope::Everything | crate::palette::Scope::Files
                        ) {
                            self.palette.apply_search(palette_scope, &query, paths);
                        }
                    }
                    Err(error) => {
                        self.palette.fail_search(self.palette.scope(), &query);
                        if self
                            .palette
                            .note_error(format!("File search failed: {error}"))
                        {
                            self.toasts.danger("File search failed", error);
                        }
                    }
                }
                true
            }
            UiEvent::Symbols {
                generation,
                scope,
                query,
                result,
            } => {
                if !self.accept_generation(generation) {
                    return false;
                }
                if self.workbench.scope() != Some(scope) {
                    return false;
                }
                match result {
                    Ok(results) => {
                        let hits = results
                            .matches
                            .into_iter()
                            .map(|hit| crate::palette::SymbolHit {
                                path: hit.path,
                                line: hit.line,
                                text: hit.text,
                            })
                            .collect();
                        if self.palette.scope() == crate::palette::Scope::Symbols {
                            self.palette.apply_symbols(
                                crate::palette::Scope::Symbols,
                                &query,
                                hits,
                            );
                        }
                    }
                    Err(error) => {
                        self.palette
                            .fail_search(crate::palette::Scope::Symbols, &query);
                        if self
                            .palette
                            .note_error(format!("Declaration search failed: {error}"))
                        {
                            self.toasts.danger("Declaration search failed", error);
                        }
                    }
                }
                true
            }
            UiEvent::Rebase {
                generation,
                scope,
                result,
            } => {
                if !self.accept_generation(generation) {
                    return false;
                }
                let before = self.workbench.changes.rebase.clone();
                let after = result.as_ref().ok().map(|state| (**state).clone());
                self.workbench
                    .apply_rebase(scope, result.map(|state| *state));
                // A step's answer differs from what was on screen; an ordinary
                // read answers what the panel already had. Refreshing only on a
                // step keeps the read from re-arming itself.
                if before.is_some() && before != after {
                    self.workbench.refresh();
                }
                true
            }
            UiEvent::SessionOpened {
                generation,
                session_id,
            } => {
                if !self.accept_generation(generation) {
                    return false;
                }
                // The daemon answers an already-open path with the session
                // that holds it, so this selects it whether or not it is new.
                if self.rows.iter().any(|row| row.id == session_id) {
                    self.select(session_id);
                } else {
                    self.select_when_listed = Some(session_id);
                }
                true
            }
            UiEvent::CommandResult {
                generation,
                label,
                ok,
                message,
                took_us,
            } => {
                if !self.accept_generation(generation) {
                    return false;
                }
                if label == "ResizeTerminal" {
                    self.terminal.requested = None;
                }
                if label == "DeleteExternalSession" && !ok {
                    self.history.error = Some(message.clone());
                }
                if label == "CreateCommit" {
                    self.commit.committing = false;
                    if ok {
                        self.commit.message = rune::TextEdit::new("");
                        self.commit.committed = true;
                        self.workbench.refresh();
                    } else {
                        self.commit.error = Some(message.clone());
                    }
                }
                if label == "CreateAgentSession" && !ok && self.compose.launching {
                    self.compose.launching = false;
                    self.compose.error = Some(message.clone());
                }
                if label == "CommitAndOpenPullRequest" && !ok {
                    self.compose.opening = false;
                    self.compose.error = Some(message.clone());
                }
                if label == "CreatePullRequest"
                    && !ok
                    && let Some(workspace) = self.sync.pr.as_ref().map(|pr| pr.workspace)
                {
                    self.sync.pr_opened(workspace, None, Some(message.clone()));
                }
                if label == "FetchRemote" && !ok {
                    for project in std::mem::take(&mut self.sync.fetching) {
                        self.sync
                            .fetched
                            .insert(project, format!("fetch refused: {message}"));
                    }
                }
                self.settings_result(label, ok, &message);
                crate::menus::command_result(self, label, ok, &message);
                crate::agent_ops::command_result(self, label, ok, &message);
                self.review_command_result(label, ok, &message);
                if label == "DraftWithJuva" && !ok {
                    self.commit.drafting = false;
                    self.commit.error = Some(message.clone());
                }
                if label == "SetEditorAutosave" && !ok {
                    self.editor.autosave.reset();
                }
                if !ok {
                    self.toasts
                        .danger(format!("{label} failed"), message.clone());
                }
                self.last_command = Some(CommandStatus {
                    label,
                    ok,
                    message,
                    took_us,
                });
                true
            }
            UiEvent::Project { generation, reply } => {
                if !self.accept_generation(generation) {
                    return false;
                }
                crate::project_ops::apply(self, *reply);
                true
            }
            UiEvent::EditorConflict {
                generation,
                session_id,
                result,
            } => {
                if !self.accept_generation(generation) {
                    return false;
                }
                self.apply_editor_conflict(session_id, result);
                true
            }
            UiEvent::SessionTranscript {
                generation,
                session_id,
                result,
            } => {
                if !self.accept_generation(generation) {
                    return false;
                }
                crate::agent_ops::apply_transcript(self, session_id, result)
            }
            UiEvent::SessionChanges {
                generation,
                session_id,
                result,
            } => {
                if !self.accept_generation(generation) {
                    return false;
                }
                crate::session_changes::apply(self, session_id, result)
            }
            UiEvent::ReviewStarted {
                generation,
                workspace_id,
                result,
            } => {
                if !self.accept_generation(generation) {
                    return false;
                }
                self.review_started(workspace_id, result)
            }
            UiEvent::ReviewConfig {
                generation,
                workspace_id,
                result,
            } => {
                if !self.accept_generation(generation) {
                    return false;
                }
                self.apply_review_config(workspace_id, result)
            }
            UiEvent::Reviews {
                generation,
                workspace_id,
                result,
            } => {
                if !self.accept_generation(generation) {
                    return false;
                }
                self.apply_reviews(workspace_id, result)
            }
            UiEvent::Review {
                generation,
                workspace_id,
                result,
            } => {
                if !self.accept_generation(generation) {
                    return false;
                }
                self.apply_review_report(workspace_id, result)
            }
        }
    }

    /// The resize gate: attach is always safe, and the shown pane sizes its PTY
    /// once it has been measured, including a session
    /// restored at startup that nobody clicked. `ResizeTerminal` is always
    /// followed by an `AttachTerminal` from the worker. Attach state is
    /// updated here because this is the one UI-thread place that sees the shared
    /// snapshot, the pane measurement and the generation together.
    pub fn sync_terminal(&mut self) {
        self.terminal.observe_resync(
            self.terminal.shared.resync_epoch(),
            self.terminal.shared.measure_epoch(),
        );
        let target = self.active_row().and_then(|row| row.terminal_id);
        let meta = self.terminal.shared.meta();
        let measured = self.terminal.shared.measured();
        let connected = self.status != LinkStatus::Reconnecting;
        if !connected {
            return;
        }
        let Some(target) = target else {
            if meta.terminal_id.is_some() || meta.session_id != self.active {
                self.terminal.detach(self.generation, self.active);
                if self.terminal.is_fixture() {
                    self.terminal.attach_generation = self.generation;
                } else if let Some(connection) = &self.connection {
                    let _ = connection.activate(self.generation, self.active, None, None);
                    self.terminal.attach_generation = self.generation;
                }
            }
            return;
        };
        if self.terminal.attach.terminal() != Some(target)
            || self.terminal.attach.generation() != self.generation
        {
            self.terminal.begin_attach(self.generation, target);
        }
        let size = measured.unwrap_or((80, 24));
        let stale = meta.generation != self.generation
            || meta.terminal_id != Some(target)
            || self.terminal.attach_generation != self.generation;
        if stale {
            if self.terminal.is_fixture() {
                if let Some(session) = self.active {
                    self.terminal
                        .activate_fixture(self.generation, session, target, size);
                }
            } else if let Some(connection) = &self.connection {
                let _ = connection.activate(self.generation, self.active, Some(target), Some(size));
                self.terminal.attach_generation = self.generation;
                self.terminal.requested = None;
            }
        }
        self.refresh_attach_state(0);
        if !self.terminal.is_live() {
            return;
        }
        let meta = self.terminal.shared.meta();
        let Some(measured) = measured else {
            return;
        };
        if meta
            .size
            .is_some_and(|size| size.cols == measured.0 && size.rows == measured.1)
        {
            self.terminal.requested = None;
            return;
        }
        let stale_request = self
            .terminal
            .requested
            .is_some_and(|(size, at)| size != measured || at.elapsed() > Duration::from_secs(2));
        if stale_request {
            self.terminal.requested = None;
        }
        if self.terminal.requested.is_some() {
            return;
        }
        if self.terminal.is_fixture() {
            if let Some(session) = self.active {
                self.terminal
                    .activate_fixture(self.generation, session, target, measured);
            }
            return;
        }
        if let Some(connection) = &self.connection {
            let pty = domain::PtySize {
                cols: measured.0,
                rows: measured.1,
                pixel_width: 0,
                pixel_height: 0,
            };
            if connection
                .resize_and_attach(self.generation, target, pty)
                .is_ok()
            {
                self.terminal.requested = Some((measured, Instant::now()));
            }
        }
    }

    /// Promote Attaching to Live only when both the published snapshot and the
    /// measured pane exist, then release held input in order through the same
    /// encoder live input uses. This is idempotent: an already-live pane only
    /// refreshes its measured size.
    pub(crate) fn refresh_attach_state(&mut self, idx: usize) {
        let Some(target) = self.pane_target(idx) else {
            return;
        };
        let generation = self.generation;
        let pane = self.pane_mut(idx);
        let meta = pane.shared.meta();
        let measured = pane.shared.measured();
        if meta.generation == generation && meta.terminal_id == Some(target) && meta.has_grid {
            let size = meta
                .size
                .map(|size| (size.cols, size.rows))
                .or(measured)
                .unwrap_or((80, 24));
            pane.note_snapshot(generation, target, size);
        }
        pane.note_measured_if_fresh();
        if pane.is_live() {
            self.flush_held_input(idx);
        }
    }

    fn flush_held_input(&mut self, idx: usize) {
        let held = self.pane_mut(idx).take_held();
        if held.is_empty() {
            return;
        }
        let shared = Arc::clone(&self.pane(idx).shared);
        let slot = shared.lock();
        let Some(grid) = slot.grid.as_ref() else {
            eprintln!("[hold] dropped {} gesture(s): no grid", held.len());
            return;
        };
        let (mut gestures, mut bytes) = (0usize, 0u64);
        for event in held {
            let before = self.out.sent_bytes;
            let (pane, out) = self.pane_io(idx);
            match event {
                crate::terminal::attach::HeldInput::Key(ev) => {
                    let mut fx = Effects::default();
                    input::key(idx, pane, grid, &ev, out, &mut fx);
                }
                crate::terminal::attach::HeldInput::Paste(text) => {
                    input::paste_text(idx, pane, grid, &text, out);
                }
            }
            gestures += 1;
            bytes = bytes.saturating_add(self.out.sent_bytes.saturating_sub(before));
        }
        drop(slot);
        eprintln!("[hold] released {gestures} gesture(s), {bytes} bytes after attach");
        self.flush_outbox();
    }

    pub fn poll(&mut self, clock: f64, tx: &mut Tx) -> bool {
        self.metrics.note_ui_thread();
        self.sync_terminal();
        self.sync_split();
        self.sync_editor();
        self.sync_workbench();
        self.close_due();
        let mut changed = self.tabs.reveal(Instant::now());
        changed |= self.toasts.expire(Instant::now());
        changed |= self.settings.esc.expire(Instant::now());
        changed |= self.poll_files(tx);
        crate::native_menus::refresh(self);
        if clock - self.mirror_at >= 0.125 {
            for idx in 0..self.pane_count() {
                let pane = self.pane_mut(idx);
                let version = pane.shared.grid_epoch();
                let dims = {
                    let slot = pane.shared.lock();
                    slot.grid.as_ref().map(|grid| {
                        (
                            grid.cursor,
                            grid.visible.first().map_or(0, |row| row.cells.len()) as u16,
                            grid.visible.len() as u16,
                        )
                    })
                };
                if let Some((cursor, cols, rows)) = dims
                    && (version != pane.mirror_version
                        || pane.scroll != pane.mirror_scroll
                        || cursor != pane.built_cursor)
                {
                    pane.refresh_mirror(version, cols, rows);
                    changed = true;
                }
            }
            self.mirror_at = clock;
        }
        let mut floor: f64 = if self.terminal.shared.meta().has_grid {
            0.100
        } else {
            0.016
        };
        for idx in 0..self.pane_count() {
            let Some(drag) = self.pane(idx).drag else {
                continue;
            };
            // A selection held at an edge keeps scrolling with a still pointer.
            if clock - drag.stepped_at >= input::EDGE_STEP_SECS {
                changed |= self.terminal_edge_step(idx);
                if let Some(drag) = self.pane_mut(idx).drag.as_mut() {
                    drag.stepped_at = clock;
                }
            }
            floor = floor.min(input::EDGE_STEP_SECS);
        }
        tx.wake_after(Duration::from_secs_f64(floor));
        changed
    }

    fn terminal_edge_step(&mut self, idx: usize) -> bool {
        let cell = self.cell_pt();
        let shared = Arc::clone(&self.pane(idx).shared);
        let slot = shared.lock();
        let Some(grid) = slot.grid.as_ref() else {
            return false;
        };
        let (pane, out) = self.pane_io(idx);
        input::edge_scroll(idx, pane, grid, cell, out)
    }

    /// Where a pane's input goes, and whether its replica is that terminal's.
    fn pane_route(&self, idx: usize) -> Option<(TerminalId, bool)> {
        let shared = &self.pane(idx).shared;
        let terminal_id = self
            .pane_target(idx)
            .or_else(|| shared.meta().terminal_id)?;
        Some((terminal_id, shared.meta().terminal_id == Some(terminal_id)))
    }

    pub fn flush_outbox(&mut self) {
        for note in self.out.notes.drain(..) {
            eprintln!("[note] {note}");
        }
        let fetches = std::mem::take(&mut self.out.fetches);
        for (idx, request) in fetches {
            let Some((terminal_id, ready)) = self.pane_route(idx) else {
                continue;
            };
            if !ready {
                continue;
            }
            let Some(connection) = &self.connection else {
                continue;
            };
            if let Err(error) = connection.fetch_scrollback(
                self.generation,
                terminal_id,
                request.from_line,
                request.count,
            ) {
                self.command_failed("FetchScrollback", error);
            }
        }
        let mut answered = false;
        for (idx, bytes, labels) in self.out.drain_batches() {
            let Some((terminal_id, ready)) = self.pane_route(idx) else {
                continue;
            };
            let summary = format!(
                "{} bytes \"{}\" <- {}",
                bytes.len(),
                input::render_bytes(&bytes),
                labels.join(", ")
            );
            if !ready {
                eprintln!("[input] refused: no live grid for {terminal_id} ({summary})");
                continue;
            }
            self.last_input = Some((bytes.len(), summary.clone()));
            eprintln!("[input] {summary}");
            let Some(connection) = &self.connection else {
                continue;
            };
            if let Err(error) = connection.input(self.generation, terminal_id, bytes) {
                self.command_failed("WriteTerminalInput", error);
            }
            answered |= self
                .store
                .as_mut()
                .is_some_and(|store| store.answer_attention(&terminal_id));
        }
        if answered && let Some(store) = self.store.take() {
            self.set_store(store);
        }
        self.terminal.request_frame();
    }

    pub fn terminal_key(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool {
        if self.global_key(ev) {
            return true;
        }
        if !self.session_shown() || crate::menus::tree_menu_key(ev, tx) {
            return false;
        }
        let idx = self.focused_pane();
        self.observe_terminal_resync();
        self.last_focus = tx.focused().map(str::to_owned);
        self.terminal.focus_terminal = true;
        if !self.pane(idx).is_live() {
            if ev.mods.cmd {
                if ev.is_command('v') {
                    tx.request_paste();
                    return true;
                }
                return false;
            }
            self.refresh_attach_state(idx);
            let pane = self.pane_mut(idx);
            if !pane.is_live() {
                if pane.hold_key(ev) {
                    eprintln!(
                        "[hold] held {:?}{} ({}) — {} bytes, {} gestures",
                        ev.key,
                        input::mod_suffix(ev.mods),
                        pane.attach.state_name(),
                        pane.attach.held_bytes(),
                        pane.attach.held_len()
                    );
                } else {
                    eprintln!(
                        "[hold] refused {:?}: 64 KiB hold buffer full ({} refused)",
                        ev.key,
                        pane.attach.refused()
                    );
                }
                return true;
            }
        }
        let shared = Arc::clone(&self.pane(idx).shared);
        let slot = shared.lock();
        let Some(grid) = slot.grid.as_ref() else {
            return false;
        };
        let mut fx = Effects::default();
        let (pane, out) = self.pane_io(idx);
        let taken = input::key(idx, pane, grid, ev, out, &mut fx);
        drop(slot);
        if taken {
            self.apply_effects(fx, tx);
        }
        taken
    }

    fn observe_terminal_resync(&mut self) {
        for idx in 0..=usize::from(self.split.pane.is_some()) {
            let pane = self.pane_mut(idx);
            pane.observe_resync(pane.shared.resync_epoch(), pane.shared.measure_epoch());
        }
    }

    pub fn terminal_pointer(&mut self, idx: usize, pointer: Pointer, tx: &mut Tx) {
        if self.link_press(idx, &pointer, tx) {
            return;
        }
        self.focus_pane(idx);
        let cell = self.cell_pt();
        let shared = Arc::clone(&self.pane(idx).shared);
        let slot = shared.lock();
        let Some(grid) = slot.grid.as_ref() else {
            return;
        };
        let mut fx = Effects::default();
        let (pane, out) = self.pane_io(idx);
        input::pointer(idx, pane, grid, pointer, cell, out, &mut fx);
        drop(slot);
        self.apply_effects(fx, tx);
    }

    pub fn terminal_wheel(&mut self, idx: usize, ev: &rune::WheelEvent, tx: &mut Tx) -> bool {
        let cell = self.cell_pt();
        let shared = Arc::clone(&self.pane(idx).shared);
        let slot = shared.lock();
        let Some(grid) = slot.grid.as_ref() else {
            return false;
        };
        let (pane, out) = self.pane_io(idx);
        let taken = input::wheel(idx, pane, grid, ev, cell, out);
        drop(slot);
        if taken {
            self.flush_outbox();
            tx.live_only();
        }
        taken
    }

    pub fn terminal_secondary(&mut self, idx: usize, x: f32, y: f32, tx: &mut Tx) {
        let cell = self.cell_pt();
        let shared = Arc::clone(&self.pane(idx).shared);
        let slot = shared.lock();
        let Some(grid) = slot.grid.as_ref() else {
            return;
        };
        let pointer = Pointer {
            phase: Press::Up,
            x,
            y,
            clicks: 1,
            cancelled: false,
            shift: tx.mods().shift,
            plain: false,
        };
        let mut fx = Effects::default();
        let (pane, out) = self.pane_io(idx);
        input::secondary(idx, pane, grid, pointer, cell, out, &mut fx);
        drop(slot);
        self.apply_effects(fx, tx);
    }

    /// A paste goes to whichever surface the active session shows.
    pub fn paste(&mut self, text: &str, tx: &mut Tx) {
        if !self.session_shown() {
            return;
        }
        if self.active_line_editor().is_some() {
            self.editor
                .queue(domain::EditorInputEvent::Text(text.to_string()));
            self.flush_editor();
        } else {
            self.terminal_paste(text, tx);
        }
    }

    pub fn terminal_paste(&mut self, text: &str, tx: &mut Tx) {
        let idx = self.focused_pane();
        self.observe_terminal_resync();
        self.last_focus = tx.focused().map(str::to_owned);
        self.terminal.focus_terminal = true;
        if !self.pane(idx).is_live() {
            self.refresh_attach_state(idx);
            let pane = self.pane_mut(idx);
            if !pane.is_live() {
                if pane.hold_paste(text) {
                    eprintln!(
                        "[hold] held paste {} bytes ({} held total)",
                        text.len(),
                        pane.attach.held_bytes()
                    );
                } else {
                    eprintln!(
                        "[hold] refused paste {} bytes: 64 KiB hold buffer full ({} refused)",
                        text.len(),
                        pane.attach.refused()
                    );
                }
                return;
            }
        }
        let shared = Arc::clone(&self.pane(idx).shared);
        let slot = shared.lock();
        let Some(grid) = slot.grid.as_ref() else {
            return;
        };
        let (pane, out) = self.pane_io(idx);
        input::paste_text(idx, pane, grid, text, out);
        drop(slot);
        self.flush_outbox();
        tx.live_only();
    }

    pub fn terminal_ime(&mut self, ev: &Ime, tx: &mut Tx) -> bool {
        if !self.session_shown() {
            return false;
        }
        let idx = self.focused_pane();
        self.observe_terminal_resync();
        if !self.pane(idx).is_live() {
            let pane = self.pane_mut(idx);
            match ev {
                Ime::Commit(text) => {
                    let clean: String = text.chars().filter(|c| !c.is_control()).collect();
                    if pane.hold_paste(&clean) {
                        eprintln!(
                            "[hold] held IME commit {} bytes ({} held total)",
                            clean.len(),
                            pane.attach.held_bytes()
                        );
                    } else {
                        eprintln!(
                            "[hold] refused IME commit: 64 KiB hold buffer full ({} refused)",
                            pane.attach.refused()
                        );
                    }
                    return true;
                }
                Ime::Preedit(text, _) => {
                    pane.preedit = text.chars().take(input::PREEDIT_MAX_CHARS).collect();
                    pane.request_frame();
                    return true;
                }
                Ime::Enabled | Ime::Disabled => return true,
            }
        }
        let shared = Arc::clone(&self.pane(idx).shared);
        let slot = shared.lock();
        let Some(grid) = slot.grid.as_ref() else {
            return false;
        };
        let _ = grid;
        let (pane, out) = self.pane_io(idx);
        let taken = input::ime(idx, pane, ev, out);
        drop(slot);
        if taken {
            self.flush_outbox();
            tx.live_only();
        }
        taken
    }

    pub fn terminal_window_focus(&mut self, focused: bool, tx: &mut Tx) {
        if !focused {
            self.tabs.cancel_gesture();
        }
        let idx = self.focused_pane();
        let shared = Arc::clone(&self.pane(idx).shared);
        let slot = shared.lock();
        let Some(grid) = slot.grid.as_ref() else {
            return;
        };
        let (pane, out) = self.pane_io(idx);
        input::focus(idx, pane, grid, focused, out);
        drop(slot);
        self.flush_outbox();
        tx.live_only();
    }

    fn apply_effects(&mut self, fx: Effects, tx: &mut Tx) {
        if let Some(text) = fx.copy {
            tx.copy(text);
        }
        if fx.request_paste {
            tx.request_paste();
        }
        self.flush_outbox();
        tx.live_only();
    }

    pub fn request_create(&mut self) {
        let workspace = self
            .store
            .as_ref()
            .and_then(|store| {
                let active = self.active?;
                let session = store.sessions.iter().find(|session| session.id == active)?;
                Some(session.workspace_id)
            })
            .or_else(|| self.workspaces.first().map(|workspace| workspace.id));
        let Some(workspace_id) = workspace else {
            self.command_failed("CreateShellSession", "no workspace in the snapshot");
            return;
        };
        match &self.connection {
            Some(connection) => {
                if let Err(error) = connection.create_shell_session(workspace_id) {
                    self.command_failed("CreateShellSession", error);
                }
            }
            None => self.worker_not_ready("CreateShellSession"),
        }
    }

    /// Closes a session: at once when it is no longer running, after a
    /// confirmation when it is.
    pub fn request_close(&mut self, session: SessionId) {
        let live = self.store.as_ref().is_some_and(|store| {
            store
                .sessions
                .iter()
                .any(|candidate| candidate.id == session && candidate.state.is_active())
        });
        if live {
            self.confirm_close = Some(session);
        } else {
            self.send_close(session, false);
        }
    }

    pub fn confirm_close_session(&mut self) {
        if let Some(session) = self.confirm_close.take() {
            self.send_close(session, true);
        }
    }

    /// Closes without asking: kills it first while it still runs.
    pub fn close_now(&mut self, session: SessionId) {
        let live = self.store.as_ref().is_some_and(|store| {
            store
                .sessions
                .iter()
                .any(|candidate| candidate.id == session && candidate.state.is_active())
        });
        self.send_close(session, live);
    }

    fn send_close(&mut self, session: SessionId, live: bool) {
        if self.connection.is_some() {
            self.remember_closed_editor(session);
        }
        let Some(connection) = &self.connection else {
            return self.worker_not_ready("CloseSession");
        };
        if live {
            if connection.kill_session(session).is_ok() {
                self.pending_close.insert(session);
            }
        } else {
            let _ = connection.close_session(session);
        }
    }

    /// Closes every killed session that has reached a terminal state.
    fn close_due(&mut self) {
        if self.pending_close.is_empty() {
            return;
        }
        let Some(store) = &self.store else {
            return;
        };
        let due: Vec<SessionId> = self
            .pending_close
            .iter()
            .copied()
            .filter(|id| {
                store
                    .sessions
                    .iter()
                    .find(|session| session.id == *id)
                    .is_none_or(|session| !session.state.is_active())
            })
            .collect();
        for id in due {
            self.pending_close.remove(&id);
            if let Some(connection) = &self.connection
                && store.sessions.iter().any(|session| session.id == id)
            {
                let _ = connection.close_session(id);
            }
        }
    }

    /// Opens the rename prompt with the session's current name.
    pub fn request_rename(&mut self) {
        let Some(row) = self.active_row() else {
            return;
        };
        let mut edit = rune::TextEdit::new(row.title.clone());
        edit.focused = true;
        self.rename = Some((row.id, edit));
    }

    pub fn finish_rename(&mut self) {
        let Some((session_id, edit)) = self.rename.take() else {
            return;
        };
        let title = edit.text.trim().to_string();
        if title.is_empty() {
            return;
        }
        match &self.connection {
            Some(connection) => {
                let _ = connection.rename_session(session_id, title);
            }
            None => self.worker_not_ready("RenameSession"),
        }
    }

    /// Cmd-Shift-A: the default agent from `ui.default_agent`, a terminal,
    /// or the launch palette when the preference asks or is stale.
    /// The agent "Resolve with AI" hands its prompt to; see `launch::resolver`.
    fn prompt_agent(
        &self,
    ) -> Result<(domain::AgentProviderId, Option<domain::AgentProfileId>), String> {
        use crate::launch::{Launchables, RESOLVE_AGENT_KEY, parse, resolver, visibility_key};
        let Some(store) = &self.store else {
            return Err("Not connected to the daemon yet.".into());
        };
        let installed: Vec<domain::AgentProviderId> = self
            .installed_agents()
            .into_iter()
            .map(|row| row.provider)
            .collect();
        let profiles: Vec<(domain::AgentProfileId, domain::AgentProviderId)> = store
            .agent_profiles
            .iter()
            .map(|profile| (profile.id, profile.provider_id.clone()))
            .collect();
        let can = Launchables {
            visible: |key: &str| store.app_state_value(&visibility_key(key)) != Some("false"),
            installed: &installed,
            profiles: &profiles,
            takes_prompt: |provider: &domain::AgentProviderId| {
                store.providers.iter().any(|info| {
                    info.descriptor.id == *provider
                        && info.descriptor.capabilities.supports_initial_prompt
                })
            },
        };
        resolver(
            &parse(store.app_state_value(crate::launch::DEFAULT_AGENT_KEY)),
            store.app_state_value(RESOLVE_AGENT_KEY),
            &can,
        )
    }

    pub fn launch_default(&mut self) {
        use crate::launch::{Launch, parse, resolve, visibility_key};
        let Some(store) = &self.store else {
            return self.open_launch_palette();
        };
        let installed: Vec<domain::AgentProviderId> = self
            .installed_agents()
            .into_iter()
            .map(|row| row.provider)
            .collect();
        let profiles: Vec<(domain::AgentProfileId, domain::AgentProviderId)> = store
            .agent_profiles
            .iter()
            .map(|profile| (profile.id, profile.provider_id.clone()))
            .collect();
        let agent = parse(store.app_state_value(crate::launch::DEFAULT_AGENT_KEY));
        let visible = |key: &str| store.app_state_value(&visibility_key(key)) != Some("false");
        match resolve(&agent, visible, &installed, &profiles) {
            Launch::Ask => self.open_launch_palette(),
            Launch::Shell => self.request_create(),
            Launch::Agent { provider, profile } => {
                self.sync_workbench_workspace();
                if let (Some(connection), Some(workspace)) =
                    (&self.connection, self.workbench.workspace)
                    && connection
                        .create_agent(workspace, provider, profile, None, false)
                        .is_ok()
                {
                    self.workbench.show(CenterMode::Session);
                }
            }
        }
    }

    /// The palette scoped to what can be launched: a terminal or an agent.
    pub fn open_launch_palette(&mut self) {
        self.open_palette(crate::palette::Scope::Launch);
    }

    pub fn request_refresh(&mut self) {
        if let Some(connection) = &self.connection
            && let Err(error) = connection.refresh_snapshot()
        {
            self.command_failed("GetSnapshot", error);
        }
    }

    pub fn request_reconnect(&mut self) {
        if let Some(connection) = &self.connection
            && let Err(error) = connection.force_reconnect()
        {
            self.command_failed("Reconnect", error);
        }
    }

    /// Records a refused command and leaves a toast: the debug row is only
    /// visible with `FORGE_UI_DEBUG`, and a failed gesture otherwise looked
    /// like a shortcut that did nothing.
    pub fn command_failed(&mut self, label: &'static str, message: impl Into<String>) {
        let message = message.into();
        self.toasts
            .danger(format!("{label} failed"), message.clone());
        self.last_command = Some(CommandStatus {
            label,
            ok: false,
            message,
            took_us: 0,
        });
    }

    fn worker_not_ready(&mut self, label: &'static str) {
        self.command_failed(label, "client worker is still starting");
    }
}

pub fn poll(state: &mut AppState, clock: f64, tx: &mut Tx) -> bool {
    state.poll(clock, tx)
}

/// The tab's marker: waiting for the person
/// outranks everything; an editor or a shell is never busy, since only an
/// agent's hooks report a turn; and a keystroke into an idle agent is a
/// guard the daemon raises, not a turn.
fn row_status(session: &domain::Session) -> Option<SessionStatus> {
    if session.state.is_terminal() {
        return None;
    }
    if session.activity.state == ActivityState::Waiting {
        return Some(SessionStatus::Attention);
    }
    if session.kind == SessionKind::Editor || session.agent_provider_id.is_none() {
        return None;
    }
    match session.activity.state {
        ActivityState::Working
            if session.activity.evidence != Some(domain::ActivityEvidence::Input) =>
        {
            Some(SessionStatus::Busy)
        }
        ActivityState::Starting => Some(SessionStatus::Busy),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::fixture_store;
    use std::path::Path;

    fn state() -> AppState {
        let store =
            fixture_store(&Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/snapshot.json"))
                .expect("fixture snapshot decodes");
        AppState::for_bench(store, Arc::new(Metrics::default()), SharedTerminal::new())
    }

    #[test]
    fn cmd_k_opens_the_combined_palette_with_native_modifiers() {
        let mut state = state();
        let event = KeyEvent {
            key: rune::Key::Command('k'),
            mods: rune::Mods {
                cmd: true,
                logo: true,
                ..rune::Mods::default()
            },
        };
        assert!(state.global_key(&event));
        assert!(state.palette.open);
        assert_eq!(state.palette.scope(), crate::palette::Scope::Everything);
    }

    #[test]
    fn ctrl_tab_holds_the_switcher_and_control_up_commits_it() {
        let mut state = state();
        let ctrl = rune::Mods {
            ctrl: true,
            ..rune::Mods::default()
        };
        let tab = KeyEvent {
            key: rune::Key::Tab,
            mods: ctrl,
        };
        state.global_key(&tab);
        state.global_key(&tab);
        assert!(state.tabs.shown_gesture().is_some());
        state.terminal_modifiers(rune::Mods::default(), &mut rune::Tx::default());
        assert!(state.tabs.gesture.is_none());
        assert!(state.active.is_some(), "the committed tab stays active");
    }

    #[test]
    fn a_closed_strip_order_keeps_the_active_session_and_sorts_by_remembered_ids() {
        let mut state = state();
        let workspace = state.workspaces[0].id;
        let ids: Vec<SessionId> = state.tab_rows().iter().map(|row| row.id).collect();
        if ids.len() < 2 {
            return;
        }
        let active = state.active.expect("an active session");
        let mut moved = ids.clone();
        moved.swap(0, 1);
        state.tabs.set_order(workspace, moved.clone());
        let after: Vec<SessionId> = state.tab_rows().iter().map(|row| row.id).collect();
        assert_eq!(after, moved);
        assert_eq!(
            state.active,
            Some(active),
            "reordering leaves the active tab"
        );
    }

    #[test]
    fn a_tab_order_is_stored_under_the_original_client_key() {
        let mut state = state();
        let workspace = state.workspaces[0].id;
        state.tabs.set_order(workspace, vec![state.rows[0].id]);
        state.persist_tab_order(workspace);
        let stored = crate::tabs::parse_order(state.pref(crate::tabs::TAB_ORDER_KEY));
        assert_eq!(
            stored.get(&workspace.to_string()),
            Some(&vec![state.rows[0].id])
        );
    }

    #[test]
    fn only_the_active_sessions_terminal_may_set_the_clipboard() {
        let state = state();
        let active = state.active.expect("an active session");
        let store = state.store.as_ref().expect("store");
        let terminal_of = |pick: &dyn Fn(&domain::Session) -> bool| {
            store
                .sessions
                .iter()
                .find(|session| pick(session) && session.terminal_id.is_some())
                .and_then(|session| session.terminal_id)
                .expect("a session with a terminal")
        };
        assert!(state.shows_terminal(terminal_of(&|session| session.id == active)));
        assert!(!state.shows_terminal(terminal_of(&|session| session.id != active)));
    }

    #[test]
    fn a_stale_generation_never_mutates_the_replica() {
        let mut state = state();
        let mut tx = Tx::default();
        let before = state.rows[0].title.clone();
        state.apply_events(
            vec![
                UiEvent::Connecting {
                    generation: 1,
                    detail: "one".to_string(),
                },
                UiEvent::Connected {
                    generation: 1,
                    detail: "one".to_string(),
                    editor_surface: client::EditorSurface::Cells,
                },
            ],
            &mut tx,
        );
        assert_eq!(state.generation, 1);

        let mut updated = state.store.as_ref().expect("store").sessions[0].clone();
        updated.title.user = Some("from the old generation".to_string());
        state.apply_events(
            vec![
                UiEvent::Connecting {
                    generation: 2,
                    detail: "two".to_string(),
                },
                UiEvent::Daemon {
                    generation: 1,
                    event: Box::new(client::DaemonEvent::SessionUpdated(updated)),
                },
                UiEvent::CommandResult {
                    generation: 1,
                    label: "RenameSession",
                    ok: true,
                    message: "old".to_string(),
                    took_us: 1,
                },
            ],
            &mut tx,
        );
        assert_eq!(state.generation, 2);
        assert_eq!(state.rows[0].title, before);
        assert!(state.last_command.is_none());
        assert!(
            state
                .metrics
                .stale_events
                .load(std::sync::atomic::Ordering::Relaxed)
                >= 2
        );
    }

    #[test]
    fn a_snapshot_keeps_the_active_session_when_it_survives() {
        let mut state = state();
        let second = state.rows[1].id;
        state.select(second);
        let store = state.store.take().expect("store");
        state.set_store(store);
        assert_eq!(state.active, Some(second));
        assert_eq!(
            state.active_text.as_deref(),
            Some(state.rows[1].id_text.as_str())
        );
    }

    #[test]
    fn selecting_a_session_focuses_the_terminal() {
        let mut state = state();
        let second = state.rows[1].id;
        state.select(second);
        assert!(state.terminal.focus_terminal);
    }

    #[test]
    fn a_fixture_activation_publishes_one_grid_and_keeps_the_old_one() {
        let mut state = state();
        let first = state.rows[0].clone();
        state.generation = 1;
        state
            .terminal
            .activate_fixture(1, first.id, first.terminal_id.unwrap(), (80, 24));
        assert_eq!(state.terminal.shared.meta().terminal_id, first.terminal_id);
        let second = state.rows[1].clone();
        state
            .terminal
            .activate_fixture(1, second.id, second.terminal_id.unwrap(), (80, 24));
        assert_eq!(state.terminal.shared.meta().terminal_id, second.terminal_id);
    }

    #[test]
    fn a_restored_pane_sizes_its_pty_without_a_click() {
        let shared = SharedTerminal::new();
        let (mut state, terminal) = live_state_with_grid(&shared);
        let rx = {
            let (handle, rx) = ConnectionHandle::test_handle(Arc::new(Metrics::default()));
            state.connection = Some(handle);
            rx
        };
        shared.set_measured(138, 42);
        state.sync_terminal();
        match rx.try_recv() {
            Ok(crate::connection::Command::ResizeAndAttach {
                size, terminal: id, ..
            }) => {
                assert_eq!(id, terminal);
                assert_eq!((size.cols, size.rows), (138, 42));
            }
            other => panic!("expected ResizeAndAttach, got {other:?}"),
        }
    }

    fn live_state_with_grid(shared: &Arc<SharedTerminal>) -> (AppState, TerminalId) {
        let metrics = Arc::new(Metrics::default());
        let mut state = state();
        state.terminal = crate::terminal::pane::TerminalUi::new(Arc::clone(shared), false);
        state.generation = 1;
        state.status = LinkStatus::Connected;
        let (handle, _rx) = ConnectionHandle::test_handle(metrics);
        state.connection = Some(handle);
        let first = state.rows[0].clone();
        let terminal = first.terminal_id.unwrap();
        shared.publish_grid(
            1,
            first.id,
            terminal,
            crate::terminal::fixture::grid(80, 24, crate::terminal::fixture::ModeSet::Plain, true),
            domain::PtySize {
                cols: 80,
                rows: 24,
                pixel_width: 0,
                pixel_height: 0,
            },
        );
        state.terminal.attach_generation = 1;
        (state, terminal)
    }

    fn text(text: &str) -> KeyEvent {
        KeyEvent {
            key: rune::Key::Text(text.to_string()),
            mods: rune::Mods::default(),
        }
    }

    fn attaching_state(
        shared: &Arc<SharedTerminal>,
    ) -> (
        AppState,
        flume::Receiver<crate::connection::Command>,
        TerminalId,
    ) {
        let metrics = Arc::new(Metrics::default());
        let mut state = state();
        state.terminal = crate::terminal::pane::TerminalUi::new(Arc::clone(shared), false);
        state.generation = 1;
        state.status = LinkStatus::Connected;
        let (handle, rx) = ConnectionHandle::test_handle(metrics);
        state.connection = Some(handle);
        let first = state.rows[0].clone();
        let terminal = first.terminal_id.unwrap();
        state.terminal.begin_attach(1, terminal);
        state.terminal.attach_generation = 1;
        (state, rx, terminal)
    }

    #[test]
    fn input_before_snapshot_is_held_then_sent_once() {
        let shared = SharedTerminal::new();
        let (mut state, rx, terminal) = attaching_state(&shared);
        let mut tx = Tx::default();
        assert!(state.terminal_key(&text("a"), &mut tx));
        assert_eq!(state.terminal.attach.held_len(), 1);
        assert_eq!(state.terminal.attach.held_bytes(), 17);
        assert!(
            matches!(rx.try_recv(), Err(flume::TryRecvError::Empty)),
            "no socket write before the snapshot"
        );

        let first = state.rows[0].clone();
        shared.publish_grid(
            1,
            first.id,
            terminal,
            crate::terminal::fixture::grid(80, 24, crate::terminal::fixture::ModeSet::Plain, true),
            domain::PtySize {
                cols: 80,
                rows: 24,
                pixel_width: 0,
                pixel_height: 0,
            },
        );
        shared.set_measured(80, 24);
        state.sync_terminal();
        assert!(state.terminal.is_live());
        assert_eq!(state.terminal.attach.held_len(), 0);
        match rx.try_recv() {
            Ok(crate::connection::Command::Input {
                generation,
                terminal: id,
                bytes,
            }) => {
                assert_eq!(generation, 1);
                assert_eq!(id, terminal);
                assert_eq!(bytes, b"a");
            }
            other => panic!("expected one Input, got {other:?}"),
        }
        assert!(matches!(rx.try_recv(), Err(flume::TryRecvError::Empty)));
    }

    #[test]
    fn held_input_is_released_on_the_first_fresh_measure_after_resync() {
        let shared = SharedTerminal::new();
        let (mut state, rx, terminal) = attaching_state(&shared);
        let first = state.rows[0].clone();
        shared.publish_grid(
            1,
            first.id,
            terminal,
            crate::terminal::fixture::grid(80, 24, crate::terminal::fixture::ModeSet::Plain, true),
            domain::PtySize {
                cols: 80,
                rows: 24,
                pixel_width: 0,
                pixel_height: 0,
            },
        );
        shared.set_measured(80, 24);
        state.sync_terminal();
        assert!(state.terminal.is_live());
        let _ = rx.try_recv();

        let snapshot = crate::terminal::fixture::snapshot(
            80,
            24,
            crate::terminal::fixture::ModeSet::Plain,
            true,
        );
        shared.resync(1, terminal, &snapshot);
        state.sync_terminal();
        assert!(
            !state.terminal.is_live(),
            "resync waits for a fresh measure"
        );
        let mut tx = Tx::default();
        assert!(state.terminal_key(&text("x"), &mut tx));
        assert_eq!(state.terminal.attach.held_len(), 1);
        assert!(matches!(rx.try_recv(), Err(flume::TryRecvError::Empty)));

        shared.set_measured(80, 24);
        state.sync_terminal();
        assert!(state.terminal.is_live());
        match rx.try_recv() {
            Ok(crate::connection::Command::Input { bytes, .. }) => assert_eq!(bytes, b"x"),
            other => panic!("expected resync release Input, got {other:?}"),
        }
    }

    #[test]
    fn disconnect_drops_held_input_and_never_replays_it() {
        let shared = SharedTerminal::new();
        let (mut state, rx, _terminal) = attaching_state(&shared);
        let mut tx = Tx::default();
        assert!(state.terminal_key(&text("lost"), &mut tx));
        assert_eq!(state.terminal.attach.held_len(), 1);
        state.apply_events(
            vec![UiEvent::Disconnected {
                generation: 1,
                reason: "test disconnect".to_string(),
            }],
            &mut tx,
        );
        assert_eq!(state.terminal.attach.held_len(), 0);
        assert_eq!(state.terminal.attach.held_bytes(), 0);
        assert!(!state.terminal.is_live());
        assert!(matches!(rx.try_recv(), Err(flume::TryRecvError::Empty)));
    }

    #[test]
    fn a_terminal_switch_drops_held_input() {
        let shared = SharedTerminal::new();
        let (mut state, _rx, _terminal) = attaching_state(&shared);
        let mut tx = Tx::default();
        assert!(state.terminal_key(&text("old"), &mut tx));
        assert_eq!(state.terminal.attach.held_len(), 1);
        state.select(state.rows[1].id);
        assert_eq!(state.terminal.attach.held_len(), 0);
    }

    #[test]
    fn after_reconnect_new_input_works() {
        let shared = SharedTerminal::new();
        let (mut state, rx, terminal) = attaching_state(&shared);
        let mut tx = Tx::default();
        assert!(state.terminal_key(&text("old"), &mut tx));
        assert_eq!(state.terminal.attach.held_len(), 1);

        state.apply_events(
            vec![
                UiEvent::Connecting {
                    generation: 2,
                    detail: "reconnect".to_string(),
                },
                UiEvent::Connected {
                    generation: 2,
                    detail: "connected".to_string(),
                    editor_surface: client::EditorSurface::Cells,
                },
            ],
            &mut tx,
        );
        assert_eq!(state.terminal.attach.held_len(), 0);

        let first = state.rows[0].clone();
        shared.publish_grid(
            2,
            first.id,
            terminal,
            crate::terminal::fixture::grid(80, 24, crate::terminal::fixture::ModeSet::Plain, true),
            domain::PtySize {
                cols: 80,
                rows: 24,
                pixel_width: 0,
                pixel_height: 0,
            },
        );
        shared.set_measured(80, 24);
        state.sync_terminal();
        assert!(state.terminal.is_live());

        let mut commands = Vec::new();
        while let Ok(command) = rx.try_recv() {
            commands.push(command);
        }
        assert!(
            commands.iter().any(|command| matches!(
                command,
                crate::connection::Command::Activate { generation: 2, .. }
            )),
            "the new generation re-attaches: {commands:?}"
        );
        assert!(
            !commands.iter().any(|command| matches!(
                command,
                crate::connection::Command::Input { bytes, .. } if bytes == b"old"
            )),
            "old held input must never be replayed: {commands:?}"
        );

        assert!(state.terminal_key(&text("n"), &mut tx));
        match rx.try_recv() {
            Ok(crate::connection::Command::Input {
                generation, bytes, ..
            }) => {
                assert_eq!(generation, 2);
                assert_eq!(bytes, b"n");
            }
            other => panic!("expected new-generation Input, got {other:?}"),
        }
    }

    #[test]
    fn a_measured_pane_sends_resize_and_attach() {
        let shared = SharedTerminal::new();
        let (mut state, terminal) = live_state_with_grid(&shared);
        let rx = {
            let (handle, rx) = ConnectionHandle::test_handle(Arc::new(Metrics::default()));
            state.connection = Some(handle);
            rx
        };
        shared.set_measured(120, 40);
        state.sync_terminal();
        match rx.try_recv() {
            Ok(crate::connection::Command::ResizeAndAttach {
                size, terminal: id, ..
            }) => {
                assert_eq!(id, terminal);
                assert_eq!((size.cols, size.rows), (120, 40));
            }
            other => panic!("expected ResizeAndAttach, got {other:?}"),
        }
    }

    #[test]
    fn an_unmeasured_pane_never_sends_resize() {
        let shared = SharedTerminal::new();
        let (mut state, _) = live_state_with_grid(&shared);
        let rx = {
            let (handle, rx) = ConnectionHandle::test_handle(Arc::new(Metrics::default()));
            state.connection = Some(handle);
            rx
        };
        state.sync_terminal();
        assert!(matches!(rx.try_recv(), Err(flume::TryRecvError::Empty)));
    }

    #[test]
    fn the_focused_terminal_selection_copies_through_the_outbox() {
        let mut state = state();
        state.generation = 1;
        let first = state.rows[0].clone();
        state
            .terminal
            .activate_fixture(1, first.id, first.terminal_id.unwrap(), (80, 24));
        state.terminal.shared.set_measured(80, 24);
        state.sync_terminal();
        assert!(state.terminal.is_live());
        state.terminal.selection = Some(crate::terminal::selection::Selection::new(
            (1, {
                let slot = state.terminal.shared.lock();
                slot.grid
                    .as_ref()
                    .map_or(0, |grid| grid.scrollback_len as i64)
            }),
            crate::terminal::selection::Granularity::Cell,
        ));
        let mut tx = Tx::default();
        state.terminal_key(
            &KeyEvent {
                key: rune::Key::Command('c'),
                mods: rune::Mods {
                    cmd: true,
                    ..rune::Mods::default()
                },
            },
            &mut tx,
        );
        assert!(state.terminal.selection.is_some());
    }

    fn strip_row(
        workspace: domain::WorkspaceId,
        kind: SessionKind,
        live: bool,
    ) -> super::SessionRow {
        let id = domain::SessionId::new();
        super::SessionRow {
            id,
            kind,
            agent: kind == SessionKind::Agent,
            provider: None,
            terminal_id: live.then(domain::TerminalId::new),
            id_text: id.to_string(),
            title: String::new(),
            workspace_id: workspace,
            status: None,
            running: live,
            ended: None,
            file: None,
        }
    }

    #[test]
    fn the_strip_shows_one_checkout_as_session_scope_does() {
        use super::{rows_in_strip, strip_workspace};
        let (a, b) = (domain::WorkspaceId::new(), domain::WorkspaceId::new());
        let rows = vec![
            strip_row(a, SessionKind::Shell, true),
            strip_row(b, SessionKind::Shell, true),
            strip_row(a, SessionKind::Agent, false),
            strip_row(a, SessionKind::Editor, false),
        ];
        let ids = |workspace| {
            rows_in_strip(&rows, workspace)
                .iter()
                .map(|row| row.id)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            ids(Some(a)),
            vec![rows[0].id, rows[3].id],
            "another worktree's sessions and a session without a terminal stay out"
        );
        assert!(
            ids(None).is_empty(),
            "no checkout is no tabs, never all of them"
        );
        assert_eq!(strip_workspace(Some(b), Some(a), Some(a)), Some(b));
        assert_eq!(strip_workspace(None, Some(a), Some(b)), Some(a));
        assert_eq!(strip_workspace(None, None, Some(b)), Some(b));
    }

    #[test]
    fn a_hidden_or_missing_agent_is_not_offered() {
        use crate::test_support::provider;
        let mut state = state();
        let store = state.store.as_mut().expect("store");
        store.providers = vec![
            provider("claude", true, true, true),
            provider("cursor", true, true, false),
            provider("opencode", false, true, false),
        ];
        store
            .app_state
            .push((crate::launch::visibility_key("cursor"), "false".into()));
        let offered: Vec<String> = state
            .installed_agents()
            .into_iter()
            .map(|row| row.provider.to_string())
            .collect();
        assert_eq!(
            offered,
            vec!["claude"],
            "cursor is hidden in Settings › Agents and opencode is not installed"
        );
    }

    #[test]
    fn shells_lead_the_strip_and_an_editor_is_not_one() {
        let a = domain::WorkspaceId::new();
        let rows = vec![
            strip_row(a, SessionKind::Agent, true),
            strip_row(a, SessionKind::Shell, true),
            strip_row(a, SessionKind::Editor, true),
            strip_row(a, SessionKind::Shell, true),
            strip_row(a, SessionKind::Agent, true),
        ];
        let order: Vec<_> = super::rows_in_strip(&rows, Some(a))
            .iter()
            .map(|row| row.id)
            .collect();
        assert_eq!(
            order,
            vec![rows[1].id, rows[3].id, rows[0].id, rows[2].id, rows[4].id],
            "shells first, then the rest in creation order; a new agent goes last"
        );
    }

    #[test]
    fn close_reaches_a_session_only_while_its_terminal_is_shown() {
        assert!(super::closes_session(CenterMode::Session));
        for mode in [
            CenterMode::Settings,
            CenterMode::Changes,
            CenterMode::Files,
            CenterMode::Projects,
            CenterMode::Compare,
            CenterMode::PrDetail,
            CenterMode::PrCompose,
        ] {
            assert!(!super::closes_session(mode), "{mode:?}");
        }
    }

    #[test]
    fn markers_follow_session_work() {
        let state = state();
        let mut session = state.store.as_ref().expect("store").sessions[0].clone();
        session.state = domain::SessionState::Running;
        session.kind = SessionKind::Agent;
        session.agent_provider_id = Some(domain::AgentProviderId::new("claude"));
        let with = |session: &domain::Session, activity: ActivityState, evidence| {
            let mut session = session.clone();
            session.activity.state = activity;
            session.activity.evidence = evidence;
            super::row_status(&session)
        };
        let hook = Some(domain::ActivityEvidence::Hook);
        let input = Some(domain::ActivityEvidence::Input);
        assert_eq!(
            with(&session, ActivityState::Working, hook),
            Some(SessionStatus::Busy)
        );
        assert_eq!(
            with(&session, ActivityState::Working, input),
            None,
            "a keystroke into an idle agent is not a turn"
        );
        assert_eq!(
            with(&session, ActivityState::Waiting, hook),
            Some(SessionStatus::Attention)
        );
        let mut shell = session.clone();
        shell.kind = SessionKind::Shell;
        shell.agent_provider_id = None;
        assert_eq!(
            with(&shell, ActivityState::Working, hook),
            None,
            "a shell is never busy"
        );
        let mut editor = shell.clone();
        editor.kind = SessionKind::Editor;
        assert_eq!(with(&editor, ActivityState::Working, hook), None);
        assert_eq!(
            with(&editor, ActivityState::Waiting, hook),
            Some(SessionStatus::Attention),
            "waiting for the person outranks the kind"
        );
        session.state = domain::SessionState::Exited {
            code: Some(0),
            signal: None,
        };
        assert_eq!(with(&session, ActivityState::Waiting, hook), None);
    }
}
