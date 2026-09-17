//! Positions as byte offsets, and the carets a view carries.
//!
//! Byte offsets are the internal unit; grapheme columns and display cells are
//! conversions (`metrics`), never a second source of truth. Line and column in
//! [`LineCol`] are 0-based — the 1-based line a person reads is the view's job.
//!
//! A [`Selection`] is a *set* of [`Cursor`]s, like CodeMirror's: one of them is
//! primary and is what the status line reports and the viewport follows, and a
//! mutation applies to all of them in one transaction. The single-caret case is
//! a set of one and is what almost every buffer has, so the ordering and
//! merging rules below exist to keep the other case from ever producing two
//! carets that overlap.

use crate::limits::MAX_CURSORS;

/// A half-open byte range, always normalized so `start <= end`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Range {
    pub start: usize,
    pub end: usize,
}

impl Range {
    #[must_use]
    pub fn new(start: usize, end: usize) -> Self {
        if start <= end {
            Self { start, end }
        } else {
            Self {
                start: end,
                end: start,
            }
        }
    }

    #[must_use]
    pub fn empty(at: usize) -> Self {
        Self { start: at, end: at }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.end - self.start
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }

    #[must_use]
    pub fn contains(&self, offset: usize) -> bool {
        self.start <= offset && offset < self.end
    }
}

/// A 0-based line and a byte column within it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct LineCol {
    pub line: usize,
    pub column: usize,
}

impl LineCol {
    #[must_use]
    pub fn new(line: usize, column: usize) -> Self {
        Self { line, column }
    }
}

/// One caret: `head` is where it is, `anchor` the end it left behind.
///
/// `goal_column` survives vertical movement so a run of Up/Down through short
/// lines returns to the column it started from, which recomputing from the
/// caret cannot do.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Cursor {
    pub anchor: usize,
    pub head: usize,
    pub goal_column: Option<usize>,
}

impl Cursor {
    #[must_use]
    pub fn caret(at: usize) -> Self {
        Self {
            anchor: at,
            head: at,
            goal_column: None,
        }
    }

    #[must_use]
    pub fn new(anchor: usize, head: usize) -> Self {
        Self {
            anchor,
            head,
            goal_column: None,
        }
    }

    #[must_use]
    pub fn range(&self) -> Range {
        Range::new(self.anchor, self.head)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }

    /// Collapse to the caret, keeping the goal column.
    #[must_use]
    pub fn collapsed(&self) -> Self {
        Self {
            anchor: self.head,
            head: self.head,
            goal_column: self.goal_column,
        }
    }

    #[must_use]
    pub fn with_head(&self, head: usize, extend: bool) -> Self {
        Self {
            anchor: if extend { self.anchor } else { head },
            head,
            goal_column: None,
        }
    }

    /// Shift both ends by a signed byte delta, clamped at zero.
    #[must_use]
    fn shifted(&self, delta: isize) -> Self {
        let move_one = |at: usize| (at as isize + delta).max(0) as usize;
        Self {
            anchor: move_one(self.anchor),
            head: move_one(self.head),
            goal_column: self.goal_column,
        }
    }
}

/// Every caret in the buffer: sorted, non-overlapping, never empty.
///
/// The invariant is the whole type. Two carets that overlap have no defined
/// result for an edit — the transaction that carried them would be rejected as
/// overlapping — so they are merged the moment one is added rather than
/// resolved later.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Selection {
    cursors: Vec<Cursor>,
    /// Index into `cursors`. The one the viewport follows and the status line
    /// reports; always in range.
    primary: usize,
}

impl Default for Selection {
    fn default() -> Self {
        Self::one(Cursor::default())
    }
}

impl Selection {
    #[must_use]
    pub fn caret(at: usize) -> Self {
        Self::one(Cursor::caret(at))
    }

    #[must_use]
    pub fn new(anchor: usize, head: usize) -> Self {
        Self::one(Cursor::new(anchor, head))
    }

