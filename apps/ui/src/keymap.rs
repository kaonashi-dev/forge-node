//! App-wide chords, matched before a terminal or an editor sees the key, so a
//! bound chord never leaks into a PTY. `BINDINGS` is the shipped table, in
//! Tauri's order; a person's `ui.keymap` overrides one action at a time and is
//! consulted first.

use rune::{Chord, ChordKey, ChordMod, Key, KeyEvent};

use crate::workbench::Panel;

/// Where the merged keymap is stored, alongside `ui.tab_order`.
pub const KEYMAP_KEY: &str = "ui.keymap";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    CommandPalette,
    /// ⇧⌘P: the palette narrowed to commands.
    CommandPaletteCommands,
    /// ⇧⌘O: the palette narrowed to sessions and checkouts.
    GoToPalette,
    FilePalette,
    NewShell,
    NewWorktree,
    CloseSession,
    NewAgent,
    Settings,
    ToggleSidebar,
    /// Shows a sidebar view, or hides the sidebar when it already shows it.
    TogglePanel(Panel),
    CycleViews,
    NextSession,
    PreviousSession,
    /// Ctrl-Tab: the MRU ring, held until Control comes up.
    SwitchTabNext,
    /// Ctrl-Shift-Tab.
    SwitchTabPrevious,
    /// The strip's Nth tab, 1-based.
    FocusTab(usize),
    FindInFiles,
    /// A terminal beside the active one.
    SplitPane,
    TerminalZoom(Zoom),
    ReopenClosedTab,
    PreviousCodeView,
    NextCodeView,
    CloseOtherViews,
    CloseViewsToRight,
    MoveTabLeft,
    MoveTabRight,
    GoToDefinition,
    GoBack,
    EditorZoomIn,
    EditorZoomOut,
    EditorZoomReset,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Zoom {
    In,
    Out,
    Reset,
}

impl Action {
    /// The stable name `ui.keymap` stores, the same ids Tauri writes.
    #[must_use]
    pub fn id(self) -> &'static str {
        match self {
            Action::CommandPalette => "open_command_palette",
            Action::CommandPaletteCommands => "find_command",
            Action::GoToPalette => "go_to",
            Action::FilePalette => "open_file_palette",
            Action::NewShell => "new_terminal",
            Action::NewWorktree => "new_worktree",
            Action::CloseSession => "close_session",
            Action::NewAgent => "new_agent",
            Action::Settings => "open_settings",
            Action::ToggleSidebar => "toggle_sidebar",
            Action::TogglePanel(Panel::Projects) => "toggle_projects",
            Action::TogglePanel(Panel::Files) => "toggle_files",
            Action::TogglePanel(Panel::History) => "toggle_history",
            Action::TogglePanel(Panel::PullRequests) => "toggle_pull_requests",
            Action::TogglePanel(Panel::Git) => "toggle_git",
            Action::TogglePanel(Panel::Processes) => "toggle_processes",
            Action::CycleViews => "cycle_sidebar_views",
            Action::NextSession => "next_session",
            Action::PreviousSession => "previous_session",
            Action::SwitchTabNext => "switch_tab_next",
            Action::SwitchTabPrevious => "switch_tab_previous",
            Action::FocusTab(_) => "focus_session",
            Action::FindInFiles => "find_in_project",
            Action::SplitPane => "split_pane",
            Action::TerminalZoom(Zoom::In) => "terminal_zoom_in",
            Action::TerminalZoom(Zoom::Out) => "terminal_zoom_out",
            Action::TerminalZoom(Zoom::Reset) => "terminal_zoom_reset",
            Action::ReopenClosedTab => "reopen_closed_tab",
            Action::PreviousCodeView => "previous_code_view",
            Action::NextCodeView => "next_code_view",
            Action::CloseOtherViews => "close_other_views",
            Action::CloseViewsToRight => "close_views_to_right",
            Action::MoveTabLeft => "move_tab_left",
            Action::MoveTabRight => "move_tab_right",
            Action::GoToDefinition => "go_to_definition",
            Action::GoBack => "go_back",
            Action::EditorZoomIn => "editor_zoom_in",
            Action::EditorZoomOut => "editor_zoom_out",
            Action::EditorZoomReset => "editor_zoom_reset",
        }
    }

    /// The tab number a `focus_session` chord carries, if any.
    #[must_use]
    pub fn argument(self) -> Option<usize> {
        match self {
            Action::FocusTab(index) => Some(index),
            _ => None,
        }
    }

    /// Where the chord belongs, for Settings › Keyboard's Context column.
    #[must_use]
    pub fn context(self) -> &'static str {
        match self {
            Action::TogglePanel(_) | Action::CycleViews | Action::ToggleSidebar => "Sidebar",
            Action::NextSession
            | Action::PreviousSession
            | Action::SwitchTabNext
            | Action::SwitchTabPrevious
            | Action::FocusTab(_)
            | Action::CloseSession
            | Action::ReopenClosedTab
            | Action::PreviousCodeView
            | Action::NextCodeView
            | Action::MoveTabLeft
            | Action::MoveTabRight => "Sessions",
            Action::CommandPalette
            | Action::CommandPaletteCommands
            | Action::GoToPalette
            | Action::FilePalette
            | Action::NewShell
            | Action::NewWorktree
            | Action::NewAgent
            | Action::Settings
            | Action::FindInFiles
            | Action::CloseOtherViews
            | Action::CloseViewsToRight => "Global",
            Action::SplitPane | Action::TerminalZoom(_) => "Terminal",
            Action::GoToDefinition
            | Action::GoBack
            | Action::EditorZoomIn
            | Action::EditorZoomOut
            | Action::EditorZoomReset => EDITOR,
        }
    }

    #[must_use]
    pub fn label(self) -> String {
        match self {
            Action::FocusTab(index) => format!("Session tab {index}"),
            Action::TogglePanel(panel) => format!("{panel:?}"),
            other => BINDINGS
                .iter()
                .find(|binding| binding.action == other)
                .map_or(other.id(), |binding| binding.label)
                .to_string(),
        }
    }

    /// Parse one stored override. An unknown id is dropped rather than turned
    /// into a chord with no handler.
    #[must_use]
    pub fn from_id(id: &str, argument: Option<usize>) -> Option<Action> {
        Some(match id {
            "open_command_palette" => Action::CommandPalette,
            "find_command" => Action::CommandPaletteCommands,
            "go_to" => Action::GoToPalette,
            "open_file_palette" => Action::FilePalette,
            "new_terminal" => Action::NewShell,
            "new_worktree" => Action::NewWorktree,
            "close_session" => Action::CloseSession,
            "new_agent" => Action::NewAgent,
            "open_settings" => Action::Settings,
            "toggle_sidebar" => Action::ToggleSidebar,
            "toggle_projects" => Action::TogglePanel(Panel::Projects),
            "toggle_files" => Action::TogglePanel(Panel::Files),
            "toggle_history" => Action::TogglePanel(Panel::History),
            "toggle_pull_requests" => Action::TogglePanel(Panel::PullRequests),
            "toggle_git" => Action::TogglePanel(Panel::Git),
            "toggle_processes" => Action::TogglePanel(Panel::Processes),
            "cycle_sidebar_views" => Action::CycleViews,
            "next_session" => Action::NextSession,
            "previous_session" => Action::PreviousSession,
            "switch_tab_next" => Action::SwitchTabNext,
            "switch_tab_previous" => Action::SwitchTabPrevious,
            "focus_session" => Action::FocusTab(argument.filter(|n| (1..=9).contains(n))?),
            "find_in_project" => Action::FindInFiles,
            "split_pane" => Action::SplitPane,
            "terminal_zoom_in" => Action::TerminalZoom(Zoom::In),
            "terminal_zoom_out" => Action::TerminalZoom(Zoom::Out),
            "terminal_zoom_reset" => Action::TerminalZoom(Zoom::Reset),
            "reopen_closed_tab" => Action::ReopenClosedTab,
            "previous_code_view" => Action::PreviousCodeView,
            "next_code_view" => Action::NextCodeView,
            "close_other_views" => Action::CloseOtherViews,
            "close_views_to_right" => Action::CloseViewsToRight,
            "move_tab_left" => Action::MoveTabLeft,
            "move_tab_right" => Action::MoveTabRight,
            "go_to_definition" => Action::GoToDefinition,
            "go_back" => Action::GoBack,
            "editor_zoom_in" => Action::EditorZoomIn,
            "editor_zoom_out" => Action::EditorZoomOut,
            "editor_zoom_reset" => Action::EditorZoomReset,
            _ => return None,
        })
    }
}

