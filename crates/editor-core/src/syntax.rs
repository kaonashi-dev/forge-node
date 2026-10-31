//! Line-oriented syntax colouring for a terminal editor.
//!
//! A handful of scopes, not a parse tree: a wrong colour is worse than none, so
//! an unknown grammar stays plain and a construct this does not understand
//! falls back to [`Scope::Plain`] rather than guessing.
//!
//! The renderer paints one row at a time, so the document is scanned once into
//! per-line spans and rows are looked up. Scanning per row would be quadratic,
//! and scanning only the visible rows cannot work: a block comment opened
//! above decides the colour of everything below it.

use std::collections::HashSet;

use crate::text::Text;

/// What a span of text is. Mirrors the GUI's scope list so one theme covers
/// both surfaces.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    Comment,
    Keyword,
    ControlKeyword,
    String,
    Number,
    Type,
    Function,
    Property,
    Constant,
    Plain,
}

/// A coloured run inside one line, as byte offsets into that line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub scope: Scope,
}

/// Which grammar a buffer is read with.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Grammar {
    Rust,
    CLike,
    Prisma,
    Sql,
    Hcl,
    Python,
    Json,
    Keyed,
    Shell,
    Markdown,
    Html,
    Css,
    Nix,
    Dockerfile,
    /// `.http` / `.rest` request files: requests, headers and `{{variables}}`.
    Http,
    /// No colouring: an extension this build does not know.
    #[default]
    None,
}

impl Grammar {
    /// The grammar for a path's extension, `None` when it is not one of these.
    ///
    /// Extension only: sniffing content would have to be undone the moment a
    /// person types, and a shebang is a later milestone, not a guess.
    #[must_use]
    pub fn for_path(path: &str) -> Self {
        let name = path.rsplit('/').next().unwrap_or(path);
        let lower = name.to_ascii_lowercase();
        // Dotenv files are leading-dot basenames: the extension rule below would
        // treat `.env` as a name with no extension and `.env.local` as `local`.
        if lower == ".env" || lower.starts_with(".env.") {
            return Self::Keyed;
        }
        // Makefile / GNUmakefile have no extension; recipes are shell-shaped.
        if lower == "makefile" || lower == "gnumakefile" {
            return Self::Shell;
        }
        // `Dockerfile.ci` carries a stage after the dot, not an extension.
        if lower == "dockerfile" || lower.starts_with("dockerfile.") || lower == "containerfile" {
            return Self::Dockerfile;
        }
        let Some(dot) = name.rfind('.').filter(|at| *at > 0) else {
            return Self::None;
        };
        match name[dot + 1..].to_ascii_lowercase().as_str() {
            "rs" => Self::Rust,
            "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" | "go" | "java" | "kt" | "kts" | "c"
            | "h" | "cc" | "cpp" | "hpp" | "cs" | "swift" | "scala" | "php" | "dart" => Self::CLike,
            "prisma" => Self::Prisma,
            "sql" | "psql" | "pgsql" | "mysql" => Self::Sql,
            "tf" | "tfvars" | "hcl" => Self::Hcl,
            "py" | "pyi" => Self::Python,
            "json" => Self::Json,
            // Helm chart templates (`.tpl`) are keyed YAML with `{{ }}` actions.
            "toml" | "yaml" | "yml" | "ini" | "cfg" | "conf" | "env" | "tpl" => Self::Keyed,
            "sh" | "bash" | "zsh" | "fish" | "mk" | "make" => Self::Shell,
            "md" | "markdown" | "mdx" => Self::Markdown,
            "html" | "htm" | "xhtml" | "xml" | "svg" => Self::Html,
            "css" => Self::Css,
            "nix" => Self::Nix,
            "dockerfile" | "containerfile" => Self::Dockerfile,
            "http" | "rest" => Self::Http,
            _ => Self::None,
        }
    }
}

/// The colouring of one document, indexed by line.
#[derive(Debug, Default)]
pub struct Syntax {
    lines: Vec<Vec<Span>>,
    /// Whether the scanner was at top level at each line's first byte.
    ///
    /// The restart points an incremental rescan may use: anywhere else the
    /// colour depends on a construct opened above, and resuming there would
    /// read a block comment's body as code.
    safe: Vec<bool>,
    /// Lines the last scan actually walked. Telemetry, and what a test asserts
    /// against to prove an edit did not re-colour the whole buffer.
    scanned: std::ops::Range<usize>,
}

impl Syntax {
    /// Scan `text`. An unknown grammar produces nothing, which is plain.
    #[must_use]
    pub fn parse(text: &str, grammar: Grammar) -> Self {
        if grammar == Grammar::None {
            return Self::default();
        }
        let mut out = Scanner::new(text, 0);
        out.run(grammar);
        let lines = out.lines;
        let scanned = 0..lines.len();
        Self {
            lines,
            safe: out.safe,
            scanned,
        }
    }

    /// Re-colour only what an edit can have changed.
    ///
    /// `first_line` and `last_line` are the touched lines in the *old* text, as
    /// `Applied` reports them, and `line_delta` how many lines the edit gained
    /// or lost. The scan restarts on the
    /// nearest line above the edit that the previous one passed at top level,
    /// and stops on the first line boundary below it that both scans agree is
    /// top level: a span is a column pair inside its own line, so everything
    /// past that point is reusable exactly as it stands.
    /// Consumes the previous scan to retain untouched span allocations.
    #[must_use]
    pub fn edited(
        mut self,
        text: &Text,
        grammar: Grammar,
        first_line: usize,
        last_line: usize,
        line_delta: isize,
    ) -> Self {
        if grammar == Grammar::None {
            return Self::default();
        }
        // Nothing to resume from: the previous scan is not this document's.
        if self.safe.is_empty() {
            return Self::parse(text.as_str(), grammar);
        }
        let from = (0..=first_line.min(self.safe.len().saturating_sub(1)))
            .rev()
            .find(|line| self.safe[*line])
            .unwrap_or(0);
        let mut out = Scanner::new(&text.as_str()[text.line_start(from)..], from);
        out.resume = Some(Resume {
            after_line: usize::try_from(last_line as isize + line_delta)
                .map_or(first_line, |line| line.max(first_line)),
            old_safe: &self.safe,
            line_delta,
        });
        out.run(grammar);

        let scanned = from..out.stopped.unwrap_or(from + out.lines.len());
        let old_end = if let Some(stop) = out.stopped {
            // The checkpoint row belongs to the reusable tail, not the replacement.
            out.lines.truncate(stop - from);
            out.safe.truncate(stop - from);
            usize::try_from(stop as isize - line_delta).unwrap_or(0)
        } else {
            self.lines.len()
        };
        // A bad span must cost a rescan, not the editor process and its draft.
        if old_end < from || old_end > self.lines.len() || old_end > self.safe.len() {
            return Self::parse(text.as_str(), grammar);
        }
        self.lines.splice(from..old_end, out.lines);
        self.safe.splice(from..old_end, out.safe);
        self.scanned = scanned;
        self
    }

    /// Lines the last scan walked, for a cost assertion.
    #[must_use]
    pub fn scanned_lines(&self) -> std::ops::Range<usize> {
        self.scanned.clone()
    }

    /// The spans on one 0-based line; empty when it has none.
    #[must_use]
    pub fn line(&self, index: usize) -> &[Span] {
        self.lines.get(index).map_or(&[], Vec::as_slice)
    }

    /// The scope covering a byte offset within a line.
    #[must_use]
    pub fn scope_at(&self, line: usize, offset: usize) -> Scope {
        self.line(line)
            .iter()
            .find(|span| offset >= span.start && offset < span.end)
            .map_or(Scope::Plain, |span| span.scope)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.lines.iter().all(Vec::is_empty)
    }
}

/// Where an incremental scan may hand back to the spans it already has.
struct Resume<'a> {
    /// The last line of the edit in the *new* text. A resync at or above it
    /// would reuse spans the edit invalidated.
    after_line: usize,
    /// `safe` from the scan being reused, in *its* line numbering.
    old_safe: &'a [bool],
    /// New line index minus old line index.
    line_delta: isize,
}

/// Walks the document once, emitting spans split at every newline.
struct Scanner<'a> {
    text: &'a str,
    bytes: &'a [u8],
    at: usize,
    /// Start of the line `at` is in, and that line's index within this scan.
    line_start: usize,
    line: usize,
    /// Document line this scan's line 0 is, for a scan that starts part-way in.
    line_base: usize,
    lines: Vec<Vec<Span>>,
    safe: Vec<bool>,
    resume: Option<Resume<'a>>,
    /// Document line the scan stopped on, when it resynced.
    stopped: Option<usize>,
}

impl<'a> Scanner<'a> {
    fn new(text: &'a str, line_base: usize) -> Self {
        Self {
            text,
            bytes: text.as_bytes(),
            at: 0,
            line_start: 0,
            line: 0,
            line_base,
            lines: vec![Vec::new()],
            safe: vec![false],
            resume: None,
            stopped: None,
        }
    }

    fn run(&mut self, grammar: Grammar) {
        match grammar {
            Grammar::Rust => self.c_like(&rust_keywords(), true),
            Grammar::CLike => self.c_like(&c_keywords(), false),
            Grammar::Prisma => self.c_like(&prisma_keywords(), false),
            Grammar::Sql => self.sql(),
            Grammar::Hcl => self.hcl(),
            Grammar::Python => self.python(),
            Grammar::Json => self.json(),
            Grammar::Keyed => self.keyed(),
            Grammar::Shell => self.shell(),
            Grammar::Markdown => self.markdown(),
            Grammar::Html => self.html(),
            Grammar::Css => self.css(),
            Grammar::Nix => self.nix(),
            Grammar::Dockerfile => self.dockerfile(),
            Grammar::Http => self.http(),
            Grammar::None => {}
        }
    }

    /// Record a top-level line boundary, and say whether the scan may stop.
    ///
    /// Called at every grammar loop's head: `at == line_start` there means no
    /// construct is open, which is the only state a later scan can resume from.
    fn checkpoint(&mut self) -> bool {
        if self.at != self.line_start {
            return false;
        }
        if self.line >= self.safe.len() {
            self.safe.resize(self.line + 1, false);
        }
        self.safe[self.line] = true;
        let here = self.line_base + self.line;
        let Some(resume) = &self.resume else {
            return false;
        };
        if here <= resume.after_line {
            return false;
        }
        let Ok(old) = usize::try_from(here as isize - resume.line_delta) else {
            return false;
        };
        if resume.old_safe.get(old) != Some(&true) {
            return false;
        }
        self.stopped = Some(here);
        true
    }

    fn byte(&self, at: usize) -> u8 {
        self.bytes.get(at).copied().unwrap_or(0)
    }

    fn done(&self) -> bool {
        self.at >= self.bytes.len()
    }

    /// Advance one byte, tracking which line we are on.
    ///
    /// Stops at the end rather than running past it: a scan looking for a
    /// closer that never comes (`/*` at the end of a file) bumps more times
    /// than there are bytes, and an `at` past the end slices out of bounds
    /// when the span is emitted.
    fn bump(&mut self) {
        if self.done() {
            return;
        }
        // A whole character, not a byte: every `Mark` and every span bound is
        // a slice index into the document, and one that lands inside a
        // multi-byte character panics. ASCII — which is all the grammars match
        // on — advances by one either way.
        let width = match self.byte(self.at) {
            0x00..=0x7f => 1,
            0xc0..=0xdf => 2,
            0xe0..=0xef => 3,
            0xf0..=0xf7 => 4,
            // A continuation byte here means the scan already lost the
            // boundary; stepping one byte finds it again.
            _ => 1,
        };
        if self.byte(self.at) == b'\n' {
            self.line += 1;
            self.line_start = self.at + 1;
            self.lines.push(Vec::new());
            self.safe.push(false);
        }
        self.at = (self.at + width).min(self.bytes.len());
    }

    /// Where a token starts, captured before it is consumed.
    fn mark(&self) -> Mark {
        Mark {
            at: self.at,
            line: self.line,
            column: self.at - self.line_start,
        }
    }

    /// Emit `[mark, self.at)` as `scope`, split at every newline it crosses.
    ///
    /// Splitting here is what lets the renderer look a row up: a block comment
    /// is one construct to the scanner and one span per row to the painter.
    /// The start position is carried in rather than recomputed — searching the
    /// document for it would make colouring quadratic in its length.
    fn emit(&mut self, mark: Mark, scope: Scope) {
        let to = self.at;
        if mark.at >= to || scope == Scope::Plain {
            return;
        }
        let mut start = mark.at;
        let mut line = mark.line;
        let mut column = mark.column;
        while start < to {
            match self.text[start..to].find('\n') {
                Some(offset) => {
                    let end = start + offset;
                    if end > start {
                        self.push(line, column, column + (end - start), scope);
                    }
                    start = end + 1;
                    line += 1;
                    column = 0;
                }
                None => {
                    self.push(line, column, column + (to - start), scope);
                    break;
                }
            }
        }
    }