    #[must_use]
    pub fn one(cursor: Cursor) -> Self {
        Self {
            cursors: vec![cursor],
            primary: 0,
        }
    }

    /// Build from carets in any order, merging the ones that touch.
    ///
    /// `primary` names which of `cursors` stays primary; it follows its cursor
    /// through the sort and through a merge that swallows it.
    #[must_use]
    pub fn many(cursors: Vec<Cursor>, primary: usize) -> Self {
        if cursors.is_empty() {
            return Self::default();
        }
        let keep = cursors.get(primary).copied().unwrap_or(cursors[0]);
        let mut ordered: Vec<Cursor> = cursors;
        ordered.sort_by_key(|cursor| (cursor.range().start, cursor.range().end));
        let mut merged: Vec<Cursor> = Vec::with_capacity(ordered.len());
        for cursor in ordered {
            match merged.last_mut() {
                // Touching counts: two carets at the same offset are one
                // caret, and an edit would otherwise be told to write twice in
                // the same place.
                Some(last) if cursor.range().start <= last.range().end => {
                    let range = Range::new(
                        last.range().start.min(cursor.range().start),
                        last.range().end.max(cursor.range().end),
                    );
                    // Keep the later caret's direction: it is the one the
                    // person just moved.
                    let forward = cursor.head >= cursor.anchor;
                    *last = if forward {
                        Cursor::new(range.start, range.end)
                    } else {
                        Cursor::new(range.end, range.start)
                    };
                }
                _ => merged.push(cursor),
            }
        }
        // A cap and not a refusal: `Ctrl-D` through ten thousand matches is a
        // gesture with a reasonable intent and an unreasonable result, and the
        // ones nearest the primary are the ones being worked on.
        if merged.len() > MAX_CURSORS {
            let at = merged
                .iter()
                .position(|cursor| cursor.range().start >= keep.range().start)
                .unwrap_or(0);
            let start = at
                .saturating_sub(MAX_CURSORS / 2)
                .min(merged.len() - MAX_CURSORS);
            merged = merged[start..start + MAX_CURSORS].to_vec();
        }
        let primary = merged
            .iter()
            .position(|cursor| cursor.range().contains(keep.head) || cursor.head == keep.head)
            .unwrap_or(0);
        Self {
            cursors: merged,
            primary,
        }
    }

    /// Every caret, in document order.
    #[must_use]
    pub fn cursors(&self) -> &[Cursor] {
        &self.cursors
    }

    /// The caret the viewport follows and the status line reports.
    #[must_use]
    pub fn primary(&self) -> Cursor {
        self.cursors[self.primary]
    }

    /// Which caret is primary, as an index into [`Self::cursors`].
    #[must_use]
    pub fn primary_index(&self) -> usize {
        self.primary
    }

    #[must_use]
    pub fn count(&self) -> usize {
        self.cursors.len()
    }

    /// Whether there is more than one caret.
    #[must_use]
    pub fn is_multiple(&self) -> bool {
        self.cursors.len() > 1
    }

    #[must_use]
    pub fn anchor(&self) -> usize {
        self.primary().anchor
    }

    #[must_use]
    pub fn head(&self) -> usize {
        self.primary().head
    }

    #[must_use]
    pub fn goal_column(&self) -> Option<usize> {
        self.primary().goal_column
    }

    /// The primary caret's range. Every other caret has its own.
    #[must_use]
    pub fn range(&self) -> Range {
        self.primary().range()
    }

