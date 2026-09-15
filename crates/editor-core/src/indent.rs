//! What a buffer indents with, and what Enter and a bracket do about it.
//!
//! Sniffed from the file rather than configured: a buffer that already uses
//! tabs must not gain spaces because a setting somewhere says two. The rules
//! here are shallow on purpose — one level in after an opener, one level out
//! for a closer on its own line — because a wrong indent a person has to undo
//! is worse than none.

use crate::metrics::TAB_WIDTH;
use crate::syntax::Grammar;
use crate::text::Text;

/// How many lines are read to decide what a buffer indents with.
///
/// The answer never changes far into a file, and this runs once per open.
const DETECT_LINES: usize = 2_000;

/// The whitespace one level of indentation is made of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IndentUnit {
    Tab,
    Spaces(usize),
}

impl IndentUnit {
    /// The text of one level.
    #[must_use]
    pub fn text(self) -> String {
        match self {
            Self::Tab => "\t".to_string(),
            Self::Spaces(width) => " ".repeat(width),
        }
    }

    /// Display columns one level is worth.
    #[must_use]
    pub fn width(self) -> usize {
        match self {
            Self::Tab => TAB_WIDTH,
            Self::Spaces(width) => width,
        }
    }
}

impl Default for IndentUnit {
    fn default() -> Self {
        Self::Spaces(TAB_WIDTH)
    }
}

/// How typing behaves in one buffer.
///
/// The adapter sets it from the path and the file's own whitespace; the core
/// never looks at a path, and a command never guesses a language.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InputStyle {
    pub indent: IndentUnit,
    /// Insert the closer when an opener is typed, and skip over one that is
    /// already there. Off means every bracket is typed literally.
    pub close_brackets: bool,
    pub grammar: Grammar,
}

/// What a buffer indents with, read off what it already does.
///
/// Tabs win on a tie: a file with both is a file somebody edited with the
/// wrong setting, and matching the majority is the least surprising repair.
#[must_use]
pub fn detect(text: &Text) -> IndentUnit {
    let mut tabbed = 0_usize;
    let mut spaced = 0_usize;
    let mut steps: [usize; 9] = [0; 9];
    let mut previous: Option<usize> = None;
    for index in 0..text.line_count().min(DETECT_LINES) {
        let line = text.line(index);
        let leading = line.len() - line.trim_start().len();
        if leading == line.len() {
            // A blank line indents nothing and says nothing about the file.
            continue;
        }
        match line.as_bytes().first() {
            Some(b'\t') => tabbed += 1,
            Some(b' ') => {
                spaced += 1;
                let width = line.bytes().take_while(|byte| *byte == b' ').count();
                if let Some(step) = previous.and_then(|before| width.checked_sub(before)) {
                    if (1..steps.len()).contains(&step) {
                        steps[step] += 1;
                    }
                }
                previous = Some(width);
            }
            _ => previous = Some(0),
        }
    }
    if tabbed >= spaced && tabbed > 0 {
        return IndentUnit::Tab;
    }
    let best = steps
        .iter()
        .enumerate()
        .skip(1)
        .max_by_key(|(step, count)| (**count, std::cmp::Reverse(*step)))
        .filter(|(_, count)| **count > 0)
        .map(|(step, _)| step);
    IndentUnit::Spaces(best.unwrap_or(TAB_WIDTH))
}

/// The leading whitespace of the line `offset` is on.
#[must_use]
pub fn leading(text: &Text, offset: usize) -> String {
    let line = text.line(text.line_of_offset(offset));
    line.chars()
        .take_while(|character| *character == ' ' || *character == '\t')
        .collect()
}

/// Whether the text before `offset` on its line opens a block.
///
/// A brace, a bracket or a paren for every grammar; a colon as well for
/// Python, where that is what a block is made of.
#[must_use]
pub fn opens_block(text: &Text, offset: usize, grammar: Grammar) -> bool {
    let line = text.line_of_offset(offset);
    let before = &text.as_str()[text.line_start(line)..offset];
    let last = before.trim_end().chars().next_back();
    match last {
        Some('{' | '[' | '(') => true,
        Some(':') => grammar == Grammar::Python,
        _ => false,
    }
}

