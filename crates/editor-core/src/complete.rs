//! Completions from the words the buffer already contains.
//!
//! No language server and no index: the identifiers in front of a person are
//! the ones they are about to type again, and that is a suggestion this editor
//! can make without leaving the process. A candidate that is only ever wrong —
//! a keyword fragment, the word being typed itself — is not offered, because a
//! list that has to be dismissed costs more than no list.

use std::collections::BTreeSet;

use crate::movement::word_span;
use crate::text::Text;

/// Most candidates one list holds.
///
/// A list is read, not scrolled: past this the prefix is too short to be a
/// question, and the answer would be the buffer's vocabulary rather than a
/// suggestion.
pub const MAX_CANDIDATES: usize = 12;

/// Shortest prefix that asks a question.
///
/// One character matches most of a file. Two is where a person has committed
/// to a word.
pub const MIN_PREFIX: usize = 2;

/// How much of the buffer is scanned for words.
///
/// This runs at a keystroke, so it is bounded like every other per-mutation
/// pass. A 512 KiB window around the caret is far more vocabulary than a list
/// of twelve can show.
pub const MAX_SCAN_BYTES: usize = 512 * 1024;

/// What a completion would replace, and with what.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Completions {
    /// Byte range of the prefix being completed.
    pub from: usize,
    pub to: usize,
    /// Candidates, shortest first then alphabetical, none equal to the prefix.
    pub words: Vec<String>,
}

/// Words in the buffer that continue the identifier at `caret`.
///
/// `None` when there is no prefix worth completing: the caret is not in a word,
/// the word is too short, or nothing else in the buffer starts with it.
#[must_use]
pub fn at(text: &Text, caret: usize) -> Option<Completions> {
    let (from, to) = word_span(text, caret);
    // Only a prefix: completing from the middle of a word would replace the
    // half the person has already read.
    if to != caret || caret - from < MIN_PREFIX {
        return None;
    }
    let prefix = &text.as_str()[from..caret];
    if prefix.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    let window = window(text, caret);
    let mut found: BTreeSet<&str> = BTreeSet::new();
    for word in words(&text.as_str()[window.0..window.1]) {
        if word.len() > prefix.len() && word.starts_with(prefix) {
            found.insert(word);
        }
    }
    if found.is_empty() {
        return None;
    }
    let mut words: Vec<String> = found.into_iter().map(str::to_string).collect();
    // Shortest first: the nearest completion of a prefix is the likeliest, and
    // alphabetical inside a length keeps the list stable as the buffer changes.
    words.sort_by(|left, right| left.len().cmp(&right.len()).then_with(|| left.cmp(right)));
    words.truncate(MAX_CANDIDATES);
    Some(Completions {
        from,
        to: caret,
        words,
    })
}

/// The slice of the buffer words are taken from, on character boundaries.
fn window(text: &Text, caret: usize) -> (usize, usize) {
    let half = MAX_SCAN_BYTES / 2;
    let body = text.as_str();
    let mut start = caret.saturating_sub(half);
    while start > 0 && !text.is_boundary(start) {
        start -= 1;
    }
    let mut end = (caret + half).min(body.len());
    while end < body.len() && !text.is_boundary(end) {
        end += 1;
    }
    (start, end)
}

/// Identifier-shaped runs, in order.
fn words(body: &str) -> impl Iterator<Item = &str> {
    body.split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .filter(|word| word.len() >= MIN_PREFIX && !word.starts_with(|c: char| c.is_ascii_digit()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(body: &str) -> Text {
        Text::new(body.to_string())
    }

    #[test]
    fn a_prefix_is_completed_from_the_buffers_own_words() {
        let body = text("let total = 0;\nlet totals = 1;\nlet to");
        let found = at(&body, body.len()).expect("candidates");
        assert_eq!(found.words, vec!["total", "totals"]);
        assert_eq!(&body.as_str()[found.from..found.to], "to");
    }

    /// A list that has to be dismissed costs more than no list.
    #[test]
    fn nothing_is_offered_where_there_is_no_question() {
        // Too short to be a question.
        assert!(at(&text("total\nt"), 7).is_none());
        // Not at the end of a word: completing would replace what was read.
        let body = text("total total");
        assert!(at(&body, 2).is_none());
        // Nothing else starts with it.
        assert!(at(&text("alpha beta\nzz"), 13).is_none());
        // A number is not an identifier.
        assert!(at(&text("1234 12"), 7).is_none());
    }

    /// The word being typed is not a completion of itself.
    #[test]
    fn the_prefix_itself_is_never_a_candidate() {
        let body = text("total\ntotal");
        assert!(at(&body, body.len()).is_none());
        let with_longer = text("total totally\ntotal");
        let found = at(&with_longer, with_longer.len()).expect("candidates");
        assert_eq!(found.words, vec!["totally"]);
    }

    #[test]
    fn the_list_is_capped_and_shortest_first() {
        let mut body = String::new();
        for n in 0..MAX_CANDIDATES * 3 {
            body.push_str(&format!("value{n:04} "));
        }
        body.push_str("val");
        let body = text(&body);
        let found = at(&body, body.len()).expect("candidates");
        assert_eq!(found.words.len(), MAX_CANDIDATES);
        assert!(found.words.windows(2).all(|pair| pair[0] <= pair[1]));
    }
}
