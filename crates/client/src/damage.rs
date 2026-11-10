use domain::TerminalDelta;

/// What of a terminal viewport a frame has to repaint.
///
/// Latest-wins publication must [`merge`](Self::merge) the damage of every
/// skipped delta: dropping a delta's rows while keeping its grid paints a stale row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RowDamage {
    /// Specific visible row indices.
    Rows(Vec<u16>),
    /// The whole viewport: attach, resync, resize, or any scrolled viewport,
    /// whose rows all shift when history moves.
    Full,
}

impl Default for RowDamage {
    /// No damage.
    fn default() -> Self {
        Self::Rows(Vec::new())
    }
}

impl RowDamage {
    /// Damage for a live terminal. A scroll moves every row, and the rows the
    /// delta names are only the ones that *also* changed content, so a
    /// scrolled delta is full however few rows it lists.
    #[must_use]
    pub fn of_delta(delta: &TerminalDelta) -> Self {
        if delta.scrolled_lines > 0 {
            Self::Full
        } else {
            Self::of_rewrite(delta)
        }
    }

    /// Damage for a surface that rewrites its grid in place and never shows
    /// history (the editor TUI), where `scrolled_lines` must not force a full repaint.
    #[must_use]
    pub fn of_rewrite(delta: &TerminalDelta) -> Self {
        Self::Rows(delta.changed_rows().map(|(index, _)| index).collect())
    }

    /// Merge two damages, widening to [`RowDamage::Full`] when either is.
    #[must_use]
    pub fn merge(self, other: RowDamage) -> RowDamage {
        match (self, other) {
            (Self::Full, _) | (_, Self::Full) => Self::Full,
            (Self::Rows(mut a), Self::Rows(b)) => {
                a.extend(b);
                a.sort_unstable();
                a.dedup();
                Self::Rows(a)
            }
        }
    }

    /// The damaged rows, or `None` when every row is.
    #[must_use]
    pub fn rows(&self) -> Option<&[u16]> {
        match self {
            Self::Rows(rows) => Some(rows),
            Self::Full => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::{CellPatch, Cursor, Row, TermModes};

    fn delta(rows: &[u16], patched: &[u16], scrolled_lines: u32) -> TerminalDelta {
        TerminalDelta {
            seq: 1,
            rows: rows.iter().map(|&i| (i, Row::blank(1))).collect(),
            patches: patched
                .iter()
                .map(|&line| CellPatch {
                    line,
                    first: 0,
                    row: Row::blank(1),
                })
                .collect(),
            scrollback_len: 0,
            scrollback_generation: 0,
            scrolled_lines,
            cursor: Cursor::default(),
            modes: TermModes::default(),
        }
    }

    #[test]
    fn a_delta_damages_its_rows_and_patched_lines() {
        assert_eq!(
            RowDamage::of_delta(&delta(&[4], &[1], 0)),
            RowDamage::Rows(vec![4, 1])
        );
    }

    #[test]
    fn a_scrolled_delta_damages_the_whole_viewport() {
        assert_eq!(RowDamage::of_delta(&delta(&[4], &[], 2)), RowDamage::Full);
    }

    #[test]
    fn a_rewrite_ignores_scrolled_lines() {
        assert_eq!(
            RowDamage::of_rewrite(&delta(&[4], &[], 2)),
            RowDamage::Rows(vec![4])
        );
    }

    #[test]
    fn merging_damage_widens_to_full() {
        assert_eq!(
            RowDamage::Rows(vec![1]).merge(RowDamage::Full),
            RowDamage::Full,
            "a resync in the same batch invalidates the row list"
        );
        assert_eq!(
            RowDamage::Rows(vec![2, 1]).merge(RowDamage::Rows(vec![1, 3])),
            RowDamage::Rows(vec![1, 2, 3])
        );
        assert_eq!(RowDamage::Full.rows(), None);
    }
}
