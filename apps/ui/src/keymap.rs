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
}

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
