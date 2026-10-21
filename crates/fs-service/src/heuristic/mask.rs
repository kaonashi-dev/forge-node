//! Blanks comments and string contents so a word inside either is never read
//! as code. Masking is length-preserving and keeps every `\r` and `\n`, so the
//! masked text splits into the same lines at the same byte offsets.

use languages::Language;

/// How one language spells comments and strings.
pub(super) struct Syntax<'a> {
    line_comments: Vec<&'a str>,
    block_comments: Vec<(&'a str, &'a str)>,
    quotes: Vec<u8>,
    rust: bool,
    go: bool,
    php: bool,
    /// An unknown language: `#` starts a comment only before a blank, so
    /// `#define` and `#[attr]` stay code.
    spaced_hash: bool,
    /// Bytes that can begin a comment, a string or a character literal:
    /// everything else in code is skipped without a look.
    starts: [bool; 256],
}

impl<'a> Syntax<'a> {
    pub(super) fn of(language: Option<&'a Language>) -> Syntax<'a> {
        let Some(language) = language else {
            return Syntax {
                line_comments: vec!["//"],
                block_comments: vec![("/*", "*/")],
                quotes: vec![b'"', b'\''],
                rust: false,
                go: false,
                php: false,
                spaced_hash: true,
                starts: [false; 256],
            }
            .indexed();
        };
        Syntax {
            line_comments: language.line_comments.iter().map(String::as_str).collect(),
            block_comments: language
                .block_comments
                .iter()
                .map(|(open, close)| (open.as_str(), close.as_str()))
                .collect(),
            quotes: language
                .quotes
                .iter()
                .filter_map(|quote| match quote.as_bytes() {
                    [byte] => Some(*byte),
                    _ => None,
                })
                .collect(),
            rust: language.id == "rust",
            go: language.id == "go",
            php: language.id == "php",
            spaced_hash: false,
            starts: [false; 256],
        }
        .indexed()
    }

    fn indexed(mut self) -> Self {
        let firsts = self
            .line_comments
            .iter()
            .chain(self.block_comments.iter().map(|(open, _)| open))
            .filter_map(|marker| marker.bytes().next())
            .chain(self.quotes.iter().copied())
            .chain([b'\''])
            .chain(self.spaced_hash.then_some(b'#'))
            .chain(if self.rust { &b"rbc"[..] } else { &[] }.iter().copied());
        for byte in firsts {
            self.starts[usize::from(byte)] = true;
        }
        self
    }
}

#[derive(Clone, Copy)]
enum State {
    Code,
    Block {
        index: usize,
        depth: u32,
    },
    Str {
        quote: u8,
        triple: bool,
        raw: Option<usize>,
    },
}

pub(super) fn mask(text: &str, syntax: &Syntax<'_>) -> String {
    let bytes = text.as_bytes();
    let mut out = bytes.to_vec();
    let mut state = State::Code;
    let mut i = 0;
    while i < bytes.len() {
        match state {
            State::Code => {
                i += next(&bytes[i..], |b| syntax.starts[usize::from(b)]);
                let Some(&byte) = bytes.get(i) else {
                    break;
                };
                if let Some(index) = block_opener(bytes, i, syntax) {
                    let open = syntax.block_comments[index].0.len();
                    blank(&mut out[i..i + open]);
                    state = State::Block { index, depth: 1 };
                    i += open;
                    continue;
                }
                if line_comment(bytes, i, syntax) {
                    let end = i + next(&bytes[i..], |b| b == b'\n');
                    blank(&mut out[i..end]);
                    i = end;
                    continue;
                }
                if syntax.rust {
                    if let Some((prefix, hashes)) = rust_raw_string(bytes, i) {
                        state = State::Str {
                            quote: b'"',
                            triple: false,
                            raw: Some(hashes),
                        };
                        i += prefix;
                        continue;
                    }
                }
                if syntax.quotes.contains(&byte) {
                    let triple = !syntax.rust && bytes[i..].starts_with(&[byte, byte, byte]);
                    state = State::Str {
                        quote: byte,
                        triple,
                        raw: (syntax.go && byte == b'`').then_some(0),
                    };
                    i += if triple { 3 } else { 1 };
                    continue;
                }
                if byte == b'\'' {
                    if let Some(len) = char_literal(&text[i..]) {
                        blank(&mut out[i + 1..i + len - 1]);
                        i += len;
                        continue;
                    }
                }
                i += 1;
            }
            State::Block { index, depth } => {
                let (open, close) = syntax.block_comments[index];
                let (open, close) = (open.as_bytes(), close.as_bytes());
                let run = next(&bytes[i..], |b| {
                    b == close[0] || syntax.rust && b == open[0]
                });
                blank(&mut out[i..i + run]);
                i += run;
                if i >= bytes.len() {
                    break;
                }
                if bytes[i..].starts_with(close) {
                    blank(&mut out[i..i + close.len()]);
                    i += close.len();
                    state = if depth > 1 {
                        State::Block {
                            index,
                            depth: depth - 1,
                        }
                    } else {
                        State::Code
                    };
                } else if syntax.rust && bytes[i..].starts_with(open) {
                    blank(&mut out[i..i + open.len()]);
                    i += open.len();
                    state = State::Block {
                        index,
                        depth: depth + 1,
                    };
                } else {
                    blank(&mut out[i..=i]);
                    i += 1;
                }
            }
            State::Str { quote, triple, raw } => {
                let run = next(&bytes[i..], |b| {
                    b == quote || b == b'\n' || raw.is_none() && b == b'\\'
                });
                blank(&mut out[i..i + run]);
                i += run;
                let Some(&byte) = bytes.get(i) else {
                    break;
                };
                if byte == b'\\' {
                    // Blank the escaped character whole: cutting a UTF-8
                    // sequence would make the masked text longer than the
                    // source and shift every later offset.
                    let escaped = text[i + 1..].chars().next().map_or(0, char::len_utf8);
                    let end = i + 1 + escaped;
                    blank(&mut out[i..end]);
                    i = end;
                    continue;
                }
                if byte == quote {
                    let close = match raw {
                        Some(hashes) => {
                            let tail = &bytes[i + 1..];
                            (tail.len() >= hashes && tail[..hashes].iter().all(|&b| b == b'#'))
                                .then_some(1 + hashes)
                        }
                        None if triple => {
                            bytes[i..].starts_with(&[quote, quote, quote]).then_some(3)
                        }
                        None => Some(1),
                    };
                    if let Some(close) = close {
                        i += close;
                        state = State::Code;
                    } else {
                        blank(&mut out[i..=i]);
                        i += 1;
                    }
                    continue;
                }
                // An unclosed single-line string must not swallow the file.
                if !(triple || raw.is_some() || quote == b'`' || syntax.rust) {
                    state = State::Code;
                }
                i += 1;
            }
        }
    }
    String::from_utf8(out)
        .unwrap_or_else(|error| String::from_utf8_lossy(error.as_bytes()).into_owned())
}

/// Bytes before the first one `stop` accepts, or all of them.
fn next(bytes: &[u8], stop: impl Fn(u8) -> bool) -> usize {
    bytes.iter().position(|&b| stop(b)).unwrap_or(bytes.len())
}

fn blank(bytes: &mut [u8]) {
    for byte in bytes {
        if *byte != b'\n' && *byte != b'\r' {
            *byte = b' ';
        }
    }
}

fn block_opener(bytes: &[u8], at: usize, syntax: &Syntax<'_>) -> Option<usize> {
    syntax.block_comments.iter().position(|(open, _)| {
        bytes[at..].starts_with(open.as_bytes())
            // Ruby's `=begin` only opens a comment in the first column.
            && (!open.starts_with('=') || at == 0 || bytes[at - 1] == b'\n')
    })
}

fn line_comment(bytes: &[u8], at: usize, syntax: &Syntax<'_>) -> bool {
    let rest = &bytes[at..];
    if syntax.spaced_hash && rest[0] == b'#' {
        return rest.get(1).is_none_or(|next| next.is_ascii_whitespace());
    }
    syntax.line_comments.iter().any(|marker| {
        rest.starts_with(marker.as_bytes())
            && (*marker != "#"
                // `$#` and `${#list}` are shell expansions; `#[Attr]` is PHP 8.
                || !(at > 0 && matches!(bytes[at - 1], b'$' | b'{')
                    || syntax.php && rest.get(1) == Some(&b'[')))
    })
}

/// `r"…"`, `r#"…"#`, `br"…"`: the bytes before the opening quote and the
/// number of `#`s the closing quote needs.
fn rust_raw_string(bytes: &[u8], at: usize) -> Option<(usize, usize)> {
    let word = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    let mut i = at;
    if matches!(bytes[i], b'b' | b'c') {
        i += 1;
    }
    if bytes.get(i) != Some(&b'r') || at > 0 && word(bytes[at - 1]) {
        return None;
    }
    i += 1;
    let hashes = bytes[i..].iter().take_while(|&&b| b == b'#').count();
    (bytes.get(i + hashes) == Some(&b'"')).then_some((i + hashes + 1 - at, hashes))
}

/// The length of a character literal starting at `text[0] == '\''`, when
/// the quote is one. Where `'` is not a string quote (Rust, Swift, Lisp) it is
/// also a lifetime, a label or a quoted symbol, which never close this close.
fn char_literal(text: &str) -> Option<usize> {
    let mut chars = text[1..].char_indices();
    let (_, first) = chars.next()?;
    if first == '\\' {
        let escaped = text[2..].chars().next()?.len_utf8();
        return text[2 + escaped..]
            .char_indices()
            .take(10)
            .find(|(_, c)| *c == '\'')
            .map(|(at, _)| 2 + escaped + at + 1);
    }
    if first == '\'' || first == '\n' {
        return None;
    }
    let (at, second) = chars.next()?;
    (second == '\'').then_some(at + 2)
}

#[cfg(test)]
mod tests {
    use super::*;
    use languages::Registry;

