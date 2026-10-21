//! A prebuilt SCIP index read from the checkout: precise while it is fresh.
//!
//! Ranges are interpreted in each document's `position_encoding`. An index
//! that leaves it unspecified (written before the field existed) is read as
//! UTF-16 code units, LSP's unit, which the earliest SCIP indexers inherited;
//! for ASCII lines every encoding agrees.

use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::SystemTime;

use fs_service::definition::{
    byte_for_utf16, utf16_column, Answer, Candidate, Confidence, DefinitionProvider,
    DefinitionQuery, Target,
};
use fs_service::FsError;
use protobuf::Message as _;
use scip::types::occurrence::Typed_range;
use scip::types::{PositionEncoding, SymbolRole};

use crate::text;

pub const DEFAULT_INDEX_NAME: &str = "index.scip";

/// Largest index file read, by `metadata`, before it is parsed.
pub const DEFAULT_MAX_INDEX_BYTES: u64 = 256 * 1024 * 1024;

/// Parsed indexes kept at once. A parsed index is the largest thing this
/// crate holds, so the cache covers the checkout in use and one other.
const CACHE_CAPACITY: usize = 2;

/// Definitions from an `index.scip` (or another configured name) at the
/// checkout root.
///
/// Loaded on the first query for a root and cached by the file's length and
/// modification time. An index answers for a source file only while that file
/// is no newer than the index and has no unsaved edits; a caret on an indexed
/// occurrence is then resolved exactly and the answer is authoritative.
pub struct ScipProvider {
    index_names: Vec<String>,
    max_index_bytes: u64,
    cache: Mutex<Vec<Entry>>,
    loads: AtomicUsize,
}

struct Entry {
    root: PathBuf,
    index: PathBuf,
    len: u64,
    modified: SystemTime,
    value: Arc<Loaded>,
}

enum Loaded {
    Ready(ScipIndex),
    /// Kept so a corrupt index is not re-read on every query.
    Unreadable(String),
}

impl Default for ScipProvider {
    fn default() -> Self {
        Self::new(vec![DEFAULT_INDEX_NAME.to_owned()])
    }
}

impl ScipProvider {
    /// Index files are looked up at the checkout root in this order; a name
    /// that is absolute or climbs is ignored, and none at all means
    /// [`DEFAULT_INDEX_NAME`].
    #[must_use]
    pub fn new(index_names: Vec<String>) -> Self {
        let mut index_names: Vec<String> = index_names
            .into_iter()
            .filter(|name| text::is_plain_relative(name))
            .collect();
        if index_names.is_empty() {
            index_names.push(DEFAULT_INDEX_NAME.to_owned());
        }
        Self {
            index_names,
            max_index_bytes: DEFAULT_MAX_INDEX_BYTES,
            cache: Mutex::new(Vec::new()),
            loads: AtomicUsize::new(0),
        }
    }

    /// Refuse index files larger than `bytes`.
    #[must_use]
    pub fn with_max_index_bytes(mut self, bytes: u64) -> Self {
        self.max_index_bytes = bytes;
        self
    }

    fn locate(&self, root: &Path) -> Option<(PathBuf, std::fs::Metadata)> {
        self.index_names.iter().find_map(|name| {
            let path = root.join(name);
            let meta = std::fs::metadata(&path).ok()?;
            meta.is_file().then_some((path, meta))
        })
    }

    fn cached(
        &self,
        root: &Path,
        index: &Path,
        len: u64,
        modified: SystemTime,
    ) -> Option<Arc<Loaded>> {
        let mut cache = self.cache.lock().unwrap_or_else(PoisonError::into_inner);
        let at = cache.iter().position(|entry| {
            entry.root == root
                && entry.index == index
                && entry.len == len
                && entry.modified == modified
        })?;
        let entry = cache.remove(at);
        let value = Arc::clone(&entry.value);
        cache.insert(0, entry);
        Some(value)
    }

