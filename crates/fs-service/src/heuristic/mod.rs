//! The grep heuristic behind go to definition: `git grep -l` names the files
//! that mention a symbol, the closest are read first within a budget, and
//! each is scanned with its language's comments, strings and bodies known.

mod mask;
mod rank;
mod scope;
#[cfg(test)]
mod tests;

use std::fs::{self, File};
use std::io::Read;
use std::path::Path;

use git_service::{run_git_prefix, GitError};
use languages::{Language, Registry};

use crate::{
    is_binary, list_via_walk, resolve_inside, FsError, SearchMatch, SearchResults, MAX_FILE_BYTES,
};
pub(crate) use rank::Kind;
use rank::{Dialect, Site, MAX_SIGNATURE_BYTES};
use scope::{Container, Scope};

/// Files read for one search. Ordered closest first, so the budget is spent
/// where the answer most likely is rather than in `git grep`'s path order.
const MAX_FILES: usize = 512;
const MAX_READ_BYTES: usize = 32 * 1024 * 1024;
/// `git grep -l -z` output: paths only, so this is thousands of files.
const MAX_LIST_BYTES: usize = 1024 * 1024;

/// Reserved words in most languages that read them: a click on one is never
/// a request for its declaration, and grepping for it costs a full scan.
const KEYWORDS: &[&str] = &[
    "if",
    "else",
    "elif",
    "elsif",
    "unless",
    "until",
    "for",
    "foreach",
    "while",
    "do",
    "switch",
    "case",
    "return",
    "break",
    "continue",
    "goto",
    "try",
    "throw",
    "throws",
    "typeof",
    "instanceof",
    "sizeof",
    "await",
    "yield",
    "import",
    "export",
    "in",
    "fi",
    "esac",
    "lambda",
    "nonlocal",
    "except",
    "this",
    "self",
    "super",
    "null",
    "nil",
    "true",
    "false",
    "None",
    "True",
    "False",
    "undefined",
    "void",
    "fn",
    "func",
    "function",
    "def",
    "class",
    "struct",
    "enum",
    "trait",
    "impl",
    "interface",
    "let",
    "const",
    "var",
    "pub",
    "public",
    "private",
    "protected",
    "static",
    "mut",
    "namespace",
];

/// Whether `symbol` is a name a language could have declared.
///
/// The gate that keeps the grep a fixed-string search: anything else — a
/// space, a quote, a metacharacter — is answered with no hits rather than
/// handed to git.
pub(crate) fn is_identifier(symbol: &str) -> bool {
    let mut chars = symbol.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if symbol.len() > 128 {
        return false;
    }
    if !(first.is_alphabetic() || first == '_' || first == '$') {
        return false;
    }
    chars.all(|c| c.is_alphanumeric() || c == '_' || c == '$')
}

pub(crate) fn is_searchable(symbol: &str) -> bool {
    is_identifier(symbol) && !KEYWORDS.contains(&symbol)
}

/// One declaration found by [`scan_text`].
pub(crate) struct Found {
    /// 1-based.
    pub(crate) line: u32,
    /// 1-based, in characters.
    pub(crate) column: u32,
    pub(crate) kind: Kind,
    /// A binding outside every function, class and block.
    pub(crate) top_level: bool,
    pub(crate) text: String,
}

impl Found {
    /// Declarations before bindings and members, whichever file they are in:
    /// a local `let config` must not outrank `struct config` next door.
    fn tier(&self) -> u8 {
        match self.kind {
            Kind::Keyword | Kind::Head => 0,
            Kind::Binding if self.top_level => 0,
            Kind::Binding | Kind::Member => 1,
        }
    }
}

/// The declarations of `symbol` in `text`, read as `path`'s language, at most
/// one per line. Prose and data files declare nothing.
pub(crate) fn scan_text(path: &str, text: &str, symbol: &str, out: &mut Vec<Found>) {
    let language = Registry::builtin().language_for_path(path);
    if language.is_some_and(|language| !language.declarations) || !text.contains(symbol) {
        return;
    }
    let dialect = Dialect::of(language);
    let masked = mask::mask(text, &mask::Syntax::of(language));
    let mut hits = masked.match_indices(symbol).map(|(at, _)| at).peekable();
    let mut scope = Scope::default();
    let mut offset = 0;
    let mut lines = text
        .split_inclusive('\n')
        .zip(masked.split_inclusive('\n'))
        .enumerate();
    while let Some((index, (raw_line, raw_code))) = lines.next() {
        let Some(&hit) = hits.peek() else {
            break;
        };
        let end = offset + raw_line.len();
        let (line, code) = (without_newline(raw_line), without_newline(raw_code));
        // A minified line is not a declaration anyone navigates to; it is
        // skipped quietly so it cannot mark every search partial.
        if hit < end && line.len() <= MAX_SIGNATURE_BYTES {
            let following = lines.clone().map(|(_, (_, code))| without_newline(code));
            let site = (masked.as_str(), offset, code);
            if let Some((at, kind, container)) =
                best_on_line(dialect, &scope, site, symbol, following)
            {
                let top_level = matches!(container, None | Some(Container::Module))
                    && !(dialect.indented_blocks() && line.starts_with(char::is_whitespace));
                out.push(Found {
                    line: u32::try_from(index + 1).unwrap_or(u32::MAX),
                    column: u32::try_from(line[..at].chars().count() + 1).unwrap_or(u32::MAX),
                    kind,
                    top_level,
                    text: line.to_string(),
                });
            }
        }
        while hits.next_if(|at| *at < end).is_some() {}
        scope.feed(&masked, offset, offset + code.len());
        scope.end_line(end, dialect.newline_ends_statement());
        offset = end;
    }
}

