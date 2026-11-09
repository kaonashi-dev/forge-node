//! The command palette: app commands filtered as you type, and files of the
//! active workspace ranked by the daemon (`SearchFiles`, kind `Name`). The
//! daemon owns the ranking and its cached index; this side never walks a disk.

use rune::TextEdit;

use crate::workbench::{CenterMode, Panel};

/// Files asked for per query; a palette shows a screenful, not a listing.
pub const FILE_LIMIT: u32 = 50;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    NewShell,
    Show(CenterMode),
    Panel(crate::workbench::Panel),
    ToggleSidebar,
    NewWorktree,
    /// A base, or `"system"`.
    Theme(String),
    NewAgent(domain::AgentProviderId),
    RunAction(domain::ProjectActionId),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Entry {
    Command { label: String, command: Command },
    File(String),
}

/// The chord that runs a command, shown beside it as Tauri's palette does.
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
        _ => return None,
    })
}

impl Entry {
    #[must_use]
    pub fn label(&self) -> &str {
        match self {
            Entry::Command { label, .. } => label,
            Entry::File(path) => path,
        }
    }
}

pub struct Palette {
    pub open: bool,
    /// Opened with Cmd-P: files only.
    pub files_only: bool,
    /// Opened with Cmd-Shift-A: a terminal or an agent only.
    pub launch: bool,
    pub edit: TextEdit,
    pub selected: usize,
    files: Vec<String>,
    /// Installed agent providers when the palette opened: (id, name).
    agents: Vec<(domain::AgentProviderId, String)>,
    /// The active workspace's approved actions when the palette opened.
    actions: Vec<(domain::ProjectActionId, String)>,
    /// The query `files` answers, so a late answer to an older query is dropped.
    files_for: String,
    asked: Option<String>,
    /// The checkout the last search was asked in; another one's answer is
    /// never listed under it.
    pub asked_in: Option<crate::workbench::Scope>,
}

impl Default for Palette {
    fn default() -> Self {
        Self {
            open: false,
            files_only: false,
            launch: false,
            edit: TextEdit::new(""),
            selected: 0,
            files: Vec::new(),
            agents: Vec::new(),
            actions: Vec::new(),
            files_for: String::new(),
            asked: None,
            asked_in: None,
        }
    }
}

fn commands() -> Vec<Entry> {
    let mut out = vec![
        ("New Terminal", Command::NewShell),
        ("New Worktree", Command::NewWorktree),
        ("Projects", Command::Panel(Panel::Projects)),
        ("Files", Command::Panel(Panel::Files)),
        ("History", Command::Panel(Panel::History)),
        ("Pull Requests", Command::Panel(Panel::PullRequests)),
        ("Git", Command::Panel(Panel::Git)),
        ("Processes", Command::Panel(Panel::Processes)),
        ("Toggle Sidebar", Command::ToggleSidebar),
        ("Settings", Command::Show(CenterMode::Settings)),
        ("Theme: Match System", Command::Theme("system".into())),
    ];
    let mut themes: Vec<(String, Command)> = Vec::new();
    for base in crate::theme::bases() {
        if let Some(theme) = crate::theme::theme(base) {
            themes.push((
                format!("Theme: {}", theme.label),
                Command::Theme(base.clone()),
            ));
        }
    }
    let mut entries: Vec<Entry> = out
        .drain(..)
        .map(|(label, command)| Entry::Command {
            label: label.to_string(),
            command,
        })
        .collect();
    entries.extend(
        themes
            .into_iter()
            .map(|(label, command)| Entry::Command { label, command }),
    );
    entries
}

impl Palette {
    pub fn show(&mut self, files_only: bool) {
        self.show_with(files_only, "", Vec::new());
    }

    /// Opens with `query` typed and the agents a person may start.
    pub fn show_with(
        &mut self,
        files_only: bool,
        query: &str,
        agents: Vec<(domain::AgentProviderId, String)>,
    ) {
        self.agents = agents;
        self.open = true;
        self.files_only = files_only;
        self.launch = false;
        self.edit = TextEdit::new(query);
        self.edit.focused = true;
        self.selected = 0;
        self.files.clear();
        self.files_for.clear();
        self.asked = None;
    }

    pub fn show_launch(&mut self, agents: Vec<(domain::AgentProviderId, String)>) {
        self.show_with(false, "", agents);
        self.launch = true;
    }

