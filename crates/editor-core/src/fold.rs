//! Which lines can be folded away, by indentation.
//!
//! Indentation and not a grammar: it is the one rule that is right for Rust,
//! TypeScript, Python, YAML and Markdown at once, and a fold that disagrees
//! with the brace a person can see is worse than no fold at all. A region is a
//! line and everything indented under it, so it is exactly what the eye already
//! reads as a block.

use crate::metrics::display_column;
use crate::text::Text;

/// How many lines are scanned for regions.
///
/// The same order as the colouring cap: past it the buffer is shown plain, and
/// a fold gutter that disagreed with the colours would be its own defect.
const MAX_SCAN_LINES: usize = 100_000;

/// A block that can be folded: `header` stays, `header+1..=last` hide.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Region {
    pub header: usize,
    pub last: usize,
}

impl Region {
    /// Lines this region hides when it is folded.
    #[must_use]
    pub fn hidden(&self) -> std::ops::RangeInclusive<usize> {
        self.header + 1..=self.last
    }
}

/// Every foldable region, outermost first, in document order.
///
/// A blank line belongs to whichever block surrounds it rather than ending
/// one: a function with a paragraph break in the middle is still one block.
#[must_use]
pub fn regions(text: &Text) -> Vec<Region> {
    let count = text.line_count().min(MAX_SCAN_LINES);
    let mut out = Vec::new();
    // Open blocks as (header line, its indent). A line at or under an open
    // block's indent closes it.
    let mut open: Vec<(usize, usize)> = Vec::new();
    let mut last_content = 0_usize;
    for line in 0..count {
        let body = text.line(line);
        if body.trim().is_empty() {
            continue;
        }
        let indent = display_column(body, body.len() - body.trim_start().len());
        while let Some(&(header, level)) = open.last() {
            if indent > level {
                break;
            }
            open.pop();
            if last_content > header {
                out.push(Region {
                    header,
                    last: last_content,
                });
            }
        }
        open.push((line, indent));
        last_content = line;
    }
    for (header, _) in open {
        if last_content > header {
            out.push(Region {
                header,
                last: last_content,
            });
        }
    }
    out.sort_unstable_by_key(|region| (region.header, std::cmp::Reverse(region.last)));
    out
}

/// The innermost region whose header is at or above `line` and that covers it.
///
/// What `Alt-F` folds: the block the caret is in, not the file it is in.
#[must_use]
pub fn enclosing(regions: &[Region], line: usize) -> Option<Region> {
    regions
        .iter()
        .filter(|region| region.header == line || (region.header < line && region.last >= line))
        .max_by_key(|region| region.header)
        .copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(body: &str) -> Text {
        Text::new(body.to_string())
    }

    #[test]
    fn a_block_is_a_line_and_what_is_indented_under_it() {
        let body = text("fn f() {\n    let x = 1;\n    let y = 2;\n}\nfn g() {}\n");
        let found = regions(&body);
        assert_eq!(
            found,
            vec![Region { header: 0, last: 2 }],
            "the closing brace is at the header's own indent, so it is outside"
        );
    }

    #[test]
    fn nested_blocks_each_get_a_region() {
        let body = text("a:\n  b:\n    c: 1\n    d: 2\n  e: 3\n");
        let found = regions(&body);
        assert_eq!(
            found,
            vec![Region { header: 0, last: 4 }, Region { header: 1, last: 3 },]
        );
    }

    /// A paragraph break inside a function does not end it.
    #[test]
    fn a_blank_line_does_not_close_a_block() {
        let body = text("fn f() {\n    one();\n\n    two();\n}\n");
        assert_eq!(regions(&body), vec![Region { header: 0, last: 3 }]);
    }

    #[test]
    fn a_file_with_no_indentation_has_nothing_to_fold() {
        assert!(regions(&text("a\nb\nc\n")).is_empty());
        assert!(regions(&text("")).is_empty());
    }

    #[test]
    fn the_enclosing_region_is_the_innermost_one() {
        let body = text("a:\n  b:\n    c: 1\n  d: 2\n");
        let found = regions(&body);
        assert_eq!(enclosing(&found, 2).map(|r| r.header), Some(1));
        assert_eq!(enclosing(&found, 3).map(|r| r.header), Some(0));
        assert_eq!(enclosing(&found, 0).map(|r| r.header), Some(0));
    }

    /// A tab is a tab stop wide, so a tabbed and a spaced file nest the same.
    #[test]
    fn indentation_is_measured_in_display_columns() {
        let tabbed = text("fn f() {\n\tone();\n}\n");
        assert_eq!(regions(&tabbed), vec![Region { header: 0, last: 1 }]);
    }
}
