//! Positions as byte offsets, plus the one selection a view carries.
//!
//! Byte offsets are the internal unit; grapheme columns and display cells are
//! conversions (`metrics`), never a second source of truth. Line and column in
//! [`LineCol`] are 0-based — the 1-based line a person reads is the view's job.

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

/// Where a view is pointing: `head` is the caret, `anchor` the fixed end.
///
/// `goal_column` survives vertical movement so a run of Up/Down through short
/// lines returns to the column it started from, which recomputing from the
/// caret cannot do.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Selection {
    pub anchor: usize,
    pub head: usize,
    pub goal_column: Option<usize>,
}

impl Selection {
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
        assert_eq!(selection.with_head(9, true).anchor, 2);
        assert_eq!(selection.with_head(9, false).anchor, 9);
    }
}