fn without_newline(line: &str) -> &str {
    let line = line.strip_suffix('\n').unwrap_or(line);
    line.strip_suffix('\r').unwrap_or(line)
}

/// `line` is `(masked file, offset of the line in it, the masked line)`.
fn best_on_line<'a>(
    dialect: Dialect,
    scope: &Scope,
    line: (&str, usize, &str),
    symbol: &str,
    following: impl Iterator<Item = &'a str> + Clone,
) -> Option<(usize, Kind, Option<Container>)> {
    let (masked, offset, code) = line;
    let mut best: Option<(usize, Kind, Option<Container>)> = None;
    for (at, _) in code.match_indices(symbol) {
        let whole = !code[..at]
            .chars()
            .next_back()
            .is_some_and(|c| dialect.word_char(c))
            && !code[at + symbol.len()..]
                .chars()
                .next()
                .is_some_and(|c| dialect.word_char(c));
        if !whole {
            continue;
        }
        let here;
        let scope = if code[..at].contains(['{', '}', '(', ')', '[', ']']) {
            let mut local = scope.clone();
            local.feed(masked, offset, offset + at);
            here = local;
            &here
        } else {
            scope
        };
        let site = Site {
            code,
            at,
            len: symbol.len(),
            container: scope.container(masked, dialect),
            in_brackets: scope.in_brackets(),
            closed_typedef: scope.closed_typedef(masked),
        };
        if let Some(kind) = rank::rank(dialect, &site, following.clone()) {
            if best.is_none_or(|(_, previous, _)| kind < previous) {
                best = Some((at, kind, site.container));
            }
        }
    }
    best
}

/// Declaration candidates for `symbol` under the canonical `root`, and how
/// many lead the list as declarations rather than local bindings or members.
///
/// Declarations come first wherever they are; within a tier, the files
/// closest to `preferred` (the file the caret is in) come first.
pub(crate) fn search(
    root: &Path,
    symbol: &str,
    limit: usize,
    preferred: Option<&str>,
) -> Result<(SearchResults, usize), FsError> {
    let mut budget = Budget {
        files: MAX_FILES,
        bytes: MAX_READ_BYTES,
    };
    search_within(root, symbol, limit, preferred, &mut budget)
}

fn search_within(
    root: &Path,
    symbol: &str,
    limit: usize,
    preferred: Option<&str>,
    budget: &mut Budget,
) -> Result<(SearchResults, usize), FsError> {
    if !is_searchable(symbol) {
        let empty = SearchResults {
            matches: Vec::new(),
            truncated: false,
        };
        return Ok((empty, 0));
    }
    let (paths, mut truncated) = candidate_files(root, symbol)?;
    let registry = Registry::builtin();
    let source = preferred.map(|path| Source {
        path,
        directory: parent(path),
        family: registry.language_for_path(path).map(Language::family),
    });
    let mut files: Vec<(u8, String)> = paths
        .into_iter()
        .filter_map(|path| {
            let language = registry.language_for_path(&path);
            if language.is_some_and(|language| !language.declarations) || looks_minified(&path) {
                return None;
            }
            Some((proximity(&path, language, source.as_ref()), path))
        })
        .collect();
    files.sort_by_key(|(proximity, _)| *proximity);

    // Tier-0 candidates per proximity: once enough sit closer than the next
    // file, nothing in it or after it can make the cut.
    let mut strong = [0usize; 4];
    let mut scored: Vec<(Rank, SearchMatch)> = Vec::new();
    let mut found = Vec::new();
    for (order, (proximity, path)) in files.iter().enumerate() {
        if strong[..usize::from(*proximity)].iter().sum::<usize>() >= limit {
            truncated = true;
            break;
        }
        let Some(text) = read_candidate(root, path, budget, &mut truncated) else {
            if budget.files == 0 || budget.bytes == 0 {
                truncated = true;
                break;
            }
            continue;
        };
        scan_text(path, &text, symbol, &mut found);
        for hit in found.drain(..) {
            let tier = hit.tier();
            if tier == 0 {
                strong[usize::from(*proximity)] += 1;
            }
            scored.push((
                (tier, *proximity, hit.kind, order, hit.line),
                SearchMatch {
                    path: path.clone(),
                    line: hit.line,
                    column: hit.column,
                    text: hit.text,
                    before: Vec::new(),
                    after: Vec::new(),
                },
            ));
        }
    }
    truncated |= scored.len() > limit;
    if scored.len() > limit {
        scored.select_nth_unstable_by(limit, |a, b| a.0.cmp(&b.0));
        scored.truncate(limit);
    }
    scored.sort_unstable_by(|a, b| a.0.cmp(&b.0));
    let declarations = scored.iter().take_while(|(rank, _)| rank.0 == 0).count();
    let results = SearchResults {
        matches: scored.into_iter().map(|(_, hit)| hit).collect(),
        truncated,
    };
    Ok((results, declarations))
}

