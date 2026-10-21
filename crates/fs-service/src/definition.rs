//! Go to definition as an ordered chain of providers.
//!
//! A provider answers with candidates and a [`Confidence`]; [`resolve`] asks
//! them in order, merges, and stops at the first *authoritative* non-empty
//! answer, so a language server that is ready shadows the syntax index, which
//! shadows the grep heuristic. Every provider runs off the core lock and must
//! honour the query's deadline, cancel flag and byte budgets.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use crate::FsError;

/// A caret: 1-based line and 1-based UTF-16 column, as the editor wire counts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Position {
    pub line: u32,
    pub utf16_column: u32,
}

/// One go-to-definition question.
pub struct DefinitionQuery<'a> {
    /// Canonical checkout root.
    pub root: &'a Path,
    /// Workspace-relative path of the file the caret is in.
    pub source: &'a str,
    /// Already validated as an identifier: never a pattern.
    pub symbol: &'a str,
    pub position: Option<Position>,
    /// The editor's unsaved text of `source`, when it differs from disk.
    pub buffer: Option<&'a str>,
    pub limit: usize,
    pub deadline: Instant,
    pub cancel: &'a AtomicBool,
}

impl DefinitionQuery<'_> {
    /// A newer lookup replaced this one, or its time is up.
    #[must_use]
    pub fn expired(&self) -> bool {
        self.cancel.load(Ordering::Relaxed) || Instant::now() >= self.deadline
    }
}

/// How much a candidate can be trusted, worst first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Confidence {
    /// A line that looks like a declaration.
    Heuristic,
    /// A parse says this is a declaration of that name.
    Syntactic,
    /// A prebuilt index (ctags, SCIP) names it.
    Indexed,
    /// A binding in scope at the caret.
    Local,
    /// A language server resolved it.
    Semantic,
}

/// Where a candidate lives.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Target {
    /// Workspace-relative, inside the checkout.
    Workspace(String),
    /// Canonical absolute path outside the checkout (a crate in
    /// `~/.cargo/registry`, a toolchain's sources). Only a provider that
    /// resolved the name may produce one; it is opened read-only.
    External(PathBuf),
}

/// One place a symbol may be declared.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub target: Target,
    /// 1-based.
    pub line: u32,
    /// 1-based UTF-16 column of the name, or 0 when unknown.
    pub column: u32,
    /// The declaring line, clamped by the provider.
    pub text: String,
    pub confidence: Confidence,
}

/// One provider's answer.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Answer {
    pub candidates: Vec<Candidate>,
    /// A budget ran out before the provider saw everything.
    pub truncated: bool,
    /// Non-empty and trustworthy enough that later providers add only noise.
    pub authoritative: bool,
    /// Why the answer is weaker than it could be ("indexing").
    pub note: Option<String>,
}

/// One way of answering a [`DefinitionQuery`].
pub trait DefinitionProvider: Send + Sync {
    /// Short label for logs and the editor's status ("lsp", "syntax").
    fn name(&self) -> &'static str;

    /// Blocking; called off the core lock. A provider that does not cover the
    /// source's language answers with an empty, non-authoritative [`Answer`].
    fn find(&self, query: &DefinitionQuery<'_>) -> Result<Answer, FsError>;
}

/// The merged answer of a chain.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Resolved {
    /// Best first, capped at the query's limit.
    pub candidates: Vec<Candidate>,
    pub truncated: bool,
    /// The provider whose answer ended the chain, if one was authoritative.
    pub decided_by: Option<&'static str>,
    pub notes: Vec<String>,
}

