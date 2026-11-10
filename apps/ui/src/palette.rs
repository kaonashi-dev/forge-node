//! The command palette: four typed scopes (`>`, `/`, `@`, `#`) plus the
//! mixed list ⌘K opens on. Commands are filtered here; files and symbols are
//! ranked by the daemon (`SearchFiles`), so this side never walks a checkout.

use domain::{SessionId, SessionKind, WorkspaceId};
use rune::TextEdit;

use crate::workbench::{CenterMode, Panel};

/// Files asked for per query; a palette shows a screenful, not a listing.
pub const FILE_LIMIT: u32 = 50;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    NewShell,
    Show(CenterMode),
    Panel(Panel),
    ToggleSidebar,
    NewWorktree,
    /// A base, or `"system"`.
    Theme(String),
    NewAgent(domain::AgentProviderId, Option<domain::AgentProfileId>),
    RunAction(domain::ProjectActionId),
    Settings(crate::settings::Section),
    FindInFiles,
    SplitPane,
    JoinPanes,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Scope {
    #[default]
    Everything,
    Commands,
    Files,
    Places,
    Symbols,
    Launch,
}

/// The four leading characters a query can start with, whatever shortcut
/// opened the palette.
pub const MODE_PREFIXES: [(char, Scope); 4] = [
    ('>', Scope::Commands),
    ('/', Scope::Files),
    ('@', Scope::Places),
    ('#', Scope::Symbols),
];

impl Scope {
    /// The chip at the start of the field: its typed prefix and its name.
    #[must_use]
    pub fn chip(self) -> Option<(char, &'static str)> {
        match self {
            Scope::Everything => None,
            Scope::Commands => Some(('>', "Command")),
            Scope::Files => Some(('/', "File")),
            Scope::Places => Some(('@', "Go to")),
            Scope::Symbols => Some(('#', "Symbol")),
            Scope::Launch => Some(('▸', "New")),
        }
    }

    #[must_use]
    pub fn placeholder(self) -> &'static str {
        match self {
            Scope::Everything => "Type a command…",
            Scope::Commands => "Run a command…",
            Scope::Files => "Open a file in this workspace…",
            Scope::Places => "Go to a session or a checkout…",
            Scope::Symbols => "Go to a symbol…",
            Scope::Launch => "Start a terminal or an agent…",
        }
    }
}

/// What a typed query asks for, and the text left once the mode prefix is read
/// off. The prefix stays in the field, so deleting it returns to the scope the
/// palette opened on.
#[must_use]
pub fn parse_query(opened: Scope, raw: &str) -> (Scope, &str) {
    let leading = raw.trim_start();
    for (prefix, scope) in MODE_PREFIXES {
        if let Some(rest) = leading.strip_prefix(prefix) {
            return (scope, rest.trim_start());
        }
    }
    (opened, raw.trim())
}

