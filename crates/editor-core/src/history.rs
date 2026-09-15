//! Undo and redo as inverse transactions, under a byte budget.
//!
//! Every applied transaction is recorded with the text it displaced, so undo
//! restores content *and* selection. Only a run of contiguous typing coalesces
//! into one entry; a paste or a replace-all never does, because undoing half of
//! one is not a state the person ever saw.

use crate::limits::{MAX_HISTORY_BYTES, MAX_HISTORY_ENTRIES};
use crate::selection::{Range, Selection};
use crate::transaction::{Edit, Origin, Transaction};

/// What the caller may show about a history that has been trimmed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HistoryStats {
    pub undo_entries: usize,
    pub redo_entries: usize,
    pub retained_bytes: usize,
    /// Entries dropped to stay inside the budget. Non-zero means the oldest
    /// edits can no longer be undone, which a status line has to be able to say.
    pub dropped: usize,
}

#[derive(Clone, Debug)]
struct Entry {
    undo: Transaction,
    redo: Transaction,
    /// Content identity on each side. Undo and redo restore one of these rather
    /// than counting, so a saved state stays recognizable after a trim.
    state_before: u64,
    state_after: u64,
    retained: usize,
    /// Offset the caret reached after `redo`: the next keystroke joins this
    /// entry only if it continues exactly there.
    resume_at: usize,
}

#[derive(Debug, Default)]
pub struct History {
    undo: Vec<Entry>,
    redo: Vec<Entry>,
    retained: usize,
    dropped: usize,
    sealed: bool,
}

impl History {
    /// Record `redo` and its inverse `undo`, coalescing with the previous entry
    /// when both are ordinary typing that continued where the last one stopped.
    pub fn record(
        &mut self,
        undo: Transaction,
        redo: Transaction,
        state_before: u64,
        state_after: u64,
        resume_at: usize,
    ) {
        self.redo.clear();
        let retained = undo.retained_bytes() + redo.retained_bytes();

        if !self.sealed {
            if let Some(last) = self.undo.last_mut() {
                if let Some((merged_undo, merged_redo)) = coalesce(last, &undo, &redo) {
                    last.undo = merged_undo;
                    last.redo = merged_redo;
                    last.state_after = state_after;
                    last.resume_at = resume_at;
                    last.retained += retained;
                    self.retained += retained;
                    self.trim();
                    return;
                }
            }
        }

        self.sealed = false;
        self.retained += retained;
        self.undo.push(Entry {
            undo,
            redo,
            state_before,
            state_after,
            retained,
            resume_at,
        });
        self.trim();
    }

    /// Stop the next edit from joining the current typing run.
    pub fn seal(&mut self) {
        self.sealed = true;
    }

    /// The transaction that undoes the newest entry, and the content state it
    /// restores.
    pub fn pop_undo(&mut self) -> Option<(Transaction, u64)> {
        let entry = self.undo.pop()?;
        let transaction = entry.undo.clone();
        let state = entry.state_before;
        self.redo.push(entry);
        self.sealed = true;
        Some((transaction, state))
    }

    pub fn pop_redo(&mut self) -> Option<(Transaction, u64)> {
        let entry = self.redo.pop()?;
        let transaction = entry.redo.clone();
        let state = entry.state_after;
        self.undo.push(entry);
        self.sealed = true;
        Some((transaction, state))
    }

    #[must_use]
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    #[must_use]
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    #[must_use]
    pub fn stats(&self) -> HistoryStats {
        HistoryStats {
            undo_entries: self.undo.len(),
            redo_entries: self.redo.len(),
            retained_bytes: self.retained,
            dropped: self.dropped,
        }
    }

    /// Drop the oldest entries until the budget holds. Dropping the newest
    /// would make the next undo skip the edit the person just made.
    fn trim(&mut self) {
        while self.undo.len() > MAX_HISTORY_ENTRIES
            || (self.retained > MAX_HISTORY_BYTES && self.undo.len() > 1)
        {
            let oldest = self.undo.remove(0);
            self.retained -= oldest.retained;
            self.dropped += 1;
        }
    }
}

/// Merge a keystroke into the entry before it, or refuse.
///
/// Refusing is the safe answer: a rejected merge costs one extra undo step,
/// a wrong one loses text. Both sides must be single-edit, the newcomer a pure
/// insertion resuming exactly where the entry left off.
fn coalesce(
    last: &Entry,
    undo: &Transaction,
    redo: &Transaction,
) -> Option<(Transaction, Transaction)> {
    if !last.redo.origin.coalesces() || !redo.origin.coalesces() {
        return None;
    }
    let previous_redo = single(&last.redo)?;
    let previous_undo = single(&last.undo)?;
    let addition = single(redo)?;
    let _ = single(undo)?;
    if !addition.range.is_empty() || addition.range.start != last.resume_at {
        return None;
    }

    let mut insert = previous_redo.insert.clone();
    insert.push_str(&addition.insert);
    let merged_redo = rebuild(
        vec![Edit::replace(previous_redo.range, insert.clone())],
        last.redo.origin,
        last.redo.selection_before,
        redo.selection_after,
    );
    // The inverse of the merged edit: remove everything inserted so far and put
    // back the text the first edit displaced.
    let start = previous_redo.range.start;
    let merged_undo = rebuild(
        vec![Edit::replace(
            Range::new(start, start + insert.len()),
            previous_undo.insert.clone(),
        )],
        Origin::History,
        undo.selection_before,
        last.undo.selection_after,
    );
    Some((merged_undo, merged_redo))
}