    fn masked(path: &str, text: &str) -> String {
        let syntax = Syntax::of(Registry::builtin().language_for_path(path));
        let out = mask(text, &syntax);
        assert_eq!(out.len(), text.len());
        out
    }

    #[test]
    fn comments_and_strings_become_blanks_on_the_same_lines() {
        assert_eq!(
            masked("a.ts", "a /* b\nc */ d // e\r\nf 'g' \"h\" `i\nj` k"),
            "a     \n     d     \r\nf ' ' \" \" ` \n ` k"
        );
        assert_eq!(
            masked("a.py", "x = 1  # y\n\"\"\"\ndef ghost():\n\"\"\"\nz"),
            "x = 1     \n\"\"\"\n            \n\"\"\"\nz"
        );
        assert_eq!(masked("a.rb", "=begin\nx\n=end\ny"), "      \n \n    \ny");
        assert_eq!(masked("a.lua", "--[[ x\n]] y -- z"), "      \n   y     ");
    }

    #[test]
    fn rust_lifetimes_stay_code_and_char_literals_do_not() {
        assert_eq!(
            masked("a.rs", "impl<'a> Parser<'a> { '{' '\\'' b'\"' }"),
            "impl<'a> Parser<'a> { ' ' '  ' b' ' }"
        );
        assert_eq!(masked("a.rs", "fn f<'a, 'b>()"), "fn f<'a, 'b>()");
        assert_eq!(
            masked("a.rs", "r#\"x \" y\"# /* a /* b */ c */ z"),
            "r#\"     \"#                   z"
        );
        assert_eq!(masked("a.rs", "\"a\nb\" c"), "\" \n \" c");
    }

    #[test]
    fn hashes_are_comments_only_where_the_language_says_so() {
        assert_eq!(masked("a.c", "#define X 1"), "#define X 1");
        assert_eq!(masked("a.ts", "#secret() {}"), "#secret() {}");
        assert_eq!(masked("a.sh", "echo $# # n"), "echo $#    ");
        assert_eq!(masked("Makefile", "#x\n# y\n#[a]"), "#x\n   \n#[a]");
    }

    #[test]
    fn an_unclosed_single_line_string_ends_at_the_line() {
        assert_eq!(masked("a.c", "x = \"abc\ny"), "x = \"   \ny");
        assert_eq!(masked("a.go", "`a\\`b"), "`  `b");
    }
}
