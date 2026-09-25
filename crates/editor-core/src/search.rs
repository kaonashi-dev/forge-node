//! Literal search and replace over the buffer's bytes.
//!
//! The number of matches a UI lists and the number a replacement may rewrite
//! are different budgets: showing the first 5 000 is a display decision, and
//! replacing only those would silently rewrite part of a file. A replacement
//! over budget is refused whole.
//!
//! A regular expression is opt-in and compiled once per query, because the
//! pattern arrives one keystroke at a time and compiling per scan would be the
//! cost of the search. The engine is `regex`, whose guarantee is linear time in
//! the haystack: there is no catastrophic backtracking to budget against, so
//! the caps here are on the *pattern* (its length and its compiled size),
//! which is the input a person can actually make unbounded.

use std::cell::RefCell;

use crate::limits::{MAX_PATTERN_BYTES, MAX_REGEX_SIZE, MAX_REPLACE_MATCHES, MAX_SEARCH_RESULTS};
use crate::selection::Range;
use crate::text::Text;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Query {
    pub pattern: String,
    pub case_sensitive: bool,
    pub whole_word: bool,
    /// Read `pattern` as a regular expression rather than as literal bytes.
    pub regex: bool,
}

/// Why a pattern cannot be searched with.
///
/// Only a regular expression can be invalid: a literal is always a literal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum QueryError {
    /// The pattern is longer than [`MAX_PATTERN_BYTES`].
    TooLong { limit: usize },
    /// `regex` could not parse it, or the compiled program was over budget.
    Invalid(String),
}

impl std::fmt::Display for QueryError {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooLong { limit } => write!(out, "pattern is longer than {limit} bytes"),
            Self::Invalid(reason) => write!(out, "{reason}"),
        }
    }
}

thread_local! {
    /// The last compiled pattern, keyed on what it was compiled from.
    ///
    /// Find-as-you-type asks the same question once per keystroke and then once
    /// per scan inside it; compiling each time is what would make a regular
    /// expression feel slower than a literal. One slot, because there is one
    /// live query.
    static COMPILED: RefCell<Option<(Query, regex::Regex)>> = const { RefCell::new(None) };
}

impl Query {
    #[must_use]
    pub fn literal(pattern: impl Into<String>) -> Self {
        Self {
            pattern: pattern.into(),
            case_sensitive: true,
            whole_word: false,
            regex: false,
        }
    }

    #[must_use]
    pub fn regular_expression(mut self) -> Self {
        self.regex = true;
        self
    }

    /// Whether this query can be searched with, and why not when it cannot.
    ///
    /// Called when the pattern changes, not per scan: the compiled program is
    /// cached, so a live regex costs one compile per edit of the pattern.
    pub fn validate(&self) -> Result<(), QueryError> {
        if self.pattern.len() > MAX_PATTERN_BYTES {
            return Err(QueryError::TooLong {
                limit: MAX_PATTERN_BYTES,
            });
        }
        if !self.regex || self.pattern.is_empty() {
            return Ok(());
        }
        self.compiled(|program| program.map(|_| ()))
    }

    /// Run `visit` with this query's compiled program, compiling if it must.
    fn compiled<T>(&self, visit: impl FnOnce(Result<&regex::Regex, QueryError>) -> T) -> T {
        COMPILED.with(|slot| {
            let mut slot = slot.borrow_mut();
            if slot.as_ref().is_none_or(|(query, _)| query != self) {
                match self.build() {
                    Ok(program) => *slot = Some((self.clone(), program)),
                    Err(error) => {
                        *slot = None;
                        return visit(Err(error));
                    }
                }
            }
            let program = &slot.as_ref().expect("just compiled").1;
            visit(Ok(program))
        })
    }

