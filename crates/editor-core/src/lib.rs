//! The editing engine: one document, versioned transactions, history, search.
//!
//! Every mutation enters through [`Document::apply`], so undo, the dirty flag
//! and the version counter cannot be bypassed by a second write path. This
//! crate owns no terminal, no filesystem, no clipboard and no Forge type: an
//! adapter reads bytes, hands them here, and asks for the text back to save.

pub mod brackets;
mod command;
mod document;
mod history;
pub mod limits;
pub mod metrics;
pub mod movement;
mod search;
mod selection;
mod syntax;
mod transaction;

pub use command::{execute, Command, Outcome, Refusal};
pub use document::{Applied, Document, DocumentVersion, LoadError, Snapshot};
pub use history::HistoryStats;
pub use search::{
    count_matches, count_matches_before, find_all, find_in, find_next, find_previous, Match,
    Matches, Query, QueryError, ReplaceOutcome,
};
pub use selection::{LineCol, Range, Selection};
pub use syntax::{Grammar, Scope, Span, Syntax};
pub use transaction::{Edit, EditError, Origin, Transaction};

pub use crate::text::Text;

mod text;
