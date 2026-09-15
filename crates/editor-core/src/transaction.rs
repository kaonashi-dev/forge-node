//! The one shape a mutation may take.
//!
//! Typing, paste, replace-all and a Markdown block edit are the same kind of
//! thing: a set of range replacements applied together or not at all. A path
//! that edits the buffer without building one of these escapes undo and the
//! version counter, which is why `Document` exposes no other entry.

use crate::selection::{Range, Selection};

/// One range replacement. An empty `range` inserts; an empty `insert` deletes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edit {
    pub range: Range,
    pub insert: String,
}

impl Edit {
    #[must_use]
    pub fn insert(at: usize, text: impl Into<String>) -> Self {
        Self {
            range: Range::empty(at),
            insert: text.into(),
        }
    }

    #[must_use]
    pub fn delete(range: Range) -> Self {
        Self {
            range,
            insert: String::new(),
        }
    }

    #[must_use]
    pub fn replace(range: Range, text: impl Into<String>) -> Self {
        Self {
            range,
            insert: text.into(),
        }
    }
}

/// Why an edit happened. History groups by it, and a view can tell a person's
/// keystroke from a preview round trip without inspecting the text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    /// A typed character or a deletion at the caret.
    Input,
    /// Bracketed paste or a clipboard command: one undo unit however long.
    Paste,
    /// A single replacement from find-and-replace.
    Replace,
    /// Replace-all: one undo unit however many matches.
    ReplaceAll,
    /// Reapplied by undo or redo; never recorded again.
    History,
    /// A block edited in a preview surface and sent back.
    Preview,
    /// Content taken from disk after an external change.
    Reload,
}

impl Origin {
    /// Whether two consecutive edits of this origin may share an undo entry.
    #[must_use]
    pub fn coalesces(self) -> bool {
        matches!(self, Origin::Input)
    }
}

/// Edits applied as a unit, with the selection on both sides.
#[derive(Clone, Debug)]
pub struct Transaction {
    edits: Vec<Edit>,
    pub origin: Origin,
    pub selection_before: Selection,
    pub selection_after: Option<Selection>,
}

impl Transaction {
    /// Build a transaction, sorting the edits and refusing overlaps.
    ///
    /// Overlapping ranges have no single defined result, so they are rejected
    /// here rather than resolved by application order.
    pub fn new(
        mut edits: Vec<Edit>,
        origin: Origin,
        selection_before: Selection,
    ) -> Result<Self, EditError> {
        edits.sort_by_key(|edit| (edit.range.start, edit.range.end));
        for pair in edits.windows(2) {
            if pair[0].range.end > pair[1].range.start {
                return Err(EditError::Overlapping);
            }
        }
        Ok(Self {
            edits,
            origin,
            selection_before,
            selection_after: None,
        })
    }

    #[must_use]
    pub fn with_selection_after(mut self, selection: Selection) -> Self {
        self.selection_after = Some(selection);
        self
    }

    #[must_use]
    pub fn edits(&self) -> &[Edit] {
        &self.edits
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.edits
            .iter()
            .all(|edit| edit.range.is_empty() && edit.insert.is_empty())
    }

    /// Bytes this transaction adds, minus those it removes.
    #[must_use]
    pub fn size_delta(&self) -> isize {
        self.edits
            .iter()
            .map(|edit| edit.insert.len() as isize - edit.range.len() as isize)
            .sum()
    }

    /// Text retained if this transaction is kept in history.
    #[must_use]
    pub fn retained_bytes(&self) -> usize {
        self.edits
            .iter()
            .map(|edit| edit.insert.len() + edit.range.len())
            .sum()
    }
}

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum EditError {
    #[error("the buffer is read-only")]
    ReadOnly,
    #[error("edit ranges overlap")]
    Overlapping,
    #[error("range {start}..{end} is outside the document or splits a character")]
    OutOfBounds { start: usize, end: usize },
    #[error("the result would be {size} bytes, over the {limit} byte limit")]
    TooLarge { size: usize, limit: usize },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn selection() -> Selection {
        Selection::caret(0)
    }

    #[test]
    fn edits_are_sorted_and_overlaps_refused() {
        let ok = Transaction::new(
            vec![Edit::insert(5, "b"), Edit::insert(1, "a")],
            Origin::Input,
            selection(),
        )
        .expect("disjoint edits");
        assert_eq!(ok.edits()[0].range.start, 1);

        let clash = Transaction::new(
            vec![
                Edit::delete(Range::new(0, 4)),
                Edit::delete(Range::new(2, 6)),
            ],
            Origin::Input,
            selection(),
        );
        assert!(matches!(clash, Err(EditError::Overlapping)));
    }

    #[test]
    fn touching_ranges_are_not_an_overlap() {
        let transaction = Transaction::new(
            vec![
                Edit::delete(Range::new(0, 2)),
                Edit::delete(Range::new(2, 4)),
            ],
            Origin::ReplaceAll,
            selection(),
        );
        assert!(transaction.is_ok());
    }

    #[test]
    fn size_delta_counts_both_directions() {
        let transaction = Transaction::new(
            vec![Edit::replace(Range::new(0, 4), "ab")],
            Origin::Replace,
            selection(),
        )
        .expect("valid");
        assert_eq!(transaction.size_delta(), -2);
        assert_eq!(transaction.retained_bytes(), 6);
    }
}
