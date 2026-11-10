//! Project content search (⇧⌘F): one query, one debounce, one `SearchFiles`
//! of kind `Content` (a `git grep -F` in the daemon), its hits grouped by file
//! with their context merged into runs. Hits are found once, when the answer
//! lands, never while drawing.

use std::collections::{BTreeMap, HashSet};
use std::ops::Range;
use std::time::{Duration, Instant};

use domain::{SearchMatch, SearchResults};
use rune::TextEdit;

use super::Scope;
use crate::file_icons::FileIcon;

/// Below this a content search would match too much of the checkout.
pub const MIN_CONTENT_QUERY: usize = 2;
/// Typing settles before a `git grep` is spent on it.
pub const DEBOUNCE: Duration = Duration::from_millis(250);

#[must_use]
pub fn should_search(query: &str) -> bool {
    query.trim().chars().count() >= MIN_CONTENT_QUERY
}

/// Every occurrence of `needle` in `text`, left to right and never
/// overlapping. ASCII case only: the daemon's `git grep -F -i` runs under
/// `LC_ALL=C`, and a Unicode fold could change a length and shift offsets.
#[must_use]
pub fn hit_ranges(text: &str, needle: &str) -> Vec<Range<usize>> {
    let mut hits = Vec::new();
    if needle.is_empty() {
        return hits;
    }
    let hay = text.to_ascii_lowercase();
    let want = needle.to_ascii_lowercase();
    let mut from = 0;
    while let Some(at) = hay[from..].find(&want) {
        let start = from + at;
        hits.push(start..start + want.len());
        from = start + want.len();
    }
    hits
}

#[derive(Debug, PartialEq, Eq)]
pub struct ExcerptLine {
    pub line: u32,
    pub text: String,
    pub hit: bool,
    /// Byte ranges of the needle in `text`.
    pub hits: Vec<Range<usize>>,
}

/// Each line of one file's excerpts: its text and whether it is a hit.
type FileLines = BTreeMap<u32, (String, bool)>;

#[derive(Debug)]
pub struct FileHits {
    pub path: String,
    pub icon: FileIcon,
    pub hits: usize,
    /// Runs of consecutive lines; a gap between runs is lines nobody asked for.
    pub excerpts: Vec<Vec<ExcerptLine>>,
}

/// Each hit's context merged into contiguous runs, per file, in daemon
/// order: two hits whose context touches share one run.
#[must_use]
pub fn build_excerpts(matches: &[SearchMatch], needle: &str) -> Vec<FileHits> {
    let mut files: Vec<(String, usize, FileLines)> = Vec::new();
    for found in matches {
        let at = match files.iter().position(|(path, ..)| *path == found.path) {
            Some(at) => at,
            None => {
                files.push((found.path.clone(), 0, BTreeMap::new()));
                files.len() - 1
            }
        };
        let (_, hits, lines) = &mut files[at];
        *hits += 1;
        let first = found.line.saturating_sub(found.before.len() as u32);
        for (offset, text) in found.before.iter().enumerate() {
            let line = first + offset as u32;
            if line >= 1 {
                lines.entry(line).or_insert_with(|| (text.clone(), false));
            }
        }
        lines.insert(found.line, (found.text.clone(), true));
        for (offset, text) in found.after.iter().enumerate() {
            let line = found.line + 1 + offset as u32;
            lines.entry(line).or_insert_with(|| (text.clone(), false));
        }
    }
    files
        .into_iter()
        .map(|(path, hits, lines)| {
            let mut excerpts: Vec<Vec<ExcerptLine>> = Vec::new();
            for (line, (text, hit)) in lines {
                let entry = ExcerptLine {
                    line,
                    hits: if hit {
                        hit_ranges(&text, needle)
                    } else {
                        Vec::new()
                    },
                    text,
                    hit,
                };
                match excerpts.last_mut() {
                    Some(run) if run.last().is_some_and(|last| last.line + 1 == line) => {
                        run.push(entry);
                    }
                    _ => excerpts.push(vec![entry]),
                }
            }
            FileHits {
                icon: crate::file_icons::for_path(&path),
                path,
                hits,
                excerpts,
            }
        })
        .collect()
}