/// The context whose chords [`Keymap::editor_action`] answers; they outrank
/// the app's while a line editor holds the keyboard.
pub const EDITOR: &str = "Editor";

/// One row of Settings › Keyboard: what a chord does, and how it is written.
pub struct Binding {
    pub label: &'static str,
    /// A Rune chord spec, read by [`parse_chord`]: `mod+shift+t`, `ctrl+tab`.
    pub chord: &'static str,
    pub action: Action,
}

impl Binding {
    /// The chord as a person reads it. An unparseable row falls back to the
    /// spec rather than disappearing from Settings.
    #[must_use]
    pub fn display(&self) -> String {
        if self.chord.is_empty() {
            return "Not bound".to_string();
        }
        parse_chord(self.chord).map_or_else(|| self.chord.to_string(), |chord| chord.label())
    }
}

/// Every chord the app ships, in Settings' order.
pub const BINDINGS: &[Binding] = &[
    Binding {
        label: "Command palette",
        chord: "mod+k",
        action: Action::CommandPalette,
    },
    Binding {
        label: "Go to…",
        chord: "mod+shift+o",
        action: Action::GoToPalette,
    },
    Binding {
        label: "Find command…",
        chord: "mod+shift+p",
        action: Action::CommandPaletteCommands,
    },
    Binding {
        label: "Open file",
        chord: "mod+p",
        action: Action::FilePalette,
    },
    Binding {
        label: "Open file (⌘O)",
        chord: "mod+o",
        action: Action::FilePalette,
    },
    Binding {
        label: "New terminal",
        chord: "mod+t",
        action: Action::NewShell,
    },
    Binding {
        label: "New agent",
        chord: "mod+shift+a",
        action: Action::NewAgent,
    },
    Binding {
        label: "New worktree",
        chord: "mod+shift+n",
        action: Action::NewWorktree,
    },
    Binding {
        label: "Close session",
        chord: "mod+w",
        action: Action::CloseSession,
    },
    Binding {
        label: "Settings",
        chord: "mod+,",
        action: Action::Settings,
    },
    Binding {
        label: "Toggle sidebar",
        chord: "mod+b",
        action: Action::ToggleSidebar,
    },
    Binding {
        label: "Projects",
        chord: "mod+1",
        action: Action::TogglePanel(Panel::Projects),
    },
    Binding {
        label: "Files",
        chord: "mod+2",
        action: Action::TogglePanel(Panel::Files),
    },
    Binding {
        label: "Next sidebar view",
        chord: "mod+3",
        action: Action::CycleViews,
    },
    Binding {
        label: "History",
        chord: "mod+shift+e",
        action: Action::TogglePanel(Panel::History),
    },
    Binding {
        label: "Pull requests",
        chord: "mod+shift+r",
        action: Action::TogglePanel(Panel::PullRequests),
    },
    Binding {
        label: "Git",
        chord: "mod+shift+g",
        action: Action::TogglePanel(Panel::Git),
    },
    Binding {
        label: "Processes",
        chord: "",
        action: Action::TogglePanel(Panel::Processes),
    },
    Binding {
        label: "Next session",
        chord: "mod+shift+]",
        action: Action::NextSession,
    },
    Binding {
        label: "Previous session",
        chord: "mod+shift+[",
        action: Action::PreviousSession,
    },
    Binding {
        label: "Switch tab, by last use",
        chord: "ctrl+tab",
        action: Action::SwitchTabNext,
    },
    Binding {
        label: "Switch tab back, by last use",
        chord: "ctrl+shift+tab",
        action: Action::SwitchTabPrevious,
    },
    Binding {
        label: "Session tab 1",
        chord: "alt+1",
        action: Action::FocusTab(1),
    },
    Binding {
        label: "Session tab 2",
        chord: "alt+2",
        action: Action::FocusTab(2),
    },
    Binding {
        label: "Session tab 3",
        chord: "alt+3",
        action: Action::FocusTab(3),
    },
    Binding {
        label: "Session tab 4",
        chord: "alt+4",
        action: Action::FocusTab(4),
    },
    Binding {
        label: "Session tab 5",
        chord: "alt+5",
        action: Action::FocusTab(5),
    },
    Binding {
        label: "Session tab 6",
        chord: "alt+6",
        action: Action::FocusTab(6),
    },
    Binding {
        label: "Session tab 7",
        chord: "alt+7",
        action: Action::FocusTab(7),
    },
    Binding {
        label: "Session tab 8",
        chord: "alt+8",
        action: Action::FocusTab(8),
    },
    Binding {
        label: "Session tab 9",
        chord: "alt+9",
        action: Action::FocusTab(9),
    },
    Binding {
        label: "Find in files",
        chord: "mod+shift+f",
        action: Action::FindInFiles,
    },
    Binding {
        label: "Split pane",
        chord: "mod+d",
        action: Action::SplitPane,
    },
    Binding {
        label: "Terminal: Zoom In",
        chord: "mod+=",
        action: Action::TerminalZoom(Zoom::In),
    },
    Binding {
        label: "Terminal: Zoom Out",
        chord: "mod+-",
        action: Action::TerminalZoom(Zoom::Out),
    },
    Binding {
        label: "Terminal: Actual Size",
        chord: "mod+0",
        action: Action::TerminalZoom(Zoom::Reset),
    },
    Binding {
        label: "Reopen closed tab",
        chord: "mod+shift+t",
        action: Action::ReopenClosedTab,
    },
    Binding {
        label: "Previous file",
        chord: "mod+alt+left",
        action: Action::PreviousCodeView,
    },
    Binding {
        label: "Next file",
        chord: "mod+alt+right",
        action: Action::NextCodeView,
    },
    Binding {
        label: "Close other files",
        chord: "mod+shift+w",
        action: Action::CloseOtherViews,
    },
    Binding {
        label: "Close files to the right",
        chord: "mod+alt+w",
        action: Action::CloseViewsToRight,
    },
    Binding {
        label: "Move tab left",
        chord: "mod+alt+shift+left",
        action: Action::MoveTabLeft,
    },
    Binding {
        label: "Move tab right",
        chord: "mod+alt+shift+right",
        action: Action::MoveTabRight,
    },
    Binding {
        label: "Go to definition",
        chord: "mod+b",
        action: Action::GoToDefinition,
    },
    Binding {
        label: "Go back",
        chord: "ctrl+-",
        action: Action::GoBack,
    },
    Binding {
        label: "Editor: bigger text",
        chord: "mod+=",
        action: Action::EditorZoomIn,
    },
    Binding {
        label: "Editor: smaller text",
        chord: "mod+-",
        action: Action::EditorZoomOut,
    },
    Binding {
        label: "Editor: default text size",
        chord: "mod+0",
        action: Action::EditorZoomReset,
    },
];

