//! Go to definition from a parse: tree-sitter grammars compiled into the
//! daemon, asked lazily.
//!
//! Nothing is indexed up front. A lookup greps the checkout for files that
//! contain the word, parses the closest of them within a budget, and keeps
//! each file's definitions until it changes. A binding in scope at the caret
//! is answered from the source file alone.

mod cache;
mod extract;
mod grammars;

#[cfg(test)]
mod tests;

use std::fs::{self, File};
use std::io::Read;
use std::path::{Component, Path};
use std::sync::Arc;

use fs_service::definition::{
    Answer, Candidate, Confidence, DefinitionProvider, DefinitionQuery, Target,
};
use fs_service::{FsError, MAX_FILE_BYTES};
use git_service::{run_git_prefix, GitError};
use languages::{Language, Registry};
use tree_sitter::Parser;

use crate::cache::{Cache, Definition, Stamp};
use crate::grammars::{compiled, Compiled, GRAMMARS};

/// Files a lookup reads definitions from, cached or not.
const MAX_FILES_VISITED: usize = 2048;
/// Files a lookup parses; the rest must come from the cache.
const MAX_FILES_PARSED: usize = 256;
/// Bytes a lookup parses.
const MAX_BYTES_PARSED: u64 = 16 * 1024 * 1024;
/// `git grep -l` output kept: about twenty thousand paths.
const MAX_GREP_BYTES: usize = 1024 * 1024;
const CACHE_ENTRIES: usize = 8192;
const CACHE_BYTES: usize = 32 * 1024 * 1024;

/// Definitions from tree-sitter tags queries, plus scope-aware locals.
pub struct SyntaxProvider {
    cache: Cache,
    #[cfg(test)]
    parses: std::sync::atomic::AtomicUsize,
}

impl Default for SyntaxProvider {
    fn default() -> Self {
        Self::new()
    }
}

/// The grammar ids compiled in, as `languages.toml` names them.
#[must_use]
pub fn grammar_ids() -> &'static [&'static str] {
    const IDS: [&str; GRAMMARS.len()] = {
        let mut ids = [""; GRAMMARS.len()];
        let mut index = 0;
        while index < GRAMMARS.len() {
            ids[index] = GRAMMARS[index].id;
            index += 1;
        }
        ids
    };
    &IDS
}

/// Whether a workspace path is in a language this provider parses.
#[must_use]
pub fn supports(path: &str) -> bool {
    grammar_for(Registry::builtin(), path).is_some()
}

fn grammar_for<'r>(registry: &'r Registry, path: &str) -> Option<(&'r Language, &'r str)> {
    let language = registry.language_for_path(path)?;
    let grammar = language.grammar.as_deref()?;
    grammar_ids()
        .contains(&grammar)
        .then_some((language, grammar))
}

/// A workspace-relative path that names nothing outside the checkout lexically.
fn is_plain_relative(path: &str) -> bool {
    !path.is_empty()
        && Path::new(path)
            .components()
            .all(|component| matches!(component, Component::Normal(_) | Component::CurDir))
}

/// Why the scan stopped before it saw every file, mildest first.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Cut {
    None,
    /// The caller's `limit`: there may be more, but not better.
    Limit,
    /// A budget, the deadline or a cancel.
    Budget,
}

struct Scan<'q, 'a> {
    query: &'q DefinitionQuery<'a>,
    parser: Parser,
    visited: usize,
    parsed_files: usize,
    parsed_bytes: u64,
}

impl Scan<'_, '_> {
    fn expired(&self) -> bool {
        self.query.expired()
    }

    fn parse_definitions(&mut self, compiled: &Compiled, text: &str) -> Option<Vec<Definition>> {
        let query = self.query;
        let expired = || query.expired();
        let tree = extract::parse(&mut self.parser, compiled, text, &expired)?;
        extract::definitions(compiled, &tree, text, &expired)
    }
}