    /// What the field says before anything is typed, as Tauri words it.
    #[must_use]
    pub fn placeholder(&self) -> &'static str {
        if self.files_only {
            "Open a file in this workspace…"
        } else if self.launch {
            "Start a terminal or an agent…"
        } else {
            "Type a command…"
        }
    }

    /// Whether a file answer for the current query is still on its way.
    #[must_use]
    pub fn searching(&self) -> bool {
        let query = self.query();
        !query.is_empty() && self.files_for != query && !self.launch
    }

    pub fn set_actions(&mut self, actions: Vec<(domain::ProjectActionId, String)>) {
        self.actions = actions;
    }

    pub fn close(&mut self) {
        self.open = false;
        self.edit.focused = false;
    }

    #[must_use]
    pub fn query(&self) -> &str {
        self.edit.text.trim()
    }

    /// What is listed for the current query: matching commands, best first
    /// by `fuzzy::rank`, then the files the daemon ranked for this query.
    #[must_use]
    pub fn entries(&self) -> Vec<Entry> {
        let query = self.query();
        let mut out: Vec<Entry> = if self.files_only {
            Vec::new()
        } else {
            let commands: Vec<Entry> = self
                .agents
                .iter()
                .map(|(id, name)| Entry::Command {
                    label: format!("New {name}"),
                    command: Command::NewAgent(id.clone()),
                })
                .chain(self.actions.iter().map(|(id, label)| Entry::Command {
                    label: format!("Run: {label}"),
                    command: Command::RunAction(*id),
                }))
                .chain(commands())
                .filter(|entry| {
                    !self.launch
                        || matches!(
                            entry,
                            Entry::Command {
                                command: Command::NewShell | Command::NewAgent(_),
                                ..
                            }
                        )
                })
                .collect();
            let order = crate::fuzzy::rank(commands.iter().map(Entry::label), query);
            let mut commands: Vec<Option<Entry>> = commands.into_iter().map(Some).collect();
            order
                .into_iter()
                .filter_map(|index| commands[index].take())
                .collect()
        };
        // The last answer stays while the next is in flight, as Tauri's
        // `nameSearchState` holds it, so typing one more letter does not
        // empty the list and refill it.
        let held = self.files_for == query || self.asked.as_deref() == Some(query);
        if !query.is_empty() && held && !self.launch {
            out.extend(self.files.iter().cloned().map(Entry::File));
        }
        out
    }

    /// The file search to send now, once per query.
    pub fn take_search(&mut self) -> Option<String> {
        if !self.open {
            return None;
        }
        let query = self.query().to_string();
        if query.is_empty() || self.launch || self.asked.as_deref() == Some(query.as_str()) {
            return None;
        }
        self.asked = Some(query.clone());
        Some(query)
    }

    /// A failed search settles the query it was asked for, with nothing
    /// listed; a failure for a query since replaced is dropped.
    pub fn fail_search(&mut self, query: &str) {
        if query != self.query() {
            return;
        }
        self.files_for = query.to_string();
        self.files.clear();
    }

    /// Drops the held answer: the checkout it was asked in is no longer
    /// the window's.
    pub fn forget_files(&mut self) {
        self.files.clear();
        self.files_for.clear();
        self.asked = None;
        self.asked_in = None;
    }

    pub fn apply_search(&mut self, query: &str, paths: Vec<String>) {
        if query != self.query() {
            return;
        }
        self.files_for = query.to_string();
        self.files = paths;
        self.selected = self.selected.min(self.entries().len().saturating_sub(1));
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

    #[must_use]
    pub fn chosen(&self) -> Option<Entry> {
        self.entries().into_iter().nth(self.selected)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typed(palette: &mut Palette, text: &str) {
        palette.edit = TextEdit::new(text);
    }

    #[test]
    fn commands_filter_as_a_subsequence() {
        let mut palette = Palette::default();
        palette.show(false);
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
        palette.show(false);
        typed(&mut palette, "nt");
        let first = palette.entries().first().map(|e| e.label().to_string());
        assert_eq!(first.as_deref(), Some("New Terminal"), "a word start wins");
    }

    #[test]
    fn files_are_asked_once_and_only_the_current_answer_is_listed() {
        let mut palette = Palette::default();
        palette.show(true);
        assert_eq!(palette.take_search(), None, "nothing typed, nothing asked");
        typed(&mut palette, "main");
        assert_eq!(palette.take_search(), Some("main".into()));
        assert_eq!(palette.take_search(), None);
        palette.apply_search("mai", vec!["old.rs".into()]);
        assert!(
            palette.entries().is_empty(),
            "an answer to an older query is dropped"
        );
        palette.apply_search("main", vec!["src/main.rs".into()]);
        assert_eq!(palette.entries(), vec![Entry::File("src/main.rs".into())]);
    }

    #[test]
    fn the_last_answer_is_held_while_the_next_is_in_flight() {
        let mut palette = Palette::default();
        palette.show(true);
        typed(&mut palette, "mai");
        assert_eq!(palette.take_search(), Some("mai".into()));
        palette.apply_search("mai", vec!["src/main.rs".into()]);
        typed(&mut palette, "main");
        assert_eq!(palette.take_search(), Some("main".into()));
        assert!(
            palette.searching(),
            "busy until the asked query is answered"
        );
        assert_eq!(
            palette.entries(),
            vec![Entry::File("src/main.rs".into())],
            "the previous answer stays while the next is asked"
        );
        palette.apply_search("main", vec!["src/main.rs".into(), "main.c".into()]);
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
        palette.show(true);
        typed(&mut palette, "main");
        assert_eq!(palette.take_search(), Some("main".into()));
        palette.fail_search("mai");
        assert!(
            palette.searching(),
            "a failure for a replaced query is dropped"
        );
        palette.fail_search("main");
        assert!(!palette.searching(), "its own failure settles it");
        assert!(palette.entries().is_empty());
        palette.apply_search("main", vec!["a.rs".into()]);
        palette.forget_files();
        assert!(
            palette.entries().is_empty(),
            "another checkout's answer is dropped"
        );
    }

    #[test]
    fn the_selection_wraps() {
        let mut palette = Palette::default();
        palette.show(false);
        let count = palette.entries().len();
        palette.move_selection(-1);
        assert_eq!(palette.selected, count - 1);
        palette.move_selection(1);
        assert_eq!(palette.selected, 0);
    }

    #[test]
    fn installed_agents_are_offered_first() {
        let mut palette = Palette::default();
        let claude = domain::AgentProviderId::new("claude");
        palette.show_launch(vec![(claude.clone(), "Claude Code".into())]);
        assert_eq!(
            palette.chosen(),
            Some(Entry::Command {
                label: "New Claude Code".into(),
                command: Command::NewAgent(claude),
            })
        );
        let labels: Vec<String> = palette
            .entries()
            .iter()
            .map(|e| e.label().to_string())
            .collect();
        assert_eq!(
            labels,
            vec!["New Claude Code".to_string(), "New Terminal".to_string()]
        );
        assert_eq!(palette.placeholder(), "Start a terminal or an agent…");
    }
}