/// What Option and a digit type on a US layout. Matched by the character,
/// not the key, because Rune reports characters: a layout where Option-2 is
/// `@` (Spanish, among others) keeps typing `@` instead of losing it to a
/// tab switch.
const OPTION_DIGITS: [char; 9] = ['¡', '™', '£', '¢', '∞', '§', '¶', '•', 'ª'];

/// Parse a stored spec. Rune's `mod+shift+t` is canonical; Tauri's older
/// `cmd-shift-t` is accepted so a preference written before the migration still
/// means what it meant.
#[must_use]
pub fn parse_chord(spec: &str) -> Option<Chord> {
    if let Ok(chord) = spec.parse::<Chord>() {
        return Some(chord);
    }
    legacy_spec(spec)?.parse::<Chord>().ok()
}

fn legacy_spec(spec: &str) -> Option<String> {
    let mut mods: Vec<&str> = Vec::new();
    let mut rest = spec;
    while let Some((head, tail)) = rest.split_once('-') {
        match head {
            "cmd" | "command" | "meta" => mods.push("mod"),
            "ctrl" | "control" => mods.push("ctrl"),
            "alt" | "option" | "opt" => mods.push("alt"),
            "shift" => mods.push("shift"),
            _ => break,
        }
        rest = tail;
    }
    if mods.is_empty() || rest.is_empty() {
        return None;
    }
    Some(format!("{}+{rest}", mods.join("+")))
}

