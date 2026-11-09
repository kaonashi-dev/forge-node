//! The UI-thread replica: shell state plus the terminal's local view state.
//! Terminal deltas arrive through `SharedTerminal`; only shell events use
//! `AppHandle::update`, so a frame of output never rebuilds the sidebar.

use std::cell::Cell;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use client::Store;
use domain::{ActivityState, SessionId, SessionKind, SessionState, TerminalId, WorkspaceId};
use rune::{Ime, KeyEvent, Tx};
use rune_kit::shell::{PaneState, TabDragState};

use crate::connection::{ConnectionHandle, Metrics, UiEvent};
use crate::editor::EditorUi;
use crate::terminal::SharedTerminal;
use crate::terminal::input::{self, Effects, Outbox, Pointer, Press};
use crate::terminal::pane::TerminalUi;
use crate::terminal::renderer::CELL_PT;
use crate::workbench::{CenterMode, Workbench};

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
    /// `None` for an editor the daemon presents as lines, whose PTY carries
    /// no screen this side should paint.
    pub terminal_id: Option<TerminalId>,
    pub id_text: String,
    pub title: String,
    pub workspace: String,
    pub detail: String,
    pub status: Option<SessionStatus>,
    pub running: bool,
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
    pub tabs: TabDragState,
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
    pub editor_surface: protocol::EditorSurface,
    pub editor: EditorUi,
    pub workbench: Workbench,
    pub palette: crate::palette::Palette,
    pub projects: crate::projects::Projects,
    pub commit: crate::commit::CommitDialog,
    pub sync: crate::sync::Sync,
    /// What a text field edits while its dialog is closing; never shown.
    pub scratch_edit: rune::TextEdit,
    /// The session being renamed and the name being typed.
    pub rename: Option<(SessionId, rune::TextEdit)>,
    /// Shows the client's probes (socket counts, Refresh, Reconnect): on for
    /// a fixture, or with `FORGE_UI_DEBUG` set.
    pub debug: bool,
    /// A workspace picked in Projects; Changes and Files show it instead of
    /// the active session's until a session is selected.
    pub chosen_workspace: Option<WorkspaceId>,
    /// `ui.theme_base` from the daemon, shared with Tauri: a `tokens.ts`
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
            sidebar: PaneState::new(276.0).range(180.0, 480.0),
            tabs: TabDragState::new(),
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
            editor_surface: protocol::EditorSurface::Cells,
            editor: EditorUi::default(),
            workbench: Workbench::default(),
            palette: crate::palette::Palette::default(),
            projects: crate::projects::Projects::default(),
            commit: crate::commit::CommitDialog::default(),
            sync: crate::sync::Sync::default(),
            scratch_edit: rune::TextEdit::new(""),
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
        if self.active == Some(id) {
            self.terminal.focus_terminal = true;
            self.terminal.interacted = true;
            return;
        }
        self.active = Some(id);
        self.active_text = self
            .rows
            .iter()
            .find(|row| row.id == id)
            .map(|row| row.id_text.clone());
        self.terminal.focus_terminal = true;
        self.terminal.interacted = true;
        match self.active_row().and_then(|row| row.terminal_id) {
            Some(target) => self.terminal.begin_attach(self.generation, target),
            None => self.terminal.on_disconnect(),
        }
        self.switch_started.set(Some(Instant::now()));
    }

    fn set_store(&mut self, store: Store) {
        self.rows = store
            .sessions
            .iter()
            .map(|session| {
                let workspace = store
                    .workspaces
                    .iter()
                    .find(|workspace| workspace.id == session.workspace_id)
                    .map(|workspace| workspace.label())
                    .unwrap_or_else(|| "unknown workspace".to_string());
                let lines = session.kind == SessionKind::Editor
                    && self.editor_surface == protocol::EditorSurface::Dom;
                SessionRow {
                    id: session.id,
                    kind: session.kind,
                    terminal_id: session.terminal_id.filter(|_| !lines),
                    id_text: session.id.to_string(),
                    title: session
                        .title
                        .resolve(session.editor.as_ref().map_or("untitled", |editor| {
                            editor.path.rsplit('/').next().unwrap_or(&editor.path)
                        }))
                        .to_string(),
                    workspace,
                    detail: format!(
                        "{} · {}",
                        kind_label(session.kind),
                        state_label(&session.state)
                    ),
                    status: row_status(session),
                    running: session.state.is_active(),
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
        self.store = Some(store);
        self.terminal.set_fixture_titles(
            self.fixture_titles
                .iter()
                .map(|(terminal, title)| (*terminal, title.clone())),
        );
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
        }
        if let Some(text) = self.pending_copy.take() {
            tx.copy(text);
        }
        if let Some(offset) = self.editor_reveal.take() {
            let [_, viewport] = self.editor.scroll.get();
            self.editor.scroll.set([offset, viewport]);
            tx.scroll_to_instant(crate::editor::SCROLL_KEY, 0.0, offset);
        }
    }

    /// The active session, when it is an editor this side paints as lines.
    #[must_use]
    pub fn active_line_editor(&self) -> Option<SessionId> {
        self.active_row()
            .filter(|row| {
                row.kind == SessionKind::Editor
                    && self.editor_surface == protocol::EditorSurface::Dom
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

    pub fn editor_key(&mut self, ev: &KeyEvent, _tx: &mut Tx) -> bool {
        if self.global_key(ev) {
            return true;
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
        let (Some(connection), Some(workspace)) = (&self.connection, self.workbench.workspace)
        else {
            return;
        };
        if connection.open_editor(workspace, path.to_string()).is_ok() {
            self.workbench.show(CenterMode::Session);
        }
    }

    /// An app chord, taken before any surface sees the key.
    pub fn global_key(&mut self, ev: &KeyEvent) -> bool {
        use crate::keymap::Action;
        let Some(action) = crate::keymap::action_for(ev) else {
            return false;
        };
        match action {
            Action::CommandPalette => {
                let agents = self.installed_agents();
                let actions = self.palette_actions();
                self.palette.show_with(false, "", agents);
                self.palette.set_actions(actions);
            }
            Action::FilePalette => self.palette.show(true),
            Action::NewShell => self.request_create(),
            Action::NewAgent => self.open_launch_palette(),
            Action::Settings => {
                let next = if self.workbench.mode == CenterMode::Settings {
                    CenterMode::Session
                } else {
                    CenterMode::Settings
                };
                self.show_mode(next);
            }
            Action::NextSession => self.step_session(1),
            Action::PreviousSession => self.step_session(-1),
        }
        true
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

    /// Providers the daemon found installed, for the palette's agent entries.
    fn installed_agents(&self) -> Vec<(domain::AgentProviderId, String)> {
        let Some(store) = &self.store else {
            return Vec::new();
        };
        store
            .providers
            .iter()
            .filter(|info| {
                matches!(
                    info.detection.status,
                    domain::DetectionStatus::Installed { .. }
                )
            })
            .map(|info| {
                (
                    info.descriptor.id.clone(),
                    info.descriptor.display_name.clone(),
                )
            })
            .collect()
    }

    fn step_session(&mut self, delta: isize) {
        let count = self.rows.len();
        if count == 0 {
            return;
        }
        let at = self
            .active
            .and_then(|id| self.rows.iter().position(|row| row.id == id))
            .unwrap_or(0);
        let next = (at as isize + delta).rem_euclid(count as isize) as usize;
        let id = self.rows[next].id;
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
                Command::Show(mode) => self.show_mode(mode),
                Command::Theme(base) => self.choose_theme(base),
                Command::RunAction(action) => {
                    self.sync_workbench_workspace();
                    if let Some(workspace) = self.workbench.workspace {
                        self.run_action(workspace, action);
                    }
                }
                Command::NewAgent(provider) => {
                    self.sync_workbench_workspace();
                    if let (Some(connection), Some(workspace)) =
                        (&self.connection, self.workbench.workspace)
                    {
                        let _ = connection.create_agent(workspace, provider, None);
                        self.workbench.show(CenterMode::Session);
                    }
                }
            },
            Entry::File(path) => {
                self.sync_workbench_workspace();
                if let (Some(connection), Some(workspace)) =
                    (&self.connection, self.workbench.workspace)
                    && connection.open_editor(workspace, path).is_ok()
                {
                    self.workbench.show(CenterMode::Session);
                }
            }
        }
    }

    /// Opens the commit dialog for the workspace Changes shows, and asks Juva
    /// for a draft.
    pub fn start_commit(&mut self) {
        let Some(workspace) = self.workbench.workspace else {
            return;
        };
        self.commit.open(workspace);
        match &self.connection {
            Some(connection) => {
                let _ = connection.draft_commit(workspace);
            }
            None => self.worker_not_ready("DraftWithJuva"),
        }
    }

    pub fn create_commit(&mut self) {
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
        if let Some((_, workspace, branch)) = self.workbench_project() {
            self.sync.open_pr(workspace, branch.as_deref());
        }
    }

    pub fn create_pull_request(&mut self) {
        let Some(pr) = self.sync.pr.as_mut() else {
            return;
        };
        let Some((title, body)) = pr.ready() else {
            return;
        };
        pr.opening = true;
        pr.outcome = None;
        let workspace = pr.workspace;
        match &self.connection {
            Some(connection) => {
                let _ = connection.open_pull_request(workspace, title, body);
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

    /// Hands the stopped operation to the first installed agent, with the
    /// shared prompt; the daemon launches it like any other agent.
    pub fn resolve_with_agent(&mut self) {
        let Some(state) = self.workbench.changes.rebase.as_ref() else {
            return;
        };
        let prompt = domain::conflict_prompt::conflict_prompt(state);
        let Some((provider, _)) = self.installed_agents().into_iter().next() else {
            self.workbench.changes.rebase_error = Some("no agent is installed".into());
            return;
        };
        let (Some(connection), Some(workspace)) = (&self.connection, self.workbench.workspace)
        else {
            return;
        };
        if connection
            .create_agent(workspace, provider, Some(prompt))
            .is_ok()
        {
            self.workbench.show(CenterMode::Session);
        }
    }

    pub fn rebase_step(&mut self, step: crate::workbench::RebaseStep) {
        let (Some(connection), Some(workspace)) = (&self.connection, self.workbench.workspace)
        else {
            return;
        };
        let _ = connection.rebase_step(workspace, step);
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

    /// Shows Changes or Files for `workspace` rather than the active session's.
    pub fn inspect(&mut self, workspace: WorkspaceId, mode: CenterMode) {
        self.chosen_workspace = Some(workspace);
        self.show_mode(mode);
    }

    pub fn create_worktree(&mut self) {
        let Some(project) = self.projects.new_worktree.take() else {
            return;
        };
        let branch = self.projects.branch.text.trim().to_string();
        if branch.is_empty() {
            self.projects.new_worktree = Some(project);
            return;
        }
        match &self.connection {
            Some(connection) => {
                let _ = connection.create_worktree(project, branch);
            }
            None => self.worker_not_ready("CreateWorktree"),
        }
    }

    /// The first press arms, the second removes: a worktree is a checkout.
    pub fn remove_worktree(&mut self, workspace: WorkspaceId) {
        if self.projects.confirm_remove != Some(workspace) {
            self.projects.confirm_remove = Some(workspace);
            return;
        }
        self.projects.confirm_remove = None;
        match &self.connection {
            Some(connection) => {
                let _ = connection.remove_worktree(workspace);
            }
            None => self.worker_not_ready("RemoveWorktree"),
        }
    }

    /// Picks a base, or `"system"`, and saves it where Tauri keeps it.
    pub fn choose_theme(&mut self, value: String) {
        if let Some(connection) = &self.connection {
            let _ = connection.set_app_state(crate::theme::THEME_KEY, &value);
        }
        self.theme_pref = Some(value);
    }

    /// The base to paint with: the saved one, the system's pair for
    /// `"system"`, and calm-dark when nothing is saved, as in Tauri.
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
    }

    /// The workspace the active session runs in, or the first one.
    fn sync_workbench_workspace(&mut self) {
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

    fn sync_workbench(&mut self) {
        if let Some(query) = self.palette.take_search() {
            self.sync_workbench_workspace();
            if let (Some(connection), Some(workspace)) =
                (&self.connection, self.workbench.workspace)
            {
                let _ = connection.read(
                    self.generation,
                    workspace,
                    crate::workbench::Read::Search(query),
                );
            }
        }
        if self.workbench.mode == CenterMode::Session {
            return;
        }
        self.sync_workbench_workspace();
        let reads = self.workbench.take_reads();
        let (Some(connection), Some(workspace)) = (&self.connection, self.workbench.workspace)
        else {
            return;
        };
        for read in reads {
            let _ = connection.read(self.generation, workspace, read);
        }
    }

    fn sync_editor(&mut self) {
        self.editor.set_session(self.active_line_editor());
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
                true
            }
            UiEvent::Disconnected { generation, reason } => {
                if !self.accept_generation(generation) {
                    return false;
                }
                self.status = LinkStatus::Reconnecting;
                self.status_detail = reason;
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
                self.status = LinkStatus::Connected;
                true
            }
            UiEvent::Daemon { generation, event } => {
                if !self.accept_generation(generation) {
                    return false;
                }
                if let protocol::DaemonEvent::ClipboardStore { terminal_id, text } = event.as_ref()
                {
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
                if let protocol::DaemonEvent::EditorFrame { session_id, frame } = *event {
                    if let Some(offset) = self.editor.apply_frame(session_id, frame) {
                        self.editor_reveal = Some(offset);
                    }
                    return true;
                }
                match event.as_ref() {
                    protocol::DaemonEvent::RemoteRefsUpdated {
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
                    protocol::DaemonEvent::PullRequestOpened {
                        workspace_id,
                        url,
                        error,
                    } => {
                        self.sync
                            .pr_opened(*workspace_id, url.clone(), error.clone());
                        return true;
                    }
                    protocol::DaemonEvent::PullRequestsUpdated { .. } => return true,
                    _ => {}
                }
                if let protocol::DaemonEvent::JuvaDraftReady {
                    workspace_id,
                    draft,
                    fell_back,
                } = event.as_ref()
                {
                    self.commit.apply_draft(*workspace_id, draft, *fell_back);
                    return true;
                }
                let reset = matches!(event.as_ref(), protocol::DaemonEvent::FactoryReset);
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
                    protocol::DaemonEvent::SessionCreated(_)
                        | protocol::DaemonEvent::SessionUpdated(_)
                        | protocol::DaemonEvent::SessionRemoved { .. }
                        | protocol::DaemonEvent::ProjectAdded(_)
                        | protocol::DaemonEvent::ProjectUpdated(_)
                        | protocol::DaemonEvent::ProjectRemoved { .. }
                        | protocol::DaemonEvent::WorkspaceCreated(_)
                        | protocol::DaemonEvent::WorkspaceUpdated(_)
                        | protocol::DaemonEvent::WorkspaceRemoved { .. }
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
                rebuild
            }
            UiEvent::Diff {
                generation,
                workspace_id,
                result,
            } => {
                if !self.accept_generation(generation) {
                    return false;
                }
                self.workbench
                    .apply_diff(workspace_id, result.map(|diff| *diff));
                true
            }
            UiEvent::Directory {
                generation,
                workspace_id,
                path,
                result,
            } => {
                if !self.accept_generation(generation) {
                    return false;
                }
                self.workbench.apply_directory(workspace_id, &path, result);
                true
            }
            UiEvent::File {
                generation,
                workspace_id,
                result,
            } => {
                if !self.accept_generation(generation) {
                    return false;
                }
                self.workbench
                    .apply_file(workspace_id, result.map(|file| *file));
                true
            }
            UiEvent::Search {
                generation,
                query,
                result,
            } => {
                if !self.accept_generation(generation) {
                    return false;
                }
                if let Ok(paths) = result {
                    self.palette.apply_search(&query, paths);
                }
                true
            }
            UiEvent::Rebase {
                generation,
                workspace_id,
                result,
            } => {
                if !self.accept_generation(generation) {
                    return false;
                }
                let finished = self.workbench.changes.rebase.is_some()
                    && result.as_ref().is_ok_and(|state| state.operation.is_none());
                self.workbench
                    .apply_rebase(workspace_id, result.map(|state| *state));
                if finished {
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
                if label == "CreateCommit" {
                    self.commit.committing = false;
                    if ok {
                        self.commit.close();
                        self.workbench.refresh();
                    } else {
                        self.commit.error = Some(message.clone());
                    }
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
                if label == "DraftWithJuva" && !ok {
                    self.commit.drafting = false;
                    self.commit.error = Some(message.clone());
                }
                self.last_command = Some(CommandStatus {
                    label,
                    ok,
                    message,
                    took_us,
                });
                true
            }
        }
    }

    /// The resize gate: attach is always safe, `ResizeTerminal` only for a pane
    /// that has been measured **and** the user has interacted with, and it is
    /// always followed by an `AttachTerminal` from the worker. Attach state is
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
        self.refresh_attach_state();
        if !self.terminal.is_live() {
            return;
        }
        let meta = self.terminal.shared.meta();
        let Some(measured) = measured else {
            return;
        };
        if !self.terminal.interacted {
            return;
        }
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
    fn refresh_attach_state(&mut self) {
        let Some(target) = self.active_row().and_then(|row| row.terminal_id) else {
            return;
        };
        let meta = self.terminal.shared.meta();
        let measured = self.terminal.shared.measured();
        if meta.generation == self.generation && meta.terminal_id == Some(target) && meta.has_grid {
            let size = meta
                .size
                .map(|size| (size.cols, size.rows))
                .or(measured)
                .unwrap_or((80, 24));
            self.terminal.note_snapshot(self.generation, target, size);
        }
        self.terminal.note_measured_if_fresh();
        if self.terminal.is_live() {
            self.flush_held_input();
        }
    }

    fn flush_held_input(&mut self) {
        let held = self.terminal.take_held();
        if held.is_empty() {
            return;
        }
        let shared = Arc::clone(&self.terminal.shared);
        let slot = shared.lock();
        let Some(grid) = slot.grid.as_ref() else {
            eprintln!("[hold] dropped {} gesture(s): no grid", held.len());
            return;
        };
        let (mut gestures, mut bytes) = (0usize, 0u64);
        for event in held {
            let before = self.out.sent_bytes;
            match event {
                crate::terminal::attach::HeldInput::Key(ev) => {
                    let mut fx = Effects::default();
                    input::key(0, &mut self.terminal, grid, &ev, &mut self.out, &mut fx);
                }
                crate::terminal::attach::HeldInput::Paste(text) => {
                    input::paste_text(0, &mut self.terminal, grid, &text, &mut self.out);
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
        self.sync_editor();
        self.sync_workbench();
        let mut changed = false;
        if clock - self.mirror_at >= 0.125 {
            let version = self.terminal.shared.grid_epoch();
            let dims = {
                let slot = self.terminal.shared.lock();
                slot.grid.as_ref().map(|grid| {
                    (
                        grid.cursor,
                        grid.visible.first().map_or(0, |row| row.cells.len()) as u16,
                        grid.visible.len() as u16,
                    )
                })
            };
            if let Some((cursor, cols, rows)) = dims
                && (version != self.terminal.mirror_version
                    || self.terminal.scroll != self.terminal.mirror_scroll
                    || cursor != self.terminal.built_cursor)
            {
                self.terminal.refresh_mirror(version, cols, rows);
                changed = true;
            }
            self.mirror_at = clock;
        }
        let floor = if self.terminal.shared.meta().has_grid {
            0.100
        } else {
            0.016
        };
        tx.wake_after(Duration::from_secs_f64(floor));
        changed
    }

    pub fn flush_outbox(&mut self) {
        let Some(terminal_id) = self
            .active_row()
            .and_then(|row| row.terminal_id)
            .or_else(|| self.terminal.shared.meta().terminal_id)
        else {
            self.out = Outbox::default();
            return;
        };
        let ready = self.terminal.shared.meta().terminal_id == Some(terminal_id);
        for note in self.out.notes.drain(..) {
            eprintln!("[note] {note}");
        }
        for (_, request) in self.out.fetches.drain(..) {
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
                self.last_command = Some(CommandStatus {
                    label: "FetchScrollback",
                    ok: false,
                    message: error,
                    took_us: 0,
                });
            }
        }
        for (_, bytes, labels) in self.out.drain_batches() {
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
                self.last_command = Some(CommandStatus {
                    label: "WriteTerminalInput",
                    ok: false,
                    message: error,
                    took_us: 0,
                });
            }
        }
        self.terminal.request_frame();
    }

    pub fn terminal_key(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool {
        if self.global_key(ev) {
            return true;
        }
        if !self.session_shown() {
            return false;
        }
        self.observe_terminal_resync();
        self.last_focus = tx.focused().map(str::to_owned);
        self.terminal.interacted = true;
        self.terminal.focus_terminal = true;
        if !self.terminal.is_live() {
            if ev.mods.cmd {
                if ev.is_command('v') {
                    tx.request_paste();
                    return true;
                }
                return false;
            }
            self.refresh_attach_state();
            if !self.terminal.is_live() {
                if self.terminal.hold_key(ev) {
                    eprintln!(
                        "[hold] held {:?}{} ({}) — {} bytes, {} gestures",
                        ev.key,
                        input::mod_suffix(ev.mods),
                        self.terminal.attach.state_name(),
                        self.terminal.attach.held_bytes(),
                        self.terminal.attach.held_len()
                    );
                } else {
                    eprintln!(
                        "[hold] refused {:?}: 64 KiB hold buffer full ({} refused)",
                        ev.key,
                        self.terminal.attach.refused()
                    );
                }
                return true;
            }
        }
        let shared = Arc::clone(&self.terminal.shared);
        let slot = shared.lock();
        let Some(grid) = slot.grid.as_ref() else {
            return false;
        };
        let mut fx = Effects::default();
        let taken = input::key(0, &mut self.terminal, grid, ev, &mut self.out, &mut fx);
        drop(slot);
        if taken {
            self.apply_effects(fx, tx);
        }
        taken
    }

    fn observe_terminal_resync(&mut self) {
        self.terminal.observe_resync(
            self.terminal.shared.resync_epoch(),
            self.terminal.shared.measure_epoch(),
        );
    }

    pub fn terminal_pointer(&mut self, pointer: Pointer, tx: &mut Tx) {
        let shared = Arc::clone(&self.terminal.shared);
        let slot = shared.lock();
        let Some(grid) = slot.grid.as_ref() else {
            return;
        };
        self.terminal.interacted = true;
        self.terminal.focus_terminal = true;
        let mut fx = Effects::default();
        input::pointer(
            0,
            &mut self.terminal,
            grid,
            pointer,
            CELL_PT,
            &mut self.out,
            &mut fx,
        );
        drop(slot);
        self.apply_effects(fx, tx);
    }

    pub fn terminal_wheel(&mut self, ev: &rune::WheelEvent, tx: &mut Tx) -> bool {
        let shared = Arc::clone(&self.terminal.shared);
        let slot = shared.lock();
        let Some(grid) = slot.grid.as_ref() else {
            return false;
        };
        self.terminal.interacted = true;
        let taken = input::wheel(0, &mut self.terminal, grid, ev, CELL_PT, &mut self.out);
        drop(slot);
        if taken {
            self.flush_outbox();
            tx.live_only();
        }
        taken
    }

    pub fn terminal_secondary(&mut self, x: f32, y: f32, tx: &mut Tx) {
        let shared = Arc::clone(&self.terminal.shared);
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
        };
        let mut fx = Effects::default();
        input::secondary(
            0,
            &mut self.terminal,
            grid,
            pointer,
            CELL_PT,
            &mut self.out,
            &mut fx,
        );
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
        self.observe_terminal_resync();
        self.last_focus = tx.focused().map(str::to_owned);
        self.terminal.interacted = true;
        self.terminal.focus_terminal = true;
        if !self.terminal.is_live() {
            self.refresh_attach_state();
            if !self.terminal.is_live() {
                if self.terminal.hold_paste(text) {
                    eprintln!(
                        "[hold] held paste {} bytes ({} held total)",
                        text.len(),
                        self.terminal.attach.held_bytes()
                    );
                } else {
                    eprintln!(
                        "[hold] refused paste {} bytes: 64 KiB hold buffer full ({} refused)",
                        text.len(),
                        self.terminal.attach.refused()
                    );
                }
                return;
            }
        }
        let shared = Arc::clone(&self.terminal.shared);
        let slot = shared.lock();
        let Some(grid) = slot.grid.as_ref() else {
            return;
        };
        input::paste_text(0, &mut self.terminal, grid, text, &mut self.out);
        drop(slot);
        self.flush_outbox();
        tx.live_only();
    }

    pub fn terminal_ime(&mut self, ev: &Ime, tx: &mut Tx) -> bool {
        if !self.session_shown() {
            return false;
        }
        self.observe_terminal_resync();
        if !self.terminal.is_live() {
            match ev {
                Ime::Commit(text) => {
                    let clean: String = text.chars().filter(|c| !c.is_control()).collect();
                    if self.terminal.hold_paste(&clean) {
                        eprintln!(
                            "[hold] held IME commit {} bytes ({} held total)",
                            clean.len(),
                            self.terminal.attach.held_bytes()
                        );
                    } else {
                        eprintln!(
                            "[hold] refused IME commit: 64 KiB hold buffer full ({} refused)",
                            self.terminal.attach.refused()
                        );
                    }
                    return true;
                }
                Ime::Preedit(text, _) => {
                    self.terminal.preedit = text.chars().take(input::PREEDIT_MAX_CHARS).collect();
                    self.terminal.request_frame();
                    return true;
                }
                Ime::Enabled | Ime::Disabled => return true,
            }
        }
        let shared = Arc::clone(&self.terminal.shared);
        let slot = shared.lock();
        let Some(grid) = slot.grid.as_ref() else {
            return false;
        };
        let _ = grid;
        let taken = input::ime(0, &mut self.terminal, ev, &mut self.out);
        drop(slot);
        if taken {
            self.flush_outbox();
            tx.live_only();
        }
        taken
    }

    pub fn terminal_window_focus(&mut self, focused: bool, tx: &mut Tx) {
        let shared = Arc::clone(&self.terminal.shared);
        let slot = shared.lock();
        let Some(grid) = slot.grid.as_ref() else {
            return;
        };
        input::focus(0, &mut self.terminal, grid, focused, &mut self.out);
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
            self.last_command = Some(CommandStatus {
                label: "CreateShellSession",
                ok: false,
                message: "no workspace in the snapshot".to_string(),
                took_us: 0,
            });
            return;
        };
        match &self.connection {
            Some(connection) => {
                if let Err(error) = connection.create_shell_session(workspace_id) {
                    self.last_command = Some(CommandStatus {
                        label: "CreateShellSession",
                        ok: false,
                        message: error,
                        took_us: 0,
                    });
                }
            }
            None => self.worker_not_ready("CreateShellSession"),
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

    /// The palette scoped to what can be launched: a terminal or an agent.
    pub fn open_launch_palette(&mut self) {
        let agents = self.installed_agents();
        self.palette.show_launch(agents);
    }

    pub fn request_refresh(&mut self) {
        if let Some(connection) = &self.connection
            && let Err(error) = connection.refresh_snapshot()
        {
            self.last_command = Some(CommandStatus {
                label: "GetSnapshot",
                ok: false,
                message: error,
                took_us: 0,
            });
        }
    }

    pub fn request_reconnect(&mut self) {
        if let Some(connection) = &self.connection
            && let Err(error) = connection.force_reconnect()
        {
            self.last_command = Some(CommandStatus {
                label: "Reconnect",
                ok: false,
                message: error,
                took_us: 0,
            });
        }
    }

    fn worker_not_ready(&mut self, label: &'static str) {
        self.last_command = Some(CommandStatus {
            label,
            ok: false,
            message: "client worker is still starting".to_string(),
            took_us: 0,
        });
    }
}

pub fn poll(state: &mut AppState, clock: f64, tx: &mut Tx) -> bool {
    state.poll(clock, tx)
}

fn kind_label(kind: SessionKind) -> &'static str {
    match kind {
        SessionKind::Shell => "shell",
        SessionKind::Agent => "agent",
        SessionKind::Editor => "editor",
        SessionKind::Task => "task",
        _ => "session",
    }
}

fn state_label(state: &SessionState) -> &'static str {
    match state {
        SessionState::Starting => "starting",
        SessionState::Running => "running",
        SessionState::Exited { .. } => "exited",
        SessionState::Failed { .. } => "failed",
        SessionState::Orphaned => "orphaned",
        _ => "unknown",
    }
}

fn row_status(session: &domain::Session) -> Option<SessionStatus> {
    if session.state.is_terminal() {
        return None;
    }
    match session.activity.state {
        ActivityState::Working | ActivityState::Starting => Some(SessionStatus::Busy),
        ActivityState::Waiting => Some(SessionStatus::Attention),
        ActivityState::Idle | ActivityState::Unknown | ActivityState::Unrecognized => None,
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
                    editor_surface: protocol::EditorSurface::Cells,
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
                    event: Box::new(protocol::DaemonEvent::SessionUpdated(updated)),
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
    fn selecting_a_session_interacts_with_the_terminal() {
        let mut state = state();
        assert!(!state.terminal.interacted);
        let second = state.rows[1].id;
        state.select(second);
        assert!(state.terminal.interacted);
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
    fn the_resize_gate_does_not_send_anything_without_measurement() {
        let mut state = state();
        state.generation = 1;
        state.status = LinkStatus::Connected;
        let first = state.rows[0].clone();
        state
            .terminal
            .activate_fixture(1, first.id, first.terminal_id.unwrap(), (80, 24));
        state.terminal.interacted = false;
        state.terminal.shared.set_measured(120, 40);
        state.sync_terminal();
        assert!(state.terminal.requested.is_none());
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
        state.terminal.interacted = true;
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
        state.terminal.interacted = true;
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
                    editor_surface: protocol::EditorSurface::Cells,
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
    fn a_measured_interacted_pane_sends_resize_and_attach() {
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
}