    fn push(&mut self, line: usize, start: usize, end: usize, scope: Scope) {
        if line >= self.lines.len() {
            self.lines.resize(line + 1, Vec::new());
        }
        self.lines[line].push(Span { start, end, scope });
    }
}

/// A position remembered before a token is consumed.
#[derive(Clone, Copy)]
struct Mark {
    at: usize,
    line: usize,
    column: usize,
}

/// Keywords that read as control flow, coloured apart from the rest.
fn control() -> HashSet<&'static str> {
    [
        "if", "else", "for", "while", "loop", "do", "switch", "case", "match", "break", "continue",
        "return", "throw", "try", "catch", "finally", "await", "yield", "goto",
    ]
    .into_iter()
    .collect()
}

fn rust_keywords() -> HashSet<&'static str> {
    [
        "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum",
        "extern", "false", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move",
        "mut", "pub", "ref", "return", "self", "Self", "static", "struct", "super", "trait",
        "true", "type", "unsafe", "use", "where", "while", "union",
    ]
    .into_iter()
    .collect()
}

fn c_keywords() -> HashSet<&'static str> {
    [
        "abstract",
        "as",
        "async",
        "await",
        "break",
        "case",
        "catch",
        "class",
        "const",
        "continue",
        "declare",
        "default",
        "delete",
        "do",
        "else",
        "enum",
        "export",
        "extends",
        "false",
        "final",
        "finally",
        "for",
        "from",
        "func",
        "function",
        "go",
        "if",
        "implements",
        "import",
        "in",
        "instanceof",
        "interface",
        "let",
        "new",
        "null",
        "package",
        "private",
        "protected",
        "public",
        "readonly",
        "return",
        "static",
        "struct",
        "super",
        "switch",
        "this",
        "throw",
        "throws",
        "true",
        "try",
        "type",
        "typeof",
        "undefined",
        "var",
        "void",
        "while",
        "with",
        "yield",
    ]
    .into_iter()
    .collect()
}

fn prisma_keywords() -> HashSet<&'static str> {
    [
        "datasource",
        "enum",
        "false",
        "generator",
        "model",
        "null",
        "true",
        "type",
        "view",
    ]
    .into_iter()
    .collect()
}

fn sql_keywords() -> HashSet<&'static str> {
    [
        "add",
        "all",
        "alter",
        "and",
        "as",
        "asc",
        "between",
        "by",
        "cascade",
        "check",
        "column",
        "constraint",
        "create",
        "cross",
        "database",
        "default",
        "delete",
        "desc",
        "distinct",
        "drop",
        "else",
        "end",
        "except",
        "exists",
        "false",
        "foreign",
        "from",
        "full",
        "grant",
        "group",
        "having",
        "if",
        "in",
        "index",
        "inner",
        "insert",
        "intersect",
        "into",
        "is",
        "join",
        "key",
        "left",
        "like",
        "limit",
        "not",
        "null",
        "offset",
        "on",
        "or",
        "order",
        "outer",
        "over",
        "primary",
        "references",
        "returning",
        "revoke",
        "right",
        "schema",
        "select",
        "set",
        "table",
        "then",
        "true",
        "union",
        "unique",
        "update",
        "using",
        "values",
        "view",
        "when",
        "where",
        "with",
    ]
    .into_iter()
    .collect()
}

fn sql_control() -> HashSet<&'static str> {
    ["case", "when", "then", "else", "end", "if"]
        .into_iter()
        .collect()
}

fn hcl_keywords() -> HashSet<&'static str> {
    [
        "check",
        "data",
        "dynamic",
        "ephemeral",
        "false",
        "import",
        "lifecycle",
        "locals",
        "module",
        "moved",
        "null",
        "output",
        "provider",
        "provisioner",
        "resource",
        "terraform",
        "true",
        "variable",
    ]
    .into_iter()
    .collect()
}

fn hcl_control() -> HashSet<&'static str> {
    ["for", "in", "if", "else"].into_iter().collect()
}

fn nix_keywords() -> HashSet<&'static str> {
    ["assert", "in", "inherit", "let", "or", "rec", "with"]
        .into_iter()
        .collect()
}

fn nix_builtins() -> HashSet<&'static str> {
    [
        "abort",
        "baseNameOf",
        "builtins",
        "derivation",
        "dirOf",
        "fetchTarball",
        "import",
        "isNull",
        "map",
        "removeAttrs",
        "throw",
        "toString",
    ]
    .into_iter()
    .collect()
}

fn dockerfile_instructions() -> HashSet<&'static str> {
    [
        "ADD",
        "ARG",
        "CMD",
        "COPY",
        "ENTRYPOINT",
        "ENV",
        "EXPOSE",
        "FROM",
        "HEALTHCHECK",
        "LABEL",
        "MAINTAINER",
        "ONBUILD",
        "RUN",
        "SHELL",
        "STOPSIGNAL",
        "USER",
        "VOLUME",
        "WORKDIR",
    ]
    .into_iter()
    .collect()
}

fn http_methods() -> HashSet<&'static str> {
    [
        "CONNECT", "DELETE", "GET", "GRAPHQL", "HEAD", "OPTIONS", "PATCH", "POST", "PUT", "TRACE",
    ]
    .into_iter()
    .collect()
}

fn is_nix_word(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'\'')
}

/// Where an `.http` file's scan is inside one request.
#[derive(Clone, Copy, PartialEq, Eq)]
enum HttpPart {
    /// Before the request line: comments, `@name = value`, blank lines.
    Preamble,
    Headers,
    Body,
}

fn is_word(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'$'
}

fn is_markup_name_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_' || byte == b':'
}

fn is_markup_name(byte: u8) -> bool {
    is_markup_name_start(byte) || byte.is_ascii_digit() || byte == b'-' || byte == b'.'
}

fn is_css_name_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_' || byte == b'-' || byte >= 0x80
}

fn is_css_name(byte: u8) -> bool {
    is_css_name_start(byte) || byte.is_ascii_digit()
}

/// Which part of a CSS statement the scanner is in.
#[derive(Clone, Copy, PartialEq, Eq)]
enum CssPart {
    Selector,
    Property,
    Value,
    AtRule,
}

fn raw_text_tag(name: &str) -> Option<&'static str> {
    ["script", "style", "textarea", "title"]
        .into_iter()
        .find(|tag| name.eq_ignore_ascii_case(tag))
}

