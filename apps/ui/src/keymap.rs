//! App-wide chords, matched before a terminal or an editor sees the key, so a
//! bound chord never leaks into a PTY. The chords are Tauri's defaults
//! (`apps/tauri/src/actions/actions.ts`), so the two clients agree.

use rune::{Key, KeyEvent};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    CommandPalette,
    FilePalette,
    NewShell,
    Settings,
    NextSession,
    PreviousSession,
}

#[must_use]
pub fn action_for(event: &KeyEvent) -> Option<Action> {
    let Key::Command(ch) = event.key else {
        return None;
    };
    let mods = event.mods;
    if mods.alt || mods.ctrl {
        return None;
    }
    Some(match (ch.to_ascii_lowercase(), mods.shift) {
        ('k', false) => Action::CommandPalette,
        ('p', false) => Action::FilePalette,
        ('t', false) => Action::NewShell,
        (',', false) => Action::Settings,
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

    #[test]
    fn the_chords_match_tauris_defaults() {
        assert_eq!(action_for(&chord('k', false)), Some(Action::CommandPalette));
        assert_eq!(action_for(&chord('p', false)), Some(Action::FilePalette));
        assert_eq!(action_for(&chord('t', false)), Some(Action::NewShell));
        assert_eq!(action_for(&chord(',', false)), Some(Action::Settings));
        assert_eq!(action_for(&chord('}', true)), Some(Action::NextSession));
        assert_eq!(action_for(&chord('[', true)), Some(Action::PreviousSession));
    }

    #[test]
    fn unbound_and_plain_keys_go_on() {
        assert_eq!(
            action_for(&chord('c', false)),
            None,
            "copy is the surface's"
        );
        assert_eq!(action_for(&chord('p', true)), None);
        let plain = KeyEvent {
            key: Key::Text("k".into()),
            mods: rune::Mods::default(),
        };
        assert_eq!(action_for(&plain), None);
    }
}