/// The symbol text a `#` query asks for, when it can be one. Definition search
/// takes a fixed string, never a pattern, and the daemon's grep is word-bounded;
/// this keeps punctuation that could only be a regex out of the request.
#[must_use]
pub fn symbol_text(raw: &str) -> Option<&str> {
    let text = raw.trim();
    let mut chars = text.chars();
    let first = chars.next()?;
    (first.is_alphabetic() || matches!(first, '_' | '$'))
        .then(|| chars.all(|ch| ch.is_alphanumeric() || matches!(ch, '_' | '$' | ':')))
        .filter(|all| *all)
        .map(|_| text)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SymbolHit {
    pub path: String,
    pub line: u32,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Entry {
    Command {
        label: String,
        command: Command,
    },
    File(String),
    Session {
        id: SessionId,
        label: String,
        note: String,
        agent: Option<String>,
        kind: SessionKind,
        waiting: bool,
    },
    Checkout {
        id: WorkspaceId,
        label: String,
        note: String,
    },
    Symbol(SymbolHit),
}

/// A search the palette wants made. The answer carries the same query back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Search {
    Files(String),
    Symbols(String),
}

impl Entry {
    #[must_use]
    pub fn label(&self) -> &str {
        match self {
            Entry::Command { label, .. } => label,
            Entry::File(path) => path,
            Entry::Session { label, .. } => label,
            Entry::Checkout { label, .. } => label,
            Entry::Symbol(hit) => &hit.path,
        }
    }

    /// The text a fuzzy query scores against. A session's branch or a file's
    /// directory is half of what makes one row the right one.
    #[must_use]
    pub fn search_text(&self) -> String {
        match self {
            Entry::Command { label, .. } => label.clone(),
            Entry::File(path) => path.clone(),
            Entry::Session { label, note, .. } => format!("{label} {note}"),
            Entry::Checkout { label, note, .. } => format!("{label} {note}"),
            Entry::Symbol(hit) => format!("{} {}", hit.path, hit.text),
        }
    }
}

/// The chord that runs a command, shown beside it.
/// This is the shipped fallback; a running app asks its keymap first.
#[must_use]
pub fn chord(command: &Command) -> Option<&'static str> {
    Some(match command {
        Command::NewShell => "⌘T",
        Command::NewWorktree => "⇧⌘N",
        Command::Panel(Panel::Projects) => "⌘1",
        Command::Panel(Panel::Files) => "⌘2",
        Command::Panel(Panel::History) => "⇧⌘E",
        Command::Panel(Panel::PullRequests) => "⇧⌘R",
        Command::Panel(Panel::Git) => "⇧⌘G",
        Command::ToggleSidebar => "⌘B",
        Command::Show(CenterMode::Settings) => "⌘,",
        Command::FindInFiles => "⇧⌘F",
        Command::SplitPane => "⌘D",
        _ => return None,
    })
}

pub struct Palette {
    pub open: bool,
    opened: Scope,
    pub edit: TextEdit,
    pub selected: usize,
    files: Vec<String>,
    symbols: Vec<SymbolHit>,
    /// Installed agent launch rows when the palette opened; profiles included.
    agents: Vec<crate::agent_ops::Choice>,
    /// The active workspace's approved actions when the palette opened.
    actions: Vec<(domain::ProjectActionId, String)>,
    /// Sessions and checkouts `@` lists.
    places: Vec<Entry>,
    /// What ⌘P lists before anything is typed, set when it opens.
    recent: Vec<String>,
    /// The answer held for `(scope, text)`, so a late reply from an older
    /// query cannot land under the next one.
    answer_for: Option<(Scope, String)>,
    /// The query a search is on its way for.
    asked: Option<(Scope, String)>,
    /// What the palette shows beside a failed search or a refused symbol.
    pub error: Option<String>,
    /// Chord labels from the running keymap, by command.
    hints: Vec<(Command, String)>,
    /// The checkout the last search was asked in; another one's answer is
    /// never listed under it.
    pub asked_in: Option<crate::workbench::Scope>,
}

impl Default for Palette {
    fn default() -> Self {
        Self {
            open: false,
            opened: Scope::Everything,
            edit: TextEdit::new(""),
            selected: 0,
            files: Vec::new(),
            symbols: Vec::new(),
            recent: Vec::new(),
            agents: Vec::new(),
            actions: Vec::new(),
            places: Vec::new(),
            answer_for: None,
            asked: None,
            error: None,
            hints: Vec::new(),
            asked_in: None,
        }
    }
}

fn commands() -> Vec<(String, Command)> {
    let out = vec![
        ("New Terminal".to_string(), Command::NewShell),
        ("New Worktree".to_string(), Command::NewWorktree),
        ("Projects".to_string(), Command::Panel(Panel::Projects)),
        ("Files".to_string(), Command::Panel(Panel::Files)),
        ("Find in Files".to_string(), Command::FindInFiles),
        ("History".to_string(), Command::Panel(Panel::History)),
        (
            "Pull Requests".to_string(),
            Command::Panel(Panel::PullRequests),
        ),
        ("Git".to_string(), Command::Panel(Panel::Git)),
        ("Processes".to_string(), Command::Panel(Panel::Processes)),
        ("Split Pane".to_string(), Command::SplitPane),
        ("Join Panes".to_string(), Command::JoinPanes),
        ("Toggle Sidebar".to_string(), Command::ToggleSidebar),
        ("Settings".to_string(), Command::Show(CenterMode::Settings)),
        (
            "Theme: Match System".to_string(),
            Command::Theme("system".into()),
        ),
    ];
    let mut named: Vec<(String, Command)> = crate::settings::Section::ALL
        .iter()
        .map(|section| {
            (
                format!("Settings: {}", section.label()),
                Command::Settings(*section),
            )
        })
        .collect();
    for base in crate::theme::bases() {
        if let Some(theme) = crate::theme::theme(base) {
            named.push((
                format!("Theme: {}", theme.label),
                Command::Theme(base.clone()),
            ));
        }
    }
    let mut all = out;
    all.extend(named);
    all
}