    fn build(&self) -> Result<regex::Regex, QueryError> {
        if self.pattern.len() > MAX_PATTERN_BYTES {
            return Err(QueryError::TooLong {
                limit: MAX_PATTERN_BYTES,
            });
        }
        let body = if self.whole_word {
            format!(r"\b(?:{})\b", self.pattern)
        } else {
            self.pattern.clone()
        };
        regex::RegexBuilder::new(&body)
            .case_insensitive(!self.case_sensitive)
            // A pattern can name a program far larger than itself
            // (`(a{1000}){1000}`); this is the allocation that bound goes on.
            .size_limit(MAX_REGEX_SIZE)
            .dfa_size_limit(MAX_REGEX_SIZE)
            // `.` stopping at a line break is what a person typing into a find
            // bar means, and it keeps one match inside one line.
            .multi_line(true)
            .build()
            .map_err(|error| QueryError::Invalid(error.to_string()))
    }

    #[must_use]
    pub fn case_insensitive(mut self) -> Self {
        self.case_sensitive = false;
        self
    }

    #[must_use]
    pub fn whole_word(mut self) -> Self {
        self.whole_word = true;
        self
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.pattern.is_empty()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Match {
    pub range: Range,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Matches {
    pub found: Vec<Match>,
    /// Matches exist past the last one listed. A counter that hides this reads
    /// as "5000 results" when the answer is "at least 5000".
    pub truncated: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReplaceOutcome {
    Replaced(usize),
    /// More matches than one transaction may carry; nothing was replaced.
    OverBudget {
        limit: usize,
    },
}

/// Every match, up to `MAX_SEARCH_RESULTS`.
#[must_use]
pub fn find_all(text: &Text, query: &Query) -> Matches {
    let mut found = Vec::new();
    let mut truncated = false;
    for range in scan(text, query) {
        if found.len() == MAX_SEARCH_RESULTS {
            truncated = true;
            break;
        }
        found.push(Match { range });
    }
    Matches { found, truncated }
}

/// How many matches exist, counted without keeping them, up to `limit`.
#[must_use]
pub fn count_matches(text: &Text, query: &Query, limit: usize) -> usize {
    if query.regex {
        return regex_matches(text, query, 0, |found| found.take(limit).count());
    }
    scan(text, query).take(limit).count()
}

/// Matches that overlap `window`, in ascending order.
///
/// Bounded by the window and not by the document: highlighting is a viewport
/// decoration, and scanning a 2 MiB buffer on every keystroke to colour thirty
/// rows is the shape `docs/performance.md` calls ruinous. The scan starts a
/// pattern's length before the window so a match that begins above it and
/// reaches in is still found.
#[must_use]
pub fn find_in(text: &Text, query: &Query, window: Range) -> Vec<Range> {
    if query.is_empty() {
        return Vec::new();
    }
    let mut start = window.start.saturating_sub(query.pattern.len());
    while start > 0 && !text.is_boundary(start) {
        start -= 1;
    }
    let visible = |found: &mut dyn Iterator<Item = Range>| {
        found
            .take_while(|range| range.start < window.end)
            .filter(|range| range.end > window.start)
            .collect()
    };
    if query.regex {
        return regex_matches(text, query, start, visible);
    }
    visible(&mut scan_from(text, query, start))
}

/// How many matches start before `offset`, up to `limit`.
///
/// The `n` of an `n/m`: the caller already knows the match it is on, and this
/// says where it sits without materialising the ones over it.
#[must_use]
pub fn count_matches_before(text: &Text, query: &Query, offset: usize, limit: usize) -> usize {
    let before = |found: &mut dyn Iterator<Item = Range>| {
        found
            .take_while(|range| range.start < offset)
            .take(limit)
            .count()
    };
    if query.regex {
        return regex_matches(text, query, 0, before);
    }
    before(&mut scan(text, query))
}

/// The first match at or after `from`, wrapping to the start of the buffer.
#[must_use]
pub fn find_next(text: &Text, query: &Query, from: usize) -> Option<Range> {
    scan(text, query)
        .find(|range| range.start >= from)
        .or_else(|| scan(text, query).next())
}

/// The last match before `from`, wrapping to the end of the buffer.
#[must_use]
pub fn find_previous(text: &Text, query: &Query, from: usize) -> Option<Range> {
    let before = scan(text, query)
        .take_while(|range| range.end <= from)
        .last();
    before.or_else(|| scan(text, query).last())
}

/// Ranges to rewrite for a replace-all, or the refusal.
///
/// Returns the ranges rather than performing the edit: the document owns every
/// mutation, and this way the caller gets one transaction it can undo at once.
pub fn replace_all_ranges(text: &Text, query: &Query) -> Result<Vec<Range>, ReplaceOutcome> {
    if query.is_empty() {
        return Ok(Vec::new());
    }
    let mut ranges = Vec::new();
    for range in scan(text, query) {
        if ranges.len() == MAX_REPLACE_MATCHES {
            return Err(ReplaceOutcome::OverBudget {
                limit: MAX_REPLACE_MATCHES,
            });
        }
        ranges.push(range);
    }
    Ok(ranges)
}

/// Non-overlapping matches in ascending order.
fn scan<'a>(text: &'a Text, query: &'a Query) -> impl Iterator<Item = Range> + 'a {
    scan_from(text, query, 0)
}

/// [`scan`] from a byte offset, which must be a character boundary.
///
/// A regular expression is matched eagerly into a `Vec` rather than lazily:
/// the compiled program lives in a thread-local the iterator cannot borrow
/// across a yield, and the result is bounded by the same caps the literal path
/// has; a caller that needs only a prefix uses [`regex_matches`] instead. A
/// pattern that does not compile matches nothing, which is what an unfinished
/// `(` being typed should do.
fn scan_from<'a>(
    text: &'a Text,
    query: &'a Query,
    from: usize,
) -> Box<dyn Iterator<Item = Range> + 'a> {
    if query.regex {
        return Box::new(scan_regex(text, query, from).into_iter());
    }
    Box::new(scan_literal(text, query, from))
}

/// Regex matches from `from`, capped like every other listing.
fn scan_regex(text: &Text, query: &Query, from: usize) -> Vec<Range> {
    regex_matches(text, query, from, |found| {
        found.take(MAX_REPLACE_MATCHES).collect()
    })
}

/// Hand `consume` the regex's matches from `from`, lazily.
///
/// A count or a viewport needs a prefix of the matches, and going through
/// [`scan_regex`] would first collect up to [`MAX_REPLACE_MATCHES`] of them
/// across the whole buffer — on every find keystroke and every edit while the
/// panel is open. An invalid or empty pattern hands over no matches.
fn regex_matches<T: Default>(
    text: &Text,
    query: &Query,
    from: usize,
    consume: impl FnOnce(&mut dyn Iterator<Item = Range>) -> T,
) -> T {
    if query.pattern.is_empty() {
        return T::default();
    }
    let haystack = text.as_str();
    let from = from.min(haystack.len());
    query.compiled(|program| {
        let Ok(program) = program else {
            return T::default();
        };
        let mut found = program
            .find_iter(&haystack[from..])
            // An empty match (`a*` against `b`) would otherwise be returned
            // once per byte and never advance a find-next.
            .filter(|found| found.end() > found.start())
            .map(|found| Range::new(from + found.start(), from + found.end()));
        consume(&mut found)
    })
}

fn scan_literal<'a>(
    text: &'a Text,
    query: &'a Query,
    from: usize,
) -> impl Iterator<Item = Range> + 'a {
    let haystack = text.as_str();
    let mut at = from.min(haystack.len());
    let pattern_len = query.pattern.len();
    std::iter::from_fn(move || {
        if query.pattern.is_empty() {
            return None;
        }
        loop {
            let found = if query.case_sensitive {
                haystack[at..].find(&query.pattern).map(|index| at + index)
            } else {
                find_case_insensitive(&haystack[at..], &query.pattern).map(|index| at + index)
            }?;
            // A case-insensitive hit can differ in byte length from the needle
            // only for characters this comparison never folds, so the match
            // keeps the needle's length and stays on a char boundary.
            let end = found + pattern_len;
            if !text.is_boundary(found) || !text.is_boundary(end) {
                at = found + 1;
                while !text.is_boundary(at) {
                    at += 1;
                }
                continue;
            }
            at = end.max(found + 1);
            if query.whole_word && !is_word_bounded(haystack, found, end) {
                continue;
            }
            return Some(Range::new(found, end));
        }
    })
}

