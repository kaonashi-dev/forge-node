//! Which kind of `{ … }` body a masked line sits in, from the words before
//! each opening brace. Approximate by design: an unbalanced brace in an
//! unmasked regex only blurs the kinds, it never hides a keyword declaration.

use super::rank::{first_word, Dialect};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Container {
    /// A function body, a control-flow block, an object or dict literal.
    Block,
    Enum,
    Struct,
    /// A class, interface or object type body: its members are declarations.
    Class,
    /// A body whose items are as visible as top-level ones (`impl`, `namespace`).
    Module,
    /// The braces of `import { … }`, `export { … }` or `use a::{ … }`.
    Import,
}

/// An open brace, named by the head before it only when asked: most bodies
/// are never asked about, and naming one reads its head word by word.
#[derive(Clone, Copy)]
struct Frame {
    head: (usize, usize),
    parens: usize,
}

const MAX_FRAMES: usize = 64;
const MAX_HEAD_BYTES: usize = 256;

#[derive(Clone, Default)]
pub(super) struct Scope {
    frames: Vec<Frame>,
    overflow: usize,
    /// Where the code since the last `;`, `{` or `}` starts: what an opening
    /// brace belongs to. An offset, so no line is ever copied to keep it.
    head_start: usize,
    parens: usize,
    last_closed: Option<(usize, usize)>,
}

impl Scope {
    /// The innermost body, or `None` at the top level. `text` is the masked
    /// file this scope was fed.
    pub(super) fn container(&self, text: &str, dialect: Dialect) -> Option<Container> {
        if self.overflow > 0 {
            return Some(Container::Block);
        }
        let (start, end) = self.frames.last()?.head;
        Some(classify(head(text, start, end), dialect))
    }

    pub(super) fn in_brackets(&self) -> bool {
        self.parens > 0
    }

    /// Whether the last `}` closed a `typedef struct { … }`.
    pub(super) fn closed_typedef(&self, text: &str) -> bool {
        self.last_closed
            .is_some_and(|(start, end)| first_word(&text[start..end]) == "typedef")
    }

    /// Reads `text[from..to]`, where `text` is the whole masked file.
    pub(super) fn feed(&mut self, text: &str, from: usize, to: usize) {
        for (index, byte) in text.as_bytes()[from..to].iter().enumerate() {
            let at = from + index;
            match byte {
                b'{' => {
                    let frame = Frame {
                        head: (self.head_start, at),
                        parens: self.parens,
                    };
                    self.parens = 0;
                    if self.frames.len() < MAX_FRAMES && self.overflow == 0 {
                        self.frames.push(frame);
                    } else {
                        self.overflow += 1;
                    }
                    self.head_start = at + 1;
                }
                b'}' => {
                    if self.overflow > 0 {
                        self.overflow -= 1;
                    } else if let Some(frame) = self.frames.pop() {
                        self.parens = frame.parens;
                        self.last_closed = Some(frame.head);
                    }
                    self.head_start = at + 1;
                }
                b';' => self.head_start = at + 1,
                b'(' | b'[' => self.parens += 1,
                b')' | b']' => self.parens = self.parens.saturating_sub(1),
                _ => {}
            }
        }
    }

    /// Languages without `;` end a statement at a newline outside brackets.
    pub(super) fn end_line(&mut self, next_line: usize, newline_ends_statement: bool) {
        if newline_ends_statement && self.parens == 0 {
            self.head_start = next_line;
        }
    }
}

/// The last [`MAX_HEAD_BYTES`] of `text[start..end]`.
fn head(text: &str, start: usize, end: usize) -> &str {
    let mut start = start.max(end.saturating_sub(MAX_HEAD_BYTES)).min(end);
    while !text.is_char_boundary(start) {
        start += 1;
    }
    &text[start..end]
}