impl Palette {
    pub fn show(&mut self, scope: Scope) {
        self.show_with(scope, "", Vec::new());
    }

    /// Opens with `query` typed and the agents a person may start.
    pub fn show_with(&mut self, scope: Scope, query: &str, agents: Vec<crate::agent_ops::Choice>) {
        self.agents = agents;
        self.open = true;
        self.opened = scope;
        self.edit = TextEdit::new(query);
        self.edit.focused = true;
        self.selected = 0;
        self.files.clear();
        self.symbols.clear();
        self.recent.clear();
        self.answer_for = None;
        self.asked = None;
        self.error = None;
        self.asked_in = None;
    }

    /// Files opened lately, listed while ⌘P's query is empty.
    pub fn set_recent(&mut self, recent: Vec<String>) {
        self.recent = recent;
    }

    pub fn set_actions(&mut self, actions: Vec<(domain::ProjectActionId, String)>) {
        self.actions = actions;
    }

    pub fn set_agents(&mut self, agents: Vec<crate::agent_ops::Choice>) {
        self.agents = agents;
    }

    /// Sessions and checkouts, newest context first, from the replica.
    pub fn set_places(&mut self, places: Vec<Entry>) {
        self.places = places;
    }

    /// What the running keymap writes beside each command.
    pub fn set_hints(&mut self, hints: Vec<(Command, String)>) {
        self.hints = hints;
    }

    /// The chord a row shows: the keymap's, else the shipped one.
    #[must_use]
    pub fn chord(&self, command: &Command) -> Option<&str> {
        self.hints
            .iter()
            .find(|(candidate, _)| candidate == command)
            .map(|(_, label)| label.as_str())
            .or_else(|| chord(command))
    }

    pub fn close(&mut self) {
        self.open = false;
        self.edit.focused = false;
    }

    /// The scope and search text the field currently asks for.
    #[must_use]
    pub fn parsed(&self) -> (Scope, &str) {
        parse_query(self.opened, &self.edit.text)
    }

    #[must_use]
    pub fn scope(&self) -> Scope {
        self.parsed().0
    }

