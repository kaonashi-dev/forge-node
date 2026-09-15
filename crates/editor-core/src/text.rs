//! Byte-exact text storage with an incrementally maintained line index.
//!
//! The buffer is the file's bytes, terminators included, so a round trip is
//! exact by construction and a mixed-EOL file is never normalized. The line
//! index shifts on an edit instead of being rescanned, because rebuilding it
//! per keystroke is the cost this document model exists to avoid.

use crate::selection::{LineCol, Range};

#[derive(Clone, Default)]
pub struct Text {
    bytes: String,
    /// Byte offset of every line's first byte. Always starts at 0, so the
    /// length is the line count and a trailing terminator yields a last, empty
    /// line the way an offset model requires.
    line_starts: Vec<usize>,
}

impl Text {
    #[must_use]
    pub fn new(bytes: String) -> Self {
        let line_starts = scan_line_starts(&bytes);
        Self { bytes, line_starts }
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.bytes
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    #[must_use]
    pub fn line_count(&self) -> usize {
        self.line_starts.len()
    }

    /// First byte of `line`, clamped to the last line.
    #[must_use]
    pub fn line_start(&self, line: usize) -> usize {
        let line = line.min(self.line_starts.len() - 1);
        self.line_starts[line]
    }

    /// Last byte of `line` before its terminator.
    #[must_use]
    pub fn line_end(&self, line: usize) -> usize {
        let line = line.min(self.line_starts.len() - 1);
        let start = self.line_starts[line];
        match self.line_starts.get(line + 1) {
            // Only a `\r` that precedes the `\n` is a terminator; a lone `\r`
            // at the end of the file is content and must stay reachable.
            Some(&next) => {
                let end = next - 1;
                if end > start && self.bytes.as_bytes()[end - 1] == b'\r' {
                    end - 1
                } else {
                    end
                }
            }
            None => self.bytes.len(),
        }
    }

    /// The terminator `line` ends with: `"\r\n"`, `"\n"`, or `""` for the
    /// last line of a file that does not end in one.
    #[must_use]
    pub fn line_terminator(&self, line: usize) -> &str {
        let line = line.min(self.line_starts.len() - 1);
        match self.line_starts.get(line + 1) {
            Some(&next) => &self.bytes[self.line_end(line)..next],
            None => "",
        }
    }

    /// The line's characters without its terminator.
    #[must_use]
    pub fn line(&self, line: usize) -> &str {
        &self.bytes[self.line_start(line)..self.line_end(line)]
    }

    #[must_use]
    pub fn line_of_offset(&self, offset: usize) -> usize {
        let offset = offset.min(self.bytes.len());
        self.line_starts.partition_point(|&start| start <= offset) - 1
    }

    #[must_use]
    pub fn line_col(&self, offset: usize) -> LineCol {
        let offset = self.clamp_offset(offset);
        let line = self.line_of_offset(offset);
        LineCol {
            line,
            column: offset - self.line_starts[line],
        }
    }

    /// Offset of `column` bytes into `line`, clamped to that line's end.
    #[must_use]
    pub fn offset_of(&self, position: LineCol) -> usize {
        let line = position.line.min(self.line_starts.len() - 1);
        let start = self.line_starts[line];
        let end = self.line_end(line);
        self.clamp_offset((start + position.column).min(end))
    }

    #[must_use]
    pub fn slice(&self, range: Range) -> &str {
        let range = self.clamp_range(range);
        &self.bytes[range.start..range.end]
    }

    /// Nearest offset inside the buffer that is a `char` boundary, rounded down.
    #[must_use]
    pub fn clamp_offset(&self, offset: usize) -> usize {
        let mut offset = offset.min(self.bytes.len());
        while !self.bytes.is_char_boundary(offset) {
            offset -= 1;
        }
        offset
    }

    #[must_use]
    pub fn clamp_range(&self, range: Range) -> Range {
        let start = self.clamp_offset(range.start);
        Range {
            start,
            end: self.clamp_offset(range.end).max(start),
        }
    }

    #[must_use]
    pub fn is_boundary(&self, offset: usize) -> bool {
        offset <= self.bytes.len() && self.bytes.is_char_boundary(offset)
    }

    /// Replace `range` with `insert`, moving only the line starts the edit can
    /// reach. Callers validate the range first; this clamps rather than panics.
    pub fn replace(&mut self, range: Range, insert: &str) {
        let range = self.clamp_range(range);
        let removed = range.end - range.start;
        let from = self
            .line_starts
            .partition_point(|&start| start <= range.start);
        let to = self
            .line_starts
            .partition_point(|&start| start <= range.end);

        // Shift the untouched tail before splicing, while its indices still hold.
        if insert.len() >= removed {
            let delta = insert.len() - removed;
            for start in &mut self.line_starts[to..] {
                *start += delta;
            }
        } else {
            let delta = removed - insert.len();
            for start in &mut self.line_starts[to..] {
                *start -= delta;
            }
        }

        let fresh: Vec<usize> = insert
            .match_indices('\n')
            .map(|(at, _)| range.start + at + 1)
            .collect();
        self.line_starts.splice(from..to, fresh);
        self.bytes.replace_range(range.start..range.end, insert);
    }

    #[must_use]
    pub fn into_string(self) -> String {
        self.bytes
    }
}

impl std::fmt::Debug for Text {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Text")
            .field("bytes", &self.bytes.len())
            .field("lines", &self.line_starts.len())
            .finish()
    }
}

