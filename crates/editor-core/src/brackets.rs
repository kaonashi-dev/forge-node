//! The bracket a caret is next to, and the one that closes it.
//!
//! Depth counting over bytes, not a parse: the grammar already said which
//! bytes are string or comment, and a `)` inside either is text rather than a
//! closer. The scan is capped, because an unmatched brace at the top of a file
//! would otherwise walk it whole on every caret move.

use crate::syntax::{Scope, Syntax};
use crate::text::Text;

/// How far a match is looked for, in bytes either side of the caret.
///
/// A caret move is not a mutation, so this runs on a rung where the document
/// scan does not: past this the answer is "no visible pair", which is what a
/// person reading a screenful of code was going to conclude anyway.
pub const MAX_SCAN_BYTES: usize = 64 * 1024;

const PAIRS: [(u8, u8); 3] = [(b'(', b')'), (b'[', b']'), (b'{', b'}')];

/// The two byte offsets of the pair around the caret, ordered.
///
/// A caret *at* an opener matches forward, a caret just *after* a closer
/// matches backward — the same rule CodeMirror's `bracketMatching` uses, so a
/// caret between `)` and `(` prefers the one it just typed past.
#[must_use]
pub fn matching(text: &Text, syntax: &Syntax, caret: usize) -> Option<(usize, usize)> {
    let bytes = text.as_str().as_bytes();
    if let Some(at) = caret.checked_sub(1) {
        if let Some(open) = closer_at(bytes, at) {
            if !is_literal(text, syntax, at) {
                return scan_back(text, syntax, at, open).map(|found| (found, at));
            }
        }
    }
    let close = opener_at(bytes, caret)?;
    if is_literal(text, syntax, caret) {
        return None;
    }
    scan_forward(text, syntax, caret, close).map(|found| (caret, found))
}

fn opener_at(bytes: &[u8], at: usize) -> Option<u8> {
    let byte = *bytes.get(at)?;
    PAIRS
        .iter()
        .find(|(open, _)| *open == byte)
        .map(|(_, close)| *close)
}

fn closer_at(bytes: &[u8], at: usize) -> Option<u8> {
    let byte = *bytes.get(at)?;
    PAIRS
        .iter()
        .find(|(_, close)| *close == byte)
        .map(|(open, _)| *open)
}

/// Whether the byte at `at` is inside a string or a comment.
fn is_literal(text: &Text, syntax: &Syntax, at: usize) -> bool {
    let line = text.line_of_offset(at);
    let column = at - text.line_start(line);
    matches!(
        syntax.scope_at(line, column),
        Scope::String | Scope::Comment
    )
}

fn scan_forward(text: &Text, syntax: &Syntax, from: usize, close: u8) -> Option<usize> {
    let bytes = text.as_str().as_bytes();
    let open = bytes[from];
    let stop = (from + MAX_SCAN_BYTES).min(bytes.len());
    let mut depth = 0_i32;
    for (at, byte) in bytes[from..stop].iter().copied().enumerate() {
        let at = from + at;
        if byte != open && byte != close {
            continue;
        }
        if is_literal(text, syntax, at) {
            continue;
        }
        depth += if byte == open { 1 } else { -1 };
        if depth == 0 {
            return Some(at);
        }
    }
    None
}

fn scan_back(text: &Text, syntax: &Syntax, from: usize, open: u8) -> Option<usize> {
    let bytes = text.as_str().as_bytes();
    let close = bytes[from];
    let stop = from.saturating_sub(MAX_SCAN_BYTES);
    let mut depth = 0_i32;
    for (at, byte) in bytes[stop..=from].iter().copied().enumerate().rev() {
        let at = stop + at;
        if byte != open && byte != close {
            continue;
        }
        if is_literal(text, syntax, at) {
            continue;
        }
        depth += if byte == close { 1 } else { -1 };
        if depth == 0 {
            return Some(at);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::Grammar;

    fn fixture(body: &str) -> (Text, Syntax) {
        (
            Text::new(body.to_string()),
            Syntax::parse(body, Grammar::Rust),
        )
    }

    #[test]
    fn a_caret_on_an_opener_finds_its_closer() {
        let (text, syntax) = fixture("fn f(a: (u8, u8)) {}");
        assert_eq!(matching(&text, &syntax, 4), Some((4, 16)));
        assert_eq!(matching(&text, &syntax, 8), Some((8, 15)));
    }

    /// A caret just past a closer matches backwards, so typing `)` shows the
    /// pair it completed rather than nothing.
    #[test]
    fn a_caret_after_a_closer_finds_its_opener() {
        let (text, syntax) = fixture("fn f(a: (u8, u8)) {}");
        assert_eq!(matching(&text, &syntax, 17), Some((4, 16)));
    }

    /// A brace inside a string or a comment is text, and neither opens a pair
    /// nor closes one.
    #[test]
    fn brackets_in_strings_and_comments_are_not_pairs() {
        let (text, syntax) = fixture("let s = \"(\";\nfn f() {}\n");
        // The `(` inside the string has no partner and is not itself a pair.
        assert_eq!(matching(&text, &syntax, 9), None);
        let (text, syntax) = fixture("fn f(/* ) */) {}");
        assert_eq!(matching(&text, &syntax, 4), Some((4, 12)));
    }

    #[test]
    fn an_unmatched_bracket_answers_nothing() {
        let (text, syntax) = fixture("fn f(a");
        assert_eq!(matching(&text, &syntax, 4), None);
        let (text, syntax) = fixture("a)");
        assert_eq!(matching(&text, &syntax, 2), None);
    }

    #[test]
    fn a_caret_nowhere_near_a_bracket_answers_nothing() {
        let (text, syntax) = fixture("let x = 1;");
        assert_eq!(matching(&text, &syntax, 4), None);
    }

    /// The scan is capped, so an unmatched brace at the top of a large file
    /// costs a bounded walk on every caret move rather than the whole buffer.
    #[test]
    fn the_scan_gives_up_past_its_budget() {
        let body = format!("({}", "x".repeat(MAX_SCAN_BYTES + 16));
        let (text, syntax) = fixture(&body);
        assert_eq!(matching(&text, &syntax, 0), None);
    }
}
