//! Byte budgets, checked before an allocation rather than after it.
//!
//! A caller that ignores these is the defect `docs/performance.md` describes:
//! a cap consulted once the whole thing is already resident bounds the answer,
//! not the peak.

/// Largest document the editor opens or keeps, in bytes of valid UTF-8.
///
/// Matches `fs-service::MAX_FILE_BYTES`, because the integrated path reads
/// through that service and a document it refuses must not open here either.
pub const MAX_DOCUMENT_BYTES: usize = 2 * 1024 * 1024;

/// Undo/redo text retained for one document.
///
/// Per document, not per instance: the 64 MiB instance budget belongs to
/// whoever owns the instance, which this crate deliberately does not.
pub const MAX_HISTORY_BYTES: usize = 4 * 1024 * 1024;

/// Undo entries retained for one document, whatever their size.
pub const MAX_HISTORY_ENTRIES: usize = 4096;

/// Matches a search reports at once. A UI lists these; it is not the number a
/// replacement may rewrite.
pub const MAX_SEARCH_RESULTS: usize = 5_000;

/// Matches one replace-all may rewrite. Above this the whole replacement is
/// refused, because half a replace-all is not a replace-all.
pub const MAX_REPLACE_MATCHES: usize = 200_000;

/// Longest find pattern, literal or regular expression.
///
/// Clamped before the pattern reaches the engine: a compiled program's size is
/// a function of its source, and this is the input a person can grow without
/// bound by holding a key down.
pub const MAX_PATTERN_BYTES: usize = 1024;

/// Largest compiled regular-expression program, in bytes.
///
/// `(a{1000}){1000}` is a short pattern that names a very large program;
/// `regex` refuses past this instead of allocating it.
pub const MAX_REGEX_SIZE: usize = 1024 * 1024;
