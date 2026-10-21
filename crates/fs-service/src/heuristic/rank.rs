//! Whether one occurrence of a name, in masked code, is where it is declared.

use std::borrow::Cow;

use languages::Language;

use super::scope::Container;

/// The declaration grammar a file is read with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Dialect {
    Rust,
    /// TypeScript and JavaScript.
    Script,
    Python,
    Go,
    Kotlin,
    Swift,
    Scala,
    Ruby,
    Lisp,
    Shell,
    Php,
    Lua,
    Elixir,
    Haskell,
    Sql,
    Prisma,
    /// C, C++ and Objective-C.
    C,
    /// Other languages whose declarations may start with a type (Java, C#, Dart).
    Typed,
    Other,
}

impl Dialect {
    pub(super) fn of(language: Option<&Language>) -> Dialect {
        let Some(language) = language else {
            return Dialect::Other;
        };
        match (language.id.as_str(), language.family()) {
            ("rust", _) => Dialect::Rust,
            (_, "typescript") => Dialect::Script,
            ("python", _) => Dialect::Python,
            ("go", _) => Dialect::Go,
            ("kotlin", _) => Dialect::Kotlin,
            ("swift", _) => Dialect::Swift,
            ("scala", _) => Dialect::Scala,
            ("ruby", _) => Dialect::Ruby,
            (_, "lisp") => Dialect::Lisp,
            ("shell", _) => Dialect::Shell,
            ("php", _) => Dialect::Php,
            ("lua", _) => Dialect::Lua,
            ("elixir", _) => Dialect::Elixir,
            ("haskell", _) => Dialect::Haskell,
            ("sql", _) => Dialect::Sql,
            ("prisma", _) => Dialect::Prisma,
            (_, "c") => Dialect::C,
            _ if language.typed_declarations => Dialect::Typed,
            _ => Dialect::Other,
        }
    }

    pub(super) fn c_like(self) -> bool {
        matches!(self, Dialect::C | Dialect::Typed)
    }

    pub(super) fn newline_ends_statement(self) -> bool {
        matches!(
            self,
            Dialect::Python
                | Dialect::Go
                | Dialect::Kotlin
                | Dialect::Swift
                | Dialect::Scala
                | Dialect::Ruby
                | Dialect::Lisp
                | Dialect::Shell
                | Dialect::Lua
                | Dialect::Elixir
                | Dialect::Haskell
        )
    }

    /// Nesting is indentation (or `end`), not braces, so a top-level
    /// binding is one in the first column.
    pub(super) fn indented_blocks(self) -> bool {
        matches!(
            self,
            Dialect::Python
                | Dialect::Ruby
                | Dialect::Lisp
                | Dialect::Shell
                | Dialect::Lua
                | Dialect::Elixir
                | Dialect::Haskell
        )
    }

    pub(super) fn word_char(self, c: char) -> bool {
        c.is_alphanumeric() || c == '_' || c == '$' || self == Dialect::Lisp && c == '-'
    }

    /// Whether `name(args) {` with no keyword is a declaration. In Kotlin,
    /// Swift and Groovy it is as often a call with a trailing lambda.
    fn bare_heads(self, container: Option<Container>) -> bool {
        match self {
            Dialect::Script | Dialect::Shell => true,
            Dialect::C | Dialect::Typed => container != Some(Container::Block),
            _ => false,
        }
    }
}

/// What kind of declaration a line is, best first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Kind {
    /// A declaring keyword, a typed function, an enum variant, a macro.
    Keyword,
    /// A method signature with no keyword: `name(args) {`.
    Head,
    /// `let`, `const`, `x :=`, a module-level assignment.
    Binding,
    /// A field or property inside a class or struct body.
    Member,
}

/// One whole-word occurrence of the name in a masked line.
pub(super) struct Site<'a> {
    pub(super) code: &'a str,
    pub(super) at: usize,
    pub(super) len: usize,
    pub(super) container: Option<Container>,
    pub(super) in_brackets: bool,
    pub(super) closed_typedef: bool,
}