/// The closer an opener wants, or `None` when the byte is not one.
#[must_use]
pub fn closer_for(opener: char) -> Option<char> {
    match opener {
        '(' => Some(')'),
        '[' => Some(']'),
        '{' => Some('}'),
        '"' => Some('"'),
        '\'' => Some('\''),
        '`' => Some('`'),
        _ => None,
    }
}

/// Whether an opener typed at `offset` should bring its closer.
///
/// Only when what follows is nothing, whitespace or another closer — the rule
/// CodeMirror's `closeBrackets` uses. Typing `(` before a word is somebody
/// wrapping it, and a closer dropped in the middle would have to be deleted.
#[must_use]
pub fn wants_closer(text: &Text, offset: usize, opener: char) -> bool {
    let after = text.as_str()[offset..].chars().next();
    let free = match after {
        None => true,
        Some(character) => {
            character.is_whitespace() || matches!(character, ')' | ']' | '}' | ',' | ';' | '.')
        }
    };
    if !free {
        return false;
    }
    // A quote is its own closer, so `it's` would gain one every time. Only
    // open one when the character before is not a word either.
    if matches!(opener, '"' | '\'' | '`') {
        let before = text.as_str()[..offset].chars().next_back();
        if before.is_some_and(|character| character.is_alphanumeric() || character == '_') {
            return false;
        }
    }
    true
}

/// Whether typing `closer` at `offset` should step over one already there.
#[must_use]
pub fn skips_closer(text: &Text, offset: usize, closer: char) -> bool {
    text.as_str()[offset..]
        .chars()
        .next()
        .is_some_and(|next| next == closer)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(body: &str) -> Text {
        Text::new(body.to_string())
    }

    #[test]
    fn a_tabbed_file_indents_with_tabs() {
        let body = text("fn f() {\n\tlet x = 1;\n\tif x {\n\t\tg();\n\t}\n}\n");
        assert_eq!(detect(&body), IndentUnit::Tab);
    }

    #[test]
    fn a_spaced_file_reports_the_step_it_uses() {
        let two = text("a:\n  b:\n    c: 1\n  d: 2\n");
        assert_eq!(detect(&two), IndentUnit::Spaces(2));
        let four = text("fn f() {\n    let x = 1;\n    if x {\n        g();\n    }\n}\n");
        assert_eq!(detect(&four), IndentUnit::Spaces(4));
    }

    /// Nothing to go on is four spaces, the terminal convention the tab stop
    /// already uses.
    #[test]
    fn a_file_with_no_indentation_falls_back() {
        assert_eq!(detect(&text("a\nb\nc\n")), IndentUnit::Spaces(TAB_WIDTH));
        assert_eq!(detect(&text("")), IndentUnit::Spaces(TAB_WIDTH));
    }

    #[test]
    fn a_line_that_opens_a_block_is_recognised_per_grammar() {
        let body = text("fn f() {\nif x:\nlet y = 1;\n");
        assert!(opens_block(&body, 8, Grammar::Rust));
        assert!(!opens_block(&body, 14, Grammar::Rust));
        assert!(opens_block(&body, 14, Grammar::Python));
        assert!(!opens_block(&body, 25, Grammar::Rust));
    }

    #[test]
    fn a_closer_is_only_added_where_it_would_not_be_in_the_way() {
        let body = text("ab cd");
        // Before a word: the person is wrapping it, and a closer would have to
        // be deleted.
        assert!(!wants_closer(&body, 0, '('));
        assert!(wants_closer(&body, 2, '('));
        assert!(wants_closer(&body, 5, '('));

        // A quote after a word is an apostrophe, not an opening quote.
        assert!(!wants_closer(&body, 2, '\''), "`ab'` is not an open quote");
        let spaced = text("a ");
        assert!(wants_closer(&spaced, 2, '\''));
    }

    #[test]
    fn a_closer_already_there_is_stepped_over() {
        let body = text("f()");
        assert!(skips_closer(&body, 2, ')'));
        assert!(!skips_closer(&body, 2, ']'));
        assert!(!skips_closer(&body, 3, ')'));
    }

    #[test]
    fn leading_whitespace_is_copied_verbatim() {
        let body = text("\t  x\ny\n");
        assert_eq!(leading(&body, 4), "\t  ");
        assert_eq!(leading(&body, 5), "");
    }
}