fn classify(head: &str, dialect: Dialect) -> Container {
    let last = head
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("");
    let words = || {
        head.split(|c: char| !(c.is_alphanumeric() || c == '_'))
            .filter(|word| !word.is_empty())
    };
    if is_import_head(last) {
        Container::Import
    } else if let Some(container) = words().find_map(|word| keyword_container(word, dialect)) {
        // `struct point *make(void) {` is a function returning a struct.
        if matches!(container, Container::Enum | Container::Struct)
            && dialect.c_like()
            && head.contains('(')
        {
            Container::Block
        } else {
            container
        }
    } else if dialect == Dialect::Script
        && matches!(
            first_word(
                last.trim_start_matches("export ")
                    .trim_start_matches("declare ")
            ),
            "type"
        )
        && last.contains('=')
    {
        Container::Class
    } else if words().any(|word| matches!(word, "fn" | "function" | "func" | "def" | "fun")) {
        Container::Block
    } else if words().any(|word| {
        matches!(
            word,
            "impl" | "trait" | "mod" | "namespace" | "module" | "extension" | "extern" | "global"
        )
    }) {
        Container::Module
    } else {
        Container::Block
    }
}

fn keyword_container(word: &str, dialect: Dialect) -> Option<Container> {
    match word {
        "enum" => Some(Container::Enum),
        "struct" | "union" => Some(Container::Struct),
        "model" if dialect == Dialect::Prisma => Some(Container::Struct),
        "class" | "interface" | "mixin" => Some(Container::Class),
        "object" if matches!(dialect, Dialect::Kotlin | Dialect::Scala) => Some(Container::Class),
        "protocol" if dialect == Dialect::Swift => Some(Container::Class),
        "record" if dialect.c_like() => Some(Container::Class),
        _ => None,
    }
}

fn is_import_head(last: &str) -> bool {
    let last = last
        .strip_prefix("pub ")
        .or_else(|| {
            last.strip_prefix("pub(")
                .and_then(|rest| rest.split_once(')'))
                .map(|(_, rest)| rest.trim_start())
        })
        .unwrap_or(last);
    matches!(first_word(last), "import" | "use") || matches!(last, "export" | "export type")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn containers(dialect: Dialect, text: &str) -> Vec<Option<Container>> {
        let mut scope = Scope::default();
        let mut offset = 0;
        text.split_inclusive('\n')
            .map(|line| {
                let container = scope.container(text, dialect);
                scope.feed(text, offset, offset + line.trim_end_matches('\n').len());
                offset += line.len();
                scope.end_line(offset, false);
                container
            })
            .collect()
    }

    #[test]
    fn bodies_are_named_by_the_words_before_their_brace() {
        use Container::*;
        assert_eq!(
            containers(
                Dialect::Rust,
                "pub enum Shape {\n  A,\n}\nimpl<'a> P<'a>\nwhere T: X,\n{\n  fn f() -> impl Iterator {\n    x\n  }\n}\nuse a::{\n  B,\n};\nstruct S {\n  x: u8,\n}"
            ),
            [
                None,
                Some(Enum),
                Some(Enum),
                None,
                None,
                None,
                Some(Module),
                Some(Block),
                Some(Block),
                Some(Module),
                None,
                Some(Import),
                Some(Import),
                None,
                Some(Struct),
                Some(Struct)
            ]
        );
        assert_eq!(
            containers(
                Dialect::Script,
                "import {\n  type Session,\n} from 'x'\ntype Props = {\n  name: string\n}\nexport class A {\n  b = 1\n}"
            ),
            [
                None,
                Some(Import),
                Some(Import),
                None,
                Some(Class),
                Some(Class),
                None,
                Some(Class),
                Some(Class)
            ]
        );
    }

    #[test]
    fn a_c_typedef_is_remembered_when_its_body_closes() {
        let text = "typedef struct {\n} Point;\nstruct point *make(void) {";
        let mut scope = Scope::default();
        scope.feed(text, 0, 16);
        assert_eq!(scope.container(text, Dialect::C), Some(Container::Struct));
        scope.feed(text, 16, 25);
        assert!(scope.closed_typedef(text));
        scope.feed(text, 25, text.len());
        assert_eq!(scope.container(text, Dialect::C), Some(Container::Block));
    }

    #[test]
    fn nesting_past_the_frame_cap_still_balances() {
        let text = format!(
            "{}{}",
            "{".repeat(MAX_FRAMES + 10),
            "}".repeat(MAX_FRAMES + 10)
        );
        let mut scope = Scope::default();
        scope.feed(&text, 0, MAX_FRAMES + 10);
        assert_eq!(
            scope.container(&text, Dialect::Other),
            Some(Container::Block)
        );
        scope.feed(&text, MAX_FRAMES + 10, text.len());
        assert_eq!(scope.container(&text, Dialect::Other), None);
    }
}