fn single(transaction: &Transaction) -> Option<&Edit> {
    match transaction.edits() {
        [edit] => Some(edit),
        _ => None,
    }
}

fn rebuild(
    edits: Vec<Edit>,
    origin: Origin,
    before: Selection,
    after: Option<Selection>,
) -> Transaction {
    // A single edit can neither overlap nor need sorting, so the only error
    // `Transaction::new` defines is unreachable here.
    let mut transaction = Transaction::new(edits, origin, before)
        .unwrap_or_else(|_| Transaction::new(Vec::new(), origin, before).expect("empty is valid"));
    transaction.selection_after = after;
    transaction
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typing(at: usize, text: &str) -> Transaction {
        Transaction::new(
            vec![Edit::insert(at, text)],
            Origin::Input,
            Selection::caret(at),
        )
        .expect("valid")
        .with_selection_after(Selection::caret(at + text.len()))
    }

    fn inverse(at: usize, len: usize) -> Transaction {
        Transaction::new(
            vec![Edit::delete(Range::new(at, at + len))],
            Origin::History,
            Selection::caret(at + len),
        )
        .expect("valid")
        .with_selection_after(Selection::caret(at))
    }

    #[test]
    fn consecutive_typing_becomes_one_entry_whose_inverse_removes_all_of_it() {
        let mut history = History::default();
        history.record(inverse(0, 1), typing(0, "a"), 0, 1, 1);
        history.record(inverse(1, 1), typing(1, "b"), 1, 2, 2);
        assert_eq!(history.stats().undo_entries, 1);

        let (transaction, state) = history.pop_undo().expect("one entry");
        assert_eq!(state, 0);
        assert_eq!(transaction.edits()[0].range, Range::new(0, 2));
        assert_eq!(transaction.edits()[0].insert, "");
    }

    #[test]
    fn typing_after_a_jump_starts_a_new_entry() {
        let mut history = History::default();
        history.record(inverse(0, 1), typing(0, "a"), 0, 1, 1);
        history.record(inverse(9, 1), typing(9, "b"), 1, 2, 10);
        assert_eq!(history.stats().undo_entries, 2);
    }

    #[test]
    fn a_paste_never_joins_the_run_before_it() {
        let mut history = History::default();
        history.record(inverse(0, 1), typing(0, "a"), 0, 1, 1);
        let paste = Transaction::new(
            vec![Edit::insert(1, "xyz")],
            Origin::Paste,
            Selection::caret(1),
        )
        .expect("valid");
        history.record(inverse(1, 3), paste, 1, 2, 4);
        assert_eq!(history.stats().undo_entries, 2);
    }

    #[test]
    fn typing_over_a_selection_then_continuing_restores_the_replaced_text() {
        let mut history = History::default();
        let overwrite = Transaction::new(
            vec![Edit::replace(Range::new(0, 3), "a")],
            Origin::Input,
            Selection::new(0, 3),
        )
        .expect("valid");
        let undo_overwrite = Transaction::new(
            vec![Edit::replace(Range::new(0, 1), "xyz")],
            Origin::History,
            Selection::caret(1),
        )
        .expect("valid");
        history.record(undo_overwrite, overwrite, 0, 1, 1);
        history.record(inverse(1, 1), typing(1, "b"), 1, 2, 2);

        let (transaction, _) = history.pop_undo().expect("one entry");
        assert_eq!(transaction.edits()[0].range, Range::new(0, 2));
        assert_eq!(transaction.edits()[0].insert, "xyz");
    }

    #[test]
    fn recording_after_an_undo_drops_the_redo_branch() {
        let mut history = History::default();
        history.record(inverse(0, 1), typing(0, "a"), 0, 1, 1);
        assert!(history.pop_undo().is_some());
        assert!(history.can_redo());
        history.record(inverse(0, 1), typing(0, "z"), 0, 2, 1);
        assert!(!history.can_redo());
    }

    #[test]
    fn the_budget_drops_the_oldest_entries_and_says_so() {
        let mut history = History::default();
        let big = "x".repeat(64 * 1024);
        for index in 0..200_usize {
            history.seal();
            history.record(
                inverse(index, big.len()),
                typing(index, &big),
                index as u64,
                index as u64 + 1,
                index + big.len(),
            );
        }
        let stats = history.stats();
        assert!(stats.dropped > 0, "nothing was trimmed: {stats:?}");
        assert!(stats.retained_bytes <= MAX_HISTORY_BYTES + 4 * big.len());
    }
}