    /// Parse outside the mutex, then insert; two racing queries may both
    /// parse, and the later insert wins.
    fn load(&self, root: &Path, index: &Path, meta: &std::fs::Metadata) -> Option<Arc<Loaded>> {
        let modified = meta.modified().ok()?;
        if let Some(hit) = self.cached(root, index, meta.len(), modified) {
            return Some(hit);
        }
        self.loads.fetch_add(1, Ordering::Relaxed);
        let (value, len, modified) = read_index(index, self.max_index_bytes, meta.len(), modified);
        let value = Arc::new(value);
        let mut cache = self.cache.lock().unwrap_or_else(PoisonError::into_inner);
        cache.retain(|entry| !(entry.root == root && entry.index == index));
        cache.insert(
            0,
            Entry {
                root: root.to_path_buf(),
                index: index.to_path_buf(),
                len,
                modified,
                value: Arc::clone(&value),
            },
        );
        cache.truncate(CACHE_CAPACITY);
        Some(value)
    }

    #[cfg(test)]
    fn loads(&self) -> usize {
        self.loads.load(Ordering::Relaxed)
    }

    #[cfg(test)]
    fn cached_roots(&self) -> Vec<PathBuf> {
        let cache = self.cache.lock().unwrap_or_else(PoisonError::into_inner);
        cache.iter().map(|entry| entry.root.clone()).collect()
    }
}

/// The index and the length and mtime of what was actually read, which key
/// the cache: the file may have changed since the caller's `stat`.
fn read_index(path: &Path, cap: u64, len: u64, modified: SystemTime) -> (Loaded, u64, SystemTime) {
    let failed = |why: String| (Loaded::Unreadable(why), len, modified);
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(error) => return failed(error.to_string()),
    };
    let (len, modified) = match file.metadata() {
        Ok(meta) => (meta.len(), meta.modified().unwrap_or(modified)),
        Err(error) => return failed(error.to_string()),
    };
    if len > cap {
        return (Loaded::Unreadable(too_large(len, cap)), len, modified);
    }
    let mut bytes = Vec::with_capacity(usize::try_from(len).unwrap_or(0));
    if let Err(error) = (&mut file).take(cap + 1).read_to_end(&mut bytes) {
        return (Loaded::Unreadable(error.to_string()), len, modified);
    }
    if bytes.len() as u64 > cap {
        return (
            Loaded::Unreadable(too_large(bytes.len() as u64, cap)),
            len,
            modified,
        );
    }
    let started = std::time::Instant::now();
    let loaded = match scip::types::Index::parse_from_bytes(&bytes) {
        Ok(index) => {
            drop(bytes);
            let index = ScipIndex::build(index);
            tracing::debug!(
                path = %path.display(),
                documents = index.documents.len(),
                elapsed = ?started.elapsed(),
                "scip index loaded"
            );
            Loaded::Ready(index)
        }
        Err(error) => {
            tracing::debug!(path = %path.display(), %error, "scip index unreadable");
            Loaded::Unreadable(error.to_string())
        }
    };
    (loaded, len, modified)
}

fn too_large(len: u64, cap: u64) -> String {
    format!("SCIP index is too large ({len} bytes, limit {cap})")
}