const DECLARING_KEYWORDS: &[&str] = &[
    "fn",
    "func",
    "function",
    "class",
    "struct",
    "enum",
    "trait",
    "interface",
    "impl",
    "type",
    "typedef",
    "def",
    "defp",
    "defmodule",
    "defmacro",
    "defprotocol",
    "fun",
    "typealias",
    "defn",
    "mod",
    "module",
    "namespace",
    "union",
    "macro_rules",
    "record",
    "object",
    "protocol",
    "extension",
    "actor",
    "mixin",
    "sub",
    "proc",
];

/// Keywords that bind a name to a value. A declaration, but often a local one.
const BINDING_KEYWORDS: &[&str] = &["const", "let", "var", "static", "val", "local", "final"];

/// Words that may stand between the start of a line and a declared name.
const MODIFIERS: &[&str] = &[
    "pub",
    "export",
    "default",
    "async",
    "public",
    "private",
    "protected",
    "internal",
    "override",
    "final",
    "abstract",
    "readonly",
    "static",
    "declare",
    "virtual",
    "inline",
    "extern",
    "unsafe",
    "get",
    "set",
    "suspend",
    "operator",
    "infix",
    "tailrec",
    "expect",
    "actual",
    "open",
    "sealed",
    "data",
    "lateinit",
    "external",
    "synchronized",
    "native",
    "explicit",
    "constexpr",
    "accessor",
];

/// Modifiers a typed declaration may carry before its type.
const TYPED_MODIFIERS: &[&str] = &[
    "public",
    "private",
    "protected",
    "internal",
    "static",
    "final",
    "abstract",
    "synchronized",
    "native",
    "transient",
    "volatile",
    "virtual",
    "override",
    "sealed",
    "partial",
    "readonly",
    "async",
    "extern",
    "inline",
    "const",
    "constexpr",
    "consteval",
    "constinit",
    "unsigned",
    "signed",
    "explicit",
    "friend",
    "mutable",
    "register",
    "thread_local",
    "strictfp",
    "unsafe",
    "new",
    "required",
    "late",
    "external",
    "factory",
    "default",
];

/// Statement words that make `word name(…)` anything but a typed declaration.
const NOT_A_TYPE: &[&str] = &[
    "return",
    "new",
    "else",
    "throw",
    "case",
    "goto",
    "delete",
    "await",
    "yield",
    "typeof",
    "sizeof",
    "alignof",
    "if",
    "while",
    "for",
    "foreach",
    "switch",
    "do",
    "using",
    "lock",
    "print",
    "echo",
    "assert",
    "not",
    "and",
    "or",
    "in",
    "is",
    "as",
    "when",
    "co_return",
    "co_yield",
    "co_await",
    "import",
    "package",
    "export",
    "static_assert",
    "defer",
    "break",
    "continue",
    "fixed",
    "checked",
    "unchecked",
    "throws",
    "extends",
    "implements",
    "instanceof",
];

/// What may follow a typed signature's parameters when it has a body.
const DEFINITION_TAILS: &[&str] = &[
    "{",
    "=>",
    ":",
    "throws",
    "const",
    "override",
    "noexcept",
    "->",
    "where",
    "async",
    "sync",
    "requires",
    "final",
    "=",
    "&",
    "__attribute__",
];

const MEMBER_MODIFIERS: &[&str] = &[
    "public",
    "private",
    "protected",
    "readonly",
    "static",
    "declare",
    "abstract",
    "override",
    "accessor",
    "var",
];

/// Lines of signature read after a head that could open a multi-line one.
pub(super) const SIGNATURE_LINES: usize = 12;
pub(super) const MAX_SIGNATURE_BYTES: usize = 4096;

/// The kind of declaration `site` is, if it is one. `following` is read only
/// when the line already looks like the start of a signature.
pub(super) fn rank<'a>(
    dialect: Dialect,
    site: &Site<'_>,
    following: impl Iterator<Item = &'a str> + Clone,
) -> Option<Kind> {
    let code = site.code;
    let before = &code[..site.at];
    let after = &code[site.at + site.len..];
    if site.container == Some(Container::Import) || is_import_line(dialect, code) {
        return None;
    }
    let head = strip_decorators(before.trim());
    if head.ends_with('@') {
        return None;
    }
    if let Some(kind) = dialect_rule(dialect, site, head, after) {
        return Some(kind);
    }
    if let Some(kind) = keyword_before(head, after) {
        return Some(kind);
    }
    if let Some(kind) = container_rule(dialect, site, head, after) {
        return Some(kind);
    }
    if dialect.c_like() {
        if let Some(kind) = typed_rule(dialect, site, head, after, following.clone()) {
            return Some(kind);
        }
    }
    head_rule(dialect, site.container, head, after, following)
}

