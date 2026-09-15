//! One open document: text, selection, history, versions and the dirty flag.
//!
//! [`Document::apply`] is the only way the text changes, so read-only, the
//! byte budget, the version counter and undo cannot be bypassed. The document
//! never touches a disk: an adapter reads the bytes, hands them to
//! [`Document::from_bytes`], and asks for [`Document::text`] back to write.

use crate::history::{History, HistoryStats};
use crate::limits::MAX_DOCUMENT_BYTES;
use crate::search::{self, Query, ReplaceOutcome};
use crate::selection::{LineCol, Range, Selection};
use crate::text::Text;
use crate::transaction::{Edit, EditError, Origin, Transaction};

/// Monotonic token for "which text produced this".
///
/// Strictly increasing, undo included: a highlight or a preview computed
/// against an older version is stale even when the text matches again.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct DocumentVersion(pub u64);

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum LoadError {
    #[error("file is larger than {limit} bytes ({bytes} bytes)")]
    TooLarge { bytes: usize, limit: usize },
    #[error("file is not valid UTF-8")]
    NotUtf8,
}

/// Text handed to an adapter to write, with the state it was taken at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    version: DocumentVersion,
    state: u64,
    text: String,
}

impl Snapshot {
    #[must_use]
    pub fn version(&self) -> DocumentVersion {
        self.version
    }

    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.text.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }
}

/// What an applied transaction changed, for a caller that has to invalidate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Applied {
    pub version: DocumentVersion,
    /// Lines the edit touched, before renumbering: rows outside this span keep
    /// whatever the view already painted.
    pub first_line: usize,
    pub last_line: usize,
    /// Lines added (positive) or removed (negative) below `first_line`.
    pub line_delta: isize,
}

pub struct Document {
    text: Text,
    selection: Selection,
    history: History,
    read_only: bool,
    version: DocumentVersion,
    /// Content identity, restored by undo and redo. Separate from `version`
    /// so undoing back to the saved text clears the dirty flag.
    state: u64,
    next_state: u64,
    saved_state: Option<u64>,
    /// Revision of the bytes this document was loaded from, for a save that
    /// refuses to overwrite someone else's write.
    disk_revision: Option<String>,
}

impl Document {
    /// Decode `bytes`, refusing an oversize file before the copy and a lossy
    /// decode outright: text that changes bytes on the way in cannot be saved
    /// back without corrupting the file.
    pub fn from_bytes(bytes: &[u8], read_only: bool) -> Result<Self, LoadError> {
        if bytes.len() > MAX_DOCUMENT_BYTES {
            return Err(LoadError::TooLarge {
                bytes: bytes.len(),
                limit: MAX_DOCUMENT_BYTES,
            });
        }
        let body = std::str::from_utf8(bytes).map_err(|_| LoadError::NotUtf8)?;
        Ok(Self::from_string(body.to_string(), read_only))
    }

    #[must_use]
    pub fn from_string(body: String, read_only: bool) -> Self {
        Self {
            text: Text::new(body),
            selection: Selection::caret(0),
            history: History::default(),
            read_only,
            version: DocumentVersion(0),
            state: 0,
            next_state: 1,
            saved_state: Some(0),
            disk_revision: None,
        }
    }

    #[must_use]
    pub fn text(&self) -> &Text {
        &self.text
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        self.text.as_str()
    }

    #[must_use]
    pub fn selection(&self) -> Selection {
        self.selection
    }

    #[must_use]
    pub fn caret(&self) -> usize {
        self.selection.head
    }

    #[must_use]
    pub fn caret_line_col(&self) -> LineCol {
        self.text.line_col(self.selection.head)
    }

    #[must_use]
    pub fn version(&self) -> DocumentVersion {
        self.version
    }