pub struct Results {
    pub files: Vec<FileHits>,
    pub hits: usize,
    pub truncated: bool,
}

impl Results {
    #[must_use]
    pub fn summary(&self) -> String {
        let plural =
            |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
        let mut out = format!(
            "{} in {}",
            plural(self.hits, "match", "matches"),
            plural(self.files.len(), "file", "files")
        );
        if self.truncated {
            out.push_str(" — stopped at the hit budget");
        }
        out
    }
}

pub struct ProjectSearch {
    pub edit: TextEdit,
    /// When the query is to be sent; typing pushes it back.
    due: Option<Instant>,
    /// Ask again even when this exact question was answered.
    force: bool,
    /// The question out or answered.
    asked: Option<(Scope, String)>,
    pub results: Option<Results>,
    pub error: Option<String>,
    pub searching: bool,
    /// Files changed on disk since the results were read.
    pub stale: bool,
    pub folded: HashSet<String>,
}

impl Default for ProjectSearch {
    fn default() -> Self {
        Self {
            edit: TextEdit::new(""),
            due: None,
            force: false,
            asked: None,
            results: None,
            error: None,
            searching: false,
            stale: false,
            folded: HashSet::new(),
        }
    }
}

impl ProjectSearch {
    #[must_use]
    pub fn query(&self) -> &str {
        self.edit.text.trim()
    }

    pub fn typed(&mut self, now: Instant) {
        if should_search(self.query()) {
            self.due = Some(now + DEBOUNCE);
        } else {
            self.due = None;
            self.asked = None;
            self.results = None;
            self.searching = false;
            self.error = None;
        }
    }

    pub fn search_now(&mut self, now: Instant) {
        if should_search(self.query()) {
            self.due = Some(now);
            self.force = true;
        }
    }

    /// The checkout changed: the query stands and is asked again there.
    pub fn rescope(&mut self, now: Instant) {
        self.asked = None;
        self.results = None;
        self.searching = false;
        self.error = None;
        self.stale = false;
        if should_search(self.query()) {
            self.due = Some(now);
        }
    }

    /// The needle to send now, once it is due and not already answered.
    pub fn take(&mut self, now: Instant, scope: Option<Scope>) -> Option<String> {
        let due = self.due?;
        if due > now {
            return None;
        }
        let scope = scope?;
        self.due = None;
        let needle = self.query().to_string();
        let force = std::mem::take(&mut self.force);
        if !should_search(&needle) {
            return None;
        }
        if !force && self.asked.as_ref() == Some(&(scope, needle.clone())) {
            return None;
        }
        self.asked = Some((scope, needle.clone()));
        self.results = None;
        self.error = None;
        self.searching = true;
        Some(needle)
    }

    /// An answer to a question since replaced, or asked in another
    /// checkout, is dropped.
    pub fn apply(&mut self, scope: Scope, query: &str, result: Result<SearchResults, String>) {
        if self.asked.as_ref() != Some(&(scope, query.to_string())) {
            return;
        }
        self.searching = false;
        match result {
            Ok(results) => {
                let files = build_excerpts(&results.matches, query);
                self.results = Some(Results {
                    hits: results.matches.len(),
                    truncated: results.truncated,
                    files,
                });
                self.error = None;
                self.stale = false;
            }
            Err(error) => self.error = Some(error),
        }
    }

    pub fn files_changed(&mut self) {
        if self.results.is_some() {
            self.stale = true;
        }
    }

    pub fn toggle(&mut self, path: &str) {
        if !self.folded.remove(path) {
            self.folded.insert(path.to_string());
        }
    }