fn is_import_line(dialect: Dialect, code: &str) -> bool {
    let line = code.trim_start();
    match first_word(line) {
        "import" => true,
        "from" => dialect == Dialect::Python,
        "use" => matches!(dialect, Dialect::Rust | Dialect::Php),
        "pub" => {
            dialect == Dialect::Rust && {
                let rest = line["pub".len()..].trim_start();
                let rest = rest
                    .strip_prefix('(')
                    .and_then(|inner| inner.split_once(')'))
                    .map_or(rest, |(_, rest)| rest.trim_start());
                first_word(rest) == "use"
            }
        }
        "export" => {
            dialect == Dialect::Script && {
                let rest = line["export".len()..].trim_start();
                let rest = rest.strip_prefix("type").unwrap_or(rest).trim_start();
                rest.starts_with(['{', '*'])
            }
        }
        _ => false,
    }
}

fn dialect_rule(dialect: Dialect, site: &Site<'_>, head: &str, after: &str) -> Option<Kind> {
    let rest = after.trim_start();
    let assigns = rest.starts_with('=') && !rest.starts_with("==") && !rest.starts_with("=>");
    match dialect {
        Dialect::Python | Dialect::Ruby => {
            let annotated =
                dialect == Dialect::Python && rest.starts_with(':') && !rest.starts_with(":=");
            let binds = matches!(head, "" | "self." | "cls.")
                && (assigns && !rest.starts_with("=~") || annotated)
                && !site.in_brackets
                && site.container != Some(Container::Block);
            binds.then_some(if head.is_empty() {
                Kind::Binding
            } else {
                Kind::Member
            })
        }
        Dialect::Kotlin => {
            let keyword = head
                .split_whitespace()
                .find(|word| !MODIFIERS.contains(word))
                .filter(|word| matches!(*word, "fun" | "val" | "var"))?;
            let declares = !head.contains(['=', ';', '{', '(', ')'])
                && (rest.starts_with(['(', ':']) || assigns);
            declares.then_some(if keyword == "fun" {
                Kind::Keyword
            } else {
                Kind::Binding
            })
        }
        Dialect::Go => {
            let pos = site.code.find(":=").filter(|pos| site.at < *pos)?;
            let lhs = &site.code[..pos];
            let mut list = lhs.trim_start();
            for keyword in ["for ", "if ", "switch ", "select "] {
                if let Some(rest) = list.strip_prefix(keyword) {
                    list = rest;
                    break;
                }
            }
            let start = lhs.len() - list.len();
            (site.at >= start && list.split(',').all(|name| is_name(name.trim())))
                .then_some(Kind::Binding)
        }
        Dialect::Shell => {
            let prefix = head.split(" -").next().unwrap_or(head);
            let binds = matches!(
                prefix,
                "" | "export" | "local" | "readonly" | "declare" | "typeset"
            ) && after.starts_with('=')
                && !after.starts_with("==")
                && !site.in_brackets;
            binds.then_some(Kind::Binding)
        }
        Dialect::Haskell => {
            let previous = head.split_whitespace().last().unwrap_or("");
            (head.is_empty() && rest.starts_with("::") || matches!(previous, "data" | "newtype"))
                .then_some(Kind::Keyword)
        }
        Dialect::Lisp => {
            let form = head.split_whitespace().last()?.strip_prefix('(')?;
            (form.starts_with("def")
                || form.starts_with("cl-def")
                || form == "define"
                || form.starts_with("define-")
                || form == "ns")
                .then_some(Kind::Keyword)
        }
        Dialect::Sql => sql_rule(head),
        Dialect::Prisma => matches!(head.split_whitespace().last(), Some("model" | "view"))
            .then_some(Kind::Keyword),
        Dialect::C => {
            let typedef_name = head == "}" && site.closed_typedef && rest.starts_with([';', ',']);
            let typedef_alias = head.starts_with("typedef ") && rest.starts_with(';');
            (matches!(head, "#define" | "# define") || typedef_name || typedef_alias)
                .then_some(Kind::Keyword)
        }
        _ => None,
    }
}

