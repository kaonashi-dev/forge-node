//! App-wide chords, matched before a terminal or an editor sees the key, so a
//! bound chord never leaks into a PTY. The chords are Tauri's defaults
//! (`apps/tauri/src/actions/actions.ts`), so the two clients agree.

use rune::{Key, KeyEvent};

use crate::workbench::Panel;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    CommandPalette,
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
    /// The strip's Nth tab, 1-based.
    FocusTab(usize),
    /// A terminal beside the active one.
    SplitPane,
    TerminalZoom(Zoom),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Zoom {
    In,
    Out,
    Reset,
}

impl Action {
    /// Where the chord belongs, for Settings › Keyboard's Context column.
    #[must_use]
    pub fn context(self) -> &'static str {
        match self {
            Action::TogglePanel(_) | Action::CycleViews | Action::ToggleSidebar => "Sidebar",
            Action::NextSession
            | Action::PreviousSession
            | Action::FocusTab(_)
            | Action::CloseSession => "Sessions",
            Action::CommandPalette
            | Action::FilePalette
            | Action::NewShell
            | Action::NewWorktree
            | Action::NewAgent
            | Action::Settings => "Global",
            Action::SplitPane | Action::TerminalZoom(_) => "Terminal",
        }
    }
}

/// One row of Settings › Keyboard: what a chord does, and how it is written.
pub struct Binding {
    pub label: &'static str,
    pub chord: &'static str,
    pub action: Action,
}

/// Every chord `action_for` answers, in the order Settings lists them.
pub const BINDINGS: [Binding; 24] = [
    Binding {
        label: "Command palette",
        chord: "⌘K",
        action: Action::CommandPalette,
    },
    Binding {
        label: "Go to…",
        chord: "⇧⌘P",
        action: Action::CommandPalette,
    },
    Binding {
        label: "Open file",
        chord: "⌘P",
        action: Action::FilePalette,
    },
    Binding {
        label: "New terminal",
        chord: "⌘T",
        action: Action::NewShell,
    },
    Binding {
        label: "New agent",
        chord: "⇧⌘A",
        action: Action::NewAgent,
    },
    Binding {
        label: "New worktree",
        chord: "⇧⌘N",
        action: Action::NewWorktree,
    },
    Binding {
        label: "Close session",
        chord: "⌘W",
        action: Action::CloseSession,
    },
    Binding {
        label: "Settings",
        chord: "⌘,",
        action: Action::Settings,
    },
    Binding {
        label: "Toggle sidebar",
        chord: "⌘B",
        action: Action::ToggleSidebar,
    },
    Binding {
        label: "Projects",
        chord: "⌘1",
        action: Action::TogglePanel(Panel::Projects),
    },
    Binding {
        label: "Files",
        chord: "⌘2",
        action: Action::TogglePanel(Panel::Files),
    },
    Binding {
        label: "Next sidebar view",
        chord: "⌘3",
        action: Action::CycleViews,
    },
    Binding {
        label: "History",
        chord: "⇧⌘E",
        action: Action::TogglePanel(Panel::History),
    },
    Binding {
        label: "Pull requests",
        chord: "⇧⌘R",
        action: Action::TogglePanel(Panel::PullRequests),
    },
    Binding {
        label: "Git",
        chord: "⇧⌘G",
        action: Action::TogglePanel(Panel::Git),
    },
    Binding {
        label: "Next session",
        chord: "⇧⌘]",
        action: Action::NextSession,
    },
    Binding {
        label: "Previous session",
        chord: "⇧⌘[",
        action: Action::PreviousSession,
    },
    Binding {
        label: "Next session",
        chord: "⌃Tab",
        action: Action::NextSession,
    },
    Binding {
        label: "Previous session",
        chord: "⌃⇧Tab",
        action: Action::PreviousSession,
    },
    Binding {
        label: "Session tab 1–9",
        chord: "⌥1…9",
        action: Action::FocusTab(1),
    },
    Binding {
        label: "Split pane",
        chord: "⌘D",
        action: Action::SplitPane,
    },
    Binding {
        label: "Terminal: Zoom In",
        chord: "⌘=",
        action: Action::TerminalZoom(Zoom::In),
    },
    Binding {
        label: "Terminal: Zoom Out",
        chord: "⌘-",
        action: Action::TerminalZoom(Zoom::Out),
    },
    Binding {
        label: "Terminal: Actual Size",
        chord: "⌘0",
        action: Action::TerminalZoom(Zoom::Reset),
    },
];

/// What Option and a digit type on a US layout. Matched by the character,
/// not the key, because Rune reports characters: a layout where Option-2 is
/// `@` (Spanish, among others) keeps typing `@` instead of losing it to a
/// tab switch.
const OPTION_DIGITS: [char; 9] = ['¡', '™', '£', '¢', '∞', '§', '¶', '•', 'ª'];