    /// What the field says before anything is typed, per scope.
    #[must_use]
    pub fn placeholder(&self) -> &'static str {
        self.scope().placeholder()
    }

    /// The scope chip at the start of the field, when there is one.
    #[must_use]
    pub fn chip(&self) -> Option<(char, &'static str)> {
        self.scope().chip()
    }

    /// Whether a search answer for the current query is still on its way.
    #[must_use]
    pub fn searching(&self) -> bool {
        let (scope, text) = self.parsed();
        let wants = matches!(scope, Scope::Everything | Scope::Files)
            || (scope == Scope::Symbols && symbol_text(text).is_some());
        if !wants || text.is_empty() {
            return false;
        }
        self.answer_for.as_ref() != Some(&(scope, text.to_string()))
            && self.asked.as_ref() == Some(&(scope, text.to_string()))
    }

    /// What is listed for the current scope and query.
    #[must_use]
    pub fn entries(&self) -> Vec<Entry> {
        let (scope, text) = self.parsed();
        match scope {
            Scope::Commands => rank_entries(self.command_entries(false), text),
            Scope::Launch => rank_entries(self.command_entries(true), text),
            Scope::Files => self.file_entries(text),
            Scope::Places => rank_entries(self.places.clone(), text),
            Scope::Symbols => self.symbol_entries(text),
            Scope::Everything => {
                let mut out = rank_entries(self.command_entries(false), text);
                if !text.is_empty() && self.held(scope, text) {
                    out.extend(self.files.iter().cloned().map(Entry::File));
                }
                out
            }
        }
    }

    fn command_entries(&self, launch_only: bool) -> Vec<Entry> {
        self.agents
            .iter()
            .map(|row| Entry::Command {
                label: format!("New {}", row.label),
                command: Command::NewAgent(row.provider.clone(), row.profile),
            })
            .chain(self.actions.iter().map(|(id, label)| Entry::Command {
                label: format!("Run: {label}"),
                command: Command::RunAction(*id),
            }))
            .chain(
                commands()
                    .into_iter()
                    .map(|(label, command)| Entry::Command { label, command }),
            )
            .filter(|entry| {
                !launch_only
                    || matches!(
                        entry,
                        Entry::Command {
                            command: Command::NewShell | Command::NewAgent(..),
                            ..
                        }
                    )
            })
            .collect()
    }

    fn file_entries(&self, text: &str) -> Vec<Entry> {
        if text.is_empty() {
            return self.recent.iter().cloned().map(Entry::File).collect();
        }
        if self.answer_for.as_ref() == Some(&(Scope::Files, text.to_string())) {
            return self.files.iter().cloned().map(Entry::File).collect();
        }
        if self.asked.as_ref() == Some(&(Scope::Files, text.to_string())) {
            return if self.files.is_empty() {
                self.recent.iter().cloned().map(Entry::File).collect()
            } else {
                self.files.iter().cloned().map(Entry::File).collect()
            };
        }
        Vec::new()
    }

    fn symbol_entries(&self, text: &str) -> Vec<Entry> {
        if text.is_empty() || symbol_text(text).is_none() {
            return Vec::new();
        }
        if self.held(Scope::Symbols, text) {
            return self.symbols.iter().cloned().map(Entry::Symbol).collect();
        }
        Vec::new()
    }

    fn held(&self, scope: Scope, text: &str) -> bool {
        let current = (scope, text.to_string());
        self.answer_for.as_ref() == Some(&current) || self.asked.as_ref() == Some(&current)
    }

    /// The search to send now, once per query.
    pub fn take_search(&mut self) -> Option<Search> {
        if !self.open {
            return None;
        }
        let (scope, text) = self.parsed();
        let text = text.to_string();
        if text.is_empty() {
            return None;
        }
        let current = (scope, text.clone());
        if self.asked.as_ref() == Some(&current) {
            return None;
        }
        match scope {
            Scope::Everything | Scope::Files => {
                self.asked = Some(current);
                Some(Search::Files(text))
            }
            Scope::Symbols => {
                let Some(symbol) = symbol_text(&text) else {
                    self.error = Some("A symbol is letters, digits, _ or $.".to_string());
                    return None;
                };
                let symbol = symbol.to_string();
                self.asked = Some((scope, symbol.clone()));
                Some(Search::Symbols(symbol))
            }
            Scope::Commands | Scope::Places | Scope::Launch => None,
        }
    }

    /// A failed search settles the query it was asked for, with nothing
    /// listed; a failure for a query since replaced is dropped.
    pub fn fail_search(&mut self, scope: Scope, query: &str) {
        if self.parsed() != (scope, query) {
            return;
        }
        self.answer_for = Some((scope, query.to_string()));
        self.files.clear();
        self.symbols.clear();
    }

    /// Drops the held answer: the checkout it was asked in is no longer
    /// the window's.
    pub fn forget_files(&mut self) {
        self.files.clear();
        self.symbols.clear();
        self.answer_for = None;
        self.asked = None;
        self.asked_in = None;
    }

    pub fn apply_search(&mut self, scope: Scope, query: &str, paths: Vec<String>) {
        if self.parsed() != (scope, query) {
            return;
        }
        self.answer_for = Some((scope, query.to_string()));
        self.files = paths;
        self.symbols.clear();
        self.error = None;
        self.clamp_selection();
    }

    pub fn apply_symbols(&mut self, scope: Scope, query: &str, hits: Vec<SymbolHit>) {
        if self.parsed() != (scope, query) {
            return;
        }
        self.answer_for = Some((scope, query.to_string()));
        self.symbols = hits;
        self.files.clear();
        self.error = None;
        self.clamp_selection();
    }

    /// Sets the inline note when it changed; true then, so the caller can
    /// raise one toast rather than one per keystroke.
    pub fn note_error(&mut self, message: impl Into<String>) -> bool {
        let message = message.into();
        if self.error.as_deref() == Some(message.as_str()) {
            return false;
        }
        self.error = Some(message);
        true
    }

    pub fn clear_error(&mut self) {
        self.error = None;
    }

    #[must_use]
    pub fn empty_message(&self) -> &'static str {
        let (scope, text) = self.parsed();
        if scope == Scope::Symbols {
            if text.is_empty() {
                "Type a symbol name."
            } else if symbol_text(text).is_none() {
                "A symbol is letters, digits, _ or $."
            } else if self.searching() {
                "Searching declarations…"
            } else {
                "No declaration found."
            }
        } else if self.searching() {
            "Searching files…"
        } else {
            "Nothing matches."
        }
    }

    pub fn move_selection(&mut self, delta: isize) {
        let count = self.entries().len();
        if count == 0 {
            self.selected = 0;
            return;
        }
        let next = (self.selected as isize + delta).rem_euclid(count as isize);
        self.selected = next as usize;
    }

    fn clamp_selection(&mut self) {
        self.selected = self.selected.min(self.entries().len().saturating_sub(1));
    }

    #[must_use]
    pub fn chosen(&self) -> Option<Entry> {
        self.entries().into_iter().nth(self.selected)
    }
}