/// Ask `chain` in order and merge.
///
/// Duplicates — the same file and line from two providers — keep the higher
/// confidence and the known column. Sorted by confidence, then the order the
/// chain produced them in: each provider already ranks its own answer (a
/// declaration elsewhere above a closer local binding), and re-sorting by
/// closeness here would undo that. A provider that fails is logged and
/// skipped: the next one is the fallback.
pub fn resolve(chain: &[&dyn DefinitionProvider], query: &DefinitionQuery<'_>) -> Resolved {
    let mut resolved = Resolved::default();
    let mut merged: Vec<(Candidate, usize)> = Vec::new();
    for provider in chain {
        if query.expired() {
            resolved.truncated = true;
            break;
        }
        let answer = match provider.find(query) {
            Ok(answer) => answer,
            Err(error) => {
                tracing::debug!(provider = provider.name(), %error, "definition provider failed");
                continue;
            }
        };
        resolved.truncated |= answer.truncated;
        if let Some(note) = answer.note {
            resolved.notes.push(note);
        }
        let decisive = answer.authoritative && !answer.candidates.is_empty();
        for candidate in answer.candidates {
            let order = merged.len();
            match merged
                .iter_mut()
                .find(|(seen, _)| seen.target == candidate.target && seen.line == candidate.line)
            {
                Some((seen, _)) => {
                    if candidate.confidence > seen.confidence {
                        let column = if candidate.column == 0 {
                            seen.column
                        } else {
                            candidate.column
                        };
                        *seen = Candidate {
                            column,
                            ..candidate
                        };
                    } else if seen.column == 0 {
                        seen.column = candidate.column;
                    }
                }
                None => merged.push((candidate, order)),
            }
        }
        if decisive {
            resolved.decided_by = Some(provider.name());
            break;
        }
    }
    merged.sort_by_key(|(candidate, order)| (std::cmp::Reverse(candidate.confidence), *order));
    if merged.len() > query.limit {
        resolved.truncated = true;
        merged.truncate(query.limit);
    }
    resolved.candidates = merged.into_iter().map(|(candidate, _)| candidate).collect();
    resolved
}

/// 1-based UTF-16 column of the byte at `byte` in `line`.
///
/// A `byte` inside a character or past the end counts up to that point.
#[must_use]
pub fn utf16_column(line: &str, byte: usize) -> u32 {
    let units: usize = line
        .char_indices()
        .take_while(|(index, _)| *index < byte)
        .map(|(_, c)| c.len_utf16())
        .sum();
    u32::try_from(units + 1).unwrap_or(u32::MAX)
}

/// Byte offset in `line` of a 1-based UTF-16 column, clamped to the line.
#[must_use]
pub fn byte_for_utf16(line: &str, column: u32) -> usize {
    let mut remaining = column.saturating_sub(1) as usize;
    for (index, c) in line.char_indices() {
        if remaining == 0 {
            return index;
        }
        let width = c.len_utf16();
        if remaining < width {
            return index;
        }
        remaining -= width;
    }
    line.len()
}

/// The grep heuristic plus the npm declaration scan, as a provider.
///
/// The last link of every chain: it needs no parse and no server, so it can
/// answer for any language, and it never claims to be authoritative.
pub struct GrepProvider;

impl DefinitionProvider for GrepProvider {
    fn name(&self) -> &'static str {
        "grep"
    }

    fn find(&self, query: &DefinitionQuery<'_>) -> Result<Answer, FsError> {
        let found = crate::search_definitions(query.root, query.source, query.symbol, query.limit)?;
        Ok(Answer {
            candidates: found
                .matches
                .into_iter()
                .map(|hit| Candidate {
                    column: char_column_to_utf16(&hit.text, hit.column),
                    target: Target::Workspace(hit.path),
                    line: hit.line,
                    text: hit.text,
                    confidence: Confidence::Heuristic,
                })
                .collect(),
            truncated: found.truncated,
            authoritative: false,
            note: None,
        })
    }
}