    #[must_use]
    pub fn all_folded(&self) -> bool {
        self.results.as_ref().is_some_and(|results| {
            !results.files.is_empty()
                && results
                    .files
                    .iter()
                    .all(|file| self.folded.contains(&file.path))
        })
    }

    pub fn toggle_all(&mut self) {
        if self.all_folded() {
            self.folded.clear();
        } else if let Some(results) = &self.results {
            self.folded = results.files.iter().map(|file| file.path.clone()).collect();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::WorkspaceId;

    fn hit(path: &str, line: u32, text: &str, before: &[&str], after: &[&str]) -> SearchMatch {
        SearchMatch {
            path: path.into(),
            line,
            column: 1,
            text: text.into(),
            before: before.iter().map(|s| s.to_string()).collect(),
            after: after.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn hits_are_ascii_case_blind_and_never_overlap() {
        assert_eq!(hit_ranges("Foo foo FOO", "foo"), vec![0..3, 4..7, 8..11]);
        assert_eq!(hit_ranges("aaaa", "aa"), vec![0..2, 2..4]);
        assert_eq!(hit_ranges("é Foo", "foo"), vec![3..6], "byte offsets");
        assert!(hit_ranges("abc", "").is_empty());
    }

    #[test]
    fn touching_context_merges_into_one_run_per_file() {
        let files = build_excerpts(
            &[
                hit(
                    "a.rs",
                    3,
                    "let needle = 1;",
                    &["fn a() {", "  // x"],
                    &["}"],
                ),
                hit("a.rs", 5, "needle()", &["}"], &[]),
                hit("a.rs", 20, "needle", &[], &[]),
                hit("b.rs", 1, "needle", &["ignored before line one"], &[]),
            ],
            "needle",
        );
        assert_eq!(files.len(), 2);
        let a = &files[0];
        assert_eq!(a.hits, 3);
        assert_eq!(a.excerpts.len(), 2, "lines 1–5 touch, 20 stands alone");
        let lines: Vec<u32> = a.excerpts[0].iter().map(|line| line.line).collect();
        assert_eq!(lines, vec![1, 2, 3, 4, 5]);
        assert!(a.excerpts[0][2].hit);
        assert_eq!(a.excerpts[0][2].hits, vec![4..10]);
        assert!(!a.excerpts[0][3].hit, "context is not a hit");
        assert_eq!(files[1].excerpts[0].len(), 1, "no line zero");
    }

    #[test]
    fn a_query_is_asked_once_after_the_debounce_and_late_answers_drop() {
        let scope = Scope {
            workspace: WorkspaceId::new(),
            visit: 1,
        };
        let now = Instant::now();
        let mut search = ProjectSearch {
            edit: TextEdit::new("x"),
            ..ProjectSearch::default()
        };
        search.typed(now);
        assert_eq!(search.take(now + DEBOUNCE, Some(scope)), None, "too short");
        search.edit = TextEdit::new("needle");
        search.typed(now);
        assert_eq!(search.take(now, Some(scope)), None, "still typing");
        assert_eq!(
            search.take(now + DEBOUNCE, Some(scope)).as_deref(),
            Some("needle")
        );
        search.typed(now);
        assert_eq!(
            search.take(now + DEBOUNCE, Some(scope)),
            None,
            "the same question is not asked twice"
        );
        search.apply(scope, "needl", Ok(SearchResults::default()));
        assert!(search.searching, "an older query's answer is dropped");
        search.apply(
            scope,
            "needle",
            Ok(SearchResults {
                matches: vec![hit("a.rs", 1, "needle", &[], &[])],
                ..SearchResults::default()
            }),
        );
        assert!(!search.searching);
        assert_eq!(
            search.results.as_ref().map(Results::summary).as_deref(),
            Some("1 match in 1 file")
        );
        search.search_now(now);
        assert_eq!(
            search.take(now, Some(scope)).as_deref(),
            Some("needle"),
            "search again re-asks"
        );
    }
}