#[must_use]
pub fn action_for(event: &KeyEvent) -> Option<Action> {
    let mods = event.mods;
    match &event.key {
        // macOS keeps Cmd-Tab for its app switcher, so the held switch is Ctrl.
        Key::Tab if mods.ctrl && !mods.alt && !mods.cmd => Some(if mods.shift {
            Action::PreviousSession
        } else {
            Action::NextSession
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
        _ => None,
    }
}

fn command(ch: char, shift: bool) -> Option<Action> {
    Some(match (ch, shift) {
        ('k', false) => Action::CommandPalette,
        // Go To and Find Command open the one palette Rune has.
        ('o' | 'p', true) => Action::CommandPalette,
        ('p' | 'o', false) => Action::FilePalette,
        ('t', false) => Action::NewShell,
        ('n', true) => Action::NewWorktree,
        ('w', false) => Action::CloseSession,
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

    fn chord(ch: char, shift: bool) -> KeyEvent {
        KeyEvent {
            key: Key::Command(ch),
            mods: rune::Mods {
                cmd: true,
                shift,
                ..rune::Mods::default()
            },
        }
    }

    fn with(key: Key, mods: rune::Mods) -> KeyEvent {
        KeyEvent { key, mods }
    }

    #[test]
    fn the_chords_match_tauris_defaults() {
        assert_eq!(action_for(&chord('k', false)), Some(Action::CommandPalette));
        assert_eq!(action_for(&chord('p', false)), Some(Action::FilePalette));
        assert_eq!(action_for(&chord('o', false)), Some(Action::FilePalette));
        assert_eq!(action_for(&chord('p', true)), Some(Action::CommandPalette));
        assert_eq!(action_for(&chord('o', true)), Some(Action::CommandPalette));
        assert_eq!(action_for(&chord('t', false)), Some(Action::NewShell));
        assert_eq!(action_for(&chord('n', true)), Some(Action::NewWorktree));
        assert_eq!(action_for(&chord('w', false)), Some(Action::CloseSession));
        assert_eq!(action_for(&chord('a', true)), Some(Action::NewAgent));
        assert_eq!(action_for(&chord(',', false)), Some(Action::Settings));
        assert_eq!(action_for(&chord('b', false)), Some(Action::ToggleSidebar));
        assert_eq!(
            action_for(&chord('1', false)),
            Some(Action::TogglePanel(Panel::Projects))
        );
        assert_eq!(
            action_for(&chord('2', false)),
            Some(Action::TogglePanel(Panel::Files))
        );
        assert_eq!(action_for(&chord('3', false)), Some(Action::CycleViews));
        assert_eq!(
            action_for(&chord('E', true)),
            Some(Action::TogglePanel(Panel::History))
        );
        assert_eq!(
            action_for(&chord('r', true)),
            Some(Action::TogglePanel(Panel::PullRequests))
        );
        assert_eq!(
            action_for(&chord('g', true)),
            Some(Action::TogglePanel(Panel::Git))
        );
        assert_eq!(action_for(&chord('}', true)), Some(Action::NextSession));
        assert_eq!(action_for(&chord('[', true)), Some(Action::PreviousSession));
        assert_eq!(action_for(&chord('d', false)), Some(Action::SplitPane));
        for plus in [chord('=', false), chord('=', true), chord('+', true)] {
            assert_eq!(action_for(&plus), Some(Action::TerminalZoom(Zoom::In)));
        }
        assert_eq!(
            action_for(&chord('-', false)),
            Some(Action::TerminalZoom(Zoom::Out))
        );
        assert_eq!(
            action_for(&chord('0', false)),
            Some(Action::TerminalZoom(Zoom::Reset))
        );
    }

    #[test]
    fn ctrl_tab_walks_the_tabs_and_option_digits_pick_one() {
        let ctrl = rune::Mods {
            ctrl: true,
            ..rune::Mods::default()
        };
        assert_eq!(action_for(&with(Key::Tab, ctrl)), Some(Action::NextSession));
        assert_eq!(
            action_for(&with(
                Key::Tab,
                rune::Mods {
                    shift: true,
                    ..ctrl
                }
            )),
            Some(Action::PreviousSession)
        );
        assert_eq!(action_for(&with(Key::Tab, rune::Mods::default())), None);
        let alt = rune::Mods {
            alt: true,
            ..rune::Mods::default()
        };
        assert_eq!(
            action_for(&with(Key::Text("¡".into()), alt)),
            Some(Action::FocusTab(1))
        );
        assert_eq!(
            action_for(&with(Key::Text("ª".into()), alt)),
            Some(Action::FocusTab(9))
        );
        assert_eq!(
            action_for(&with(Key::Text("@".into()), alt)),
            None,
            "a layout's Option-2 @ is still typed"
        );
        assert_eq!(action_for(&with(Key::Text("#".into()), alt)), None);
    }

    /// A `BINDINGS` chord as the event it names; the digit range is not one.
    fn parse(chord: &str) -> Option<KeyEvent> {
        let mut mods = rune::Mods::default();
        let mut rest = chord;
        loop {
            let mut chars = rest.chars();
            match chars.next()? {
                '⌘' => mods.cmd = true,
                '⇧' => mods.shift = true,
                '⌃' => mods.ctrl = true,
                '⌥' => mods.alt = true,
                _ => break,
            }
            rest = chars.as_str();
        }
        let key = match rest {
            "Tab" => Key::Tab,
            _ if mods.cmd => {
                let mut chars = rest.chars();
                let ch = chars.next()?;
                chars.next().is_none().then_some(Key::Command(ch))?
            }
            _ => return None,
        };
        Some(KeyEvent { key, mods })
    }

    #[test]
    fn every_listed_chord_is_the_one_that_runs_its_action() {
        for binding in &BINDINGS {
            match parse(binding.chord) {
                Some(event) => assert_eq!(
                    action_for(&event),
                    Some(binding.action),
                    "{} is {}",
                    binding.label,
                    binding.chord
                ),
                None => assert_eq!(binding.action, Action::FocusTab(1)),
            }
        }
    }

    #[test]
    fn unbound_and_plain_keys_go_on() {
        assert_eq!(
            action_for(&chord('c', false)),
            None,
            "copy is the surface's"
        );
        assert_eq!(action_for(&chord('f', false)), None, "find is the editor's");
        let plain = KeyEvent {
            key: Key::Text("k".into()),
            mods: rune::Mods::default(),
        };
        assert_eq!(action_for(&plain), None);
    }
}
