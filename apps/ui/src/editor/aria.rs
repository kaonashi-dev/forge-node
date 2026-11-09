//! What a screen reader is told about the editor, from `Session.editor` alone.
//!
//! The wording matches Tauri's `editorAria.ts`, so the two clients announce the
//! same thing for the same state. Nothing here reads the rows: the caret's line
//! is the one piece of document text the editor publishes for this purpose.

use domain::EditorState;

/// Said instead of a position before the first state arrives.
pub const NO_STATE: &str = "The editor has not reported its state yet.";

#[derive(Debug, PartialEq)]
pub struct EditorAria {
    /// The caret's line and what is on it, read on every move.
    pub line: String,
    /// Position, flags and selection, as one announceable sentence.
    pub status: String,
    pub read_only: bool,
}

/// Describes the buffer. `None` is not an empty buffer: until the editor
/// reports, inventing `1:1` would be indistinguishable from a real caret.
#[must_use]
pub fn editor_aria(state: Option<&EditorState>) -> EditorAria {
    let Some(state) = state else {
        return EditorAria {
            line: NO_STATE.to_string(),
            status: NO_STATE.to_string(),
            read_only: false,
        };
    };
    let mut flags: Vec<String> = Vec::new();
    if state.read_only {
        flags.push("read only".into());
    }
    flags.push(
        if state.dirty {
            "unsaved changes"
        } else {
            "saved"
        }
        .into(),
    );
    if state.cursor_count > 1 {
        flags.push(format!("{} cursors", state.cursor_count));
    }
    if state.selection_length > 0 {
        let unit = if state.selection_length == 1 {
            "byte"
        } else {
            "bytes"
        };
        flags.push(format!("{} {unit} selected", state.selection_length));
    }
    EditorAria {
        // A reader that said nothing for an empty line would sound like one
        // that failed to announce.
        line: if state.caret_line.trim().is_empty() {
            "blank line".to_string()
        } else {
            state.caret_line.clone()
        },
        status: format!(
            "Line {} of {}, column {}. {}.",
            state.line,
            state.total_lines,
            state.column,
            flags.join(", ")
        ),
        read_only: state.read_only,
    }
}

/// What the live region says, or `""` for silence: the editor's answer to the
/// gesture just made, or a conflict, which the daemon saw and the editor did not.
#[must_use]
pub fn editor_announcement(state: Option<&EditorState>) -> String {
    match state {
        None => String::new(),
        Some(state) if state.conflict => {
            "This file changed on disk while you were editing it. Keep mine, take disk, or compare."
                .to_string()
        }
        Some(state) => state.status.trim().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> EditorState {
        EditorState {
            path: "src/main.rs".into(),
            line: 12,
            column: 4,
            total_lines: 120,
            caret_line: "    let total = 1;".into(),
            cursor_count: 1,
            ..EditorState::default()
        }
    }

    #[test]
    fn names_the_position_and_the_line() {
        let aria = editor_aria(Some(&state()));
        assert!(aria.status.contains("Line 12 of 120"));
        assert!(aria.status.contains("column 4"));
        assert!(aria.status.contains("saved"));
        assert_eq!(aria.line, "    let total = 1;");
        assert!(!aria.read_only);
    }

    #[test]
    fn says_a_blank_line_is_blank() {
        let blank = EditorState {
            caret_line: "   ".into(),
            ..state()
        };
        assert_eq!(editor_aria(Some(&blank)).line, "blank line");
    }

    #[test]
    fn announces_the_flags_a_person_cannot_see() {
        let busy = EditorState {
            dirty: true,
            read_only: true,
            cursor_count: 3,
            selection_length: 7,
            ..state()
        };
        let aria = editor_aria(Some(&busy));
        for flag in [
            "read only",
            "unsaved changes",
            "3 cursors",
            "7 bytes selected",
        ] {
            assert!(aria.status.contains(flag), "{flag}: {}", aria.status);
        }
        assert!(aria.read_only);
        let one = EditorState {
            selection_length: 1,
            ..state()
        };
        assert!(editor_aria(Some(&one)).status.contains("1 byte selected"));
    }

    #[test]
    fn says_the_state_is_unknown_rather_than_inventing_a_position() {
        let aria = editor_aria(None);
        assert_eq!(aria.line, NO_STATE);
        assert_eq!(aria.status, NO_STATE);
    }

    #[test]
    fn a_conflict_outranks_the_editors_own_message() {
        let quiet = state();
        assert_eq!(editor_announcement(Some(&quiet)), "");
        let found = EditorState {
            status: " 3 of 9 ".into(),
            ..state()
        };
        assert_eq!(editor_announcement(Some(&found)), "3 of 9");
        let moved = EditorState {
            conflict: true,
            ..found
        };
        assert!(editor_announcement(Some(&moved)).starts_with("This file changed on disk"));
    }
}