fn sql_rule(head: &str) -> Option<Kind> {
    let upper = head.to_ascii_uppercase();
    let mut words: Vec<&str> = upper.split_whitespace().collect();
    if words.first() != Some(&"CREATE") {
        return None;
    }
    if words.last().is_some_and(|word| word.ends_with('.')) {
        words.pop();
    }
    if words.ends_with(&["IF", "NOT", "EXISTS"]) {
        words.truncate(words.len() - 3);
    }
    matches!(
        words.last(),
        Some(
            &"TABLE"
                | &"VIEW"
                | &"FUNCTION"
                | &"PROCEDURE"
                | &"TRIGGER"
                | &"TYPE"
                | &"SCHEMA"
                | &"SEQUENCE"
                | &"INDEX"
                | &"DOMAIN"
        )
    )
    .then_some(Kind::Keyword)
}

fn keyword_before(head: &str, after: &str) -> Option<Kind> {
    let called = after.trim_start().starts_with(['(', '<']);
    if let Some(prefix) = head.strip_suffix("mut") {
        if prefix.ends_with(char::is_whitespace)
            && matches!(prefix.split_whitespace().last(), Some("let" | "static"))
        {
            return Some(Kind::Binding);
        }
    }
    let previous = head.rsplit(char::is_whitespace).next().unwrap_or("");
    let trimmed = previous.strip_suffix(['!', '*']).unwrap_or(previous);
    let keyword = trimmed.split('<').next().unwrap_or(trimmed);
    if DECLARING_KEYWORDS.contains(&keyword) {
        return Some(Kind::Keyword);
    }
    // `static find(` is a method; `static FIND =` a binding.
    if BINDING_KEYWORDS.contains(&keyword) && !called {
        return Some(Kind::Binding);
    }
    let owner = previous
        .strip_suffix('.')
        .or_else(|| previous.strip_suffix(':'))?;
    let earlier = head[..head.len() - previous.len()]
        .split_whitespace()
        .last();
    (is_name(owner) && matches!(earlier, Some("def" | "function" | "fun"))).then_some(Kind::Keyword)
}

fn container_rule(dialect: Dialect, site: &Site<'_>, head: &str, after: &str) -> Option<Kind> {
    let rest = after.trim_start();
    let assigns = rest.starts_with('=') && !rest.starts_with("==") && !rest.starts_with("=>");
    match (site.container?, dialect) {
        (Container::Enum, _) => {
            let head_fits = head.is_empty()
                || head.ends_with(['{', ','])
                || head == "case"
                || head.starts_with("#[") && head.ends_with(']');
            let rest_fits =
                rest.is_empty() || rest.starts_with([',', '(', '{', '}', ';']) || assigns;
            (head_fits && rest_fits).then_some(Kind::Keyword)
        }
        (Container::Struct, Dialect::Rust) => {
            let visible = head
                .strip_suffix("pub")
                .or_else(|| {
                    let open = head.rfind("pub(")?;
                    head.ends_with(')').then(|| &head[..open])
                })
                .unwrap_or(head)
                .trim_end();
            let head_fits = visible.is_empty() || visible.ends_with(['{', ',']);
            (head_fits && rest.starts_with(':') && !rest.starts_with("::")).then_some(Kind::Member)
        }
        (Container::Struct, Dialect::Go | Dialect::Prisma) => {
            let typed = after.starts_with([' ', '\t'])
                && rest.starts_with(|c: char| c.is_alphanumeric() || matches!(c, '_' | '*' | '['))
                || rest.starts_with(',');
            (head.is_empty() && typed).then_some(Kind::Member)
        }
        (Container::Class, Dialect::Go) => {
            (head.is_empty() && rest.starts_with('(')).then_some(Kind::Head)
        }
        (Container::Class, Dialect::Script | Dialect::Php) => {
            let head = head.strip_suffix('#').unwrap_or(head);
            if !head
                .split_whitespace()
                .all(|word| MEMBER_MODIFIERS.contains(&word))
            {
                return None;
            }
            let rest = rest.strip_prefix(['?', '!']).unwrap_or(rest);
            let declares = rest.is_empty()
                || rest.starts_with([';', ':'])
                || rest.starts_with('=') && !rest.starts_with("==") && !rest.starts_with("=>");
            declares.then_some(Kind::Member)
        }
        _ => None,
    }
}