fn scan_line_starts(bytes: &str) -> Vec<usize> {
    let mut starts = Vec::with_capacity(bytes.len() / 32 + 1);
    starts.push(0);
    starts.extend(bytes.match_indices('\n').map(|(at, _)| at + 1));
    starts
}

#[cfg(test)]
mod tests {
    use super::*;

    fn starts(text: &Text) -> Vec<usize> {
        text.line_starts.clone()
    }

    #[test]
    fn a_trailing_terminator_makes_a_last_empty_line() {
        let text = Text::new("a\nb\n".to_string());
        assert_eq!(text.line_count(), 3);
        assert_eq!(text.line(0), "a");
        assert_eq!(text.line(1), "b");
        assert_eq!(text.line(2), "");
    }

    #[test]
    fn mixed_terminators_survive_and_are_stripped_per_line() {
        let text = Text::new("a\r\nb\nc".to_string());
        assert_eq!(text.line(0), "a");
        assert_eq!(text.line(1), "b");
        assert_eq!(text.line(2), "c");
        assert_eq!(text.as_str(), "a\r\nb\nc");
    }

    #[test]
    fn the_line_index_matches_a_full_rescan_after_every_edit() {
        let mut text = Text::new("one\ntwo\nthree\n".to_string());
        for (range, insert) in [
            (Range::new(0, 0), "X"),
            (Range::new(4, 4), "\n"),
            (Range::new(1, 6), ""),
            (Range::new(2, 2), "a\nb\nc"),
            (Range::new(0, 3), "\n\n"),
        ] {
            text.replace(range, insert);
            assert_eq!(
                starts(&text),
                scan_line_starts(text.as_str()),
                "index drifted after replacing {range:?} with {insert:?}"
            );
        }
    }

    #[test]
    fn offsets_round_trip_through_line_and_column() {
        let text = Text::new("añ\r\nbb\n".to_string());
        for line in 0..text.line_count() {
            for offset in text.line_start(line)..=text.line_end(line) {
                if !text.is_boundary(offset) {
                    continue;
                }
                assert_eq!(text.offset_of(text.line_col(offset)), offset);
            }
        }
    }

    #[test]
    fn an_offset_inside_a_terminator_clamps_to_the_line_end() {
        // A caret cannot sit between the `\r` and the `\n`, so the pair stays
        // one unit and a save cannot end up with a stray carriage return.
        let text = Text::new("a\r\nb".to_string());
        assert_eq!(text.line_end(0), 1);
        assert_eq!(text.offset_of(LineCol::new(0, 2)), 1);
        assert_eq!(text.line_terminator(0), "\r\n");
    }

    #[test]
    fn clamping_never_splits_a_multibyte_char() {
        let text = Text::new("é".to_string());
        assert_eq!(text.clamp_offset(1), 0);
        assert_eq!(text.clamp_offset(2), 2);
        assert_eq!(text.clamp_offset(99), 2);
    }
}