fn rank_entries(entries: Vec<Entry>, query: &str) -> Vec<Entry> {
    let mut labels = Vec::with_capacity(entries.len());
    for entry in &entries {
        labels.push(entry.search_text());
    }
    let order = crate::fuzzy::rank(labels.iter().map(String::as_str), query);
    let mut slots: Vec<Option<Entry>> = entries.into_iter().map(Some).collect();
    order
        .into_iter()
        .filter_map(|index| slots[index].take())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::WorkspaceId;

    fn typed(palette: &mut Palette, text: &str) {
        palette.edit = TextEdit::new(text);
    }

    #[test]
    fn a_prefix_narrows_the_palette_and_stays_in_the_field() {
        assert_eq!(
            parse_query(Scope::Everything, ">settle"),
            (Scope::Commands, "settle")
        );
        assert_eq!(
            parse_query(Scope::Everything, "  /src/m"),
            (Scope::Files, "src/m")
        );
        assert_eq!(
            parse_query(Scope::Everything, "@forge"),
            (Scope::Places, "forge")
        );
        assert_eq!(
            parse_query(Scope::Everything, "#main"),
            (Scope::Symbols, "main")
        );
        assert_eq!(
            parse_query(Scope::Files, "main"),
            (Scope::Files, "main"),
            "no prefix keeps the scope the shortcut opened"
        );
        assert_eq!(
            parse_query(Scope::Files, ""),
            (Scope::Files, ""),
            "backspacing the prefix returns to the opened scope"
        );
    }

    #[test]
    fn a_symbol_is_a_fixed_identifier_and_never_a_pattern() {
        assert_eq!(symbol_text("  main "), Some("main"));
        assert_eq!(symbol_text("std::io"), Some("std::io"));
        assert_eq!(symbol_text("_hidden"), Some("_hidden"));
        assert_eq!(symbol_text("foo.*"), None);
        assert_eq!(symbol_text("a-b"), None);
        assert_eq!(symbol_text("("), None);
    }

    #[test]
    fn commands_filter_as_a_subsequence() {
        let mut palette = Palette::default();
        palette.show(Scope::Everything);
        assert!(palette.entries().len() > 5);
        typed(&mut palette, "projects");
        let labels: Vec<String> = palette
            .entries()
            .iter()
            .map(|e| e.label().to_string())
            .collect();
        assert_eq!(labels, vec!["Projects".to_string()]);
    }

    #[test]
    fn commands_are_ranked_best_first() {
        let mut palette = Palette::default();
        palette.show(Scope::Everything);
        typed(&mut palette, "nt");
        let first = palette.entries().first().map(|e| e.label().to_string());
        assert_eq!(first.as_deref(), Some("New Terminal"), "a word start wins");
    }

    #[test]
    fn a_command_scope_never_lists_sessions_or_files() {
        let mut palette = Palette::default();
        palette.show(Scope::Everything);
        palette.set_places(vec![Entry::Session {
            id: "00000000-0000-7000-8000-000000000001".parse().unwrap(),
            label: "Projects session".into(),
            note: "main".into(),
            agent: None,
            kind: SessionKind::Shell,
            waiting: false,
        }]);
        palette.files = vec!["src/Projects.rs".into()];
        typed(&mut palette, ">projects");
        assert!(
            palette
                .entries()
                .iter()
                .all(|entry| matches!(entry, Entry::Command { .. })),
            "the scope is the filter"
        );
    }

    #[test]
    fn files_are_asked_once_and_only_the_current_answer_is_listed() {
        let mut palette = Palette::default();
        palette.show(Scope::Files);
        assert_eq!(palette.take_search(), None, "nothing typed, nothing asked");
        typed(&mut palette, "main");
        assert_eq!(palette.take_search(), Some(Search::Files("main".into())));
        assert_eq!(palette.take_search(), None);
        palette.apply_search(Scope::Files, "mai", vec!["old.rs".into()]);
        assert!(
            palette.entries().is_empty(),
            "an answer to an older query is dropped"
        );
        palette.apply_search(Scope::Files, "main", vec!["src/main.rs".into()]);
        assert_eq!(palette.entries(), vec![Entry::File("src/main.rs".into())]);
    }

    #[test]
    fn the_last_answer_is_held_while_the_next_is_in_flight() {
        let mut palette = Palette::default();
        palette.show(Scope::Files);
        typed(&mut palette, "mai");
        assert_eq!(palette.take_search(), Some(Search::Files("mai".into())));
        palette.apply_search(Scope::Files, "mai", vec!["src/main.rs".into()]);
        typed(&mut palette, "main");
        assert_eq!(palette.take_search(), Some(Search::Files("main".into())));
        assert!(
            palette.searching(),
            "busy until the asked query is answered"
        );
        assert_eq!(
            palette.entries(),
            vec![Entry::File("src/main.rs".into())],
            "the previous answer stays while the next is asked"
        );
        palette.apply_search(
            Scope::Files,
            "main",
            vec!["src/main.rs".into(), "main.c".into()],
        );
        assert!(!palette.searching());
        assert_eq!(palette.entries().len(), 2, "replaced when the reply lands");
        typed(&mut palette, "");
        assert!(
            palette.entries().is_empty(),
            "a cleared query lists nothing"
        );
    }

    #[test]
    fn a_failure_settles_only_its_own_query() {
        let mut palette = Palette::default();
        palette.show(Scope::Files);
        typed(&mut palette, "main");
        assert_eq!(palette.take_search(), Some(Search::Files("main".into())));
        palette.fail_search(Scope::Files, "mai");
        assert!(
            palette.searching(),
            "a failure for a replaced query is dropped"
        );
        palette.fail_search(Scope::Files, "main");
        assert!(!palette.searching(), "its own failure settles it");
        assert!(palette.entries().is_empty());
        palette.apply_search(Scope::Files, "main", vec!["a.rs".into()]);
        palette.forget_files();
        assert!(
            palette.entries().is_empty(),
            "another checkout's answer is dropped"
        );
    }

    #[test]
    fn an_empty_file_query_lists_recent_files_until_something_is_typed() {
        let mut palette = Palette::default();
        palette.show(Scope::Files);
        palette.set_recent(vec!["src/lib.rs".into(), "README.md".into()]);
        assert_eq!(
            palette.entries(),
            vec![
                Entry::File("src/lib.rs".into()),
                Entry::File("README.md".into())
            ]
        );
        typed(&mut palette, "zz");
        assert!(
            palette.entries().is_empty(),
            "typing asks the daemon instead"
        );
        palette.show(Scope::Everything);
        assert!(
            palette
                .entries()
                .iter()
                .all(|entry| matches!(entry, Entry::Command { .. })),
            "the command palette opens on commands"
        );
    }

    #[test]
    fn a_place_scope_lists_sessions_and_checkouts_and_focuses_each() {
        let mut palette = Palette::default();
        palette.show(Scope::Places);
        palette.set_places(vec![
            Entry::Session {
                id: "00000000-0000-7000-8000-000000000001".parse().unwrap(),
                label: "Forge shell".into(),
                note: "forge-node · main".into(),
                agent: None,
                kind: SessionKind::Shell,
                waiting: false,
            },
            Entry::Checkout {
                id: WorkspaceId::new(),
                label: "main".into(),
                note: "Forge Node".into(),
            },
        ]);
        assert_eq!(
            palette
                .entries()
                .iter()
                .map(|entry| entry.label())
                .collect::<Vec<_>>(),
            vec!["Forge shell", "main"],
            "both places are listed in the order the replica offered them"
        );
        typed(&mut palette, "shell");
        assert_eq!(
            palette
                .entries()
                .iter()
                .map(|entry| entry.label())
                .collect::<Vec<_>>(),
            vec!["Forge shell"],
            "a session is found by its title"
        );
    }

    #[test]
    fn a_symbol_scope_asks_for_a_definition_and_shows_the_line_it_lands_on() {
        let mut palette = Palette::default();
        palette.show(Scope::Everything);
        typed(&mut palette, "#rel");
        assert_eq!(palette.scope(), Scope::Symbols);
        assert_eq!(palette.take_search(), Some(Search::Symbols("rel".into())));
        palette.apply_symbols(
            Scope::Symbols,
            "rel",
            vec![SymbolHit {
                path: "src/main.rs".into(),
                line: 12,
                text: "fn release() {".into(),
            }],
        );
        assert_eq!(
            palette.chosen(),
            Some(Entry::Symbol(SymbolHit {
                path: "src/main.rs".into(),
                line: 12,
                text: "fn release() {".into(),
            }))
        );
        assert_eq!(
            palette.chosen().unwrap().search_text(),
            "src/main.rs fn release() {"
        );
    }

    #[test]
    fn a_regex_shaped_symbol_is_refused_without_asking_the_daemon() {
        let mut palette = Palette::default();
        palette.show(Scope::Symbols);
        typed(&mut palette, ".*");
        assert_eq!(palette.take_search(), None);
        assert_eq!(
            palette.empty_message(),
            "A symbol is letters, digits, _ or $."
        );
        assert_eq!(palette.chip(), Some(('#', "Symbol")));
    }

    #[test]
    fn every_scope_has_its_own_placeholder_and_chip() {
        let mut palette = Palette::default();
        palette.show(Scope::Commands);
        assert_eq!(palette.placeholder(), "Run a command…");
        assert_eq!(palette.chip(), Some(('>', "Command")));
        palette.show(Scope::Places);
        assert_eq!(palette.placeholder(), "Go to a session or a checkout…");
        assert_eq!(palette.chip(), Some(('@', "Go to")));
        palette.show(Scope::Everything);
        assert_eq!(palette.placeholder(), "Type a command…");
        assert_eq!(palette.chip(), None);
        palette.show(Scope::Launch);
        assert_eq!(palette.placeholder(), "Start a terminal or an agent…");
    }

    #[test]
    fn the_selection_wraps() {
        let mut palette = Palette::default();
        palette.show(Scope::Everything);
        let count = palette.entries().len();
        palette.move_selection(-1);
        assert_eq!(palette.selected, count - 1);
        palette.move_selection(1);
        assert_eq!(palette.selected, 0);
    }

    #[test]
    fn installed_agents_and_profiles_are_offered_first() {
        let mut palette = Palette::default();
        let claude = domain::AgentProviderId::new("claude");
        let profile = domain::AgentProfileId::new();
        palette.show(Scope::Launch);
        palette.set_agents(vec![
            crate::agent_ops::Choice {
                key: format!("provider:{claude}"),
                label: "Claude Code".into(),
                provider: claude.clone(),
                profile: None,
            },
            crate::agent_ops::Choice {
                key: format!("profile:{profile}"),
                label: "Claude Work".into(),
                provider: claude.clone(),
                profile: Some(profile),
            },
        ]);
        assert_eq!(
            palette.chosen(),
            Some(Entry::Command {
                label: "New Claude Code".into(),
                command: Command::NewAgent(claude, None),
            })
        );
        let labels: Vec<String> = palette
            .entries()
            .iter()
            .map(|e| e.label().to_string())
            .collect();
        assert_eq!(
            labels,
            vec![
                "New Claude Code".to_string(),
                "New Claude Work".to_string(),
                "New Terminal".to_string()
            ],
            "a profile is launchable by name"
        );
        assert_eq!(palette.placeholder(), "Start a terminal or an agent…");
    }
}
