//! Find in a previewed file. Matches are found when the query or the shown
//! text changes, never per frame, and stepping only moves `current`.

use std::ops::Range;

use rune::{Key, KeyEvent, TextEdit};

/// Past this the count says `+` rather than holding every match.
pub const MAX_HITS: usize = 5_000;

/// The key the current match's highlight carries, for the scroll reveal.
pub const CURRENT_KEY: &str = "preview-find-current";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Hits {
    #[default]
    None,
    /// Each match's Markdown source byte ranges, as the rendering shows it.
    Rendered(Vec<Vec<Range<usize>>>),
    /// `(line, byte range inside that line)`.
    Lines(Vec<(usize, Range<usize>)>),
}

impl Hits {
    #[must_use]
    pub fn len(&self) -> usize {
        match self {
            Hits::None => 0,
            Hits::Rendered(hits) => hits.len(),
            Hits::Lines(hits) => hits.len(),
        }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

pub struct PreviewFind {
    pub edit: TextEdit,
    pub current: usize,
    pub hits: Hits,
    pub capped: bool,
}

impl PreviewFind {
    #[must_use]
    pub fn new() -> Self {
        let mut edit = TextEdit::new("");
        edit.focused = true;
        Self {
            edit,
            current: 0,
            hits: Hits::None,
            capped: false,
        }
    }

    /// A second ⌘F: the keyboard back in the field, its text selected.
    pub fn refocus(&mut self) {
        let len = self.edit.text.len();
        self.edit.set_selection(0, len);
        self.edit.focused = true;
    }

    pub fn set_hits(&mut self, hits: Hits, capped: bool) {
        self.hits = hits;
        self.capped = capped;
        self.current = self.current.min(self.hits.len().saturating_sub(1));
    }

    pub fn step(&mut self, delta: isize) {
        let len = self.hits.len();
        if len == 0 {
            return;
        }
        self.current = (self.current as isize + delta).rem_euclid(len as isize) as usize;
    }

    /// `3 of 12`, `No results`, or nothing before anything is typed.
    #[must_use]
    pub fn counter(&self) -> String {
        if self.edit.text.is_empty() {
            return String::new();
        }
        if self.hits.is_empty() {
            return "No results".into();
        }
        let plus = if self.capped { "+" } else { "" };
        format!("{} of {}{plus}", self.current + 1, self.hits.len())
    }

    #[must_use]
    pub fn current_line(&self) -> Option<usize> {
        match &self.hits {
            Hits::Lines(hits) => hits.get(self.current).map(|(line, _)| *line),
            _ => None,
        }
    }
}

impl Default for PreviewFind {
    fn default() -> Self {
        Self::new()
    }
}

/// What a key in the field asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldKey {
    Step(isize),
    Close,
}

#[must_use]
pub fn field_key(ev: &KeyEvent) -> Option<FieldKey> {
    let m = ev.mods;
    match &ev.key {
        Key::Enter if !m.cmd && !m.ctrl && !m.alt => {
            Some(FieldKey::Step(if m.shift { -1 } else { 1 }))
        }
        Key::Command('g') if !m.alt && !m.ctrl => {
            Some(FieldKey::Step(if m.shift { -1 } else { 1 }))
        }
        Key::Escape => Some(FieldKey::Close),
        _ => None,
    }
}

/// Case-insensitive matches in the rendered text of Markdown `src`.
#[must_use]
pub fn rendered_hits(src: &str, query: &str) -> (Vec<Vec<Range<usize>>>, bool) {
    let mut hits = rune_kit::markdown::find(src, query, rune_kit::Case::Insensitive);
    let capped = hits.len() > MAX_HITS;
    hits.truncate(MAX_HITS);
    (hits, capped)
}

/// Case-insensitive, non-overlapping matches in each shown line of `text`.
#[must_use]
pub fn line_hits(
    text: &str,
    lines: &[Range<usize>],
    query: &str,
) -> (Vec<(usize, Range<usize>)>, bool) {
    let mut out = Vec::new();
    if query.is_empty() {
        return (out, false);
    }
    // ASCII lowercasing keeps every byte offset, so one folded copy serves
    // every line; a non-ASCII query folds char by char instead.
    let folded = query
        .is_ascii()
        .then(|| (text.to_ascii_lowercase(), query.to_ascii_lowercase()));
    for (index, range) in lines.iter().enumerate() {
        let Some(line) = text.get(range.clone()) else {
            continue;
        };
        let found: Vec<Range<usize>> = match &folded {
            Some((hay, needle)) => hay[range.clone()]
                .match_indices(needle.as_str())
                .map(|(at, hit)| at..at + hit.len())
                .collect(),
            None => folded_matches(line, query),
        };
        for hit in found {
            if out.len() == MAX_HITS {
                return (out, true);
            }
            out.push((index, hit));
        }
    }
    (out, false)
}

fn folded_matches(line: &str, query: &str) -> Vec<Range<usize>> {
    let same = |a: char, b: char| a == b || a.to_lowercase().eq(b.to_lowercase());
    let mut out = Vec::new();
    let mut from = 0;
    for (start, _) in line.char_indices() {
        if start < from {
            continue;
        }
        let mut end = start;
        let mut hay = line[start..].chars();
        let matched = query.chars().all(|want| {
            hay.next().is_some_and(|got| {
                end += got.len_utf8();
                same(got, want)
            })
        });
        if matched {
            out.push(start..end);
            from = end;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &str) -> Vec<Range<usize>> {
        let mut at = 0;
        text.split_inclusive('\n')
            .map(|line| {
                let range = at..at + line.trim_end_matches('\n').len();
                at += line.len();
                range
            })
            .collect()
    }

    #[test]
    fn lines_match_without_case_and_never_overlap() {
        let text = "Alpha alpha\nbeta\naaaa\n";
        let (hits, capped) = line_hits(text, &lines(text), "ALPHA");
        assert_eq!(hits, vec![(0, 0..5), (0, 6..11)]);
        assert!(!capped);
        let (hits, _) = line_hits(text, &lines(text), "aa");
        assert_eq!(hits, vec![(2, 0..2), (2, 2..4)]);
    }

    #[test]
    fn a_non_ascii_query_reports_byte_ranges_of_the_line() {
        let text = "año ÑANDÚ ñandú\n";
        let (hits, _) = line_hits(text, &lines(text), "ñandú");
        let line = &text[lines(text)[0].clone()];
        let shown: Vec<&str> = hits.iter().map(|(_, range)| &line[range.clone()]).collect();
        assert_eq!(shown, vec!["ÑANDÚ", "ñandú"]);
    }

    #[test]
    fn rendered_matches_skip_the_markup() {
        let src = "# Title\n\nSome **bold** text, and [a link](https://bold.example).\n";
        let (hits, _) = rendered_hits(src, "bold");
        assert_eq!(
            hits.len(),
            1,
            "the link's destination is not shown: {hits:?}"
        );
        assert_eq!(&src[hits[0][0].clone()], "bold");
    }

    #[test]
    fn steps_wrap_and_the_counter_says_where() {
        let mut find = PreviewFind::new();
        find.edit = TextEdit::new("x");
        find.set_hits(Hits::Lines(vec![(0, 0..1), (1, 0..1), (2, 0..1)]), false);
        assert_eq!(find.counter(), "1 of 3");
        find.step(-1);
        assert_eq!((find.current, find.current_line()), (2, Some(2)));
        find.step(1);
        assert_eq!(find.counter(), "1 of 3");
        find.set_hits(Hits::Lines(Vec::new()), false);
        assert_eq!(find.counter(), "No results");
    }
}