    #[must_use]
    pub fn is_read_only(&self) -> bool {
        self.read_only
    }

    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.saved_state != Some(self.state)
    }

    #[must_use]
    pub fn disk_revision(&self) -> Option<&str> {
        self.disk_revision.as_deref()
    }

    #[must_use]
    pub fn history_stats(&self) -> HistoryStats {
        self.history.stats()
    }

    #[must_use]
    pub fn can_undo(&self) -> bool {
        self.history.can_undo()
    }

    #[must_use]
    pub fn can_redo(&self) -> bool {
        self.history.can_redo()
    }

    pub fn set_selection(&mut self, selection: Selection) {
        let anchor = self.text.clamp_offset(selection.anchor);
        let head = self.text.clamp_offset(selection.head);
        // A caret that jumped is a new undo unit; without this a word typed
        // here and a word typed there undo as one.
        if head != self.selection.head {
            self.history.seal();
        }
        self.selection = Selection {
            anchor,
            head,
            goal_column: selection.goal_column,
        };
    }

    pub fn set_read_only(&mut self, read_only: bool) {
        self.read_only = read_only;
    }

    /// The text to write, tagged with the state it was taken at.
    #[must_use]
    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            version: self.version,
            state: self.state,
            text: self.text.as_str().to_string(),
        }
    }

    /// Record that `snapshot` reached the disk at `revision`.
    ///
    /// Takes the snapshot rather than "the current text" because a keystroke
    /// can land while the bytes are being written: the save then confirms an
    /// older state and the buffer stays dirty, which is the truth.
    pub fn confirm_save(&mut self, snapshot: &Snapshot, revision: impl Into<String>) {
        self.saved_state = Some(snapshot.state);
        self.disk_revision = Some(revision.into());
    }

    /// Record the revision a load came from without touching the dirty flag.
    pub fn set_disk_revision(&mut self, revision: impl Into<String>) {
        self.disk_revision = Some(revision.into());
    }

    /// Start a new undo entry, whatever comes next.
    pub fn break_undo_group(&mut self) {
        self.history.seal();
    }

    /// The single entry every mutation goes through.
    ///
    /// Validates the whole transaction before touching a byte: a rejected
    /// transaction leaves the document exactly as it was.
    pub fn apply(&mut self, transaction: Transaction) -> Result<Applied, EditError> {
        if self.read_only {
            return Err(EditError::ReadOnly);
        }
        self.validate(&transaction)?;
        Ok(self.apply_validated(transaction, true))
    }

    /// Apply without recording history, for undo and redo replaying their own
    /// inverse. Going through `apply` would record the replay as a new edit.
    fn apply_validated(&mut self, transaction: Transaction, record: bool) -> Applied {
        let first_line = self.text.line_of_offset(
            transaction
                .edits()
                .first()
                .map_or(self.selection.head, |edit| edit.range.start),
        );
        let last_line = self.text.line_of_offset(
            transaction
                .edits()
                .last()
                .map_or(self.selection.head, |edit| edit.range.end),
        );

        let inverse = record.then(|| self.inverse_of(&transaction));
        let mut line_delta = 0_isize;
        let mut shift = 0_isize;
        let mut resume_at = self.selection.head;
        for edit in transaction.edits() {
            let start = (edit.range.start as isize + shift) as usize;
            let end = (edit.range.end as isize + shift) as usize;
            line_delta += newlines(&edit.insert) as isize
                - newlines(self.text.slice(Range::new(start, end))) as isize;
            self.text.replace(Range::new(start, end), &edit.insert);
            shift += edit.insert.len() as isize - (end - start) as isize;
            resume_at = start + edit.insert.len();
        }

        let state_before = self.state;
        self.state = self.next_state;
        self.next_state += 1;
        self.version = DocumentVersion(self.version.0 + 1);

        let selection = transaction
            .selection_after
            .unwrap_or_else(|| Selection::caret(resume_at));
        self.selection = Selection {
            anchor: self.text.clamp_offset(selection.anchor),
            head: self.text.clamp_offset(selection.head),
            goal_column: None,
        };

        if let Some(inverse) = inverse {
            self.history
                .record(inverse, transaction, state_before, self.state, resume_at);
        }

        Applied {
            version: self.version,
            first_line,
            last_line,
            line_delta,
        }
    }

    fn validate(&self, transaction: &Transaction) -> Result<(), EditError> {
        for edit in transaction.edits() {
            let range = edit.range;
            if range.end > self.text.len()
                || !self.text.is_boundary(range.start)
                || !self.text.is_boundary(range.end)
            {
                return Err(EditError::OutOfBounds {
                    start: range.start,
                    end: range.end,
                });
            }
        }
        let delta = transaction.size_delta();
        let size = (self.text.len() as isize + delta).max(0) as usize;
        if size > MAX_DOCUMENT_BYTES {
            return Err(EditError::TooLarge {
                size,
                limit: MAX_DOCUMENT_BYTES,
            });
        }
        Ok(())
    }

    /// The transaction that puts back exactly what `transaction` displaces.
    fn inverse_of(&self, transaction: &Transaction) -> Transaction {
        let mut edits = Vec::with_capacity(transaction.edits().len());
        let mut shift = 0_isize;
        for edit in transaction.edits() {
            let start = (edit.range.start as isize + shift) as usize;
            let end = start + edit.insert.len();
            edits.push(Edit::replace(
                Range::new(start, end),
                self.text.slice(edit.range).to_string(),
            ));
            shift += edit.insert.len() as isize - edit.range.len() as isize;
        }
        // Built from a validated transaction, so it is disjoint and ordered.
        Transaction::new(edits, Origin::History, self.selection)
            .unwrap_or_else(|_| {
                Transaction::new(Vec::new(), Origin::History, self.selection)
                    .expect("empty is valid")
            })
            .with_selection_after(transaction.selection_before)
    }

    /// Replace the selection (or insert at the caret) with `text`.
    pub fn insert(&mut self, text: &str, origin: Origin) -> Result<Applied, EditError> {
        let range = self.selection.range();
        let after = Selection::caret(range.start + text.len());
        let transaction =
            Transaction::new(vec![Edit::replace(range, text)], origin, self.selection)?
                .with_selection_after(after);
        self.apply(transaction)
    }

    /// Delete the selection, or the range `into` maps the caret to.
    ///
    /// `Ok(None)` is "there was nothing there": backspace at offset zero is not
    /// an edit, and reporting it as one would put an empty entry in history.
    /// Read-only is still a refusal, checked before the range is even computed.
    pub fn delete(
        &mut self,
        into: impl FnOnce(&Text, usize) -> usize,
        origin: Origin,
    ) -> Result<Option<Applied>, EditError> {
        if self.read_only {
            return Err(EditError::ReadOnly);
        }
        let range = if self.selection.is_empty() {
            let other = into(&self.text, self.selection.head);
            Range::new(self.selection.head, other)
        } else {
            self.selection.range()
        };
        if range.is_empty() {
            return Ok(None);
        }
        let transaction = Transaction::new(vec![Edit::delete(range)], origin, self.selection)?
            .with_selection_after(Selection::caret(range.start));
        self.apply(transaction).map(Some)
    }

    pub fn undo(&mut self) -> Option<Applied> {
        if self.read_only {
            return None;
        }
        let (transaction, state) = self.history.pop_undo()?;
        let applied = self.apply_validated(transaction, false);
        self.state = state;
        Some(applied)
    }

    pub fn redo(&mut self) -> Option<Applied> {
        if self.read_only {
            return None;
        }
        let (transaction, state) = self.history.pop_redo()?;
        let applied = self.apply_validated(transaction, false);
        self.state = state;
        Some(applied)
    }

    /// Replace every match of `query` with `replacement`, as one undo unit.
    ///
    /// All of it or none: a partial replace-all is the defect the display limit
    /// on search results would otherwise cause.
    pub fn replace_all(
        &mut self,
        query: &Query,
        replacement: &str,
    ) -> Result<ReplaceOutcome, EditError> {
        if self.read_only {
            return Err(EditError::ReadOnly);
        }
        let ranges = match search::replace_all_ranges(&self.text, query) {
            Ok(ranges) => ranges,
            Err(outcome) => return Ok(outcome),
        };
        if ranges.is_empty() {
            return Ok(ReplaceOutcome::Replaced(0));
        }
        let count = ranges.len();
        let edits = ranges
            .into_iter()
            .map(|range| Edit::replace(range, replacement))
            .collect();
        let transaction = Transaction::new(edits, Origin::ReplaceAll, self.selection)?;
        self.history.seal();
        self.apply(transaction)?;
        self.history.seal();
        Ok(ReplaceOutcome::Replaced(count))
    }

    /// Move the caret to the start of a 1-based line, the way a person counts.
    pub fn goto_line(&mut self, line_one_based: usize) {
        let line = line_one_based
            .saturating_sub(1)
            .min(self.text.line_count() - 1);
        self.set_selection(Selection::caret(self.text.line_start(line)));
    }

    /// Replace the whole buffer with content read from disk again.
    ///
    /// Recorded like any other transaction, so a reload that lands on a dirty
    /// buffer by mistake is still undoable.
    pub fn reload(
        &mut self,
        body: &str,
        revision: impl Into<String>,
    ) -> Result<Applied, EditError> {
        let was_read_only = self.read_only;
        self.read_only = false;
        let range = Range::new(0, self.text.len());
        let caret = self.selection.head.min(body.len());
        let transaction = Transaction::new(
            vec![Edit::replace(range, body)],
            Origin::Reload,
            self.selection,
        )?
        .with_selection_after(Selection::caret(caret));
        self.history.seal();
        let applied = self.apply(transaction);
        self.read_only = was_read_only;
        let applied = applied?;
        self.saved_state = Some(self.state);
        self.disk_revision = Some(revision.into());
        Ok(applied)
    }
}

fn newlines(body: &str) -> usize {
    body.as_bytes()
        .iter()
        .filter(|&&byte| byte == b'\n')
        .count()
}

impl std::fmt::Debug for Document {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Document")
            .field("bytes", &self.text.len())
            .field("lines", &self.text.line_count())
            .field("version", &self.version.0)
            .field("dirty", &self.is_dirty())
            .field("read_only", &self.read_only)
            .finish()
    }
}
