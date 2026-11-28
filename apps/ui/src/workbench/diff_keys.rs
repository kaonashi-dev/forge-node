//! The diff's own keys, as rune-kit's `DiffState::key` maps them, for a
//! window whose focus proxy holds focus while no field does: that method
//! refuses every key while anything is focused, the proxy included.

use rune::{Key, KeyEvent, Tx};
use rune_kit::diff::DiffState;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffKey {
    Hunk { forward: bool },
    File { forward: bool },
    ClearSelection,
    Copy,
}

/// `j`/`k` move between hunks and `]`/`[` between files while no field has
/// the keyboard; Escape clears a line selection and Cmd-C copies it.
#[must_use]
pub fn diff_key(ev: &KeyEvent, field_focused: bool, has_selection: bool) -> Option<DiffKey> {
    if field_focused {
        return None;
    }
    if ev.is_command('c') {
        return has_selection.then_some(DiffKey::Copy);
    }
    match &ev.key {
        Key::Text(t) if !ev.mods.cmd && !ev.mods.ctrl && !ev.mods.alt => match t.as_str() {
            "j" => Some(DiffKey::Hunk { forward: true }),
            "k" => Some(DiffKey::Hunk { forward: false }),
            "]" => Some(DiffKey::File { forward: true }),
            "[" => Some(DiffKey::File { forward: false }),
            _ => None,
        },
        Key::Escape if has_selection => Some(DiffKey::ClearSelection),
        _ => None,
    }
}

pub fn apply(state: &mut DiffState, key: DiffKey, tx: &mut Tx) -> bool {
    match key {
        DiffKey::Hunk { forward } => state.next_hunk(forward, tx),
        DiffKey::File { forward } => state.next_file(forward, tx),
        DiffKey::ClearSelection => {
            state.set_selection(None);
            true
        }
        DiffKey::Copy => match state.selected_text() {
            Some(text) => {
                tx.copy(text);
                true
            }
            None => false,
        },
    }
}

#[cfg(test)]
mod tests {
    use rune::Mods;

    use super::*;

    fn press(key: Key, mods: Mods) -> KeyEvent {
        KeyEvent { key, mods }
    }

    #[test]
    fn letters_move_only_while_no_field_has_the_keyboard() {
        let j = press(Key::Text("j".into()), Mods::default());
        assert_eq!(
            diff_key(&j, false, false),
            Some(DiffKey::Hunk { forward: true })
        );
        assert_eq!(diff_key(&j, true, false), None, "typing in a field");
        let shifted = press(
            Key::Text("]".into()),
            Mods {
                cmd: true,
                ..Mods::default()
            },
        );
        assert_eq!(
            diff_key(&shifted, false, false),
            None,
            "a chord is the app's"
        );
    }

    #[test]
    fn escape_and_copy_answer_only_for_a_selection() {
        let escape = press(Key::Escape, Mods::default());
        assert_eq!(diff_key(&escape, false, false), None);
        assert_eq!(
            diff_key(&escape, false, true),
            Some(DiffKey::ClearSelection)
        );
        let copy = press(
            Key::Command('c'),
            Mods {
                cmd: true,
                ..Mods::default()
            },
        );
        assert_eq!(diff_key(&copy, false, false), None);
        assert_eq!(diff_key(&copy, false, true), Some(DiffKey::Copy));
        assert_eq!(
            diff_key(&copy, true, true),
            None,
            "a field copies its own text"
        );
    }
}