/// `SearchMatch::column` counts characters from 1; the wire counts UTF-16.
fn char_column_to_utf16(line: &str, column: u32) -> u32 {
    if column == 0 {
        return 0;
    }
    let byte = line
        .char_indices()
        .nth(column as usize - 1)
        .map_or(line.len(), |(index, _)| index);
    utf16_column(line, byte)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    struct Fixed(&'static str, Answer);

    impl DefinitionProvider for Fixed {
        fn name(&self) -> &'static str {
            self.0
        }
        fn find(&self, _: &DefinitionQuery<'_>) -> Result<Answer, FsError> {
            Ok(self.1.clone())
        }
    }

    fn at(path: &str, line: u32, column: u32, confidence: Confidence) -> Candidate {
        Candidate {
            target: Target::Workspace(path.into()),
            line,
            column,
            text: String::new(),
            confidence,
        }
    }

    fn query<'a>(cancel: &'a AtomicBool) -> DefinitionQuery<'a> {
        DefinitionQuery {
            root: Path::new("/w"),
            source: "src/a.rs",
            symbol: "thing",
            position: None,
            buffer: None,
            limit: 3,
            deadline: Instant::now() + Duration::from_secs(5),
            cancel,
        }
    }

    #[test]
    fn an_authoritative_answer_ends_the_chain_and_outranks_what_came_before() {
        let syntax = Fixed(
            "syntax",
            Answer {
                candidates: vec![at("src/b.rs", 4, 0, Confidence::Syntactic)],
                ..Answer::default()
            },
        );
        let lsp = Fixed(
            "lsp",
            Answer {
                candidates: vec![
                    at("src/b.rs", 4, 8, Confidence::Semantic),
                    at("src/c.rs", 1, 1, Confidence::Semantic),
                ],
                authoritative: true,
                ..Answer::default()
            },
        );
        let never = Fixed(
            "grep",
            Answer {
                candidates: vec![at("src/z.rs", 9, 1, Confidence::Heuristic)],
                ..Answer::default()
            },
        );
        let cancel = AtomicBool::new(false);
        let resolved = resolve(&[&syntax, &lsp, &never], &query(&cancel));
        assert_eq!(resolved.decided_by, Some("lsp"));
        assert_eq!(
            resolved.candidates,
            vec![
                at("src/b.rs", 4, 8, Confidence::Semantic),
                at("src/c.rs", 1, 1, Confidence::Semantic)
            ]
        );
    }

    #[test]
    fn an_empty_authoritative_answer_falls_through_and_a_providers_order_stands() {
        let lsp = Fixed(
            "lsp",
            Answer {
                authoritative: true,
                note: Some("indexing".into()),
                ..Answer::default()
            },
        );
        let grep = Fixed(
            "grep",
            Answer {
                candidates: vec![
                    at("other.py", 1, 1, Confidence::Heuristic),
                    at("src/b.rs", 2, 1, Confidence::Heuristic),
                    at("src/a.rs", 3, 1, Confidence::Heuristic),
                    at("src/d.rs", 3, 1, Confidence::Heuristic),
                ],
                ..Answer::default()
            },
        );
        let cancel = AtomicBool::new(false);
        let resolved = resolve(&[&lsp, &grep], &query(&cancel));
        assert_eq!(resolved.decided_by, None);
        assert_eq!(resolved.notes, ["indexing"]);
        assert!(resolved.truncated, "four candidates over a limit of three");
        let paths: Vec<_> = resolved
            .candidates
            .iter()
            .map(|c| match &c.target {
                Target::Workspace(p) => p.as_str(),
                Target::External(_) => "",
            })
            .collect();
        assert_eq!(paths, ["other.py", "src/b.rs", "src/a.rs"]);
    }

    #[test]
    fn a_cancelled_query_asks_nobody() {
        let grep = Fixed(
            "grep",
            Answer {
                candidates: vec![at("a.rs", 1, 1, Confidence::Heuristic)],
                ..Answer::default()
            },
        );
        let cancel = AtomicBool::new(true);
        let resolved = resolve(&[&grep], &query(&cancel));
        assert!(resolved.candidates.is_empty());
        assert!(resolved.truncated);
    }

    #[test]
    fn utf16_columns_count_surrogate_pairs_and_round_trip() {
        let line = "a😀é b";
        let b = line.find('b').unwrap();
        assert_eq!(utf16_column(line, 0), 1);
        assert_eq!(utf16_column(line, b), 6);
        assert_eq!(byte_for_utf16(line, 6), b);
        assert_eq!(
            byte_for_utf16(line, 3),
            1,
            "inside a pair lands on its start"
        );
        assert_eq!(byte_for_utf16(line, 99), line.len());
        assert_eq!(char_column_to_utf16("\t😀fn x", 3), 4);
    }
}