impl Scanner<'_> {
    /// Rust, and the C family: line and block comments, quoted strings,
    /// numbers, keywords, `Type` by leading capital, `name(` as a call.
    fn c_like(&mut self, keywords: &HashSet<&'static str>, rust: bool) {
        let control = control();
        while !self.done() {
            if self.checkpoint() {
                break;
            }
            let byte = self.byte(self.at);
            let mark = self.mark();
            match byte {
                b'/' if self.at_pair(b'/', b'/') => {
                    self.take_line();
                    self.emit(mark, Scope::Comment);
                }
                b'/' if self.at_pair(b'/', b'*') => {
                    self.bump();
                    self.bump();
                    while !self.done() && !self.at_pair(b'*', b'/') {
                        self.bump();
                    }
                    self.bump();
                    self.bump();
                    self.emit(mark, Scope::Comment);
                }
                b'#' if rust && self.byte(self.at + 1) == b'[' => {
                    // An attribute is meta, and it is not a comment: it runs to
                    // its closing bracket, not to the end of the line.
                    self.take_bracketed(b'[', b']');
                    self.emit(mark, Scope::Comment);
                }
                b'"' | b'\'' | b'`' => {
                    // A Rust lifetime is not a string: `'a` has no closer.
                    if rust && byte == b'\'' && !self.is_char_literal() {
                        self.bump();
                        continue;
                    }
                    self.take_quoted(byte);
                    self.emit(mark, Scope::String);
                }
                b'0'..=b'9' => {
                    while !self.done()
                        && (self.byte(self.at).is_ascii_alphanumeric()
                            || self.byte(self.at) == b'.'
                            || self.byte(self.at) == b'_')
                    {
                        self.bump();
                    }
                    self.emit(mark, Scope::Number);
                }
                b if is_word(b) && !b.is_ascii_digit() => {
                    while !self.done() && is_word(self.byte(self.at)) {
                        self.bump();
                    }
                    let word = &self.text[mark.at..self.at];
                    let scope = if control.contains(word) {
                        Scope::ControlKeyword
                    } else if keywords.contains(word) {
                        Scope::Keyword
                    } else if word.starts_with(|c: char| c.is_ascii_uppercase()) {
                        Scope::Type
                    } else if self.peek_nonspace() == b'(' {
                        Scope::Function
                    } else {
                        Scope::Plain
                    };
                    self.emit(mark, scope);
                }
                _ => self.bump(),
            }
        }
    }

    /// SQL: `--` line comments, `/* */` blocks, quotes, case-insensitive keywords.
    fn sql(&mut self) {
        let keywords = sql_keywords();
        let control = sql_control();
        while !self.done() {
            if self.checkpoint() {
                break;
            }
            let byte = self.byte(self.at);
            let mark = self.mark();
            match byte {
                b'-' if self.byte(self.at + 1) == b'-' => {
                    self.take_line();
                    self.emit(mark, Scope::Comment);
                }
                b'/' if self.at_pair(b'/', b'*') => {
                    self.bump();
                    self.bump();
                    while !self.done() && !self.at_pair(b'*', b'/') {
                        self.bump();
                    }
                    self.bump();
                    self.bump();
                    self.emit(mark, Scope::Comment);
                }
                b'\'' | b'"' => {
                    self.take_quoted(byte);
                    self.emit(mark, Scope::String);
                }
                b'0'..=b'9' => {
                    while !self.done()
                        && (self.byte(self.at).is_ascii_alphanumeric()
                            || self.byte(self.at) == b'.'
                            || self.byte(self.at) == b'_')
                    {
                        self.bump();
                    }
                    self.emit(mark, Scope::Number);
                }
                b if is_word(b) && !b.is_ascii_digit() => {
                    while !self.done() && is_word(self.byte(self.at)) {
                        self.bump();
                    }
                    let word = self.text[mark.at..self.at].to_ascii_lowercase();
                    let scope = if control.contains(word.as_str()) {
                        Scope::ControlKeyword
                    } else if keywords.contains(word.as_str()) {
                        Scope::Keyword
                    } else if self.peek_nonspace() == b'(' {
                        Scope::Function
                    } else {
                        Scope::Plain
                    };
                    self.emit(mark, scope);
                }
                _ => self.bump(),
            }
        }
    }

    /// HCL / Terraform: `#`/`//`/`/* */` comments, quotes, block keywords, `name(`.
    fn hcl(&mut self) {
        let keywords = hcl_keywords();
        let control = hcl_control();
        while !self.done() {
            if self.checkpoint() {
                break;
            }
            let byte = self.byte(self.at);
            let mark = self.mark();
            match byte {
                b'#' => {
                    self.take_line();
                    self.emit(mark, Scope::Comment);
                }
                b'/' if self.at_pair(b'/', b'/') => {
                    self.take_line();
                    self.emit(mark, Scope::Comment);
                }
                b'/' if self.at_pair(b'/', b'*') => {
                    self.bump();
                    self.bump();
                    while !self.done() && !self.at_pair(b'*', b'/') {
                        self.bump();
                    }
                    self.bump();
                    self.bump();
                    self.emit(mark, Scope::Comment);
                }
                b'"' => {
                    self.take_quoted(b'"');
                    self.emit(mark, Scope::String);
                }
                b'0'..=b'9' => {
                    while !self.done()
                        && (self.byte(self.at).is_ascii_alphanumeric()
                            || self.byte(self.at) == b'.'
                            || self.byte(self.at) == b'_')
                    {
                        self.bump();
                    }
                    self.emit(mark, Scope::Number);
                }
                b if is_word(b) && !b.is_ascii_digit() => {
                    while !self.done() && is_word(self.byte(self.at)) {
                        self.bump();
                    }
                    let word = &self.text[mark.at..self.at];
                    let scope = if control.contains(word) {
                        Scope::ControlKeyword
                    } else if keywords.contains(word) {
                        Scope::Keyword
                    } else if self.peek_nonspace() == b'=' {
                        Scope::Property
                    } else if self.peek_nonspace() == b'(' {
                        Scope::Function
                    } else {
                        Scope::Plain
                    };
                    self.emit(mark, scope);
                }
                _ => self.bump(),
            }
        }
    }

    /// `'a` is a lifetime unless the quote closes within a few bytes.
    fn is_char_literal(&self) -> bool {
        let mut at = self.at + 1;
        if self.byte(at) == b'\\' {
            at += 1;
        }
        // One char plus a closer; anything longer is a lifetime or a label.
        for step in 0..5 {
            if self.byte(at + step) == b'\'' {
                return true;
            }
            if self.byte(at + step) == 0 || self.byte(at + step) == b'\n' {
                return false;
            }
        }
        false
    }

    fn python(&mut self) {
        let keywords: HashSet<&str> = [
            "and", "as", "assert", "async", "await", "class", "def", "del", "elif", "else",
            "except", "False", "finally", "for", "from", "global", "if", "import", "in", "is",
            "lambda", "None", "nonlocal", "not", "or", "pass", "raise", "return", "True", "try",
            "while", "with", "yield",
        ]
        .into_iter()
        .collect();
        let control = control();
        while !self.done() {
            if self.checkpoint() {
                break;
            }
            let byte = self.byte(self.at);
            let mark = self.mark();
            match byte {
                b'#' => {
                    self.take_line();
                    self.emit(mark, Scope::Comment);
                }
                b'"' | b'\'' => {
                    if self.at_triple(byte) {
                        self.bump();
                        self.bump();
                        self.bump();
                        while !self.done() && !self.at_triple(byte) {
                            self.bump();
                        }
                        self.bump();
                        self.bump();
                        self.bump();
                    } else {
                        self.take_quoted(byte);
                    }
                    self.emit(mark, Scope::String);
                }
                b'0'..=b'9' => {
                    while !self.done()
                        && (self.byte(self.at).is_ascii_alphanumeric()
                            || self.byte(self.at) == b'.')
                    {
                        self.bump();
                    }
                    self.emit(mark, Scope::Number);
                }
                b if is_word(b) && !b.is_ascii_digit() => {
                    while !self.done() && is_word(self.byte(self.at)) {
                        self.bump();
                    }
                    let word = &self.text[mark.at..self.at];
                    let scope = if control.contains(word) {
                        Scope::ControlKeyword
                    } else if keywords.contains(word) {
                        Scope::Keyword
                    } else if word.starts_with(|c: char| c.is_ascii_uppercase()) {
                        Scope::Type
                    } else if self.peek_nonspace() == b'(' {
                        Scope::Function
                    } else {
                        Scope::Plain
                    };
                    self.emit(mark, scope);
                }
                _ => self.bump(),
            }
        }
    }

    fn json(&mut self) {
        while !self.done() {
            if self.checkpoint() {
                break;
            }
            let byte = self.byte(self.at);
            let mark = self.mark();
            match byte {
                b'"' => {
                    self.take_quoted(b'"');
                    // A string followed by a colon is a key, not a value.
                    let scope = if self.peek_nonspace() == b':' {
                        Scope::Property
                    } else {
                        Scope::String
                    };
                    self.emit(mark, scope);
                }
                b'0'..=b'9' | b'-' => {
                    while !self.done()
                        && (self.byte(self.at).is_ascii_digit()
                            || matches!(self.byte(self.at), b'.' | b'e' | b'E' | b'+' | b'-'))
                    {
                        self.bump();
                    }
                    self.emit(mark, Scope::Number);
                }
                b't' | b'f' | b'n' => {
                    while !self.done() && self.byte(self.at).is_ascii_alphabetic() {
                        self.bump();
                    }
                    self.emit(mark, Scope::Constant);
                }
                _ => self.bump(),
            }
        }
    }

    /// TOML, YAML, ini, dotenv, Helm templates: a key before a separator, then a value.
    fn keyed(&mut self) {
        while !self.done() {
            if self.checkpoint() {
                break;
            }
            let mark = self.mark();
            match self.byte(self.at) {
                b'#' | b';' => {
                    self.take_line();
                    self.emit(mark, Scope::Comment);
                }
                // Helm / Go template actions inside chart YAML and `.tpl` files.
                b'{' if self.byte(self.at + 1) == b'{' => {
                    self.bump();
                    self.bump();
                    while !self.done() && !self.at_pair(b'}', b'}') {
                        self.bump();
                    }
                    self.bump();
                    self.bump();
                    self.emit(mark, Scope::Keyword);
                }
                b'"' | b'\'' => {
                    let quote = self.byte(self.at);
                    self.take_quoted(quote);
                    self.emit(mark, Scope::String);
                }
                b'[' => {
                    self.take_bracketed(b'[', b']');
                    self.emit(mark, Scope::Type);
                }
                b if is_word(b) || b == b'-' => {
                    let at_line_start = self.only_space_before();
                    while !self.done()
                        && (is_word(self.byte(self.at))
                            || matches!(self.byte(self.at), b'-' | b'.'))
                    {
                        self.bump();
                    }
                    let word = &self.text[mark.at..self.at];
                    let scope = if matches!(self.peek_nonspace(), b'=' | b':')
                        && self.keyed_property_site(mark.at)
                    {
                        Scope::Property
                    } else if at_line_start && word == "export" {
                        // dotenv: `export KEY=value`
                        Scope::Keyword
                    } else if word.chars().all(|c| c.is_ascii_digit() || c == '.') {
                        Scope::Number
                    } else if matches!(word, "true" | "false" | "null") {
                        Scope::Constant
                    } else {
                        Scope::Plain
                    };
                    self.emit(mark, scope);
                }
                _ => self.bump(),
            }
        }
    }

    fn shell(&mut self) {
        let keywords: HashSet<&str> = [
            "if", "then", "elif", "else", "fi", "for", "in", "do", "done", "while", "until",
            "case", "esac", "function", "return", "exit", "local", "export", "set", "echo",
            "source", "read",
        ]
        .into_iter()
        .collect();
        let control = control();
        while !self.done() {
            if self.checkpoint() {
                break;
            }
            let mark = self.mark();
            match self.byte(self.at) {
                b'#' => {
                    self.take_line();
                    self.emit(mark, Scope::Comment);
                }
                b'"' | b'\'' => {
                    let quote = self.byte(self.at);
                    self.take_quoted(quote);
                    self.emit(mark, Scope::String);
                }
                b'$' => {
                    self.bump();
                    if self.byte(self.at) == b'{' {
                        self.take_bracketed(b'{', b'}');
                    } else {
                        while !self.done() && is_word(self.byte(self.at)) {
                            self.bump();
                        }
                    }
                    self.emit(mark, Scope::Property);
                }
                b if is_word(b) && !b.is_ascii_digit() => {
                    while !self.done() && is_word(self.byte(self.at)) {
                        self.bump();
                    }
                    let word = &self.text[mark.at..self.at];
                    let scope = if control.contains(word) {
                        Scope::ControlKeyword
                    } else if keywords.contains(word) {
                        Scope::Keyword
                    } else {
                        Scope::Plain
                    };
                    self.emit(mark, scope);
                }
                _ => self.bump(),
            }
        }
    }

    /// Nix: `#` and `/* */` comments, `"…"` and `''…''` strings that may span
    /// lines, paths, `name =` bindings.
    fn nix(&mut self) {
        let keywords = nix_keywords();
        let builtins = nix_builtins();
        while !self.done() {
            if self.checkpoint() {
                break;
            }
            let byte = self.byte(self.at);
            let mark = self.mark();
            match byte {
                b'#' => {
                    self.take_line();
                    self.emit(mark, Scope::Comment);
                }
                b'/' if self.at_pair(b'/', b'*') => {
                    self.bump();
                    self.take_until(b"*/");
                    self.emit(mark, Scope::Comment);
                }
                b'"' => {
                    self.take_nix_string(false);
                    self.emit(mark, Scope::String);
                }
                b'\'' if self.byte(self.at + 1) == b'\'' => {
                    self.take_nix_string(true);
                    self.emit(mark, Scope::String);
                }
                b'.' | b'~' if self.nix_path_starts() => {
                    self.take_nix_path();
                    self.emit(mark, Scope::String);
                }
                b'<' if self.nix_search_path_len() > 0 => {
                    for _ in 0..self.nix_search_path_len() {
                        self.bump();
                    }
                    self.emit(mark, Scope::String);
                }
                b'0'..=b'9' => {
                    while !self.done()
                        && (self.byte(self.at).is_ascii_digit() || self.byte(self.at) == b'.')
                    {
                        self.bump();
                    }
                    self.emit(mark, Scope::Number);
                }
                b if b.is_ascii_alphabetic() || b == b'_' => {
                    // An attribute path (`a.b.c = 1`) is one binding.
                    while !self.done()
                        && !self.at_pair(b'\'', b'\'')
                        && (is_nix_word(self.byte(self.at))
                            || (self.byte(self.at) == b'.' && is_nix_word(self.byte(self.at + 1))))
                    {
                        self.bump();
                    }
                    let word = &self.text[mark.at..self.at];
                    let scope = if matches!(word, "if" | "then" | "else") {
                        Scope::ControlKeyword
                    } else if keywords.contains(word) {
                        Scope::Keyword
                    } else if matches!(word, "true" | "false" | "null") {
                        Scope::Constant
                    } else if builtins.contains(word) {
                        Scope::Function
                    } else if self.peek_nonspace() == b'=' && !self.before_double_equals() {
                        Scope::Property
                    } else {
                        Scope::Plain
                    };
                    self.emit(mark, scope);
                }
                _ => self.bump(),
            }
        }
    }

    /// A `"…"` or `''…''` string, `${…}` included. No checkpoint: a string
    /// may span lines, and a later scan must not resume inside it.
    fn take_nix_string(&mut self, indented: bool) {
        self.bump();
        if indented {
            self.bump();
        }
        let mut depth = 0usize;
        while !self.done() {
            let byte = self.byte(self.at);
            if depth > 0 {
                match byte {
                    b'{' => depth += 1,
                    b'}' => depth -= 1,
                    _ => {}
                }
                self.bump();
                continue;
            }
            if self.at_pair(b'$', b'{') {
                depth = 1;
                self.bump();
                self.bump();
                continue;
            }
            if indented {
                if self.at_pair(b'\'', b'\'') {
                    // `''$`, `'''` and `''\` escape rather than close.
                    if matches!(self.byte(self.at + 2), b'$' | b'\'' | b'\\') {
                        self.bump();
                        self.bump();
                        self.bump();
                        continue;
                    }
                    self.bump();
                    self.bump();
                    return;
                }
            } else if byte == b'\\' {
                self.bump();
                self.bump();
                continue;
            } else if byte == b'"' {
                self.bump();
                return;
            }
            self.bump();
        }
    }

    /// `./x`, `../x` or `~/x`; a lone `.` is attribute selection.
    fn nix_path_starts(&self) -> bool {
        self.at_bytes(b"./") || self.at_bytes(b"../") || self.at_bytes(b"~/")
    }

    fn take_nix_path(&mut self) {
        while !self.done()
            && !self.byte(self.at).is_ascii_whitespace()
            && !matches!(self.byte(self.at), b';' | b')' | b']' | b'}' | b',')
        {
            self.bump();
        }
    }

    /// Bytes in a `<nixpkgs/lib>` search path here, 0 when `<` is a comparison.
    fn nix_search_path_len(&self) -> usize {
        if !self.byte(self.at + 1).is_ascii_alphabetic() {
            return 0;
        }
        let mut at = self.at + 1;
        while self.byte(at).is_ascii_alphanumeric()
            || matches!(self.byte(at), b'_' | b'-' | b'.' | b'/')
        {
            at += 1;
        }
        if self.byte(at) == b'>' {
            at + 1 - self.at
        } else {
            0
        }
    }

    /// Whether the `=` after a word is the first half of `==`.
    fn before_double_equals(&self) -> bool {
        let mut at = self.at;
        while matches!(self.byte(at), b' ' | b'\t') {
            at += 1;
        }
        self.byte(at + 1) == b'='
    }

    /// Dockerfile: line-leading instructions, `#` comment lines, variables,
    /// instruction flags, `KEY=value`, and heredoc bodies left plain.
    ///
    /// A line continued with `\` is never a checkpoint: its first word is an
    /// argument, and a scan resumed there would read it as an instruction.
    fn dockerfile(&mut self) {
        let instructions = dockerfile_instructions();
        let mut continued = false;
        let mut instruction = String::new();
        let mut flags = false;
        let mut heredocs: Vec<(String, bool)> = Vec::new();
        while !self.done() {
            if !continued && self.checkpoint() {
                break;
            }
            let byte = self.byte(self.at);
            let mark = self.mark();
            match byte {
                b'\n' => {
                    let line = &self.text[self.line_start..self.at];
                    // Docker skips comment lines inside a continuation.
                    if !line.trim_start().starts_with('#') {
                        continued = line.trim_end().ends_with('\\');
                    }
                    if !continued {
                        instruction.clear();
                    }
                    self.bump();
                    for (terminator, strip_tabs) in std::mem::take(&mut heredocs) {
                        self.skip_heredoc(&terminator, strip_tabs);
                    }
                }
                b'#' if self.only_space_before() => {
                    self.take_line();
                    self.emit(mark, Scope::Comment);
                }
                b'"' | b'\'' => {
                    self.take_quoted_in_line(byte);
                    self.emit(mark, Scope::String);
                    flags = false;
                }
                b'$' => {
                    self.take_variable();
                    self.emit(mark, Scope::Property);
                    flags = false;
                }
                b'-' if flags && self.at_pair(b'-', b'-') => {
                    while !self.done()
                        && (is_word(self.byte(self.at)) || self.byte(self.at) == b'-')
                    {
                        self.bump();
                    }
                    self.emit(mark, Scope::Property);
                }
                b'<' if self.at_pair(b'<', b'<') && !instruction.is_empty() => {
                    self.bump();
                    self.bump();
                    if let Some(heredoc) = self.take_heredoc_marker() {
                        heredocs.push(heredoc);
                    }
                    self.emit(mark, Scope::Keyword);
                    flags = false;
                }
                b if b.is_ascii_alphabetic() => {
                    let line_head = !continued && self.only_space_before();
                    while !self.done()
                        && (self.byte(self.at).is_ascii_alphanumeric()
                            || self.byte(self.at) == b'_')
                    {
                        self.bump();
                    }
                    let word = self.text[mark.at..self.at].to_ascii_uppercase();
                    // `ONBUILD RUN …` and `HEALTHCHECK CMD …` name a second instruction.
                    let nested = matches!(instruction.as_str(), "ONBUILD" | "HEALTHCHECK");
                    let scope = if (line_head || nested) && instructions.contains(word.as_str()) {
                        instruction = word;
                        flags = true;
                        Scope::Keyword
                    } else if (instruction == "FROM" && word == "AS")
                        || (instruction == "HEALTHCHECK" && word == "NONE")
                    {
                        Scope::Keyword
                    } else if matches!(instruction.as_str(), "ENV" | "ARG" | "LABEL")
                        && self.byte(self.at) == b'='
                    {
                        flags = false;
                        Scope::Property
                    } else {
                        flags = false;
                        Scope::Plain
                    };
                    self.emit(mark, scope);
                }
                b'0'..=b'9' if matches!(self.byte(self.at.wrapping_sub(1)), b' ' | b'\t') => {
                    while !self.done()
                        && (self.byte(self.at).is_ascii_digit() || self.byte(self.at) == b'.')
                    {
                        self.bump();
                    }
                    if matches!(self.byte(self.at), b' ' | b'\t' | b'\n' | b'/' | 0) {
                        self.emit(mark, Scope::Number);
                    }
                    flags = false;
                }
                b' ' | b'\t' | b'\\' | b'\r' => self.bump(),
                _ => {
                    flags = false;
                    self.bump();
                }
            }
        }
    }

    /// The word after `<<` or `<<-`, unquoted, and whether tabs are stripped.
    fn take_heredoc_marker(&mut self) -> Option<(String, bool)> {
        let strip_tabs = self.byte(self.at) == b'-';
        if strip_tabs {
            self.bump();
        }
        let quote = self.byte(self.at);
        let quoted = matches!(quote, b'"' | b'\'');
        if quoted {
            self.bump();
        }
        let start = self.at;
        while !self.done() && is_word(self.byte(self.at)) {
            self.bump();
        }
        let word = self.text[start..self.at].to_owned();
        if quoted && self.byte(self.at) == quote {
            self.bump();
        }
        (!word.is_empty()).then_some((word, strip_tabs))
    }

    /// Consume a heredoc body through its terminator, leaving that line's
    /// newline to the caller. No checkpoint: the body is not Dockerfile.
    fn skip_heredoc(&mut self, terminator: &str, strip_tabs: bool) {
        while !self.done() {
            let end = self.text[self.at..]
                .find('\n')
                .map_or(self.bytes.len(), |offset| self.at + offset);
            let line = self.text[self.at..end].trim_end_matches('\r');
            let line = if strip_tabs {
                line.trim_start_matches('\t')
            } else {
                line
            };
            let last = line == terminator;
            self.take_line();
            if last {
                return;
            }
            self.bump();
        }
    }

    /// `$NAME` or `${NAME…}`, never past the line.
    fn take_variable(&mut self) {
        self.bump();
        if self.byte(self.at) == b'{' {
            while !self.done() && !matches!(self.byte(self.at), b'}' | b'\n') {
                self.bump();
            }
            if self.byte(self.at) == b'}' {
                self.bump();
            }
        } else {
            while !self.done()
                && (self.byte(self.at).is_ascii_alphanumeric() || self.byte(self.at) == b'_')
            {
                self.bump();
            }
        }
    }

    /// A quoted run that never crosses a newline, even after a backslash, so
    /// it cannot reach the next line's start and checkpoint there.
    fn take_quoted_in_line(&mut self, quote: u8) {
        self.bump();
        while !self.done() && self.byte(self.at) != b'\n' {
            let byte = self.byte(self.at);
            if byte == b'\\' && !matches!(self.byte(self.at + 1), b'\n' | 0) {
                self.bump();
                self.bump();
                continue;
            }
            self.bump();
            if byte == quote {
                return;
            }
        }
    }

    /// HTTP request files: `###` separators, comments, `@name = value`, the
    /// request line, headers, then a body coloured only for strings and
    /// `{{variables}}`.
    ///
    /// Only a line before a request line checkpoints: a header or body line's
    /// meaning depends on the request above it.
    fn http(&mut self) {
        let methods = http_methods();
        let mut part = HttpPart::Preamble;
        while !self.done() {
            if part == HttpPart::Preamble && self.checkpoint() {
                break;
            }
            while matches!(self.byte(self.at), b' ' | b'\t') {
                self.bump();
            }
            let mark = self.mark();
            let blank = matches!(self.byte(self.at), b'\n' | b'\r' | 0);
            if self.at_bytes(b"###") {
                self.take_line();
                self.emit(mark, Scope::Comment);
                part = HttpPart::Preamble;
            } else if part != HttpPart::Body
                && (self.byte(self.at) == b'#' || self.at_pair(b'/', b'/'))
            {
                self.take_line();
                self.emit(mark, Scope::Comment);
            } else {
                match part {
                    HttpPart::Preamble if blank => {}
                    HttpPart::Preamble if self.byte(self.at) == b'@' => {
                        self.bump();
                        while !self.done()
                            && (is_word(self.byte(self.at)) || self.byte(self.at) == b'-')
                        {
                            self.bump();
                        }
                        self.emit(mark, Scope::Property);
                        self.http_text(Scope::String, false);
                    }
                    HttpPart::Preamble => {
                        self.http_request_line(&methods);
                        part = HttpPart::Headers;
                    }
                    HttpPart::Headers if blank => part = HttpPart::Body,
                    HttpPart::Headers => {
                        while !self.done()
                            && (self.byte(self.at).is_ascii_alphanumeric()
                                || matches!(self.byte(self.at), b'-' | b'_'))
                        {
                            self.bump();
                        }
                        if self.at > mark.at && self.byte(self.at) == b':' {
                            self.emit(mark, Scope::Property);
                        }
                        self.http_text(Scope::Plain, false);
                    }
                    HttpPart::Body => self.http_text(Scope::Plain, true),
                }
            }
            self.take_line();
            self.bump();
        }
    }

    /// `METHOD url HTTP/version`; the method may be left out.
    fn http_request_line(&mut self, methods: &HashSet<&'static str>) {
        let mark = self.mark();
        let mut end = self.at;
        while self.byte(end).is_ascii_uppercase() {
            end += 1;
        }
        if methods.contains(&self.text[mark.at..end]) && matches!(self.byte(end), b' ' | b'\t') {
            while self.at < end {
                self.bump();
            }
            self.emit(mark, Scope::Keyword);
        }
        while matches!(self.byte(self.at), b' ' | b'\t') {
            self.bump();
        }
        self.http_word(Scope::String);
        while matches!(self.byte(self.at), b' ' | b'\t') {
            self.bump();
        }
        if self.at_bytes(b"HTTP/") {
            self.http_word(Scope::Constant);
        }
    }

    /// One whitespace-delimited token as `scope`, `{{variables}}` apart.
    fn http_word(&mut self, scope: Scope) {
        let mut mark = self.mark();
        while !self.done() && !self.byte(self.at).is_ascii_whitespace() {
            if self.at_pair(b'{', b'{') {
                self.emit(mark, scope);
                self.take_http_variable();
                mark = self.mark();
            } else {
                self.bump();
            }
        }
        self.emit(mark, scope);
    }

    /// The rest of the line as `scope`, with `{{variables}}` and, when
    /// `strings` is set, double-quoted runs coloured apart.
    fn http_text(&mut self, scope: Scope, strings: bool) {
        let mut mark = self.mark();
        while !self.done() && self.byte(self.at) != b'\n' {
            if self.at_pair(b'{', b'{') {
                self.emit(mark, scope);
                self.take_http_variable();
                mark = self.mark();
            } else if strings && self.byte(self.at) == b'"' {
                self.emit(mark, scope);
                let quoted = self.mark();
                self.take_quoted_in_line(b'"');
                self.emit(quoted, Scope::String);
                mark = self.mark();
            } else {
                self.bump();
            }
        }
        self.emit(mark, scope);
    }

    /// `{{name}}`, stopping at the line's end when it is not closed.
    fn take_http_variable(&mut self) {
        let mark = self.mark();
        self.bump();
        self.bump();
        while !self.done() && self.byte(self.at) != b'\n' && !self.at_pair(b'}', b'}') {
            self.bump();
        }
        if self.at_pair(b'}', b'}') {
            self.bump();
            self.bump();
        }
        self.emit(mark, Scope::Keyword);
    }

    /// Markdown: the marks that carry structure, not a full renderer.
    fn markdown(&mut self) {
        while !self.done() {
            if self.checkpoint() {
                break;
            }
            let mark = self.mark();
            let at_line_start = self.only_space_before();
            match self.byte(self.at) {
                b'#' if at_line_start => {
                    self.take_line();
                    self.emit(mark, Scope::Keyword);
                }
                b'>' if at_line_start => {
                    self.take_line();
                    self.emit(mark, Scope::Comment);
                }
                b'`' if self.at_triple(b'`') => {
                    self.bump();
                    self.bump();
                    self.bump();
                    while !self.done() && !self.at_triple(b'`') {
                        self.bump();
                    }
                    self.bump();
                    self.bump();
                    self.bump();
                    self.emit(mark, Scope::String);
                }
                b'`' => {
                    self.take_quoted(b'`');
                    self.emit(mark, Scope::String);
                }
                b'[' => {
                    self.take_bracketed(b'[', b']');
                    self.emit(mark, Scope::Function);
                }
                b'-' | b'*' | b'+' if at_line_start && self.byte(self.at + 1) == b' ' => {
                    self.bump();
                    self.emit(mark, Scope::ControlKeyword);
                }
                _ => self.bump(),
            }
        }
    }

    /// HTML, XML, SVG: tags, attributes, comments. Not a tree, and not a
    /// browser: `<` inside script/style/textarea/title is text, so those
    /// regions stay plain until their closer rather than being painted as
    /// nested tags.
    fn html(&mut self) {
        while !self.done() {
            if self.checkpoint() {
                break;
            }
            match self.byte(self.at) {
                b'<' => self.take_markup(),
                b'&' => self.take_entity(),
                _ => self.bump(),
            }
        }
    }

    fn take_markup(&mut self) {
        if self.at_bytes(b"<!--") {
            let mark = self.mark();
            self.take_until(b"-->");
            self.emit(mark, Scope::Comment);
            return;
        }
        if self.at_bytes(b"<![CDATA[") {
            let mark = self.mark();
            self.take_until(b"]]>");
            self.emit(mark, Scope::String);
            return;
        }
        if self.at_pair(b'<', b'!') {
            let mark = self.mark();
            self.take_until(b">");
            self.emit(mark, Scope::Keyword);
            return;
        }
        if self.at_pair(b'<', b'?') {
            let mark = self.mark();
            self.take_until(b"?>");
            self.emit(mark, Scope::Keyword);
            return;
        }
        self.take_tag();
    }

    fn take_tag(&mut self) {
        let mark = self.mark();
        self.bump();
        let closing = self.byte(self.at) == b'/';
        if closing {
            self.bump();
        }
        if !is_markup_name_start(self.byte(self.at)) {
            return;
        }
        while is_markup_name(self.byte(self.at)) {
            self.bump();
        }
        let name_from = mark.at + 1 + usize::from(closing);
        let raw = raw_text_tag(&self.text[name_from..self.at]);
        self.emit(mark, Scope::Type);

        loop {
            self.bump_markup_space();
            if self.done() {
                return;
            }
            let byte = self.byte(self.at);
            if byte == b'>' {
                let mark = self.mark();
                self.bump();
                self.emit(mark, Scope::Type);
                if !closing {
                    if let Some(name) = raw {
                        self.skip_raw_text(name);
                    }
                }
                return;
            }
            if byte == b'/' && self.byte(self.at + 1) == b'>' {
                let mark = self.mark();
                self.bump();
                self.bump();
                self.emit(mark, Scope::Type);
                return;
            }
            if is_markup_name_start(byte) {
                let mark = self.mark();
                while is_markup_name(self.byte(self.at)) {
                    self.bump();
                }
                self.emit(mark, Scope::Property);
                self.bump_markup_space();
                if self.byte(self.at) != b'=' {
                    continue;
                }
                self.bump();
                self.bump_markup_space();
                let quote = self.byte(self.at);
                if quote == b'"' || quote == b'\'' {
                    let mark = self.mark();
                    self.take_markup_quoted(quote);
                    self.emit(mark, Scope::String);
                } else {
                    let mark = self.mark();
                    while !self.done() {
                        let value = self.byte(self.at);
                        if matches!(value, b' ' | b'\t' | b'\n' | b'\r' | b'>')
                            || (value == b'/' && self.byte(self.at + 1) == b'>')
                        {
                            break;
                        }
                        self.bump();
                    }
                    self.emit(mark, Scope::String);
                }
                continue;
            }
            self.bump();
        }
    }

    /// HTML attributes do not backslash-escape: `\"` is a closer, and treating
    /// it as an escape would paint the rest of the tag as a string.
    fn take_markup_quoted(&mut self, quote: u8) {
        self.bump();
        while !self.done() {
            let byte = self.byte(self.at);
            if byte == b'\n' {
                return;
            }
            self.bump();
            if byte == quote {
                return;
            }
        }
    }

    fn take_entity(&mut self) {
        let mark = self.mark();
        self.bump();
        if self.byte(self.at) == b'#' {
            self.bump();
            if matches!(self.byte(self.at), b'x' | b'X') {
                self.bump();
            }
            let digits = self.at;
            while self.byte(self.at).is_ascii_hexdigit() {
                self.bump();
            }
            if self.at == digits {
                return;
            }
        } else {
            let name = self.at;
            while is_markup_name(self.byte(self.at)) {
                self.bump();
            }
            if self.at == name {
                return;
            }
        }
        if self.byte(self.at) != b';' {
            return;
        }
        self.bump();
        self.emit(mark, Scope::Constant);
    }

    fn skip_raw_text(&mut self, name: &str) {
        let needle = name.as_bytes();
        while !self.done() {
            if self.at_close_tag(needle) {
                return;
            }
            self.bump();
        }
    }

    fn at_close_tag(&self, name: &[u8]) -> bool {
        if self.byte(self.at) != b'<' || self.byte(self.at + 1) != b'/' {
            return false;
        }
        let start = self.at + 2;
        let Some(candidate) = self.bytes.get(start..start + name.len()) else {
            return false;
        };
        if !candidate.eq_ignore_ascii_case(name) {
            return false;
        }
        let mut at = start + name.len();
        while matches!(self.byte(at), b' ' | b'\t' | b'\n' | b'\r') {
            at += 1;
        }
        self.byte(at) == b'>'
    }

    fn bump_markup_space(&mut self) {
        while matches!(self.byte(self.at), b' ' | b'\t' | b'\n' | b'\r') {
            self.bump();
        }
    }

    /// CSS: comments, at-rules, selectors and `property: value` declarations.
    ///
    /// A line inside a rule is coloured by the `{` above it, so a checkpoint is
    /// taken only between statements at depth zero. Inside a block, a statement
    /// that reaches `{` before `;` or `}` is a nested rule, not a declaration.
    fn css(&mut self) {
        let mut depth = 0_usize;
        let mut part = CssPart::Selector;
        let mut fresh = true;
        let mut in_attr = false;
        while !self.done() {
            if depth == 0 && fresh && self.checkpoint() {
                break;
            }
            let byte = self.byte(self.at);
            let mark = self.mark();
            if self.at_pair(b'/', b'*') {
                self.bump();
                self.bump();
                self.take_until(b"*/");
                self.emit(mark, Scope::Comment);
                continue;
            }
            if byte.is_ascii_whitespace() {
                self.bump();
                continue;
            }
            match byte {
                b'{' | b'}' | b';' => {
                    self.bump();
                    match byte {
                        b'{' => depth += 1,
                        b'}' => depth = depth.saturating_sub(1),
                        _ => {}
                    }
                    part = CssPart::Selector;
                    fresh = true;
                    in_attr = false;
                    continue;
                }
                _ => {}
            }
            if fresh {
                fresh = false;
                part = if byte == b'@' {
                    CssPart::AtRule
                } else if depth == 0 || self.css_opens_block() {
                    CssPart::Selector
                } else {
                    CssPart::Property
                };
            }
            if byte == b'"' || byte == b'\'' {
                self.take_css_string(byte);
                self.emit(mark, Scope::String);
                continue;
            }
            if self.css_number_at() && part != CssPart::Property {
                self.take_css_number();
                self.emit(mark, Scope::Number);
                continue;
            }
            match (part, byte) {
                (CssPart::AtRule, b'@') => {
                    self.bump();
                    self.take_css_name();
                    self.emit(mark, Scope::Keyword);
                }
                (CssPart::Selector, b'.' | b'#') if is_css_name_start(self.byte(self.at + 1)) => {
                    self.bump();
                    self.take_css_name();
                    self.emit(mark, Scope::Type);
                }
                (CssPart::Selector, b':') => {
                    self.bump();
                    if self.byte(self.at) == b':' {
                        self.bump();
                    }
                    self.take_css_name();
                    self.emit(mark, Scope::Keyword);
                }
                (CssPart::Selector, b'[' | b']') => {
                    in_attr = byte == b'[';
                    self.bump();
                }
                (CssPart::Property, b':') => {
                    self.bump();
                    part = CssPart::Value;
                }
                (CssPart::Value, b'!') if self.byte(self.at + 1).is_ascii_alphabetic() => {
                    self.bump();
                    self.take_css_name();
                    self.emit(mark, Scope::Keyword);
                }
                (CssPart::Value, b'#') if is_css_name(self.byte(self.at + 1)) => {
                    self.bump();
                    self.take_css_name();
                    self.emit(mark, Scope::Constant);
                }
                (_, b) if is_css_name_start(b) => {
                    self.take_css_name();
                    let scope = match part {
                        CssPart::Selector if in_attr => Scope::Property,
                        CssPart::Selector => Scope::Type,
                        CssPart::Property => Scope::Property,
                        _ if self.byte(self.at) == b'(' => Scope::Function,
                        CssPart::AtRule if self.peek_nonspace() == b':' => Scope::Property,
                        _ => Scope::Plain,
                    };
                    self.emit(mark, scope);
                    if scope == Scope::Function
                        && self.text[mark.at..self.at].eq_ignore_ascii_case("url")
                    {
                        self.take_css_url();
                    }
                }
                _ => self.bump(),
            }
        }
    }

    /// Whether the statement starting here opens a block before it ends.
    fn css_opens_block(&self) -> bool {
        let mut at = self.at;
        loop {
            match self.byte(at) {
                0 if at >= self.bytes.len() => return false,
                b'{' => return true,
                b';' | b'}' => return false,
                quote @ (b'"' | b'\'') => {
                    at += 1;
                    while !matches!(self.byte(at), b'\n' | 0) && self.byte(at) != quote {
                        at += if self.byte(at) == b'\\' { 2 } else { 1 };
                    }
                    at += 1;
                }
                b'/' if self.byte(at + 1) == b'*' => {
                    at += 2;
                    while at < self.bytes.len()
                        && !(self.byte(at) == b'*' && self.byte(at + 1) == b'/')
                    {
                        at += 1;
                    }
                    at += 2;
                }
                _ => at += 1,
            }
        }
    }

    fn css_number_at(&self) -> bool {
        let digit = |at: usize| self.byte(at).is_ascii_digit();
        let unsigned = |at: usize| digit(at) || (self.byte(at) == b'.' && digit(at + 1));
        match self.byte(self.at) {
            b'-' | b'+' => unsigned(self.at + 1),
            _ => unsigned(self.at),
        }
    }

    /// A number and its unit: `-1.5em`, `50%`, `.5s`.
    fn take_css_number(&mut self) {
        if matches!(self.byte(self.at), b'-' | b'+') {
            self.bump();
        }
        while self.byte(self.at).is_ascii_digit() || self.byte(self.at) == b'.' {
            self.bump();
        }
        if self.byte(self.at) == b'%' {
            self.bump();
        } else {
            while self.byte(self.at).is_ascii_alphabetic() {
                self.bump();
            }
        }
    }

    fn take_css_name(&mut self) {
        while !self.done() && is_css_name(self.byte(self.at)) {
            self.bump();
        }
    }

    /// Like [`Self::take_quoted`] but without its checkpoint: an escaped
    /// newline inside a rule must not mark the next line as top level.
    fn take_css_string(&mut self, quote: u8) {
        self.bump();
        while !self.done() {
            let byte = self.byte(self.at);
            if byte == b'\\' {
                self.bump();
                self.bump();
                continue;
            }
            if byte == b'\n' {
                return;
            }
            self.bump();
            if byte == quote {
                return;
            }
        }
    }

    /// An unquoted `url(...)` body is one string: a data URI carries `;` and
    /// `,`, which would otherwise end the declaration.
    fn take_css_url(&mut self) {
        self.bump();
        while matches!(self.byte(self.at), b' ' | b'\t') {
            self.bump();
        }
        if matches!(self.byte(self.at), b'"' | b'\'') {
            return;
        }
        let mark = self.mark();
        while !self.done() && !matches!(self.byte(self.at), b')' | b'\n') {
            self.bump();
        }
        self.emit(mark, Scope::String);
    }

    // --- shared scanning helpers ---

    fn take_line(&mut self) {
        while !self.done() && self.byte(self.at) != b'\n' {
            self.bump();
        }
    }

    /// Advance until `needle` (consumed) or the end. No checkpoint: this is
    /// an open construct, and a later scan must not resume inside it.
    fn take_until(&mut self, needle: &[u8]) {
        while !self.done() && !self.at_bytes(needle) {
            self.bump();
        }
        for _ in needle {
            self.bump();
        }
    }

    fn at_bytes(&self, needle: &[u8]) -> bool {
        self.bytes
            .get(self.at..)
            .is_some_and(|rest| rest.starts_with(needle))
    }

    /// Consume a quoted run, honouring backslash escapes. An unterminated
    /// quote stops at the newline, so one stray `"` cannot colour the rest of
    /// the file.
    fn take_quoted(&mut self, quote: u8) {
        self.bump();
        while !self.done() {
            if self.checkpoint() {
                break;
            }
            let byte = self.byte(self.at);
            if byte == b'\\' {
                self.bump();
                self.bump();
                continue;
            }
            // A backtick or triple-quote may span lines; a plain quote may not.
            if byte == b'\n' && quote != b'`' {
                return;
            }
            self.bump();
            if byte == quote {
                return;
            }
        }
    }

    fn take_bracketed(&mut self, open: u8, close: u8) {
        let mut depth = 0;
        while !self.done() {
            if self.checkpoint() {
                break;
            }
            let byte = self.byte(self.at);
            if byte == open {
                depth += 1;
            } else if byte == close {
                depth -= 1;
                if depth == 0 {
                    self.bump();
                    return;
                }
            } else if byte == b'\n' && depth > 0 && open == b'[' {
                // A bracket left open is a typo, not a construct.
                return;
            }
            self.bump();
        }
    }

    /// Whether the next two bytes are `first` then `second`.
    fn at_pair(&self, first: u8, second: u8) -> bool {
        self.byte(self.at) == first && self.byte(self.at + 1) == second
    }

    /// Whether the next three bytes are all `byte` — a triple quote or fence.
    fn at_triple(&self, byte: u8) -> bool {
        self.byte(self.at) == byte
            && self.byte(self.at + 1) == byte
            && self.byte(self.at + 2) == byte
    }

    /// The next byte that is not a space or tab, or 0 at the end.
    fn peek_nonspace(&self) -> u8 {
        let mut at = self.at;
        while matches!(self.byte(at), b' ' | b'\t') {
            at += 1;
        }
        self.byte(at)
    }

    /// Whether only whitespace stands between here and the start of the line.
    fn only_space_before(&self) -> bool {
        self.text[self.line_start..self.at]
            .bytes()
            .all(|b| b == b' ' || b == b'\t')
    }

    /// A key site for keyed files: line-leading, or after dotenv's `export`.
    fn keyed_property_site(&self, word_start: usize) -> bool {
        let mut at = self.line_start;
        while matches!(self.byte(at), b' ' | b'\t') {
            at += 1;
        }
        if at == word_start {
            return true;
        }
        if !self
            .text
            .as_bytes()
            .get(at..)
            .is_some_and(|s| s.starts_with(b"export"))
        {
            return false;
        }
        let after = at + "export".len();
        after < word_start
            && self.text[after..word_start]
                .bytes()
                .all(|b| b == b' ' || b == b'\t')
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(body: &str) -> Text {
        Text::new(body.to_string())
    }

    fn line_offset(text: &str, line: usize) -> usize {
        if line == 0 {
            return 0;
        }
        let mut seen = 0;
        for (index, byte) in text.as_bytes().iter().enumerate() {
            if *byte == b'\n' {
                seen += 1;
                if seen == line {
                    return index + 1;
                }
            }
        }
        text.len()
    }

    /// The text one span covers, so a test names what it sees.
    fn spans(text: &str, grammar: Grammar, line: usize) -> Vec<(&str, Scope)> {
        let syntax = Syntax::parse(text, grammar);
        let body = text.lines().nth(line).unwrap_or("");
        syntax
            .line(line)
            .iter()
            .map(|span| (&body[span.start..span.end], span.scope))
            .collect()
    }

    #[test]
    fn a_grammar_is_chosen_by_extension() {
        assert_eq!(Grammar::for_path("src/main.rs"), Grammar::Rust);
        assert_eq!(Grammar::for_path("app.TSX"), Grammar::CLike);
        assert_eq!(Grammar::for_path("build.gradle.kts"), Grammar::CLike);
        assert_eq!(Grammar::for_path("a/b/notes.md"), Grammar::Markdown);
        assert_eq!(Grammar::for_path("Cargo.toml"), Grammar::Keyed);
        assert_eq!(Grammar::for_path(".env"), Grammar::Keyed);
        assert_eq!(Grammar::for_path("app/.env.local"), Grammar::Keyed);
        assert_eq!(Grammar::for_path("config.env"), Grammar::Keyed);
        assert_eq!(Grammar::for_path("Makefile"), Grammar::Shell);
        assert_eq!(Grammar::for_path("src/rules.mk"), Grammar::Shell);
        assert_eq!(Grammar::for_path("schema.prisma"), Grammar::Prisma);
        assert_eq!(Grammar::for_path("migrations/001.sql"), Grammar::Sql);
        assert_eq!(Grammar::for_path("query.PSQL"), Grammar::Sql);
        assert_eq!(Grammar::for_path("main.tf"), Grammar::Hcl);
        assert_eq!(Grammar::for_path("prod.tfvars"), Grammar::Hcl);
        assert_eq!(Grammar::for_path("shared.hcl"), Grammar::Hcl);
        assert_eq!(Grammar::for_path("values.yaml"), Grammar::Keyed);
        assert_eq!(Grammar::for_path("Chart.yml"), Grammar::Keyed);
        assert_eq!(Grammar::for_path("nginx.conf"), Grammar::Keyed);
        assert_eq!(Grammar::for_path("templates/_helpers.tpl"), Grammar::Keyed);
        assert_eq!(Grammar::for_path("index.HTML"), Grammar::Html);
        assert_eq!(Grammar::for_path("page.htm"), Grammar::Html);
        assert_eq!(Grammar::for_path("app.xhtml"), Grammar::Html);
        assert_eq!(Grammar::for_path("data.xml"), Grammar::Html);
        assert_eq!(Grammar::for_path("icon.svg"), Grammar::Html);
        assert_eq!(Grammar::for_path("styles/app.CSS"), Grammar::Css);
        assert_eq!(Grammar::for_path("flake.nix"), Grammar::Nix);
        assert_eq!(Grammar::for_path("Dockerfile"), Grammar::Dockerfile);
        assert_eq!(
            Grammar::for_path("docker/Dockerfile.ci"),
            Grammar::Dockerfile
        );
        assert_eq!(Grammar::for_path("Containerfile"), Grammar::Dockerfile);
        assert_eq!(Grammar::for_path("api.dockerfile"), Grammar::Dockerfile);
        assert_eq!(Grammar::for_path("requests/users.http"), Grammar::Http);
        assert_eq!(Grammar::for_path("smoke.rest"), Grammar::Http);
        // A dotfile's name is not an extension, and neither is a directory's.
        assert_eq!(Grammar::for_path(".gitignore"), Grammar::None);
        assert_eq!(Grammar::for_path("docs.md/main.rs"), Grammar::Rust);
        assert_eq!(Grammar::for_path("LICENSE"), Grammar::None);
    }

    #[test]
    fn an_unknown_grammar_colours_nothing() {
        assert!(Syntax::parse("anything at all\n", Grammar::None).is_empty());
    }

    #[test]
    fn rust_keywords_strings_and_calls() {
        let found = spans("fn main() { let x = \"hi\"; }\n", Grammar::Rust, 0);
        assert!(found.contains(&("fn", Scope::Keyword)), "{found:?}");
        assert!(found.contains(&("main", Scope::Function)), "{found:?}");
        assert!(found.contains(&("let", Scope::Keyword)), "{found:?}");
        assert!(found.contains(&("\"hi\"", Scope::String)), "{found:?}");
    }

    #[test]
    fn control_flow_is_its_own_scope() {
        let found = spans("if x { return 1; }\n", Grammar::Rust, 0);
        assert!(found.contains(&("if", Scope::ControlKeyword)), "{found:?}");
        assert!(
            found.contains(&("return", Scope::ControlKeyword)),
            "{found:?}"
        );
        assert!(found.contains(&("1", Scope::Number)), "{found:?}");
    }

    /// The reason the scanner is whole-document: a block comment decides the
    /// colour of lines below the one it opened on.
    #[test]
    fn a_block_comment_spans_lines_as_one_span_each() {
        let text = "let a = 1;\n/* one\n   two */\nlet b = 2;\n";
        assert_eq!(spans(text, Grammar::Rust, 1), [("/* one", Scope::Comment)]);
        assert_eq!(
            spans(text, Grammar::Rust, 2),
            [("   two */", Scope::Comment)]
        );
        // The line after it is code again.
        assert!(spans(text, Grammar::Rust, 3)
            .iter()
            .any(|(text, scope)| *text == "let" && *scope == Scope::Keyword));
    }

    /// One stray quote must not colour the rest of the file.
    #[test]
    fn an_unterminated_quote_stops_at_the_newline() {
        let text = "let a = \"oops\nlet b = 2;\n";
        assert_eq!(
            spans(text, Grammar::Rust, 0),
            [("let", Scope::Keyword), ("\"oops", Scope::String)]
        );
        assert!(spans(text, Grammar::Rust, 1)
            .iter()
            .any(|(text, scope)| *text == "let" && *scope == Scope::Keyword));
    }

    /// `'a` has no closer; treating it as a string would swallow the line.
    #[test]
    fn a_rust_lifetime_is_not_a_string() {
        let found = spans("fn f<'a>(x: &'a str) -> bool { true }\n", Grammar::Rust, 0);
        assert!(
            !found.iter().any(|(_, scope)| *scope == Scope::String),
            "a lifetime must not open a string: {found:?}"
        );
        let literal = spans("let c = 'x';\n", Grammar::Rust, 0);
        assert!(literal.contains(&("'x'", Scope::String)), "{literal:?}");
    }

    #[test]
    fn json_tells_a_key_from_a_value() {
        let found = spans("{\"name\": \"forge\", \"n\": 3}\n", Grammar::Json, 0);
        assert!(found.contains(&("\"name\"", Scope::Property)), "{found:?}");
        assert!(found.contains(&("\"forge\"", Scope::String)), "{found:?}");
        assert!(found.contains(&("3", Scope::Number)), "{found:?}");
    }

    #[test]
    fn a_keyed_file_colours_keys_sections_and_comments() {
        let text = "# note\n[table]\nkey = \"value\"\n";
        assert_eq!(spans(text, Grammar::Keyed, 0), [("# note", Scope::Comment)]);
        assert_eq!(spans(text, Grammar::Keyed, 1), [("[table]", Scope::Type)]);
        let row = spans(text, Grammar::Keyed, 2);
        assert!(row.contains(&("key", Scope::Property)), "{row:?}");
        assert!(row.contains(&("\"value\"", Scope::String)), "{row:?}");
    }

    #[test]
    fn dotenv_colours_keys_values_export_and_comments() {
        let text = "# note\nFOO=bar\nexport BAZ=\"qux\"\n";
        assert_eq!(Grammar::for_path(".env"), Grammar::Keyed);
        assert_eq!(spans(text, Grammar::Keyed, 0), [("# note", Scope::Comment)]);
        let plain = spans(text, Grammar::Keyed, 1);
        assert!(plain.contains(&("FOO", Scope::Property)), "{plain:?}");
        let exported = spans(text, Grammar::Keyed, 2);
        assert!(
            exported.contains(&("export", Scope::Keyword)),
            "{exported:?}"
        );
        assert!(exported.contains(&("BAZ", Scope::Property)), "{exported:?}");
        assert!(
            exported.contains(&("\"qux\"", Scope::String)),
            "{exported:?}"
        );
    }

    #[test]
    fn prisma_schema_colours_comments_enums_and_types() {
        let text = "// note\nmodel User { id String }\nenum Role { USER }\n";
        assert_eq!(Grammar::for_path("schema.prisma"), Grammar::Prisma);
        assert_eq!(
            spans(text, Grammar::Prisma, 0),
            [("// note", Scope::Comment)]
        );
        let model = spans(text, Grammar::Prisma, 1);
        assert!(model.contains(&("model", Scope::Keyword)), "{model:?}");
        assert!(model.contains(&("User", Scope::Type)), "{model:?}");
        assert!(model.contains(&("String", Scope::Type)), "{model:?}");
        let enumerated = spans(text, Grammar::Prisma, 2);
        assert!(
            enumerated.contains(&("enum", Scope::Keyword)),
            "{enumerated:?}"
        );
        assert!(
            enumerated.contains(&("Role", Scope::Type)),
            "{enumerated:?}"
        );
    }

    #[test]
    fn sql_colours_comments_and_case_insensitive_keywords() {
        let text = "-- note\nSELECT id FROM users WHERE name = 'x';\n/* block */\n";
        assert_eq!(Grammar::for_path("seed.sql"), Grammar::Sql);
        assert_eq!(spans(text, Grammar::Sql, 0), [("-- note", Scope::Comment)]);
        let query = spans(text, Grammar::Sql, 1);
        assert!(query.contains(&("SELECT", Scope::Keyword)), "{query:?}");
        assert!(query.contains(&("FROM", Scope::Keyword)), "{query:?}");
        assert!(query.contains(&("WHERE", Scope::Keyword)), "{query:?}");
        assert!(query.contains(&("'x'", Scope::String)), "{query:?}");
        assert_eq!(
            spans(text, Grammar::Sql, 2),
            [("/* block */", Scope::Comment)]
        );
        let lower = spans("select * from t;\n", Grammar::Sql, 0);
        assert!(lower.contains(&("select", Scope::Keyword)), "{lower:?}");
        assert!(lower.contains(&("from", Scope::Keyword)), "{lower:?}");
    }

    #[test]
    fn hcl_colours_terraform_blocks_and_comments() {
        let text = "# note\nresource \"aws_s3_bucket\" \"data\" {\n  bucket = \"x\"\n}\n";
        assert_eq!(Grammar::for_path("main.tf"), Grammar::Hcl);
        assert_eq!(spans(text, Grammar::Hcl, 0), [("# note", Scope::Comment)]);
        let block = spans(text, Grammar::Hcl, 1);
        assert!(block.contains(&("resource", Scope::Keyword)), "{block:?}");
        assert!(
            block.contains(&("\"aws_s3_bucket\"", Scope::String)),
            "{block:?}"
        );
        let attr = spans(text, Grammar::Hcl, 2);
        assert!(attr.contains(&("bucket", Scope::Property)), "{attr:?}");
        assert!(attr.contains(&("\"x\"", Scope::String)), "{attr:?}");
        let slash = spans("// c\nvariable \"n\" {}\n", Grammar::Hcl, 0);
        assert_eq!(slash, [("// c", Scope::Comment)]);
    }

    #[test]
    fn helm_template_actions_are_coloured_in_keyed_files() {
        let text = "image: {{ .Values.image }}\n# note\n";
        assert_eq!(Grammar::for_path("values.yaml"), Grammar::Keyed);
        assert_eq!(Grammar::for_path("helpers.tpl"), Grammar::Keyed);
        let row = spans(text, Grammar::Keyed, 0);
        assert!(row.contains(&("image", Scope::Property)), "{row:?}");
        assert!(
            row.contains(&("{{ .Values.image }}", Scope::Keyword)),
            "{row:?}"
        );
        assert_eq!(spans(text, Grammar::Keyed, 1), [("# note", Scope::Comment)]);
    }

    #[test]
    fn python_handles_triple_quotes_and_defs() {
        let text = "def go():\n    \"\"\"doc\n    more\"\"\"\n    return 1\n";
        let head = spans(text, Grammar::Python, 0);
        assert!(head.contains(&("def", Scope::Keyword)), "{head:?}");
        assert!(head.contains(&("go", Scope::Function)), "{head:?}");
        // The docstring covers both of its lines.
        assert!(spans(text, Grammar::Python, 1)
            .iter()
            .any(|(_, scope)| *scope == Scope::String));
        assert!(spans(text, Grammar::Python, 2)
            .iter()
            .any(|(_, scope)| *scope == Scope::String));
    }

    #[test]
    fn shell_colours_variables_and_words() {
        let found = spans("if [ -n $HOME ]; then echo \"hi\"; fi\n", Grammar::Shell, 0);
        assert!(found.contains(&("if", Scope::ControlKeyword)), "{found:?}");
        assert!(found.contains(&("$HOME", Scope::Property)), "{found:?}");
        // A `$` inside double quotes stays part of the string: interpolation
        // is a shell rule this scanner deliberately does not model.
        assert!(found.contains(&("\"hi\"", Scope::String)), "{found:?}");
    }

    #[test]
    fn markdown_colours_structure() {
        let text = "# Title\n\n- item\n\n```rs\ncode\n```\n";
        assert_eq!(
            spans(text, Grammar::Markdown, 0),
            [("# Title", Scope::Keyword)]
        );
        assert_eq!(
            spans(text, Grammar::Markdown, 2),
            [("-", Scope::ControlKeyword)]
        );
    }

    #[test]
    fn html_colours_tags_attributes_comments_and_entities() {
        let text = "<div class=\"x\" checked><!-- n --></div>\n";
        let found = spans(text, Grammar::Html, 0);
        assert!(found.contains(&("<div", Scope::Type)), "{found:?}");
        assert!(found.contains(&("class", Scope::Property)), "{found:?}");
        assert!(found.contains(&("\"x\"", Scope::String)), "{found:?}");
        assert!(found.contains(&("checked", Scope::Property)), "{found:?}");
        assert!(found.contains(&("<!-- n -->", Scope::Comment)), "{found:?}");
        assert!(found.contains(&("</div", Scope::Type)), "{found:?}");
        let entity = spans("a &amp; b\n", Grammar::Html, 0);
        assert!(entity.contains(&("&amp;", Scope::Constant)), "{entity:?}");
        let decl = spans(
            "<!DOCTYPE html>\n<?xml version=\"1.0\"?>\n",
            Grammar::Html,
            0,
        );
        assert!(
            decl.contains(&("<!DOCTYPE html>", Scope::Keyword)),
            "{decl:?}"
        );
        let pi = spans(
            "<!DOCTYPE html>\n<?xml version=\"1.0\"?>\n",
            Grammar::Html,
            1,
        );
        assert!(
            pi.contains(&("<?xml version=\"1.0\"?>", Scope::Keyword)),
            "{pi:?}"
        );
    }

    #[test]
    fn html_script_body_is_plain_so_comparisons_are_not_tags() {
        let text = "<script>\nif (a < b) {}\n</script>\n";
        assert_eq!(spans(text, Grammar::Html, 1), []);
        let close = spans(text, Grammar::Html, 2);
        assert!(close.contains(&("</script", Scope::Type)), "{close:?}");
        let cdata = spans("<![CDATA[ a < b ]]>\n", Grammar::Html, 0);
        assert!(
            cdata.contains(&("<![CDATA[ a < b ]]>", Scope::String)),
            "{cdata:?}"
        );
    }

    #[test]
    fn html_comments_span_lines_and_do_not_colour_what_follows() {
        let text = "<!-- one\ntwo -->\n<div></div>\n";
        assert_eq!(
            spans(text, Grammar::Html, 0),
            [("<!-- one", Scope::Comment)]
        );
        assert_eq!(spans(text, Grammar::Html, 1), [("two -->", Scope::Comment)]);
        let tag = spans(text, Grammar::Html, 2);
        assert!(tag.contains(&("<div", Scope::Type)), "{tag:?}");
        assert!(!tag.iter().any(|(_, scope)| *scope == Scope::Comment));
    }

    #[test]
    fn css_colours_selectors_properties_and_values() {
        let text = "/* note */\n.card > a:hover, #main {\n  --gap: 4px;\n  color: #fff !important;\n  margin: -1.5em 0 calc(100% - 2px);\n}\n";
        assert_eq!(
            spans(text, Grammar::Css, 0),
            [("/* note */", Scope::Comment)]
        );
        let selector = spans(text, Grammar::Css, 1);
        assert!(selector.contains(&(".card", Scope::Type)), "{selector:?}");
        assert!(selector.contains(&("a", Scope::Type)), "{selector:?}");
        assert!(
            selector.contains(&(":hover", Scope::Keyword)),
            "{selector:?}"
        );
        assert!(selector.contains(&("#main", Scope::Type)), "{selector:?}");
        let custom = spans(text, Grammar::Css, 2);
        assert_eq!(custom, [("--gap", Scope::Property), ("4px", Scope::Number)]);
        let colour = spans(text, Grammar::Css, 3);
        assert!(colour.contains(&("color", Scope::Property)), "{colour:?}");
        assert!(colour.contains(&("#fff", Scope::Constant)), "{colour:?}");
        assert!(
            colour.contains(&("!important", Scope::Keyword)),
            "{colour:?}"
        );
        let margin = spans(text, Grammar::Css, 4);
        assert!(margin.contains(&("-1.5em", Scope::Number)), "{margin:?}");
        assert!(margin.contains(&("calc", Scope::Function)), "{margin:?}");
        assert!(margin.contains(&("100%", Scope::Number)), "{margin:?}");
    }

    /// Inside `@media` a statement that reaches `{` is a rule, so its name is a
    /// selector rather than a property.
    #[test]
    fn css_at_rules_nest_rules_and_keep_data_uris_whole() {
        let text = "@media (min-width: 40em) {\n  a { color: red }\n}\n.i { background: url(data:image/png;base64,AA==) }\n";
        let at = spans(text, Grammar::Css, 0);
        assert!(at.contains(&("@media", Scope::Keyword)), "{at:?}");
        assert!(at.contains(&("min-width", Scope::Property)), "{at:?}");
        assert!(at.contains(&("40em", Scope::Number)), "{at:?}");
        let nested = spans(text, Grammar::Css, 1);
        assert!(nested.contains(&("a", Scope::Type)), "{nested:?}");
        assert!(nested.contains(&("color", Scope::Property)), "{nested:?}");
        let url = spans(text, Grammar::Css, 3);
        assert!(url.contains(&("url", Scope::Function)), "{url:?}");
        assert!(
            url.contains(&("data:image/png;base64,AA==", Scope::String)),
            "{url:?}"
        );
        assert!(!url.iter().any(|(text, _)| *text == "base64"), "{url:?}");
    }

    /// A declaration's line depends on the `{` above it, so an edit inside a
    /// rule may not resume on the line under the caret.
    #[test]
    fn a_css_edit_inside_a_rule_restarts_above_it() {
        let text = "a {}\n.b {\n  color: red;\n  margin: 0;\n}\nc {}\n";
        let edited = "a {}\n.b {\n  color: blue;\n  margin: 0;\n}\nc {}\n";
        let next = Syntax::parse(text, Grammar::Css).edited(&doc(edited), Grammar::Css, 2, 2, 0);
        assert!(
            next.scanned_lines().start <= 1,
            "resumed inside the rule: {:?}",
            next.scanned_lines()
        );
        assert_eq!(
            all_spans(&next, 7),
            all_spans(&Syntax::parse(edited, Grammar::Css), 7)
        );
        assert_eq!(next.scope_at(3, 2), Scope::Property);
    }

    /// Offsets index the line they are on, and never run past it.
    #[test]
    fn every_span_is_inside_its_line() {
        let text = "fn main() {\n  let s = \"a\\nb\";\n  /* c */\n}\n";
        let syntax = Syntax::parse(text, Grammar::Rust);
        for (index, line) in text.lines().enumerate() {
            for span in syntax.line(index) {
                assert!(span.start < span.end, "empty span on line {index}");
                assert!(
                    span.end <= line.len(),
                    "span {span:?} runs past line {index} ({:?})",
                    line
                );
                assert!(line.is_char_boundary(span.start) && line.is_char_boundary(span.end));
            }
        }
    }

    #[test]
    fn nix_colours_bindings_strings_paths_and_comments() {
        let text = "{ pkgs ? import <nixpkgs> {} }:\nlet\n  name = \"hi ${pkgs.hello}\";\n  src = ./src; # local\nin if name == \"x\" then rec { a.b = true; } else null\n";
        assert_eq!(
            spans(text, Grammar::Nix, 0),
            vec![("import", Scope::Function), ("<nixpkgs>", Scope::String)]
        );
        assert_eq!(spans(text, Grammar::Nix, 1), vec![("let", Scope::Keyword)]);
        assert_eq!(
            spans(text, Grammar::Nix, 2),
            vec![
                ("name", Scope::Property),
                ("\"hi ${pkgs.hello}\"", Scope::String)
            ]
        );
        assert_eq!(
            spans(text, Grammar::Nix, 3),
            vec![
                ("src", Scope::Property),
                ("./src", Scope::String),
                ("# local", Scope::Comment)
            ]
        );
        let last = spans(text, Grammar::Nix, 4);
        assert!(last.contains(&("in", Scope::Keyword)));
        assert!(last.contains(&("if", Scope::ControlKeyword)));
        // `==` is a comparison, so the word before it is not a binding.
        assert!(!last.contains(&("name", Scope::Property)));
        assert!(last.contains(&("a.b", Scope::Property)));
        assert!(last.contains(&("true", Scope::Constant)));
        assert!(last.contains(&("null", Scope::Constant)));
    }

    #[test]
    fn a_nix_indented_string_spans_lines_and_honours_its_escapes() {
        let text = "x = ''\n  echo ''${HOME} '''\n  if true\n'';\ny = 1;\n";
        assert_eq!(
            spans(text, Grammar::Nix, 1),
            vec![("  echo ''${HOME} '''", Scope::String)]
        );
        // The body's `if` is text, not a keyword.
        assert_eq!(
            spans(text, Grammar::Nix, 2),
            vec![("  if true", Scope::String)]
        );
        assert_eq!(
            spans(text, Grammar::Nix, 4),
            vec![("y", Scope::Property), ("1", Scope::Number)]
        );
    }

    #[test]
    fn dockerfile_colours_instructions_flags_and_variables() {
        let text = "# syntax=docker/dockerfile:1\nFROM rust:1.89 AS build\nCOPY --from=build /app /app\nENV PATH=$HOME/bin:${PATH}\nEXPOSE 8080/tcp\nrun echo \"hi\"\n";
        assert_eq!(
            spans(text, Grammar::Dockerfile, 0),
            vec![("# syntax=docker/dockerfile:1", Scope::Comment)]
        );
        assert_eq!(
            spans(text, Grammar::Dockerfile, 1),
            vec![("FROM", Scope::Keyword), ("AS", Scope::Keyword)]
        );
        assert_eq!(
            spans(text, Grammar::Dockerfile, 2),
            vec![("COPY", Scope::Keyword), ("--from", Scope::Property)]
        );
        assert_eq!(
            spans(text, Grammar::Dockerfile, 3),
            vec![
                ("ENV", Scope::Keyword),
                ("PATH", Scope::Property),
                ("$HOME", Scope::Property),
                ("${PATH}", Scope::Property)
            ]
        );
        assert_eq!(
            spans(text, Grammar::Dockerfile, 4),
            vec![("EXPOSE", Scope::Keyword), ("8080", Scope::Number)]
        );
        // Instructions are case-insensitive.
        assert_eq!(
            spans(text, Grammar::Dockerfile, 5),
            vec![("run", Scope::Keyword), ("\"hi\"", Scope::String)]
        );
    }

    #[test]
    fn a_continued_dockerfile_line_is_an_argument_not_an_instruction() {
        let text =
            "RUN apt-get update \\\n    # a comment\n    copy files \\\n    && true\nUSER app\n";
        assert_eq!(
            spans(text, Grammar::Dockerfile, 1),
            vec![("# a comment", Scope::Comment)]
        );
        assert_eq!(spans(text, Grammar::Dockerfile, 2), vec![]);
        assert_eq!(
            spans(text, Grammar::Dockerfile, 4),
            vec![("USER", Scope::Keyword)]
        );
        let syntax = Syntax::parse(text, Grammar::Dockerfile);
        assert_eq!(syntax.safe[..5], [true, false, false, false, true]);
    }

    #[test]
    fn a_dockerfile_heredoc_body_stays_plain() {
        let text = "RUN <<EOF\nFROM inside\nEOF\nCMD [\"x\"]\n";
        assert_eq!(
            spans(text, Grammar::Dockerfile, 0),
            vec![("RUN", Scope::Keyword), ("<<EOF", Scope::Keyword)]
        );
        assert_eq!(spans(text, Grammar::Dockerfile, 1), vec![]);
        assert_eq!(spans(text, Grammar::Dockerfile, 2), vec![]);
        assert_eq!(
            spans(text, Grammar::Dockerfile, 3),
            vec![("CMD", Scope::Keyword), ("\"x\"", Scope::String)]
        );
    }

    #[test]
    fn http_colours_requests_headers_variables_and_bodies() {
        let text = "@host = localhost:8080\n### Create\n# note\nPOST http://{{host}}/users HTTP/1.1\nContent-Type: application/json\n\n{\"name\": \"{{user}}\"}\n###\nhttps://example.com\n";
        assert_eq!(
            spans(text, Grammar::Http, 0),
            vec![
                ("@host", Scope::Property),
                (" = localhost:8080", Scope::String)
            ]
        );
        assert_eq!(
            spans(text, Grammar::Http, 1),
            vec![("### Create", Scope::Comment)]
        );
        assert_eq!(
            spans(text, Grammar::Http, 2),
            vec![("# note", Scope::Comment)]
        );
        assert_eq!(
            spans(text, Grammar::Http, 3),
            vec![
                ("POST", Scope::Keyword),
                ("http://", Scope::String),
                ("{{host}}", Scope::Keyword),
                ("/users", Scope::String),
                ("HTTP/1.1", Scope::Constant)
            ]
        );
        assert_eq!(
            spans(text, Grammar::Http, 4),
            vec![("Content-Type", Scope::Property)]
        );
        assert_eq!(
            spans(text, Grammar::Http, 6),
            vec![("\"name\"", Scope::String), ("\"{{user}}\"", Scope::String)]
        );
        assert_eq!(spans(text, Grammar::Http, 7), vec![("###", Scope::Comment)]);
        // A request line may leave the method out.
        assert_eq!(
            spans(text, Grammar::Http, 8),
            vec![("https://example.com", Scope::String)]
        );
    }

    #[test]
    fn an_http_body_line_is_not_read_as_a_comment_or_header() {
        let text = "GET /a\n\n# not a comment\nKey: value\n";
        assert_eq!(spans(text, Grammar::Http, 2), vec![]);
        assert_eq!(spans(text, Grammar::Http, 3), vec![]);
        let syntax = Syntax::parse(text, Grammar::Http);
        assert_eq!(syntax.safe[..4], [true, false, false, false]);
    }

    /// Whatever the input, scanning terminates and stays in bounds.
    #[test]
    fn hostile_input_does_not_hang_or_panic() {
        for grammar in [
            Grammar::Rust,
            Grammar::CLike,
            Grammar::Prisma,
            Grammar::Sql,
            Grammar::Hcl,
            Grammar::Python,
            Grammar::Json,
            Grammar::Keyed,
            Grammar::Shell,
            Grammar::Markdown,
            Grammar::Html,
            Grammar::Css,
            Grammar::Nix,
            Grammar::Dockerfile,
            Grammar::Http,
        ] {
            for text in [
                "",
                "\n\n\n",
                "\"",
                "'",
                "`",
                "/*",
                "[[[[[[",
                "${",
                "```",
                "<",
                "<!--",
                "<script>",
                "&",
                "a{b:url(",
                "@",
                "#",
                "-.",
                "''",
                "<<",
                "RUN <<EOF\n",
                "RUN a \\",
                "{{",
                "GET",
                "\u{1f600} é 漢字\n",
                "a\u{0}b\n",
            ] {
                let syntax = Syntax::parse(text, grammar);
                let _ = syntax.scope_at(0, 0);
            }
        }
    }

    /// Every line's spans, so an incremental scan can be compared against the
    /// full one it has to agree with.
    fn all_spans(syntax: &Syntax, lines: usize) -> Vec<Vec<Span>> {
        (0..lines).map(|line| syntax.line(line).to_vec()).collect()
    }

    /// The point of the incremental scan: an edit near the bottom of a file
    /// must not re-lex what is above it.
    #[test]
    fn an_edit_low_in_the_file_rescans_only_from_near_it() {
        let mut text: String = (0..400).map(|n| format!("let x{n} = {n};\n")).collect();
        let full = Syntax::parse(&text, Grammar::Rust);
        assert_eq!(full.scanned_lines().start, 0);

        // Line 300 gains a keyword; nothing above or below it changes shape.
        let at = line_offset(&text, 300);
        text.insert_str(at, "const Y: u8 = 1;\n");
        let next = full.edited(&doc(&text), Grammar::Rust, 300, 300, 1);
        assert!(
            next.scanned_lines().start >= 299,
            "restarted at {:?}, not just above the edit",
            next.scanned_lines()
        );
        assert!(
            next.scanned_lines().end <= 303,
            "kept scanning to {:?} instead of resyncing",
            next.scanned_lines()
        );
        assert_eq!(
            all_spans(&next, 402),
            all_spans(&Syntax::parse(&text, Grammar::Rust), 402),
            "the incremental scan disagreed with the full one"
        );
    }

    /// A block comment opened above decides the colour below it, so an edit
    /// inside one may not resume at the line under the caret.
    #[test]
    fn an_edit_inside_a_block_comment_restarts_above_it() {
        let text = "fn a() {}\n/* one\ntwo\nthree */\nfn b() {}\n";
        let full = Syntax::parse(text, Grammar::Rust);
        let edited = "fn a() {}\n/* one\ntwoX\nthree */\nfn b() {}\n";
        let next = full.edited(&doc(edited), Grammar::Rust, 2, 2, 0);
        assert!(
            next.scanned_lines().start <= 1,
            "resumed inside the comment: {:?}",
            next.scanned_lines()
        );
        assert_eq!(
            all_spans(&next, 5),
            all_spans(&Syntax::parse(edited, Grammar::Rust), 5)
        );
    }

    /// Opening a block comment recolours everything under it, and the
    /// incremental scan has to keep going until the grammar agrees again.
    #[test]
    fn opening_a_block_comment_recolours_what_is_under_it() {
        let text = "fn a() {}\nfn b() {}\nfn c() {}\n";
        let full = Syntax::parse(text, Grammar::Rust);
        let edited = "/* a() {}\nfn b() {}\nfn c() {}\n";
        let next = full.edited(&doc(edited), Grammar::Rust, 0, 0, 0);
        assert_eq!(
            all_spans(&next, 3),
            all_spans(&Syntax::parse(edited, Grammar::Rust), 3)
        );
        assert_eq!(next.scope_at(2, 0), Scope::Comment);
    }

    /// Removing lines shifts the reused tail up; the spans must land on the
    /// lines they describe, not on the ones they used to.
    #[test]
    fn deleting_lines_shifts_the_reused_tail() {
        let text: String = (0..40).map(|n| format!("let x{n} = \"s{n}\";\n")).collect();
        let full = Syntax::parse(&text, Grammar::Rust);
        let mut edited = text.clone();
        let from = line_offset(&edited, 10);
        let to = line_offset(&edited, 13);
        edited.replace_range(from..to, "");
        let next = full.edited(&doc(&edited), Grammar::Rust, 10, 10, -3);
        assert_eq!(
            all_spans(&next, 37),
            all_spans(&Syntax::parse(&edited, Grammar::Rust), 37)
        );
    }

    fn numbered_lines(range: std::ops::Range<usize>) -> String {
        range.map(|n| format!("let x{n} = \"s{n}\";\n")).collect()
    }

    /// `edited` with the old-text line span `Applied` reports, checked against a
    /// full scan and against having stayed on the incremental path.
    fn assert_edit_matches_full_scan(
        before: &str,
        after: &str,
        first_line: usize,
        last_line: usize,
        line_delta: isize,
    ) {
        let next = Syntax::parse(before, Grammar::Rust).edited(
            &doc(after),
            Grammar::Rust,
            first_line,
            last_line,
            line_delta,
        );
        let lines = after.lines().count() + 2;
        assert_eq!(
            all_spans(&next, lines),
            all_spans(&Syntax::parse(after, Grammar::Rust), lines),
            "the incremental scan disagreed with the full one"
        );
        assert!(
            next.scanned_lines().end + 5 < lines,
            "fell back to a near-full rescan: {:?} of {lines}",
            next.scanned_lines()
        );
    }

    #[test]
    fn enter_between_braces_resumes_below_the_new_lines() {
        let (head, tail) = (numbered_lines(0..10), numbered_lines(10..40));
        let before = format!("{head}fn f() {{}}\n{tail}");
        let after = format!("{head}fn f() {{\n    \n}}\n{tail}");
        assert_edit_matches_full_scan(&before, &after, 10, 10, 2);
    }

    #[test]
    fn splitting_a_line_recolours_both_halves() {
        let (head, tail) = (numbered_lines(0..10), numbered_lines(10..40));
        let before = format!("{head}let a = 1; let b = \"x\";\n{tail}");
        let after = format!("{head}let a = 1;\n let b = \"x\";\n{tail}");
        assert_edit_matches_full_scan(&before, &after, 10, 10, 1);
    }

    #[test]
    fn pasting_several_lines_mid_line_resumes_below_them() {
        let (head, tail) = (numbered_lines(0..20), numbered_lines(21..60));
        let before = format!("{head}let m = 1; let n = 2;\n{tail}");
        let after = format!(
            "{head}let m = 1; let a = \"a\";\nlet b = 'b';\nlet c = 3;\nlet d = 4;\nlet e = 5; let n = 2;\n{tail}"
        );
        assert_edit_matches_full_scan(&before, &after, 20, 20, 4);
    }

    #[test]
    fn replacing_a_selection_with_more_lines_than_it_covered() {
        let (head, tail) = (numbered_lines(0..10), numbered_lines(13..50));
        let before = format!("{head}let a = 1;\nlet b = 2;\nlet c = 3;\n{tail}");
        let after =
            format!("{head}let p = \"p\";\nlet q = 1;\nlet r = 2;\nlet s = 3;\nlet t = 4;\n{tail}");
        assert_edit_matches_full_scan(&before, &after, 10, 12, 2);
    }

    #[test]
    fn an_edit_reuses_untouched_span_allocations() {
        let mut text = "let value = 1;\n".repeat(20_000);
        let syntax = Syntax::parse(&text, Grammar::Rust);
        let before = syntax.line(0).as_ptr();
        let after = syntax.line(19_999).as_ptr();
        let at = line_offset(&text, 10_000);
        text.insert_str(at, "let inserted = 2;\n");
        let syntax = syntax.edited(&doc(&text), Grammar::Rust, 10_000, 10_000, 1);
        assert_eq!(syntax.line(0).as_ptr(), before);
        assert_eq!(syntax.line(20_000).as_ptr(), after);
        // The insert's end sits on the first byte of the line it pushed down, so
        // that line is lexed once more before the scan resyncs.
        assert_eq!(syntax.scanned_lines(), 10_000..10_002);
        assert_eq!(
            all_spans(&syntax, 20_002),
            all_spans(&Syntax::parse(&text, Grammar::Rust), 20_002)
        );
    }

    #[test]
    fn a_resumed_scanner_allocates_only_for_visited_lines() {
        let text = "let value = 1;\n".repeat(20_000);
        let syntax = Syntax::parse(&text, Grammar::Rust);
        let mut scanner = Scanner::new(&text, 0);
        scanner.resume = Some(Resume {
            after_line: 0,
            old_safe: &syntax.safe,
            line_delta: 0,
        });
        scanner.run(Grammar::Rust);
        assert_eq!(scanner.stopped, Some(1));
        assert!(scanner.lines.capacity() <= 4);
        assert!(scanner.safe.capacity() <= 8);
    }

    #[test]
    #[ignore = "manual release-mode timing"]
    fn incremental_syntax_timing() {
        let mut text = doc(&"let value = 1;\n".repeat(20_000));
        let mut syntax = Syntax::parse(text.as_str(), Grammar::Rust);
        let at = text.line_start(10_000) + "let value = ".len();
        let start = std::time::Instant::now();
        for step in 0..1_000 {
            text.replace(
                crate::selection::Range::new(at, at + 1),
                if step % 2 == 0 { "2" } else { "1" },
            );
            syntax = syntax.edited(&text, Grammar::Rust, 10_000, 10_000, 0);
            std::hint::black_box(&syntax);
        }
        eprintln!(
            "1,000 incremental edits in 20,000 lines: {:?}",
            start.elapsed()
        );
    }

    /// Every grammar's incremental scan agrees with its full one.
    #[test]
    fn an_incremental_scan_agrees_with_a_full_one_for_every_grammar() {
        let cases = [
            (Grammar::Rust, "fn a() {}\n// c\nlet s = \"x\";\n"),
            (Grammar::CLike, "function a() {}\n// c\nconst s = `x`;\n"),
            (Grammar::Prisma, "model User {\n  // c\n  name String\n}\n"),
            (Grammar::Sql, "SELECT a\n-- c\nFROM t WHERE b = 'x';\n"),
            (
                Grammar::Hcl,
                "resource \"x\" \"y\" {\n  # c\n  name = \"z\"\n}\n",
            ),
            (Grammar::Python, "def a():\n    # c\n    s = \"x\"\n"),
            (Grammar::Json, "{\n  \"a\": 1,\n  \"b\": true\n}\n"),
            (Grammar::Keyed, "[s]\na = 1\nb = \"x\"\n"),
            (Grammar::Shell, "if true; then\n  echo $HOME\nfi\n"),
            (Grammar::Markdown, "# h\n\n- one\n`code`\n"),
            (
                Grammar::Html,
                "<div class=\"x\">\n<!-- c -->\n<span>y</span>\n",
            ),
            (
                Grammar::Css,
                "/* c */\n.a,\n.b {\n  color: red;\n}\n@media (x: 1px) {\n  p { margin: 0 }\n}\n",
            ),
            (Grammar::Nix, "let\n  # c\n  s = ''\n    x\n  '';\nin s\n"),
            (
                Grammar::Dockerfile,
                "FROM a\nRUN b \\\n  c\nRUN <<EOF\nx\nEOF\n",
            ),
            (
                Grammar::Http,
                "GET /a\nAccept: x\n\n{\"a\": 1}\n###\nPOST /b\n",
            ),
        ];
        for (grammar, text) in cases {
            let full = Syntax::parse(text, grammar);
            for line in 0..text.lines().count() {
                let next = Syntax::parse(text, grammar).edited(&doc(text), grammar, line, line, 0);
                assert_eq!(
                    all_spans(&next, 8),
                    all_spans(&full, 8),
                    "{grammar:?} disagreed when resuming around line {line}"
                );
            }
        }
    }
}