fn typed_rule<'a>(
    dialect: Dialect,
    site: &Site<'_>,
    head: &str,
    after: &str,
    following: impl Iterator<Item = &'a str>,
) -> Option<Kind> {
    let head = strip_qualifier(dialect, head);
    if head.is_empty() || !typed_head(head) {
        return None;
    }
    let rest = signature_tail(after.trim_start())?;
    if rest.starts_with('(') {
        if site.container == Some(Container::Block) {
            return None;
        }
        let signature = signature(after, following);
        let tail = after_parameters(signature_tail(signature.trim_start())?)?;
        if tail.starts_with(';') {
            return Some(Kind::Head);
        }
        return DEFINITION_TAILS
            .iter()
            .any(|start| tail.starts_with(start))
            .then_some(Kind::Keyword);
    }
    let declares =
        rest.starts_with('=') && !rest.starts_with("==") || rest.starts_with([';', ',', '[']);
    declares.then_some(match site.container {
        Some(Container::Class | Container::Struct) => Kind::Member,
        _ => Kind::Binding,
    })
}

/// `[modifiers] Type<…>*&[]?` — a head that names a type and nothing else.
fn typed_head(head: &str) -> bool {
    if NOT_A_TYPE.contains(&first_word(head)) {
        return false;
    }
    let mut depth = 0usize;
    for (index, c) in head.char_indices() {
        match c {
            '<' => depth += 1,
            '>' => match depth.checked_sub(1) {
                Some(next) => depth = next,
                None => return false,
            },
            ',' if depth == 0 => return false,
            ':' if !(head[index..].starts_with("::") || head[..index].ends_with(':')) => {
                return false
            }
            '=' | '(' | ')' | ';' | '{' | '}' | '.' | '"' | '\'' | '+' | '-' | '!' | '|' | '^'
            | '%' | '/' | '@' | '#' | '\\' | '`' | '~' => return false,
            _ => {}
        }
    }
    depth == 0
        && head.ends_with(|c: char| {
            c.is_alphanumeric() || matches!(c, '_' | '*' | '&' | ']' | '>' | '?')
        })
        && head
            .split(|c: char| c.is_whitespace() || matches!(c, '*' | '&'))
            .map(keyword_token)
            .any(|word| !word.is_empty() && !TYPED_MODIFIERS.contains(&word))
}

fn head_rule<'a>(
    dialect: Dialect,
    container: Option<Container>,
    head: &str,
    after: &str,
    following: impl Iterator<Item = &'a str>,
) -> Option<Kind> {
    let head = if dialect == Dialect::Script {
        head.strip_suffix('#').unwrap_or(head).trim_end()
    } else {
        strip_qualifier(dialect, head)
    };
    let kind = signature_head_kind(head)?;
    if kind == Kind::Head && !dialect.bare_heads(container) {
        return None;
    }
    if !after.trim_start().starts_with(['(', '<', '?']) {
        return None;
    }
    let signature = signature(after, following);
    let rest = signature.trim_start();
    let rest = signature_tail(rest.strip_prefix('?').unwrap_or(rest))?;
    if !rest.starts_with('(') {
        return None;
    }
    if kind == Kind::Keyword {
        return Some(kind);
    }
    let tail = after_parameters(rest)?;
    let declared = tail.starts_with('{')
        || tail.starts_with(':')
        || dialect == Dialect::Shell && tail.starts_with('(')
        || dialect.c_like()
            && (DEFINITION_TAILS.iter().any(|start| tail.starts_with(start))
                || tail.starts_with(';')
                    && matches!(container, Some(Container::Class | Container::Struct)));
    declared.then_some(Kind::Head)
}

