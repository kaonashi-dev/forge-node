//! The command palette: app commands filtered as you type, and files of the
//! active workspace ranked by the daemon (`SearchFiles`, kind `Name`). The
//! daemon owns the ranking and its cached index; this side never walks a disk.

use rune::TextEdit;

use crate::workbench::CenterMode;

/// Files asked for per query; a palette shows a screenful, not a listing.
pub const FILE_LIMIT: u32 = 50;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    NewShell,
    RefreshSnapshot,
    Reconnect,
    Show(CenterMode),
    /// `None` follows the system.
    Theme(Option<String>),
    NewAgent(domain::AgentProviderId),
    RunAction(domain::ProjectActionId),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Entry {
    Command { label: String, command: Command },
    File(String),
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
}

impl Default for Palette {
    fn default() -> Self {
        Self {
            open: false,
            files_only: false,
            edit: TextEdit::new(""),
            selected: 0,
            files: Vec::new(),
            agents: Vec::new(),
            actions: Vec::new(),
            files_for: String::new(),
            asked: None,
        }
    }
}

fn commands() -> Vec<Entry> {
    let mut out = vec![
        ("New shell", Command::NewShell),
        ("Show session", Command::Show(CenterMode::Session)),
        ("Show changes", Command::Show(CenterMode::Changes)),
        ("Show files", Command::Show(CenterMode::Files)),
        ("Open settings", Command::Show(CenterMode::Settings)),
        ("Refresh sessions", Command::RefreshSnapshot),
        ("Reconnect to the daemon", Command::Reconnect),
        ("Theme: follow the system", Command::Theme(None)),
    ];
    let mut themes: Vec<(String, Command)> = Vec::new();
    for base in crate::theme::bases() {
        if let Some(theme) = crate::theme::theme(base) {
            themes.push((
                format!("Theme: {}", theme.label),
                Command::Theme(Some(base.clone())),
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

/// Whether every character of `query` appears in `label`, in order, ignoring case.
fn subsequence(label: &str, query: &str) -> bool {
    let mut chars = label.chars().flat_map(char::to_lowercase);
    query
        .chars()
        .flat_map(char::to_lowercase)
        .all(|wanted| chars.any(|have| have == wanted))
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
        self.edit = TextEdit::new(query);
        self.edit.focused = true;
        self.selected = 0;
        self.files.clear();
        self.files_for.clear();
        self.asked = None;
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

    /// What is listed for the current query: matching commands, then the
    /// files the daemon returned for exactly this query.
    #[must_use]
    pub fn entries(&self) -> Vec<Entry> {
        let query = self.query();
        let mut out: Vec<Entry> = if self.files_only {
            Vec::new()
        } else {
            self.agents
                .iter()
                .map(|(id, name)| Entry::Command {
                    label: format!("New agent: {name}"),
                    command: Command::NewAgent(id.clone()),
                })
                .chain(self.actions.iter().map(|(id, label)| Entry::Command {
                    label: format!("Run: {label}"),
                    command: Command::RunAction(*id),
                }))
                .chain(commands())
                .filter(|entry| subsequence(entry.label(), query))
                .collect()
        };
        if !query.is_empty() && self.files_for == query {
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
        if query.is_empty() || self.asked.as_deref() == Some(query.as_str()) {
            return None;
        }
        self.asked = Some(query.clone());
        Some(query)
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
        typed(&mut palette, "shch");
        let labels: Vec<String> = palette
            .entries()
            .iter()
            .map(|e| e.label().to_string())
            .collect();
        assert_eq!(labels, vec!["Show changes".to_string()]);
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
        palette.show_with(false, "agent", vec![(claude.clone(), "Claude Code".into())]);
        assert_eq!(
            palette.chosen(),
            Some(Entry::Command {
                label: "New agent: Claude Code".into(),
                command: Command::NewAgent(claude),
            })
        );
    }
}