/// The chord a keystroke names, for capture-to-rebind. A chord without a
/// command, control or option modifier is refused: at app level it would take
/// a letter from every terminal on screen.
pub fn capture_chord(event: &KeyEvent) -> Result<Chord, &'static str> {
    let key = match &event.key {
        Key::Command(ch) => ChordKey::Char(*ch),
        Key::Text(text) => {
            let mut chars = text.chars();
            match (chars.next(), chars.next()) {
                (Some(ch), None) => {
                    if event.mods.alt
                        && !event.mods.ctrl
                        && !event.mods.cmd
                        && !event.mods.shift
                        && let Some(index) = OPTION_DIGITS.iter().position(|glyph| *glyph == ch)
                    {
                        ChordKey::Char(char::from(b'1' + index as u8))
                    } else {
                        ChordKey::Char(ch)
                    }
                }
                _ => return Err("That key cannot be a chord."),
            }
        }
        Key::Tab => ChordKey::Tab,
        Key::Enter => ChordKey::Enter,
        Key::Escape => ChordKey::Escape,
        Key::Backspace => ChordKey::Backspace,
        Key::Delete => ChordKey::Delete,
        Key::Insert => ChordKey::Insert,
        Key::Home => ChordKey::Home,
        Key::End => ChordKey::End,
        Key::PageUp => ChordKey::PageUp,
        Key::PageDown => ChordKey::PageDown,
        Key::Left => ChordKey::Left,
        Key::Right => ChordKey::Right,
        Key::Up => ChordKey::Up,
        Key::Down => ChordKey::Down,
        Key::Function(number) if (1..=24).contains(number) => ChordKey::Function(*number),
        Key::ContextMenu => ChordKey::ContextMenu,
        Key::PrintScreen => ChordKey::PrintScreen,
        Key::ScrollLock => ChordKey::ScrollLock,
        Key::Pause => ChordKey::Pause,
        _ => return Err("That key cannot be a chord."),
    };
    let mut chord = Chord::new(key);
    if event.mods.ctrl {
        chord = chord.with(ChordMod::Ctrl);
    }
    if event.mods.alt {
        chord = chord.with(ChordMod::Alt);
    }
    if event.mods.shift {
        chord = chord.with(ChordMod::Shift);
    }
    if event.mods.cmd {
        chord = chord.with(ChordMod::Primary);
    } else if event.mods.logo {
        chord = chord.with(ChordMod::Super);
    }
    if !(event.mods.cmd || event.mods.ctrl || event.mods.alt || event.mods.logo) {
        return Err("Add ⌘, ⌃ or ⌥.");
    }
    Ok(chord)
}

/// Whether `event` is `chord`, including Option-digit glyphs that Rune reports
/// as characters rather than as the digit key.
#[must_use]
pub fn matches_event(chord: &Chord, event: &KeyEvent) -> bool {
    if chord.matches(event) {
        return true;
    }
    let (Key::Text(text), true) = (&event.key, event.mods.alt) else {
        return false;
    };
    let Some(index) = OPTION_DIGITS
        .iter()
        .position(|glyph| text == &glyph.to_string())
    else {
        return false;
    };
    let digit = KeyEvent {
        key: Key::Text(char::from(b'1' + index as u8).to_string()),
        mods: event.mods,
    };
    chord.matches(&digit)
}

/// The shipped chord for an action, before any override.
#[must_use]
pub fn default_chord(action: Action) -> Option<Chord> {
    BINDINGS
        .iter()
        .find(|binding| binding.action == action)
        .and_then(|binding| parse_chord(binding.chord))
}

/// One person's decision about one action.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Override {
    pub action: Action,
    /// `None` is an unbind, which is a real preference.
    pub chord: Option<Chord>,
}

/// The persisted keymap, already merged over [`BINDINGS`] by every lookup.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Keymap {
    overrides: Vec<Override>,
    /// `BINDINGS` with overrides applied, built when the table changes.
    resolved: Vec<Resolved>,
    seeded: bool,
}