impl DefinitionProvider for ScipProvider {
    fn name(&self) -> &'static str {
        "scip"
    }

    fn find(&self, query: &DefinitionQuery<'_>) -> Result<Answer, FsError> {
        let note = |note: String| Answer {
            note: Some(note),
            ..Answer::default()
        };
        if query.expired() {
            return Ok(Answer {
                truncated: true,
                ..Answer::default()
            });
        }
        let Some((index_path, meta)) = self.locate(query.root) else {
            return Ok(Answer::default());
        };
        if meta.len() > self.max_index_bytes {
            return Ok(note(too_large(meta.len(), self.max_index_bytes)));
        }
        let Ok(index_modified) = meta.modified() else {
            return Ok(Answer::default());
        };
        if query.buffer.is_some() {
            return Ok(note("SCIP index does not cover unsaved edits".into()));
        }
        let source_modified = text::inside(query.root, query.source)
            .and_then(|path| std::fs::metadata(path).ok())
            .and_then(|meta| meta.modified().ok());
        if source_modified.is_none_or(|source| source > index_modified) {
            return Ok(note("SCIP index is older than this file".into()));
        }
        let Some(loaded) = self.load(query.root, &index_path, &meta) else {
            return Ok(Answer::default());
        };
        match &*loaded {
            Loaded::Unreadable(why) => Ok(note(format!("SCIP index could not be read: {why}"))),
            Loaded::Ready(index) => {
                if query.expired() {
                    return Ok(Answer {
                        truncated: true,
                        ..Answer::default()
                    });
                }
                Ok(index.answer(query, index_modified))
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Encoding {
    Utf8,
    Utf16,
    Utf32,
}

impl Encoding {
    fn of(value: protobuf::EnumOrUnknown<PositionEncoding>) -> Self {
        match value.enum_value() {
            Ok(PositionEncoding::UTF8CodeUnitOffsetFromLineStart) => Self::Utf8,
            Ok(PositionEncoding::UTF32CodeUnitOffsetFromLineStart) => Self::Utf32,
            _ => Self::Utf16,
        }
    }

    /// The 0-based offset in this encoding of a 1-based UTF-16 caret column.
    fn caret(self, line: Option<&str>, utf16_column: u32) -> u32 {
        let units = utf16_column.saturating_sub(1);
        let Some(line) = line else {
            return units;
        };
        let byte = byte_for_utf16(line, utf16_column);
        let offset = match self {
            Self::Utf16 => return units,
            Self::Utf8 => byte,
            Self::Utf32 => line[..byte].chars().count(),
        };
        u32::try_from(offset).unwrap_or(u32::MAX)
    }

    /// The 1-based UTF-16 column of a 0-based offset in this encoding, or 0
    /// when the line is needed and unknown.
    fn column(self, line: Option<&str>, offset: u32) -> u32 {
        let offset_usize = offset as usize;
        match (self, line) {
            (Self::Utf16, _) => offset.saturating_add(1),
            (Self::Utf8, Some(line)) => utf16_column(line, offset_usize),
            (Self::Utf32, Some(line)) => utf16_column(
                line,
                line.char_indices()
                    .nth(offset_usize)
                    .map_or(line.len(), |(at, _)| at),
            ),
            (_, None) => 0,
        }
    }
}

/// An occurrence, 0-based and half-open, in its document's encoding.
#[derive(Clone, Copy, Debug)]
struct Span {
    start: (u32, u32),
    end: (u32, u32),
    symbol: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Site {
    document: u32,
    line: u32,
    character: u32,
}

struct Document {
    path: Box<str>,
    encoding: Encoding,
    /// Sorted by start.
    occurrences: Vec<Span>,
    multiline: bool,
}

impl Document {
    /// The innermost occurrence containing the caret; one that ends exactly
    /// at it only when nothing contains it, so a caret just past a name
    /// still resolves.
    fn symbol_at(&self, caret: (u32, u32)) -> Option<u32> {
        let upto = self.occurrences.partition_point(|span| span.start <= caret);
        let mut touching = None;
        for span in self.occurrences[..upto].iter().rev() {
            if !self.multiline && span.start.0 < caret.0 {
                break;
            }
            if caret < span.end {
                return Some(span.symbol);
            }
            if caret == span.end && touching.is_none() {
                touching = Some(span.symbol);
            }
        }
        touching
    }
}

/// The parts of an index a lookup needs, with symbol strings interned away.
#[derive(Default)]
struct ScipIndex {
    documents: Vec<Document>,
    by_path: HashMap<Box<str>, u32>,
    definitions: HashMap<u32, Vec<Site>>,
    /// Descriptor or display name to the symbols of that name with a definition.
    names: HashMap<Box<str>, Vec<u32>>,
    /// By symbol id: `local N` symbols are only visible in their own document.
    local: Vec<bool>,
}

fn span_of(occurrence: &scip::types::Occurrence) -> Option<(u32, u32, u32, u32)> {
    let (start_line, start_character, end_line, end_character) = match &occurrence.typed_range {
        Some(Typed_range::SingleLineRange(range)) => (
            range.line,
            range.start_character,
            range.line,
            range.end_character,
        ),
        Some(Typed_range::MultiLineRange(range)) => (
            range.start_line,
            range.start_character,
            range.end_line,
            range.end_character,
        ),
        None => match occurrence.range[..] {
            [line, start, end] => (line, start, line, end),
            [start_line, start, end_line, end] => (start_line, start, end_line, end),
            _ => return None,
        },
        Some(_) => return None,
    };
    let span = (
        u32::try_from(start_line).ok()?,
        u32::try_from(start_character).ok()?,
        u32::try_from(end_line).ok()?,
        u32::try_from(end_character).ok()?,
    );
    ((span.0, span.1) <= (span.2, span.3)).then_some(span)
}

/// Name of a global symbol's last descriptor (`widget` for `… demo/widget().`).
fn descriptor_name(symbol: &str) -> Option<String> {
    let parsed = scip::symbol::parse_symbol(symbol).ok()?;
    parsed
        .descriptors
        .last()
        .map(|descriptor| descriptor.name.clone())
        .filter(|name| !name.is_empty())
}

struct Interner<'d> {
    globals: HashMap<String, u32>,
    locals: HashMap<&'d str, u32>,
}

impl<'d> Interner<'d> {
    fn id(&mut self, symbol: &'d str, local: &mut Vec<bool>) -> Option<u32> {
        let is_local = scip::symbol::is_local_symbol(symbol);
        let found = if is_local {
            self.locals.get(symbol)
        } else {
            self.globals.get(symbol)
        };
        if let Some(&id) = found {
            return Some(id);
        }
        let id = u32::try_from(local.len()).ok()?;
        local.push(is_local);
        if is_local {
            self.locals.insert(symbol, id);
        } else {
            self.globals.insert(symbol.to_owned(), id);
        }
        Some(id)
    }
}

impl ScipIndex {
    fn build(index: scip::types::Index) -> Self {
        let definition = SymbolRole::Definition as i32;
        let mut out = Self::default();
        let mut globals: HashMap<String, u32> = HashMap::new();
        let mut display: HashMap<u32, String> = HashMap::new();
        for document in &index.documents {
            if !text::is_plain_relative(&document.relative_path) {
                continue;
            }
            let path = document.relative_path.as_str();
            let id = if let Some(&id) = out.by_path.get(path) {
                id
            } else {
                let Ok(id) = u32::try_from(out.documents.len()) else {
                    break;
                };
                out.documents.push(Document {
                    path: path.into(),
                    encoding: Encoding::of(document.position_encoding),
                    occurrences: Vec::new(),
                    multiline: false,
                });
                out.by_path.insert(path.into(), id);
                id
            };
            let mut interner = Interner {
                globals: std::mem::take(&mut globals),
                locals: HashMap::new(),
            };
            let mut spans = Vec::with_capacity(document.occurrences.len());
            for occurrence in &document.occurrences {
                if occurrence.symbol.is_empty() {
                    continue;
                }
                let Some((start_line, start_character, end_line, end_character)) =
                    span_of(occurrence)
                else {
                    continue;
                };
                let Some(symbol) = interner.id(&occurrence.symbol, &mut out.local) else {
                    continue;
                };
                spans.push(Span {
                    start: (start_line, start_character),
                    end: (end_line, end_character),
                    symbol,
                });
                if occurrence.symbol_roles & definition != 0 {
                    out.definitions.entry(symbol).or_default().push(Site {
                        document: id,
                        line: start_line,
                        character: start_character,
                    });
                }
            }
            for info in &document.symbols {
                if info.display_name.is_empty() || info.symbol.is_empty() {
                    continue;
                }
                if let Some(symbol) = interner.id(&info.symbol, &mut out.local) {
                    display.insert(symbol, info.display_name.clone());
                }
            }
            for &symbol in interner.locals.values() {
                if let Some(name) = display.get(&symbol) {
                    if out.definitions.contains_key(&symbol) {
                        out.names
                            .entry(name.as_str().into())
                            .or_default()
                            .push(symbol);
                    }
                }
            }
            globals = interner.globals;
            out.documents[id as usize].occurrences.extend(spans);
        }
        for (symbol, &id) in &globals {
            if !out.definitions.contains_key(&id) {
                continue;
            }
            let name = display
                .get(&id)
                .cloned()
                .or_else(|| descriptor_name(symbol));
            if let Some(name) = name {
                out.names.entry(name.into()).or_default().push(id);
            }
        }
        for document in &mut out.documents {
            document.occurrences.sort_unstable_by_key(|span| span.start);
            document.multiline = document
                .occurrences
                .iter()
                .any(|span| span.start.0 != span.end.0);
        }
        out
    }

    fn answer(&self, query: &DefinitionQuery<'_>, index_modified: SystemTime) -> Answer {
        let source = self.by_path.get(query.source).copied();
        if let (Some(position), Some(source)) = (query.position, source) {
            let document = &self.documents[source as usize];
            let character = if document.encoding == Encoding::Utf16 {
                position.utf16_column.saturating_sub(1)
            } else {
                let line = text::inside(query.root, query.source)
                    .and_then(|path| text::read_lines(&path, &[position.line]).pop().flatten());
                document
                    .encoding
                    .caret(line.as_deref(), position.utf16_column)
            };
            let caret = (position.line.saturating_sub(1), character);
            if let Some(symbol) = document.symbol_at(caret) {
                let sites = self.definitions.get(&symbol).map_or(&[][..], Vec::as_slice);
                let (candidates, truncated, fresh) = self.candidates(query, sites, index_modified);
                // A symbol the index names but does not define (another
                // package's) is empty rather than a guess by name.
                return Answer {
                    authoritative: fresh && !candidates.is_empty(),
                    candidates,
                    truncated,
                    note: None,
                };
            }
        }
        let mut sites = Vec::new();
        for &symbol in self.names.get(query.symbol).map_or(&[][..], Vec::as_slice) {
            let local = self.local.get(symbol as usize).copied().unwrap_or(false);
            for site in self.definitions.get(&symbol).map_or(&[][..], Vec::as_slice) {
                if local && Some(site.document) != source {
                    continue;
                }
                sites.push(*site);
            }
        }
        let (candidates, truncated, _) = self.candidates(query, &sites, index_modified);
        Answer {
            candidates,
            truncated,
            authoritative: false,
            note: None,
        }
    }

    /// Candidates for `sites` that still exist inside the checkout, at most
    /// the query's limit, and whether every target is no newer than the index.
    fn candidates(
        &self,
        query: &DefinitionQuery<'_>,
        sites: &[Site],
        index_modified: SystemTime,
    ) -> (Vec<Candidate>, bool, bool) {
        let mut unique: Vec<Site> = Vec::new();
        let mut truncated = false;
        for site in sites {
            if unique.contains(site) {
                continue;
            }
            if unique.len() == query.limit {
                truncated = true;
                break;
            }
            unique.push(*site);
        }
        let mut fresh = true;
        let mut read: HashMap<u32, Option<Vec<Option<String>>>> = HashMap::new();
        let mut wanted: HashMap<u32, Vec<u32>> = HashMap::new();
        for site in &unique {
            wanted
                .entry(site.document)
                .or_default()
                .push(site.line.saturating_add(1));
        }
        for (&document, lines) in &wanted {
            let path = &self.documents[document as usize].path;
            let Some(resolved) = text::inside(query.root, path) else {
                read.insert(document, None);
                continue;
            };
            let newer = std::fs::metadata(&resolved)
                .and_then(|meta| meta.modified())
                .map_or(true, |modified| modified > index_modified);
            fresh &= !newer;
            read.insert(document, Some(text::read_lines(&resolved, lines)));
        }
        let mut candidates = Vec::with_capacity(unique.len());
        for site in &unique {
            let Some(Some(texts)) = read.get(&site.document) else {
                continue;
            };
            let Some(lines) = wanted.get(&site.document) else {
                continue;
            };
            let line_number = site.line.saturating_add(1);
            let line = lines
                .iter()
                .position(|&wanted| wanted == line_number)
                .and_then(|at| texts.get(at).cloned().flatten());
            let document = &self.documents[site.document as usize];
            candidates.push(Candidate {
                target: Target::Workspace(document.path.to_string()),
                line: line_number,
                column: document.encoding.column(line.as_deref(), site.character),
                text: line.as_deref().map(text::clamp).unwrap_or_default(),
                confidence: Confidence::Indexed,
            });
        }
        (candidates, truncated, fresh)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fs_service::definition::Position;
    use scip::types::{
        Document as ScipDocument, Index, Occurrence, SingleLineRange, SymbolInformation,
    };
    use std::sync::atomic::AtomicBool;
    use std::time::{Duration, Instant};

    const WIDGET: &str = "rust-analyzer cargo demo 0.1.0 demo/widget().";
    const GADGET: &str = "rust-analyzer cargo demo 0.1.0 demo/gadget().";
    const EXTERNAL: &str = "rust-analyzer cargo std 1.0.0 io/stdout().";

    const LIB: &str = "pub fn widget() -> u8 { 1 }\n/* é */ pub fn gadget() {}\n";
    const MAIN: &str =
        "fn main() { let n = demo::widget(); n; demo::gadget(); std::io::stdout(); }\n";

    fn occurrence(line: i32, start: i32, end: i32, symbol: &str, roles: i32) -> Occurrence {
        let mut occurrence = Occurrence::new();
        occurrence.range = vec![line, start, end];
        occurrence.symbol = symbol.into();
        occurrence.symbol_roles = roles;
        occurrence
    }

    fn typed(line: i32, start: i32, end: i32, symbol: &str, roles: i32) -> Occurrence {
        let mut occurrence = Occurrence::new();
        occurrence.typed_range = Some(Typed_range::SingleLineRange(SingleLineRange {
            line,
            start_character: start,
            end_character: end,
            ..SingleLineRange::default()
        }));
        occurrence.symbol = symbol.into();
        occurrence.symbol_roles = roles;
        occurrence
    }

    fn at(text: &str, needle: &str, nth: usize) -> i32 {
        i32::try_from(text.match_indices(needle).nth(nth).unwrap().0).unwrap()
    }

    fn index() -> Index {
        let definition = SymbolRole::Definition as i32;
        let mut lib = ScipDocument::new();
        lib.relative_path = "src/lib.rs".into();
        lib.position_encoding = PositionEncoding::UTF8CodeUnitOffsetFromLineStart.into();
        let second = LIB.lines().nth(1).unwrap();
        let gadget = at(second, "gadget", 0);
        lib.occurrences = vec![
            typed(0, 7, 13, WIDGET, definition),
            occurrence(1, gadget, gadget + 6, GADGET, definition),
        ];

        let mut main = ScipDocument::new();
        main.relative_path = "src/main.rs".into();
        let widget = at(MAIN, "widget", 0);
        let gadget = at(MAIN, "gadget", 0);
        let stdout = at(MAIN, "stdout", 0);
        let n_def = at(MAIN, "n =", 0);
        let n_use = at(MAIN, " n;", 0) + 1;
        main.occurrences = vec![
            occurrence(0, widget, widget + 6, WIDGET, 0),
            occurrence(0, gadget, gadget + 6, GADGET, 0),
            occurrence(0, stdout, stdout + 6, EXTERNAL, 0),
            occurrence(0, n_def, n_def + 1, "local 0", definition),
            occurrence(0, n_use, n_use + 1, "local 0", 0),
        ];
        let mut info = SymbolInformation::new();
        info.symbol = "local 0".into();
        info.display_name = "n".into();
        main.symbols = vec![info];

        let mut stray = ScipDocument::new();
        stray.relative_path = "../outside.rs".into();
        stray.occurrences = vec![occurrence(0, 0, 6, WIDGET, definition)];

        let mut index = Index::new();
        index.documents = vec![lib, main, stray];
        index
    }

    struct Fixture {
        _dir: tempfile::TempDir,
        root: PathBuf,
        index_modified: SystemTime,
    }

    fn fixture() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/lib.rs"), LIB).unwrap();
        std::fs::write(root.join("src/main.rs"), MAIN).unwrap();
        let index_path = root.join(DEFAULT_INDEX_NAME);
        std::fs::write(&index_path, index().write_to_bytes().unwrap()).unwrap();
        let index_modified = SystemTime::now() + Duration::from_secs(60);
        File::options()
            .write(true)
            .open(&index_path)
            .unwrap()
            .set_modified(index_modified)
            .unwrap();
        Fixture {
            _dir: dir,
            root,
            index_modified,
        }
    }

    fn query<'a>(
        root: &'a Path,
        source: &'a str,
        symbol: &'a str,
        position: Option<Position>,
        cancel: &'a AtomicBool,
    ) -> DefinitionQuery<'a> {
        DefinitionQuery {
            root,
            source,
            symbol,
            position,
            buffer: None,
            limit: 10,
            deadline: Instant::now() + Duration::from_secs(10),
            cancel,
        }
    }

    fn caret_on(needle: &str, nth: usize) -> Option<Position> {
        let byte = MAIN.match_indices(needle).nth(nth).unwrap().0;
        Some(Position {
            line: 1,
            utf16_column: utf16_column(MAIN, byte) + 1,
        })
    }

    fn workspace(path: &str) -> Target {
        Target::Workspace(path.into())
    }

    #[test]
    fn a_caret_on_a_reference_resolves_to_its_definition() {
        let fixture = fixture();
        let provider = ScipProvider::default();
        let cancel = AtomicBool::new(false);
        let answer = provider
            .find(&query(
                &fixture.root,
                "src/main.rs",
                "widget",
                caret_on("widget", 0),
                &cancel,
            ))
            .unwrap();
        assert!(answer.authoritative);
        assert_eq!(
            answer.candidates,
            [Candidate {
                target: workspace("src/lib.rs"),
                line: 1,
                column: 8,
                text: "pub fn widget() -> u8 { 1 }".into(),
                confidence: Confidence::Indexed,
            }]
        );

        let gadget = provider
            .find(&query(
                &fixture.root,
                "src/main.rs",
                "gadget",
                caret_on("gadget", 0),
                &cancel,
            ))
            .unwrap();
        assert_eq!(gadget.candidates[0].line, 2);
        assert_eq!(
            gadget.candidates[0].column, 16,
            "a UTF-8 byte offset of 16 past `é` is UTF-16 column 16, not 17"
        );

        let local = provider
            .find(&query(
                &fixture.root,
                "src/main.rs",
                "n",
                caret_on(" n;", 0),
                &cancel,
            ))
            .unwrap();
        assert!(local.authoritative);
        assert_eq!(local.candidates[0].target, workspace("src/main.rs"));
        assert_eq!(
            local.candidates[0].column,
            u32::try_from(at(MAIN, "n =", 0)).unwrap() + 1
        );

        let external = provider
            .find(&query(
                &fixture.root,
                "src/main.rs",
                "stdout",
                caret_on("stdout", 0),
                &cancel,
            ))
            .unwrap();
        assert_eq!(external, Answer::default(), "defined outside the index");
    }

    #[test]
    fn without_a_caret_the_name_is_matched_but_not_trusted() {
        let fixture = fixture();
        let provider = ScipProvider::default();
        let cancel = AtomicBool::new(false);
        let answer = provider
            .find(&query(
                &fixture.root,
                "src/main.rs",
                "widget",
                None,
                &cancel,
            ))
            .unwrap();
        assert!(!answer.authoritative);
        assert_eq!(
            answer
                .candidates
                .iter()
                .map(|candidate| (&candidate.target, candidate.line))
                .collect::<Vec<_>>(),
            [(&workspace("src/lib.rs"), 1)],
            "the stray `../outside.rs` document is never a target"
        );

        let local = provider
            .find(&query(&fixture.root, "src/main.rs", "n", None, &cancel))
            .unwrap();
        assert_eq!(local.candidates.len(), 1);
        let elsewhere = provider
            .find(&query(&fixture.root, "src/lib.rs", "n", None, &cancel))
            .unwrap();
        assert!(
            elsewhere.candidates.is_empty(),
            "a local is invisible outside its file"
        );
    }

    #[test]
    fn a_file_newer_than_the_index_or_with_unsaved_edits_is_not_answered() {
        let fixture = fixture();
        let provider = ScipProvider::default();
        let cancel = AtomicBool::new(false);
        File::options()
            .write(true)
            .open(fixture.root.join("src/main.rs"))
            .unwrap()
            .set_modified(fixture.index_modified + Duration::from_secs(10))
            .unwrap();
        let stale = provider
            .find(&query(
                &fixture.root,
                "src/main.rs",
                "widget",
                caret_on("widget", 0),
                &cancel,
            ))
            .unwrap();
        assert!(stale.candidates.is_empty());
        assert_eq!(
            stale.note.as_deref(),
            Some("SCIP index is older than this file")
        );
        assert_eq!(provider.loads(), 0, "a stale source never parses the index");

        let mut edited = query(&fixture.root, "src/lib.rs", "widget", None, &cancel);
        edited.buffer = Some("pub fn widget() {}");
        let edited = provider.find(&edited).unwrap();
        assert!(edited.candidates.is_empty());
        assert!(edited.note.is_some());
    }

    #[test]
    fn an_index_over_the_cap_is_refused_before_it_is_read() {
        let fixture = fixture();
        let provider = ScipProvider::default().with_max_index_bytes(16);
        let cancel = AtomicBool::new(false);
        let answer = provider
            .find(&query(
                &fixture.root,
                "src/main.rs",
                "widget",
                None,
                &cancel,
            ))
            .unwrap();
        assert!(answer.candidates.is_empty());
        assert!(answer.note.unwrap().contains("too large"));
        assert_eq!(provider.loads(), 0);
    }

    #[test]
    fn a_parsed_index_is_reused_until_it_changes_and_the_cache_is_bounded() {
        let first = fixture();
        let provider = ScipProvider::default();
        let cancel = AtomicBool::new(false);
        let ask = |root: &Path| {
            provider
                .find(&query(root, "src/main.rs", "widget", None, &cancel))
                .unwrap()
        };
        assert_eq!(ask(&first.root).candidates.len(), 1);
        assert_eq!(ask(&first.root).candidates.len(), 1);
        assert_eq!(provider.loads(), 1);

        File::options()
            .write(true)
            .open(first.root.join(DEFAULT_INDEX_NAME))
            .unwrap()
            .set_modified(first.index_modified + Duration::from_secs(1))
            .unwrap();
        ask(&first.root);
        assert_eq!(provider.loads(), 2, "a rewritten index is parsed again");
        assert_eq!(provider.cached_roots(), std::slice::from_ref(&first.root));

        let others = [fixture(), fixture()];
        for other in &others {
            ask(&other.root);
        }
        assert_eq!(provider.loads(), 4);
        assert_eq!(
            provider.cached_roots(),
            [others[1].root.clone(), others[0].root.clone()]
        );
    }

    #[test]
    fn a_corrupt_index_is_a_note_and_is_not_reparsed() {
        let fixture = fixture();
        let index_path = fixture.root.join("broken.scip");
        std::fs::write(&index_path, [0xff, 0xff, 0xff, 0xff, 0x0f, 0x01]).unwrap();
        File::options()
            .write(true)
            .open(&index_path)
            .unwrap()
            .set_modified(fixture.index_modified)
            .unwrap();
        let provider = ScipProvider::new(vec!["broken.scip".into(), "../escape.scip".into()]);
        let cancel = AtomicBool::new(false);
        for _ in 0..2 {
            let answer = provider
                .find(&query(
                    &fixture.root,
                    "src/main.rs",
                    "widget",
                    None,
                    &cancel,
                ))
                .unwrap();
            assert!(answer.candidates.is_empty());
            assert!(answer.note.unwrap().contains("could not be read"));
        }
        assert_eq!(provider.loads(), 1);
    }

    #[test]
    fn carets_convert_between_encodings() {
        let line = "a😀é widget";
        let column = utf16_column(line, line.find('w').unwrap());
        assert_eq!(Encoding::Utf8.caret(Some(line), column), 8);
        assert_eq!(Encoding::Utf32.caret(Some(line), column), 4);
        assert_eq!(Encoding::Utf16.caret(Some(line), column), 5);
        assert_eq!(Encoding::Utf8.column(Some(line), 8), column);
        assert_eq!(Encoding::Utf32.column(Some(line), 4), column);
        assert_eq!(Encoding::Utf16.column(None, 5), column);
        assert_eq!(Encoding::Utf8.column(None, 8), 0);
    }
}
