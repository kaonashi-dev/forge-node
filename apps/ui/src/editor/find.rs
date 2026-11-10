//! Find in the open file. The editor owns the query, the search and the
//! marks; the panel sends what was typed (`EditorFind`) and draws the
//! `EditorState.find` the editor reports back.
//!
//! The field keeps its own text while typing, because the state that echoes a
//! keystroke arrives after the next one.

use domain::{EditorFind, EditorFindCommand, MAX_EDITOR_FIND_PATTERN_BYTES, SessionId};
use rune::{Key, KeyEvent, TextEdit};

#[derive(Default)]
pub struct FindField {
    pub edit: TextEdit,
    /// The open gesture the field last answered, so a second ⌘F refocuses it.
    seen: Option<(SessionId, u32)>,
}

impl FindField {
    /// Follows the panel the editor reports: each open gesture puts its
    /// pattern in the field, selected, with the keyboard there.
    pub fn follow(&mut self, session: Option<SessionId>, find: Option<&EditorFind>) {
        let (Some(session), Some(find)) = (session, find) else {
            self.seen = None;
            self.edit.focused = false;
            return;
        };
        if self.seen == Some((session, find.focus)) {
            return;
        }
        self.seen = Some((session, find.focus));
        let mut edit = TextEdit::new(find.pattern.clone());
        edit.set_selection(0, find.pattern.len());
        edit.focused = true;
        self.edit = edit;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flag {
    Case,
    Word,
    Regex,
}

/// The three toggles: what the button says, and what it is called.
pub const TOGGLES: [(Flag, &str, &str); 3] = [
    (Flag::Case, "Aa", "Match case"),
    (Flag::Word, "ab", "Match whole word"),
    (Flag::Regex, ".*", "Use regular expression"),
];

#[must_use]
pub fn flag(find: &EditorFind, which: Flag) -> bool {
    match which {
        Flag::Case => find.case_sensitive,
        Flag::Word => find.whole_word,
        Flag::Regex => find.regex,
    }
}

/// Whether the daemon would refuse `pattern`, counted in UTF-8 bytes as it counts.
#[must_use]
pub fn too_long(pattern: &str) -> bool {
    pattern.len() > MAX_EDITOR_FIND_PATTERN_BYTES
}

/// What the counter says: `3 of 12`, `12 results`, `No results`, or why not.
#[must_use]
pub fn counter(find: &EditorFind, typed: &str) -> String {
    // The refusal never reaches the editor, so its state still counts the
    // last pattern it took.
    if too_long(typed) {
        return "Pattern is too long".into();
    }
    if let Some(error) = &find.error {
        return error.clone();
    }
    if typed.is_empty() {
        return String::new();
    }
    if find.total == 0 {
        return "No results".into();
    }
    let total = format!("{}{}", find.total, if find.capped { "+" } else { "" });
    if find.index > 0 {
        return format!("{} of {total}", find.index);
    }
    if find.total == 1 && !find.capped {
        "1 result".into()
    } else {
        format!("{total} results")
    }
}

/// A search for `pattern` with the panel's flags, one of them flipped.
#[must_use]
pub fn set(find: &EditorFind, pattern: &str, flip: Option<Flag>) -> EditorFindCommand {
    let flipped = |which| flag(find, which) != (flip == Some(which));
    EditorFindCommand::Set {
        pattern: pattern.to_string(),
        case_sensitive: flipped(Flag::Case),
        whole_word: flipped(Flag::Word),
        regex: flipped(Flag::Regex),
    }
}

/// The keys the field answers itself; anything else types into it. An
/// input method holds Enter and Escape while it composes, so neither
/// reaches this.
#[must_use]
pub fn key_command(ev: &KeyEvent) -> Option<EditorFindCommand> {
    let m = ev.mods;
    match &ev.key {
        Key::Enter if !m.cmd && !m.ctrl && !m.alt => Some(if m.shift {
            EditorFindCommand::Previous
        } else {
            EditorFindCommand::Next
        }),
        Key::Escape => Some(EditorFindCommand::Close),
        Key::Command('g') if !m.alt && !m.ctrl => Some(if m.shift {
            EditorFindCommand::Previous
        } else {
            EditorFindCommand::Next
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(total: u32, index: u32, capped: bool) -> EditorFind {
        EditorFind {
            pattern: "x".into(),
            total,
            index,
            capped,
            ..EditorFind::default()
        }
    }

    #[test]
    fn the_counter_reads_as_the_original_client() {
        assert_eq!(counter(&found(12, 3, false), "x"), "3 of 12");
        assert_eq!(counter(&found(12, 0, false), "x"), "12 results");
        assert_eq!(counter(&found(5000, 0, true), "x"), "5000+ results");
        assert_eq!(counter(&found(1, 0, false), "x"), "1 result");
        assert_eq!(counter(&found(0, 0, false), "x"), "No results");
        assert_eq!(counter(&found(0, 0, false), ""), "");
        let mut invalid = found(0, 0, false);
        invalid.error = Some("unclosed group".into());
        assert_eq!(counter(&invalid, "("), "unclosed group");
        let long = "é".repeat(MAX_EDITOR_FIND_PATTERN_BYTES / 2 + 1);
        assert!(too_long(&long), "counted in bytes, not characters");
        assert_eq!(counter(&found(3, 1, false), &long), "Pattern is too long");
    }

    #[test]
    fn a_toggle_flips_one_flag_and_keeps_the_typed_pattern() {
        let mut find = found(0, 0, false);
        find.whole_word = true;
        assert_eq!(
            set(&find, "typed", Some(Flag::Regex)),
            EditorFindCommand::Set {
                pattern: "typed".into(),
                case_sensitive: false,
                whole_word: true,
                regex: true,
            }
        );
        assert_eq!(
            set(&find, "typed", Some(Flag::Word)),
            EditorFindCommand::Set {
                pattern: "typed".into(),
                case_sensitive: false,
                whole_word: false,
                regex: false,
            }
        );
    }

    #[test]
    fn each_open_gesture_refocuses_the_field_once() {
        let session = SessionId::new();
        let mut field = FindField::default();
        let mut find = found(0, 0, false);
        find.pattern = "needle".into();
        find.focus = 1;
        field.follow(Some(session), Some(&find));
        assert!(field.edit.focused);
        assert_eq!(field.edit.text, "needle");
        assert_eq!(
            field.edit.range(),
            (0, 6),
            "selected, so typing replaces it"
        );
        field.edit.focused = false;
        field.edit.text = "need".into();
        field.follow(Some(session), Some(&find));
        assert_eq!(field.edit.text, "need", "an echo does not overwrite typing");
        assert!(!field.edit.focused);
        find.focus = 2;
        field.follow(Some(session), Some(&find));
        assert!(field.edit.focused);
        assert_eq!(field.edit.text, "needle");
    }

    #[test]
    fn enter_steps_escape_closes_and_letters_type() {
        let key = |key, shift| KeyEvent {
            key,
            mods: rune::Mods {
                shift,
                ..rune::Mods::default()
            },
        };
        assert_eq!(
            key_command(&key(Key::Enter, false)),
            Some(EditorFindCommand::Next)
        );
        assert_eq!(
            key_command(&key(Key::Enter, true)),
            Some(EditorFindCommand::Previous)
        );
        assert_eq!(
            key_command(&key(Key::Escape, false)),
            Some(EditorFindCommand::Close)
        );
        assert_eq!(key_command(&key(Key::Text("n".into()), false)), None);
    }
}