impl Default for Keymap {
    fn default() -> Self {
        Self {
            overrides: Vec::new(),
            resolved: resolve(&[]),
            seeded: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Resolved {
    action: Action,
    context: &'static str,
    chord: Chord,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct StoredOverride {
    action: String,
    #[serde(default)]
    context: String,
    chord: Option<String>,
    #[serde(default)]
    argument: Option<usize>,
}

impl Keymap {
    #[must_use]
    pub fn parse(raw: Option<&str>) -> Self {
        let Some(raw) = raw else {
            return Self::default();
        };
        let Ok(rows) = serde_json::from_str::<Vec<StoredOverride>>(raw) else {
            return Self::default();
        };
        let mut overrides = Vec::new();
        for row in rows {
            let Some(action) = Action::from_id(&row.action, row.argument) else {
                continue;
            };
            let _ = row.context;
            let chord = match row.chord {
                Some(spec) => {
                    let Some(chord) = parse_chord(&spec) else {
                        continue;
                    };
                    Some(chord)
                }
                None => None,
            };
            overrides.push(Override { action, chord });
        }
        let resolved = resolve(&overrides);
        Self {
            overrides,
            resolved,
            seeded: false,
        }
    }

    /// Seed once from the daemon's stored copy; a later snapshot must not undo
    /// a rebind made in this window.
    pub fn seed_from(&mut self, raw: Option<&str>) {
        if self.seeded {
            return;
        }
        self.seeded = true;
        self.overrides = Self::parse(raw).overrides;
        self.resolved = resolve(&self.overrides);
    }

    #[must_use]
    pub fn to_json(&self) -> String {
        let rows: Vec<StoredOverride> = self
            .overrides
            .iter()
            .map(|item| StoredOverride {
                action: item.action.id().to_string(),
                context: item.action.context().to_string(),
                chord: item.chord.map(|chord| chord.to_string()),
                argument: item.action.argument(),
            })
            .collect();
        serde_json::to_string(&rows).unwrap_or_else(|_| "[]".to_string())
    }

    #[must_use]
    pub fn is_overridden(&self, action: Action) -> bool {
        self.override_for(action).is_some()
    }

    #[must_use]
    pub fn chord_for(&self, action: Action) -> Option<Chord> {
        match self.override_for(action) {
            Some(item) => item.chord,
            None => default_chord(action),
        }
    }

    /// The chord as Settings and the palette draw it, or `None` when unbound.
    #[must_use]
    pub fn chord_label(&self, action: Action) -> Option<String> {
        self.chord_for(action).map(|chord| chord.label())
    }

    pub fn set(&mut self, action: Action, chord: Option<Chord>) {
        self.overrides.retain(|item| item.action != action);
        self.overrides.push(Override { action, chord });
        self.resolved = resolve(&self.overrides);
    }

    pub fn reset(&mut self, action: Action) {
        self.overrides.retain(|item| item.action != action);
        self.resolved = resolve(&self.overrides);
    }

    fn override_for(&self, action: Action) -> Option<&Override> {
        self.overrides.iter().find(|item| item.action == action)
    }

    /// Chords in force, overrides first: a rebind beats a default chord that
    /// happens to be the same keys.
    fn effective(&self) -> &[Resolved] {
        &self.resolved
    }

    /// The app-level action this keystroke means, override before default.
    #[must_use]
    pub fn app_action(&self, event: &KeyEvent) -> Option<Action> {
        if let Some(action) = self
            .effective()
            .iter()
            .find(|resolved| resolved.context != EDITOR && matches_event(&resolved.chord, event))
            .map(|resolved| resolved.action)
        {
            return Some(action);
        }
        action_for(event).filter(|action| !self.is_overridden(*action))
    }

    /// The editor's own action, with the same override-first rule.
    #[must_use]
    pub fn editor_action(&self, event: &KeyEvent) -> Option<Action> {
        if let Some(action) = self
            .effective()
            .iter()
            .find(|resolved| resolved.context == EDITOR && matches_event(&resolved.chord, event))
            .map(|resolved| resolved.action)
        {
            return Some(action);
        }
        editor_action_for(event).filter(|action| !self.is_overridden(*action))
    }

    /// Every chord two actions claim in one context. The later one is dead,
    /// which is why Settings shows the list rather than resolving it.
    #[must_use]
    pub fn conflicts(&self) -> Vec<Conflict> {
        let mut by_chord: Vec<Conflict> = Vec::new();
        for resolved in self.effective() {
            let signature = resolved.chord.to_string();
            if let Some(conflict) = by_chord
                .iter_mut()
                .find(|item| item.context == resolved.context && item.signature == signature)
            {
                if !conflict.actions.contains(&resolved.action) {
                    conflict.actions.push(resolved.action);
                }
                continue;
            }
            by_chord.push(Conflict {
                context: resolved.context,
                chord: resolved.chord,
                signature,
                actions: vec![resolved.action],
            });
        }
        by_chord
            .into_iter()
            .filter(|item| item.actions.len() > 1)
            .collect()
    }
}

fn resolve(overrides: &[Override]) -> Vec<Resolved> {
    let mut resolved: Vec<Resolved> = Vec::new();
    for item in overrides {
        let Some(chord) = item.chord else {
            continue;
        };
        resolved.retain(|existing| existing.action != item.action);
        resolved.push(Resolved {
            action: item.action,
            context: item.action.context(),
            chord,
        });
    }
    for binding in BINDINGS {
        if overrides.iter().any(|item| item.action == binding.action) {
            continue;
        }
        if let Some(chord) = parse_chord(binding.chord) {
            resolved.push(Resolved {
                action: binding.action,
                context: binding.action.context(),
                chord,
            });
        }
    }
    resolved
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conflict {
    pub context: &'static str,
    pub chord: Chord,
    signature: String,
    pub actions: Vec<Action>,
}

impl Conflict {
    #[must_use]
    pub fn label(&self) -> String {
        self.chord.label()
    }

    #[must_use]
    pub fn action_labels(&self) -> String {
        self.actions
            .iter()
            .map(|action| action.label())
            .collect::<Vec<String>>()
            .join(" · ")
    }
}

/// The rows Settings lists: one per action, in table order. An action with
/// two default chords still gets one row, because one override moves both.
#[must_use]
pub fn binding_rows() -> Vec<&'static Binding> {
    let mut seen: Vec<Action> = Vec::new();
    BINDINGS
        .iter()
        .filter(|binding| {
            if seen.contains(&binding.action) {
                false
            } else {
                seen.push(binding.action);
                true
            }
        })
        .collect()
}

/// The default app-level chords, without a keymap. Kept for the editor's own
/// composed events and for the tests that write a keystroke by hand.
#[must_use]
pub fn action_for(event: &KeyEvent) -> Option<Action> {
    let mods = event.mods;
    match &event.key {
        // macOS keeps Cmd-Tab for its app switcher, so the held switch is Ctrl.
        Key::Tab if mods.ctrl && !mods.alt && !mods.cmd => Some(if mods.shift {
            Action::SwitchTabPrevious
        } else {
            Action::SwitchTabNext
        }),
        Key::Text(text) if mods.alt && !mods.cmd && !mods.ctrl => {
            let mut chars = text.chars();
            let (Some(ch), None) = (chars.next(), chars.next()) else {
                return None;
            };
            let digit = OPTION_DIGITS
                .iter()
                .position(|glyph| *glyph == ch)
                .map(|at| at + 1)
                .or_else(|| ch.to_digit(10).map(|digit| digit as usize))
                .filter(|digit| (1..=9).contains(digit))?;
            Some(Action::FocusTab(digit))
        }
        Key::Command(ch) if !mods.alt && !mods.ctrl => command(ch.to_ascii_lowercase(), mods.shift),
        Key::Left | Key::Right if mods.cmd && mods.alt && !mods.ctrl && !mods.shift => {
            Some(if event.key == Key::Left {
                Action::PreviousCodeView
            } else {
                Action::NextCodeView
            })
        }
        _ => None,
    }
}

/// The editor's own chords, asked before [`action_for`] while a line editor
/// holds the keyboard: ⌘B is go to definition there and the sidebar elsewhere.
#[must_use]
pub fn editor_action_for(event: &KeyEvent) -> Option<Action> {
    let mods = event.mods;
    match &event.key {
        // VS Code's chord; on macOS `ctrl` is free where zoom is `cmd`.
        Key::Text(text)
            if mods.ctrl
                && !mods.cmd
                && !mods.alt
                && !mods.shift
                && (text == "-" || text == "\u{1f}") =>
        {
            Some(Action::GoBack)
        }
        Key::Command(ch) if !mods.alt && !mods.ctrl => {
            match (ch.to_ascii_lowercase(), mods.shift) {
                ('b', false) => Some(Action::GoToDefinition),
                // `=` is the unshifted plus; `+` is the keycap people read as ⌘+.
                ('=' | '+', _) => Some(Action::EditorZoomIn),
                ('-', false) => Some(Action::EditorZoomOut),
                ('0', false) => Some(Action::EditorZoomReset),
                _ => None,
            }
        }
        _ => None,
    }
}

fn command(ch: char, shift: bool) -> Option<Action> {
    Some(match (ch, shift) {
        ('k', false) => Action::CommandPalette,
        // Go To and Find Command open the one palette Rune has, narrowed.
        ('o', true) => Action::GoToPalette,
        ('p', true) => Action::CommandPaletteCommands,
        ('p' | 'o', false) => Action::FilePalette,
        ('t', false) => Action::NewShell,
        ('t', true) => Action::ReopenClosedTab,
        ('n', true) => Action::NewWorktree,
        ('w', false) => Action::CloseSession,
        ('w', true) => Action::CloseOtherViews,
        ('a', true) => Action::NewAgent,
        (',', false) => Action::Settings,
        ('b', false) => Action::ToggleSidebar,
        ('1', false) => Action::TogglePanel(Panel::Projects),
        ('2', false) => Action::TogglePanel(Panel::Files),
        ('3', false) => Action::CycleViews,
        ('e', true) => Action::TogglePanel(Panel::History),
        ('r', true) => Action::TogglePanel(Panel::PullRequests),
        ('g', true) => Action::TogglePanel(Panel::Git),
        // A shifted bracket arrives as either glyph, depending on the layout.
        (']' | '}', true) => Action::NextSession,
        ('[' | '{', true) => Action::PreviousSession,
        ('f', true) => Action::FindInFiles,
        ('d', false) => Action::SplitPane,
        // `=` is the unshifted plus; `+` covers the keycap and a numpad plus.
        ('=' | '+', _) => Action::TerminalZoom(Zoom::In),
        ('-', false) => Action::TerminalZoom(Zoom::Out),
        ('0', false) => Action::TerminalZoom(Zoom::Reset),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune::Mods;

    fn chord(ch: char, shift: bool) -> KeyEvent {
        KeyEvent {
            key: Key::Command(ch),
            mods: Mods {
                cmd: true,
                shift,
                ..Mods::default()
            },
        }
    }

    fn with(key: Key, mods: Mods) -> KeyEvent {
        KeyEvent { key, mods }
    }

    fn plain() -> Mods {
        Mods::default()
    }

    fn ctrl() -> Mods {
        Mods {
            ctrl: true,
            ..Mods::default()
        }
    }

    fn alt() -> Mods {
        Mods {
            alt: true,
            ..Mods::default()
        }
    }

    /// A keystroke from a chord spec, for the every-default test. The specs
    /// only use forms a keyboard actually produces.
    fn event_for(spec: &str) -> Option<KeyEvent> {
        let chord = parse_chord(spec)?;
        let mods = Mods {
            cmd: chord.has(ChordMod::Primary),
            ctrl: chord.has(ChordMod::Ctrl),
            alt: chord.has(ChordMod::Alt),
            shift: chord.has(ChordMod::Shift),
            logo: chord.has(ChordMod::Super),
        };
        let key = match chord.key() {
            ChordKey::Char(ch) if mods.cmd => Key::Command(ch),
            ChordKey::Char(ch) => Key::Text(ch.to_string()),
            ChordKey::Tab => Key::Tab,
            ChordKey::Enter => Key::Enter,
            ChordKey::Escape => Key::Escape,
            ChordKey::Backspace => Key::Backspace,
            ChordKey::Delete => Key::Delete,
            ChordKey::Insert => Key::Insert,
            ChordKey::Home => Key::Home,
            ChordKey::End => Key::End,
            ChordKey::PageUp => Key::PageUp,
            ChordKey::PageDown => Key::PageDown,
            ChordKey::Left => Key::Left,
            ChordKey::Right => Key::Right,
            ChordKey::Up => Key::Up,
            ChordKey::Down => Key::Down,
            ChordKey::Function(n) => Key::Function(n),
            ChordKey::ContextMenu => Key::ContextMenu,
            ChordKey::PrintScreen => Key::PrintScreen,
            ChordKey::ScrollLock => Key::ScrollLock,
            ChordKey::Pause => Key::Pause,
            _ => return None,
        };
        Some(KeyEvent { key, mods })
    }

    #[test]
    fn every_default_resolves_to_its_action() {
        let keymap = Keymap::default();
        for binding in BINDINGS {
            if binding.chord.is_empty() {
                assert_eq!(
                    default_chord(binding.action),
                    None,
                    "{} ships unbound",
                    binding.label
                );
                continue;
            }
            let event = event_for(binding.chord)
                .unwrap_or_else(|| panic!("{} does not parse", binding.chord));
            let resolve = if binding.action.context() == EDITOR {
                keymap.editor_action(&event)
            } else {
                keymap.app_action(&event)
            };
            assert_eq!(
                resolve,
                Some(binding.action),
                "{} is {}",
                binding.label,
                binding.chord
            );
        }
    }

    #[test]
    fn the_table_has_no_conflicting_defaults() {
        assert!(
            Keymap::default().conflicts().is_empty(),
            "{:?}",
            Keymap::default().conflicts()
        );
    }

    #[test]
    fn a_chord_is_captured_as_the_parser_reads_it_back() {
        let cases = [
            (chord('k', false), "mod+k"),
            (chord('T', true), "mod+shift+t"),
            (with(Key::Tab, ctrl()), "ctrl+tab"),
            (with(Key::Text("¡".into()), alt()), "alt+1"),
            (
                with(
                    Key::Left,
                    Mods {
                        cmd: true,
                        alt: true,
                        ..plain()
                    },
                ),
                "mod+alt+left",
            ),
        ];
        for (event, expected) in cases {
            let chord = capture_chord(&event).expect("a chord");
            assert_eq!(chord.to_string(), expected, "{event:?}");
            assert_eq!(parse_chord(&chord.to_string()), Some(chord));
        }
        assert_eq!(
            capture_chord(&with(Key::Text("a".into()), plain())),
            Err("Add ⌘, ⌃ or ⌥.")
        );
        assert_eq!(
            capture_chord(&with(
                Key::Text("A".into()),
                Mods {
                    shift: true,
                    ..plain()
                }
            )),
            Err("Add ⌘, ⌃ or ⌥."),
            "Shift alone still types into a terminal"
        );
    }

    #[test]
    fn legacy_tauri_specs_parse_and_describe_the_same_keys() {
        assert_eq!(parse_chord("cmd-shift-o"), parse_chord("mod+shift+o"));
        assert_eq!(parse_chord("ctrl-tab"), parse_chord("ctrl+tab"));
        assert_eq!(parse_chord("cmd--"), parse_chord("mod+-"));
        assert_eq!(parse_chord("not-a-chord"), None);
    }

    #[test]
    fn an_override_is_consulted_before_the_default_and_reset_restores_it() {
        let mut keymap = Keymap::default();
        let next = Keymap::default();
        assert_eq!(
            next.app_action(&chord('k', false)),
            Some(Action::CommandPalette)
        );
        keymap.set(Action::CommandPalette, parse_chord("mod+j"));
        assert_eq!(
            keymap.app_action(&chord('j', false)),
            Some(Action::CommandPalette)
        );
        assert_eq!(
            keymap.app_action(&chord('k', false)),
            None,
            "the old default no longer fires"
        );
        keymap.reset(Action::CommandPalette);
        assert_eq!(
            keymap.app_action(&chord('k', false)),
            Some(Action::CommandPalette)
        );
    }

    #[test]
    fn an_unbind_stops_the_default_and_is_serialized() {
        let mut keymap = Keymap::default();
        keymap.set(Action::NewShell, None);
        assert_eq!(keymap.app_action(&chord('t', false)), None);
        let stored = keymap.to_json();
        let parsed = Keymap::parse(Some(&stored));
        assert_eq!(parsed, keymap);
        assert!(parsed.is_overridden(Action::NewShell));
        assert_eq!(parsed.chord_for(Action::NewShell), None);
    }

    #[test]
    fn malformed_storage_drops_the_one_binding_not_every_shortcut() {
        let raw = r#"[
            {"action":"open_file_palette","context":"Global","chord":"not a chord"},
            {"action":"gone_from_this_build","context":"Global","chord":"mod+q"},
            {"action":"new_terminal","context":"Global","chord":"cmd-n"}
        ]"#;
        let keymap = Keymap::parse(Some(raw));
        assert_eq!(keymap.overrides.len(), 1);
        assert_eq!(
            keymap
                .chord_for(Action::NewShell)
                .map(|chord| chord.to_string()),
            Some("mod+n".to_string()),
            "Tauri's cmd-n is the rune mod+n"
        );
        assert_eq!(Keymap::parse(Some("not json")), Keymap::default());
    }

    #[test]
    fn a_tauri_context_name_does_not_stop_an_override_from_loading() {
        let raw = r#"[{"action":"new_terminal","context":"App","chord":"cmd-9"}]"#;
        let keymap = Keymap::parse(Some(raw));
        assert_eq!(
            keymap
                .chord_for(Action::NewShell)
                .map(|chord| chord.to_string()),
            Some("mod+9".to_string()),
            "Tauri writes App, Rune writes Global; the action decides"
        );
    }

    #[test]
    fn two_actions_on_one_chord_in_one_context_are_reported() {
        let mut keymap = Keymap::default();
        keymap.set(Action::NewShell, parse_chord("mod+k"));
        let conflicts = keymap.conflicts();
        assert_eq!(conflicts.len(), 1, "{conflicts:?}");
        assert_eq!(conflicts[0].context, "Global");
        assert_eq!(
            conflicts[0].actions,
            vec![Action::NewShell, Action::CommandPalette]
        );
        assert_eq!(conflicts[0].label(), "⌘K");
        let mut same_but_other_context = Keymap::default();
        same_but_other_context.set(Action::GoToDefinition, parse_chord("mod+k"));
        assert!(
            same_but_other_context.conflicts().is_empty(),
            "the editor answers its own context"
        );
    }

    #[test]
    fn option_digits_keep_their_physical_key_on_any_layout() {
        let keymap = Keymap::default();
        assert_eq!(
            keymap.app_action(&with(Key::Text("¡".into()), alt())),
            Some(Action::FocusTab(1))
        );
        assert_eq!(
            keymap.app_action(&with(Key::Text("ª".into()), alt())),
            Some(Action::FocusTab(9))
        );
        assert_eq!(
            keymap.app_action(&with(Key::Text("@".into()), alt())),
            None,
            "a layout's Option-2 @ is still typed"
        );
    }

    #[test]
    fn ctrl_tab_is_the_switch_action_and_brackets_walk_the_strip() {
        let keymap = Keymap::default();
        assert_eq!(
            keymap.app_action(&with(Key::Tab, ctrl())),
            Some(Action::SwitchTabNext)
        );
        assert_eq!(
            keymap.app_action(&with(
                Key::Tab,
                Mods {
                    shift: true,
                    ..ctrl()
                }
            )),
            Some(Action::SwitchTabPrevious)
        );
        assert_eq!(
            keymap.app_action(&chord('}', true)),
            Some(Action::NextSession)
        );
        assert_eq!(
            keymap.app_action(&chord('[', true)),
            Some(Action::PreviousSession)
        );
    }

    #[test]
    fn a_rebind_moves_the_alternate_defaults_too() {
        let mut keymap = Keymap::default();
        assert_eq!(
            keymap.app_action(&chord('o', false)),
            Some(Action::FilePalette),
            "⌘O aliases the file palette"
        );
        keymap.set(Action::FilePalette, parse_chord("mod+shift+e"));
        assert_eq!(keymap.app_action(&chord('o', false)), None);
        assert_eq!(keymap.app_action(&chord('p', false)), None);
        assert_eq!(
            keymap.app_action(&chord('e', true)),
            Some(Action::FilePalette),
            "an override beats the default that used to own this chord"
        );
    }

    #[test]
    fn unbound_and_plain_keys_go_on() {
        let keymap = Keymap::default();
        assert_eq!(
            keymap.app_action(&chord('c', false)),
            None,
            "copy is the surface's"
        );
        assert_eq!(
            keymap.app_action(&chord('f', false)),
            None,
            "find is the editor's"
        );
        assert_eq!(
            keymap.app_action(&with(Key::Text("k".into()), plain())),
            None
        );
    }

    #[test]
    fn the_editor_lends_b_and_the_zoom_chords_only_in_its_own_context() {
        let keymap = Keymap::default();
        assert_eq!(
            keymap.app_action(&chord('b', false)),
            Some(Action::ToggleSidebar)
        );
        assert_eq!(
            keymap.editor_action(&chord('b', false)),
            Some(Action::GoToDefinition)
        );
        assert_eq!(
            keymap.editor_action(&chord('+', true)),
            Some(Action::EditorZoomIn)
        );
        let shifted_minus = with(
            Key::Command('_'),
            Mods {
                cmd: true,
                shift: true,
                ..plain()
            },
        );
        assert_eq!(
            keymap.editor_action(&shifted_minus),
            None,
            "⇧⌘- types an underscore, not zoom"
        );
        assert_eq!(keymap.editor_action(&chord('k', false)), None);
        let ctrl_minus = with(Key::Text("-".into()), ctrl());
        assert_eq!(keymap.editor_action(&ctrl_minus), Some(Action::GoBack));
        assert_eq!(keymap.app_action(&ctrl_minus), None);
    }

    #[test]
    fn the_new_chords_are_what_tauri_documents() {
        let keymap = Keymap::default();
        assert_eq!(
            keymap.app_action(&chord('o', true)),
            Some(Action::GoToPalette)
        );
        assert_eq!(
            keymap.app_action(&chord('p', true)),
            Some(Action::CommandPaletteCommands)
        );
        assert_eq!(
            keymap.app_action(&chord('w', true)),
            Some(Action::CloseOtherViews)
        );
        assert_eq!(
            keymap.app_action(&with(
                Key::Command('w'),
                Mods {
                    cmd: true,
                    alt: true,
                    ..plain()
                }
            )),
            Some(Action::CloseViewsToRight)
        );
        assert_eq!(
            keymap.app_action(&with(
                Key::Left,
                Mods {
                    cmd: true,
                    alt: true,
                    shift: true,
                    ..plain()
                }
            )),
            Some(Action::MoveTabLeft)
        );
    }

    #[test]
    fn settings_rows_show_one_row_per_action() {
        let rows = binding_rows();
        let actions: Vec<Action> = rows.iter().map(|binding| binding.action).collect();
        assert_eq!(
            actions
                .iter()
                .filter(|action| **action == Action::FilePalette)
                .count(),
            1
        );
        assert!(actions.contains(&Action::FocusTab(9)));
        assert_eq!(rows.len(), 47);
    }

    #[test]
    fn a_binding_describes_itself_in_the_platforms_notation() {
        let palette = BINDINGS
            .iter()
            .find(|binding| binding.action == Action::CommandPalette)
            .expect("a palette row");
        assert_eq!(palette.display(), "⌘K");
    }
}