    /// Whether the *primary* caret selects nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.primary().is_empty()
    }

    /// Whether every caret selects nothing.
    #[must_use]
    pub fn all_empty(&self) -> bool {
        self.cursors.iter().all(Cursor::is_empty)
    }

    /// Collapse every caret to its head.
    #[must_use]
    pub fn collapsed(&self) -> Self {
        self.mapped(|cursor| cursor.collapsed())
    }

    /// Drop back to one caret at the primary's head.
    #[must_use]
    pub fn single(&self) -> Self {
        Self::one(self.primary())
    }

    /// Move the primary caret, dropping every other one.
    ///
    /// A plain arrow key: CodeMirror collapses a multi-caret selection the same
    /// way, because a movement that kept them would need a rule per direction
    /// for what the others do.
    #[must_use]
    pub fn with_head(&self, head: usize, extend: bool) -> Self {
        Self::one(self.primary().with_head(head, extend))
    }

    /// Rebuild with `f` applied to every caret, re-sorting and merging.
    #[must_use]
    pub fn mapped(&self, f: impl Fn(Cursor) -> Cursor) -> Self {
        Self::many(self.cursors.iter().copied().map(f).collect(), self.primary)
    }

    /// Add a caret, which becomes the primary one.
    #[must_use]
    pub fn with_added(&self, cursor: Cursor) -> Self {
        let mut cursors = self.cursors.clone();
        cursors.push(cursor);
        Self::many(cursors, self.cursors.len())
    }

    /// Shift every caret by a signed byte delta.
    ///
    /// What an edit *above* the carets does to them; an edit between them
    /// cannot be expressed this way and goes through [`Self::many`] instead.
    #[must_use]
    pub fn shifted(&self, delta: isize) -> Self {
        self.mapped(|cursor| cursor.shifted(delta))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_range_normalizes_a_backwards_selection() {
        assert_eq!(Range::new(7, 2), Range { start: 2, end: 7 });
        assert_eq!(Selection::new(7, 2).range(), Range { start: 2, end: 7 });
    }

    #[test]
    fn moving_without_extending_drops_the_anchor() {
        let selection = Selection::new(2, 7);
        assert_eq!(selection.with_head(9, true).anchor(), 2);
        assert_eq!(selection.with_head(9, false).anchor(), 9);
    }

    #[test]
    fn carets_are_sorted_and_the_primary_follows_its_own() {
        let selection = Selection::caret(10).with_added(Cursor::caret(2));
        assert_eq!(
            selection
                .cursors()
                .iter()
                .map(|c| c.head)
                .collect::<Vec<_>>(),
            vec![2, 10]
        );
        assert_eq!(selection.head(), 2, "the caret just added is the primary");
    }

    /// Two carets that touch have no defined result for an edit, so they never
    /// both exist.
    #[test]
    fn overlapping_carets_merge() {
        let selection = Selection::new(0, 5).with_added(Cursor::new(3, 8));
        assert_eq!(selection.count(), 1);
        assert_eq!(selection.range(), Range::new(0, 8));

        let touching = Selection::caret(4).with_added(Cursor::caret(4));
        assert_eq!(touching.count(), 1);
    }

    #[test]
    fn a_plain_move_collapses_back_to_one_caret() {
        let many = Selection::caret(0).with_added(Cursor::caret(10));
        assert_eq!(many.count(), 2);
        assert_eq!(many.with_head(5, false).count(), 1);
    }

    /// Ten thousand matches is a gesture with a reasonable intent and an
    /// unreasonable result; the carets nearest the primary are the ones being
    /// worked on.
    #[test]
    fn the_number_of_carets_is_capped() {
        let cursors: Vec<Cursor> = (0..MAX_CURSORS * 4).map(|n| Cursor::caret(n * 2)).collect();
        let selection = Selection::many(cursors, 0);
        assert_eq!(selection.count(), MAX_CURSORS);
        assert_eq!(selection.cursors()[0].head, 0, "kept the primary's side");
    }

    #[test]
    fn mapping_moves_every_caret_and_merges_what_collides() {
        let selection = Selection::caret(0).with_added(Cursor::caret(4));
        let moved = selection.mapped(|cursor| Cursor::caret(cursor.head + 1));
        assert_eq!(
            moved.cursors().iter().map(|c| c.head).collect::<Vec<_>>(),
            vec![1, 5]
        );
        let collided = selection.mapped(|_| Cursor::caret(2));
        assert_eq!(collided.count(), 1, "two carets landing together are one");
    }
}