/// Tier, proximity, kind, file order, line: smaller is better.
type Rank = (u8, u8, Kind, usize, u32);

struct Source<'a> {
    path: &'a str,
    directory: &'a str,
    family: Option<&'a str>,
}

/// 0 the source file, 1 its language family in its directory, 2 its family
/// anywhere, 3 the rest.
fn proximity(path: &str, language: Option<&Language>, source: Option<&Source<'_>>) -> u8 {
    let Some(source) = source else {
        return 0;
    };
    if path == source.path {
        return 0;
    }
    let family = language.map(Language::family);
    match (
        family.is_some() && family == source.family,
        parent(path) == source.directory,
    ) {
        (true, true) => 1,
        (true, false) => 2,
        _ => 3,
    }
}

fn parent(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(directory, _)| directory)
}

fn looks_minified(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.contains(".min.") || name.ends_with(".map")
}

/// Workspace-relative paths of the files that contain `symbol` as a word, and
/// whether that list was cut.
fn candidate_files(root: &Path, symbol: &str) -> Result<(Vec<String>, bool), FsError> {
    let args = [
        "grep",
        "-l",
        "-z",
        "-I",
        "--no-color",
        "--untracked",
        "-w",
        "-F",
        "-e",
        symbol,
        "--",
        ".",
    ];
    let (out, cut) = run_git_prefix(Some(root), &args, MAX_LIST_BYTES)?;
    // 128 is git's "not a repository" (or one it refuses to read): the same
    // bounded walk a plain directory gets.
    if !cut && out.status == 128 {
        return walked_files(root);
    }
    // `git grep` exits 1 when nothing matches — that is success.
    if !cut && out.status != 0 && out.status != 1 {
        return Err(FsError::Git(GitError::CommandFailed {
            args: args.iter().map(|arg| (*arg).to_string()).collect(),
            stderr: out.stderr,
            status: out.status,
        }));
    }
    let mut paths: Vec<String> = out
        .stdout
        .split('\0')
        .filter(|path| !path.is_empty())
        .map(str::to_string)
        .collect();
    // A cut list ends inside a path.
    if cut && !out.stdout.ends_with('\0') {
        paths.pop();
    }
    Ok((paths, cut))
}

fn walked_files(root: &Path) -> Result<(Vec<String>, bool), FsError> {
    let tree = list_via_walk(root)?;
    let paths = tree
        .entries
        .into_iter()
        .filter(|entry| entry.kind == crate::EntryKind::File)
        .map(|entry| entry.path)
        .collect();
    Ok((paths, tree.truncated))
}

struct Budget {
    files: usize,
    bytes: usize,
}

/// The text of one candidate, or `None` for one that is not a readable text
/// file. A file over the remaining budget marks the search partial.
fn read_candidate(
    root: &Path,
    relative: &str,
    budget: &mut Budget,
    truncated: &mut bool,
) -> Option<String> {
    if budget.files == 0 {
        return None;
    }
    let mut path = root.join(relative);
    let mut metadata = fs::symlink_metadata(&path).ok()?;
    if metadata.file_type().is_symlink() {
        path = resolve_inside(root, relative).ok()?;
        metadata = fs::metadata(&path).ok()?;
    }
    if !metadata.is_file() {
        return None;
    }
    let cap = MAX_FILE_BYTES.min(budget.bytes);
    if metadata.len() > cap as u64 {
        *truncated = true;
        return None;
    }
    let file = File::open(&path).ok()?;
    let mut bytes = Vec::with_capacity(usize::try_from(metadata.len()).unwrap_or(cap));
    file.take(cap as u64 + 1).read_to_end(&mut bytes).ok()?;
    budget.files -= 1;
    budget.bytes = budget.bytes.saturating_sub(bytes.len());
    if bytes.len() > cap {
        *truncated = true;
        return None;
    }
    if is_binary(&bytes) {
        return None;
    }
    Some(
        String::from_utf8(bytes)
            .unwrap_or_else(|error| String::from_utf8_lossy(error.as_bytes()).into_owned()),
    )
}
