//! Literal search and replace over the buffer's bytes.
//!
//! The number of matches a UI lists and the number a replacement may rewrite
//! are different budgets: showing the first 5 000 is a display decision, and
//! replacing only those would silently rewrite part of a file. A replacement
//! over budget is refused whole.
//!
//! Regular expressions are not here: they need a dependency this workspace has
//! not approved, and half a regex engine would be worse than none.

use crate::limits::{MAX_REPLACE_MATCHES, MAX_SEARCH_RESULTS};
use crate::selection::Range;
use crate::text::Text;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Query {
    pub pattern: String,
    pub case_sensitive: bool,
    pub whole_word: bool,
}

impl Query {
    #[must_use]
    pub fn literal(pattern: impl Into<String>) -> Self {
        Self {
            pattern: pattern.into(),
            case_sensitive: true,
            whole_word: false,
        }
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
    scan(text, query).take(limit).count()
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
    let haystack = text.as_str();
    let mut at = 0;
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
}