/// A keyword-led head (`func (r *Repo)`) or one of only modifiers.
fn signature_head_kind(head: &str) -> Option<Kind> {
    if head.contains(['{', '=', '.', ';']) {
        return None;
    }
    let mut words = head
        .split_whitespace()
        .map(keyword_token)
        .filter(|word| !word.is_empty());
    let first = words.next().unwrap_or("");
    if DECLARING_KEYWORDS.contains(&first) {
        return Some(Kind::Keyword);
    }
    let plain = head
        .chars()
        .all(|c| c.is_alphanumeric() || c.is_whitespace() || matches!(c, '_' | '*' | '~'));
    let modifiers =
        first.is_empty() || MODIFIERS.contains(&first) && words.all(|w| MODIFIERS.contains(&w));
    (plain && modifiers).then_some(Kind::Head)
}

/// `after` plus up to [`SIGNATURE_LINES`] following lines, within
/// [`MAX_SIGNATURE_BYTES`]. A line whose parameters close and continue is
/// whole on its own, so a call never reads further.
fn signature<'s, 'a>(after: &'s str, following: impl Iterator<Item = &'a str>) -> Cow<'s, str> {
    let rest = after.trim_start();
    let rest = rest.strip_prefix('?').unwrap_or(rest);
    if signature_tail(rest)
        .and_then(after_parameters)
        .is_some_and(|tail| !tail.is_empty())
    {
        return Cow::Borrowed(after);
    }
    let mut signature = after.to_string();
    for next in following.take(SIGNATURE_LINES) {
        if signature.len() + next.len() + 1 > MAX_SIGNATURE_BYTES {
            break;
        }
        signature.push('\n');
        signature.push_str(next);
    }
    Cow::Owned(signature)
}

/// What follows the parenthesized parameters at the start of `text`.
fn after_parameters(text: &str) -> Option<&str> {
    let mut depth = 0usize;
    for (index, c) in text.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(text[index + 1..].trim_start());
                }
            }
            _ => {}
        }
    }
    None
}

/// Skips generic parameters, which can nest and contain `=>`.
fn signature_tail(after: &str) -> Option<&str> {
    let after = after.trim_start();
    if !after.starts_with('<') {
        return Some(after);
    }
    let mut depth = 0usize;
    for (index, c) in after.char_indices() {
        match c {
            '<' => depth += 1,
            '>' if !after[..index].ends_with('=') => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(after[index + 1..].trim_start());
                }
            }
            _ => {}
        }
    }
    None
}

/// `@Injectable() export class` is `export class`.
fn strip_decorators(mut head: &str) -> &str {
    while let Some(rest) = head.strip_prefix('@') {
        let end = rest
            .find(|c: char| !(c.is_alphanumeric() || matches!(c, '_' | '.' | '$')))
            .unwrap_or(rest.len());
        if end == 0 {
            return head;
        }
        let mut rest = &rest[end..];
        if rest.starts_with('(') {
            let Some(tail) = after_parameters(rest) else {
                return head;
            };
            rest = tail;
        }
        head = rest.trim_start();
    }
    head
}

/// `std::string Widget::` is `std::string`: C++ defines members out of line.
fn strip_qualifier(dialect: Dialect, mut head: &str) -> &str {
    if dialect != Dialect::C {
        return head;
    }
    while let Some(rest) = head.strip_suffix("::") {
        let start = rest
            .rfind(|c: char| !(c.is_alphanumeric() || c == '_'))
            .map_or(0, |at| at + 1);
        if start == rest.len() {
            break;
        }
        head = rest[..start].trim_end();
    }
    head
}

/// The identifier inside a token: `impl<T>` is `impl`, `(` is nothing.
fn keyword_token(raw: &str) -> &str {
    let start = raw
        .find(|c: char| c.is_alphanumeric() || c == '_')
        .unwrap_or(raw.len());
    let rest = &raw[start..];
    let end = rest
        .find(|c: char| !(c.is_alphanumeric() || c == '_'))
        .unwrap_or(rest.len());
    &rest[..end]
}

pub(super) fn first_word(text: &str) -> &str {
    let text = text.trim_start();
    let end = text
        .find(|c: char| !(c.is_alphanumeric() || c == '_'))
        .unwrap_or(text.len());
    &text[..end]
}

fn is_name(text: &str) -> bool {
    !text.is_empty()
        && !text.starts_with(|c: char| c.is_ascii_digit())
        && text
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '$')
}
