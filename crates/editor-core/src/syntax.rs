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
    Python,
    Json,
    Keyed,
    Shell,
    Markdown,
    Html,
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
        let Some(dot) = name.rfind('.').filter(|at| *at > 0) else {
            return Self::None;
        };
        match name[dot + 1..].to_ascii_lowercase().as_str() {
            "rs" => Self::Rust,
            "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" | "go" | "java" | "kt" | "kts" | "c"
            | "h" | "cc" | "cpp" | "hpp" | "cs" | "swift" | "scala" | "php" | "dart" | "prisma" => {
                Self::CLike
            }
            "py" | "pyi" => Self::Python,
            "json" => Self::Json,
            "toml" | "yaml" | "yml" | "ini" | "cfg" | "conf" | "env" => Self::Keyed,
            "sh" | "bash" | "zsh" | "fish" | "mk" | "make" => Self::Shell,
            "md" | "markdown" | "mdx" => Self::Markdown,
            "html" | "htm" | "xhtml" | "xml" | "svg" => Self::Html,
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
    /// `first_line` and `last_line` are the touched lines in the *new* text and
    /// `line_delta` how many lines it gained or lost. The scan restarts on the
    /// nearest line above the edit that the previous one passed at top level,
    /// and stops on the first line boundary below it that both scans agree is
    /// top level: a span is a column pair inside its own line, so everything
    /// past that point is reusable exactly as it stands.
    /// Consumes the previous scan to retain untouched span allocations.
    #[must_use]
    pub fn edited(
        mut self,
        text: &str,
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
            return Self::parse(text, grammar);
        }
        let from = (0..=first_line.min(self.safe.len().saturating_sub(1)))
            .rev()
            .find(|line| self.safe[*line])
            .unwrap_or(0);
        // A byte walk for the restart offset, not a lex: finding a newline is
        // memory bandwidth, and re-lexing the head is the thing being avoided.
        let mut out = Scanner::new(&text[line_offset(text, from)..], from);
        out.resume = Some(Resume {
            after_line: last_line,
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

/// Byte offset of a 0-based line.
///
/// A byte walk and not a lex: a newline search is memory bandwidth, while
/// re-colouring the head of the file is the cost this exists to avoid.
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

/// Where an incremental scan may hand back to the spans it already has.
struct Resume<'a> {
    /// The lowest line the edit touched. A resync at or above it would reuse
    /// spans the edit invalidated.
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
            Grammar::Python => self.python(),
            Grammar::Json => self.json(),
            Grammar::Keyed => self.keyed(),
            Grammar::Shell => self.shell(),
            Grammar::Markdown => self.markdown(),
            Grammar::Html => self.html(),
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

fn is_word(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'$'
}

fn is_markup_name_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_' || byte == b':'
}

fn is_markup_name(byte: u8) -> bool {
    is_markup_name_start(byte) || byte.is_ascii_digit() || byte == b'-' || byte == b'.'
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

    /// TOML, YAML, ini, dotenv: a key before a separator, then a value.
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
        assert_eq!(Grammar::for_path("schema.prisma"), Grammar::CLike);
        assert_eq!(Grammar::for_path("index.HTML"), Grammar::Html);
        assert_eq!(Grammar::for_path("page.htm"), Grammar::Html);
        assert_eq!(Grammar::for_path("app.xhtml"), Grammar::Html);
        assert_eq!(Grammar::for_path("data.xml"), Grammar::Html);
        assert_eq!(Grammar::for_path("icon.svg"), Grammar::Html);
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
        let text = "// note\nenum Role { USER }\nname String\n";
        assert_eq!(Grammar::for_path("schema.prisma"), Grammar::CLike);
        assert_eq!(
            spans(text, Grammar::CLike, 0),
            [("// note", Scope::Comment)]
        );
        let enumerated = spans(text, Grammar::CLike, 1);
        assert!(
            enumerated.contains(&("enum", Scope::Keyword)),
            "{enumerated:?}"
        );
        assert!(
            enumerated.contains(&("Role", Scope::Type)),
            "{enumerated:?}"
        );
        let field = spans(text, Grammar::CLike, 2);
        assert!(field.contains(&("String", Scope::Type)), "{field:?}");
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

    /// Whatever the input, scanning terminates and stays in bounds.
    #[test]
    fn hostile_input_does_not_hang_or_panic() {
        for grammar in [
            Grammar::Rust,
            Grammar::CLike,
            Grammar::Python,
            Grammar::Json,
            Grammar::Keyed,
            Grammar::Shell,
            Grammar::Markdown,
            Grammar::Html,
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
        let next = full.edited(&text, Grammar::Rust, 300, 300, 1);
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
        let next = full.edited(edited, Grammar::Rust, 2, 2, 0);
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
        let next = full.edited(edited, Grammar::Rust, 0, 0, 0);
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
        let next = full.edited(&edited, Grammar::Rust, 10, 10, -3);
        assert_eq!(
            all_spans(&next, 37),
            all_spans(&Syntax::parse(&edited, Grammar::Rust), 37)
        );
    }

    #[test]
    fn an_edit_reuses_untouched_span_allocations() {
        let mut text = "let value = 1;\n".repeat(20_000);
        let syntax = Syntax::parse(&text, Grammar::Rust);
        let before = syntax.line(0).as_ptr();
        let after = syntax.line(19_999).as_ptr();
        let at = line_offset(&text, 10_000);
        text.insert_str(at, "let inserted = 2;\n");
        let syntax = syntax.edited(&text, Grammar::Rust, 10_000, 10_000, 1);
        assert_eq!(syntax.line(0).as_ptr(), before);
        assert_eq!(syntax.line(20_000).as_ptr(), after);
        assert_eq!(syntax.scanned_lines(), 10_000..10_001);
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
        let mut text = "let value = 1;\n".repeat(20_000);
        let mut syntax = Syntax::parse(&text, Grammar::Rust);
        let at = line_offset(&text, 10_000) + "let value = ".len();
        let start = std::time::Instant::now();
        for step in 0..1_000 {
            text.replace_range(at..at + 1, if step % 2 == 0 { "2" } else { "1" });
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
            (Grammar::Python, "def a():\n    # c\n    s = \"x\"\n"),
            (Grammar::Json, "{\n  \"a\": 1,\n  \"b\": true\n}\n"),
            (Grammar::Keyed, "[s]\na = 1\nb = \"x\"\n"),
            (Grammar::Shell, "if true; then\n  echo $HOME\nfi\n"),
            (Grammar::Markdown, "# h\n\n- one\n`code`\n"),
            (
                Grammar::Html,
                "<div class=\"x\">\n<!-- c -->\n<span>y</span>\n",
            ),
        ];
        for (grammar, text) in cases {
            let full = Syntax::parse(text, grammar);
            for line in 0..text.lines().count() {
                let next = Syntax::parse(text, grammar).edited(text, grammar, line, line, 0);
                assert_eq!(
                    all_spans(&next, 8),
                    all_spans(&full, 8),
                    "{grammar:?} disagreed when resuming around line {line}"
                );
            }
        }
    }
}