/// ASCII-only case folding. A Turkish dotless i is not folded here, and saying
/// so is better than a wrong match in a language this cannot reason about.
fn find_case_insensitive(haystack: &str, needle: &str) -> Option<usize> {
    let haystack = haystack.as_bytes();
    let needle = needle.as_bytes();
    if needle.len() > haystack.len() {
        return None;
    }
    (0..=haystack.len() - needle.len()).find(|&start| {
        haystack[start..start + needle.len()]
            .iter()
            .zip(needle)
            .all(|(left, right)| left.eq_ignore_ascii_case(right))
    })
}

fn is_word_bounded(haystack: &str, start: usize, end: usize) -> bool {
    let before = haystack[..start].chars().next_back();
    let after = haystack[end..].chars().next();
    !before.is_some_and(is_word_char) && !after.is_some_and(is_word_char)
}

fn is_word_char(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(body: &str) -> Text {
        Text::new(body.to_string())
    }

    #[test]
    fn matches_do_not_overlap_and_run_in_order() {
        let found = find_all(&text("aaaa"), &Query::literal("aa"));
        assert_eq!(
            found.found,
            vec![
                Match {
                    range: Range::new(0, 2)
                },
                Match {
                    range: Range::new(2, 4)
                }
            ]
        );
        assert!(!found.truncated);
    }

    #[test]
    fn case_insensitive_and_whole_word_narrow_the_answer() {
        let body = text("Cat category cat");
        assert_eq!(find_all(&body, &Query::literal("cat")).found.len(), 2);
        assert_eq!(
            find_all(&body, &Query::literal("cat").case_insensitive())
                .found
                .len(),
            3
        );
        assert_eq!(
            find_all(
                &body,
                &Query::literal("cat").case_insensitive().whole_word()
            )
            .found
            .len(),
            2
        );
    }

    #[test]
    fn a_match_never_starts_inside_a_multibyte_character() {
        // The second byte of 'é' is 0xA9; searching for a byte pattern that
        // could land there must not produce a range that splits it.
        let body = text("éa");
        let found = find_all(&body, &Query::literal("a"));
        assert_eq!(found.found[0].range, Range::new(2, 3));
    }

    #[test]
    fn next_and_previous_wrap() {
        let body = text("a b a");
        assert_eq!(
            find_next(&body, &Query::literal("a"), 1),
            Some(Range::new(4, 5))
        );
        assert_eq!(
            find_next(&body, &Query::literal("a"), 5),
            Some(Range::new(0, 1))
        );
        assert_eq!(
            find_previous(&body, &Query::literal("a"), 5),
            Some(Range::new(4, 5))
        );
        assert_eq!(
            find_previous(&body, &Query::literal("a"), 0),
            Some(Range::new(4, 5))
        );
    }

    #[test]
    fn listing_truncates_where_replacing_does_not() {
        let body = text(&"x".repeat(MAX_SEARCH_RESULTS + 10));
        let listed = find_all(&body, &Query::literal("x"));
        assert_eq!(listed.found.len(), MAX_SEARCH_RESULTS);
        assert!(listed.truncated);
        let ranges = replace_all_ranges(&body, &Query::literal("x")).expect("within budget");
        assert_eq!(ranges.len(), MAX_SEARCH_RESULTS + 10);
    }

    #[test]
    fn an_empty_pattern_finds_nothing() {
        let body = text("abc");
        assert!(find_all(&body, &Query::literal("")).found.is_empty());
        assert_eq!(find_next(&body, &Query::literal(""), 0), None);
    }

    #[test]
    fn a_regular_expression_matches_where_a_literal_would_not() {
        let body = text("fn one() {}\nfn two() {}\nlet three = 3;\n");
        let query = Query::literal(r"fn \w+").regular_expression();
        let found = find_all(&body, &query);
        assert_eq!(found.found.len(), 2);
        assert_eq!(body.slice(found.found[0].range), "fn one");
        assert_eq!(body.slice(found.found[1].range), "fn two");
    }

    /// `.` stops at a line break, so one match never swallows the file.
    #[test]
    fn a_dot_does_not_cross_a_line() {
        let body = text("a\nb\n");
        let found = find_all(&body, &Query::literal("a.*").regular_expression());
        assert_eq!(found.found.len(), 1);
        assert_eq!(body.slice(found.found[0].range), "a");
    }

    #[test]
    fn case_and_whole_word_apply_to_a_regular_expression_too() {
        let body = text("Cat category\n");
        let insensitive = Query::literal("cat")
            .regular_expression()
            .case_insensitive();
        assert_eq!(find_all(&body, &insensitive).found.len(), 2);
        let bounded = insensitive.clone().whole_word();
        let found = find_all(&body, &bounded);
        assert_eq!(found.found.len(), 1);
        assert_eq!(body.slice(found.found[0].range), "Cat");
    }

    /// A pattern being typed is invalid on the way to being valid, and an
    /// unfinished group must match nothing rather than refuse the keystroke.
    #[test]
    fn an_unfinished_pattern_matches_nothing_and_says_why() {
        let query = Query::literal("(ab").regular_expression();
        assert!(matches!(query.validate(), Err(QueryError::Invalid(_))));
        assert!(find_all(&text("ab"), &query).found.is_empty());
        assert_eq!(find_next(&text("ab"), &query, 0), None);
    }

    /// An empty match would be returned once per byte and never advance a
    /// find-next; it is dropped instead.
    #[test]
    fn an_empty_match_is_not_a_match() {
        let body = text("bbb");
        let found = find_all(&body, &Query::literal("a*").regular_expression());
        assert!(found.found.is_empty());
    }

    /// The caps are on the pattern, which is the input a person can grow
    /// without bound. `regex` is linear in the haystack, so there is no
    /// backtracking blow-up to time out against.
    #[test]
    fn an_oversize_pattern_is_refused_before_it_is_compiled() {
        let long = Query::literal("a".repeat(MAX_PATTERN_BYTES + 1)).regular_expression();
        assert_eq!(
            long.validate(),
            Err(QueryError::TooLong {
                limit: MAX_PATTERN_BYTES
            })
        );
        assert!(find_all(&text("aaa"), &long).found.is_empty());

        // Short source, very large program: refused at the size limit rather
        // than allocated.
        let huge = Query::literal("(a{1000}){1000}").regular_expression();
        assert!(matches!(huge.validate(), Err(QueryError::Invalid(_))));
    }

    /// A literal is never parsed as a pattern, so `a.c` finds `a.c`.
    #[test]
    fn a_literal_query_is_still_literal() {
        let body = text("abc a.c\n");
        let found = find_all(&body, &Query::literal("a.c"));
        assert_eq!(found.found.len(), 1);
        assert_eq!(body.slice(found.found[0].range), "a.c");
    }

    /// Compiling is cached on the query, so a scan inside find-as-you-type does
    /// not pay for it twice.
    #[test]
    fn the_same_pattern_is_compiled_once() {
        let body = text("abcabc");
        let query = Query::literal("a.c").regular_expression();
        assert_eq!(find_all(&body, &query).found.len(), 2);
        let reused = COMPILED.with(|slot| slot.borrow().is_some());
        assert!(reused, "the program stayed for the next scan");
        assert_eq!(count_matches(&body, &query, 10), 2);
    }

    #[test]
    fn regex_counts_and_windows_stop_where_their_caller_does() {
        let body = text("ab ab ab ab ab\n");
        let query = Query::literal("a.").regular_expression();
        assert_eq!(count_matches(&body, &query, 3), 3);
        assert_eq!(count_matches(&body, &query, 100), 5);
        assert_eq!(count_matches_before(&body, &query, 6, 100), 2);
        assert_eq!(
            find_in(&body, &query, Range::new(4, 7)),
            vec![Range::new(3, 5), Range::new(6, 8)]
        );
        let broken = Query::literal("(").regular_expression();
        assert_eq!(count_matches(&body, &broken, 100), 0);
        assert!(find_in(&body, &broken, Range::new(0, 5)).is_empty());
    }
}