impl SyntaxProvider {
    #[must_use]
    pub fn new() -> Self {
        Self {
            cache: Cache::new(CACHE_ENTRIES, CACHE_BYTES),
            #[cfg(test)]
            parses: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    /// Forget cached definitions under `root`: one workspace-relative `path`,
    /// or every file when `None`.
    pub fn invalidate(&self, root: &Path, path: Option<&str>) {
        self.cache.invalidate(root, path);
    }

    fn count_parse(&self) {
        #[cfg(test)]
        self.parses
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    fn lookup(&self, query: &DefinitionQuery<'_>) -> Result<Answer, FsError> {
        let registry = Registry::builtin();
        if !is_plain_relative(query.source) {
            return Err(FsError::EscapesWorkspace(query.source.to_string()));
        }
        let Some((source_language, source_grammar)) = grammar_for(registry, query.source) else {
            return Ok(Answer::default());
        };
        let Some(source_compiled) = compiled(source_grammar) else {
            return Ok(Answer::default());
        };
        if query.expired() {
            return Ok(truncated());
        }
        let mut scan = Scan {
            query,
            parser: Parser::new(),
            visited: 0,
            parsed_files: 0,
            parsed_bytes: 0,
        };

        let mut source_definitions = None;
        let source_text = match query.buffer {
            Some(buffer) if buffer.len() <= MAX_FILE_BYTES => Some((buffer.to_owned(), None)),
            Some(_) => None,
            None if query.position.is_some() => read_source(query.root, query.source)?,
            None => None,
        };
        if let Some((text, stamp)) = &source_text {
            let expired = || query.expired();
            self.count_parse();
            let Some(tree) = extract::parse(&mut scan.parser, source_compiled, text, &expired)
            else {
                return Ok(truncated());
            };
            if let Some(position) = query.position {
                let local = extract::caret_byte(text, position.line, position.utf16_column)
                    .and_then(|caret| {
                        extract::locals(source_compiled, &tree, text, query.symbol, caret, &expired)
                    });
                if let Some(local) = local {
                    let mut candidates = candidates(query.source, &local, query, Confidence::Local);
                    candidates.truncate(query.limit);
                    if !candidates.is_empty() {
                        return Ok(Answer {
                            authoritative: true,
                            candidates,
                            ..Answer::default()
                        });
                    }
                }
            }
            let Some(definitions) = extract::definitions(source_compiled, &tree, text, &expired)
            else {
                return Ok(truncated());
            };
            let definitions: Arc<[Definition]> = Arc::from(definitions);
            if let Some(stamp) = stamp {
                self.cache
                    .put(query.root, query.source, *stamp, Arc::clone(&definitions));
            }
            source_definitions = Some(definitions);
        }

        let Some((files, grep_cut)) = grep_files(query.root, query.symbol)? else {
            return Ok(Answer::default());
        };
        let mut files: Vec<(String, &Language, &str)> = files
            .into_iter()
            .filter(|path| is_plain_relative(path))
            .filter(|path| source_definitions.is_none() || path != query.source)
            .filter_map(|path| {
                let (language, grammar) = grammar_for(registry, &path)?;
                Some((path, language, grammar))
            })
            .collect();
        files.sort_by_cached_key(|(path, language, _)| {
            closeness(query.source, source_language, path, language)
        });

        let mut found: Vec<Candidate> = Vec::new();
        let mut cut = if grep_cut { Cut::Budget } else { Cut::None };
        if let Some(definitions) = &source_definitions {
            found.extend(candidates(
                query.source,
                definitions,
                query,
                Confidence::Syntactic,
            ));
        }
        // Files are in closeness order, so once the source's own family has
        // answered, a file of another family can only add noise.
        let mut same_family = !found.is_empty();
        for (path, language, grammar) in &files {
            if found.len() >= query.limit {
                cut = cut.max(Cut::Limit);
                break;
            }
            if scan.expired() || scan.visited >= MAX_FILES_VISITED {
                cut = Cut::Budget;
                break;
            }
            scan.visited += 1;
            let family = language.family() == source_language.family();
            if same_family && !family {
                break;
            }
            let Some(compiled) = compiled(grammar) else {
                continue;
            };
            let definitions = match self.file_definitions(&mut scan, compiled, path)? {
                FileDefinitions::Found(definitions) => definitions,
                FileDefinitions::Skipped => continue,
                FileDefinitions::OverBudget => {
                    cut = Cut::Budget;
                    continue;
                }
                FileDefinitions::Stopped => {
                    cut = Cut::Budget;
                    break;
                }
            };
            let before = found.len();
            found.extend(candidates(path, &definitions, query, Confidence::Syntactic));
            same_family |= family && found.len() > before;
        }
        if found.len() > query.limit {
            found.truncate(query.limit);
            cut = cut.max(Cut::Limit);
        }
        // The caller's limit is no budget: what it cut ranks below what it kept.
        Ok(Answer {
            authoritative: !found.is_empty() && cut != Cut::Budget,
            truncated: cut != Cut::None,
            candidates: found,
            note: None,
        })
    }

    fn file_definitions(
        &self,
        scan: &mut Scan<'_, '_>,
        compiled: &Compiled,
        path: &str,
    ) -> Result<FileDefinitions, FsError> {
        let root = scan.query.root;
        let full = root.join(path);
        let meta = match fs::symlink_metadata(&full) {
            Ok(meta) => meta,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(FileDefinitions::Skipped)
            }
            Err(error) => return Err(FsError::Io(error)),
        };
        if !meta.is_file() || meta.len() > MAX_FILE_BYTES as u64 {
            return Ok(FileDefinitions::Skipped);
        }
        let stamp = Stamp {
            len: meta.len(),
            modified: meta.modified().ok(),
        };
        if let Some(definitions) = self.cache.get(root, path, stamp) {
            return Ok(FileDefinitions::Found(definitions));
        }
        if scan.parsed_files >= MAX_FILES_PARSED
            || scan.parsed_bytes.saturating_add(meta.len()) > MAX_BYTES_PARSED
        {
            return Ok(FileDefinitions::OverBudget);
        }
        let Some(text) = read_bounded(&full, meta.len())? else {
            return Ok(FileDefinitions::Skipped);
        };
        scan.parsed_files += 1;
        scan.parsed_bytes += meta.len();
        self.count_parse();
        let Some(definitions) = scan.parse_definitions(compiled, &text) else {
            return Ok(FileDefinitions::Stopped);
        };
        let definitions: Arc<[Definition]> = Arc::from(definitions);
        self.cache.put(root, path, stamp, Arc::clone(&definitions));
        Ok(FileDefinitions::Found(definitions))
    }
}

enum FileDefinitions {
    Found(Arc<[Definition]>),
    /// Gone, not a regular file, too large or not UTF-8.
    Skipped,
    /// Not cached, and this lookup has parsed all it may.
    OverBudget,
    /// The deadline passed or the lookup was cancelled.
    Stopped,
}

fn truncated() -> Answer {
    Answer {
        truncated: true,
        ..Answer::default()
    }
}

/// Same file, then same language family, then the rest; within each, the
/// same directory and then the most shared leading directories first.
fn closeness(
    source: &str,
    source_language: &Language,
    path: &str,
    language: &Language,
) -> (bool, bool, bool, std::cmp::Reverse<usize>) {
    let source_dir = Path::new(source).parent();
    let dir = Path::new(path).parent();
    let shared = match (source_dir, dir) {
        (Some(a), Some(b)) => a
            .components()
            .zip(b.components())
            .take_while(|(a, b)| a == b)
            .count(),
        _ => 0,
    };
    (
        path != source,
        language.family() != source_language.family(),
        dir != source_dir,
        std::cmp::Reverse(shared),
    )
}

fn candidates(
    path: &str,
    definitions: &[Definition],
    query: &DefinitionQuery<'_>,
    confidence: Confidence,
) -> Vec<Candidate> {
    definitions
        .iter()
        .filter(|definition| &*definition.name == query.symbol)
        .map(|definition| Candidate {
            target: Target::Workspace(path.to_string()),
            line: definition.line,
            column: definition.column,
            text: definition.text.to_string(),
            confidence,
        })
        .collect()
}

/// The source file's text, refusing a path that resolves outside `root`. The
/// stamp is there only when `source` is no symlink, so it can key the cache.
fn read_source(root: &Path, source: &str) -> Result<Option<(String, Option<Stamp>)>, FsError> {
    let full = match fs::canonicalize(root.join(source)) {
        Ok(full) => full,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(FsError::Io(error)),
    };
    if !full.starts_with(root) {
        return Err(FsError::EscapesWorkspace(source.to_string()));
    }
    let meta = fs::metadata(&full)?;
    if !meta.is_file() || meta.len() > MAX_FILE_BYTES as u64 {
        return Ok(None);
    }
    let stamp = (full == root.join(source)).then(|| Stamp {
        len: meta.len(),
        modified: meta.modified().ok(),
    });
    Ok(read_bounded(&full, meta.len())?.map(|text| (text, stamp)))
}

/// At most [`MAX_FILE_BYTES`], whatever the file grew to since `len` was read.
fn read_bounded(path: &Path, len: u64) -> Result<Option<String>, FsError> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(FsError::Io(error)),
    };
    let capacity = usize::try_from(len)
        .unwrap_or(MAX_FILE_BYTES)
        .min(MAX_FILE_BYTES);
    let mut bytes = Vec::with_capacity(capacity);
    file.take(MAX_FILE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_FILE_BYTES {
        return Ok(None);
    }
    Ok(String::from_utf8(bytes).ok())
}

/// Paths under `root` that contain `symbol` as a word, and whether the list
/// was cut. `None` when `root` is not inside a Git checkout.
fn grep_files(root: &Path, symbol: &str) -> Result<Option<(Vec<String>, bool)>, FsError> {
    let args = [
        "grep",
        "-l",
        "-z",
        "-I",
        "-w",
        "-F",
        "--no-color",
        "--untracked",
        "-e",
        symbol,
        "--",
        ".",
    ];
    let (out, cut) = run_git_prefix(Some(root), &args, MAX_GREP_BYTES)?;
    // `git grep` exits 1 when nothing matches, and a cut grep was killed, so
    // neither status is a failure.
    if !cut && out.status != 0 && out.status != 1 {
        if out.stderr.contains("not a git repository") {
            return Ok(None);
        }
        return Err(FsError::Git(GitError::CommandFailed {
            args: args.iter().map(|arg| (*arg).to_string()).collect(),
            stderr: out.stderr,
            status: out.status,
        }));
    }
    let mut records: Vec<&str> = out.stdout.split('\0').collect();
    if cut {
        records.pop();
    }
    Ok(Some((
        records
            .into_iter()
            .filter(|path| !path.is_empty())
            .map(str::to_string)
            .collect(),
        cut,
    )))
}

impl DefinitionProvider for SyntaxProvider {
    fn name(&self) -> &'static str {
        "syntax"
    }

    fn find(&self, query: &DefinitionQuery<'_>) -> Result<Answer, FsError> {
        self.lookup(query)
    }
}
