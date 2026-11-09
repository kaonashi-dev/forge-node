//! What a ⌘-hover or ⌘-click on one line of output can open, as Tauri's
//! `terminal/links.ts`, `references/urlref.ts`, `pathref.ts` and
//! `screenLinks.ts`. Computed for the touched line only, never per frame.

use crate::terminal::pane::Viewport;

/// Rows a soft wrap is followed either way; the cap keeps a screen of wrapped
/// prose from being joined into one string on every hovered cell.
const WRAP_LIMIT: i64 = 8;

/// One logical line, and the cell each of its characters was painted in.
pub struct LineText {
    pub text: Vec<char>,
    pub cells: Vec<(i64, u16)>,
}

/// The line around absolute row `abs`, joined across a soft wrap it takes part in.
#[must_use]
pub fn line_at(viewport: &Viewport<'_>, abs: i64) -> LineText {
    let wraps = |line: i64| viewport.row_at_abs(line).is_some_and(|row| row.wrapped);
    let mut first = abs;
    while first > 0 && abs - first < WRAP_LIMIT && wraps(first - 1) {
        first -= 1;
    }
    let mut last = abs;
    while last - abs < WRAP_LIMIT && wraps(last) && viewport.row_at_abs(last + 1).is_some() {
        last += 1;
    }
    let mut line = LineText {
        text: Vec::new(),
        cells: Vec::new(),
    };
    for at in first..=last {
        let Some(row) = viewport.row_at_abs(at) else {
            continue;
        };
        for (col, cell) in row.cells.iter().enumerate() {
            if cell.flags.contains(domain::CellFlags::WIDE_SPACER) {
                continue;
            }
            let glyph = if cell.text.is_empty() {
                " "
            } else {
                cell.text.as_str()
            };
            for ch in glyph.chars() {
                line.text.push(ch);
                line.cells.push((at, col as u16));
            }
        }
    }
    line
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Url(String),
    /// Relative to `root`, the checkout path it was placed under.
    Path {
        root: String,
        path: String,
        line: Option<u32>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Link {
    pub target: Target,
    /// One inclusive column range per absolute row the link crosses.
    pub spans: Vec<(i64, u16, u16)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Found {
    from: usize,
    to: usize,
    target: Target,
}

/// The link covering the cell at (`abs`, `col`). `roots` are checkout paths,
/// the pane's own first: a relative path is read against it.
#[must_use]
pub fn link_at(viewport: &Viewport<'_>, abs: i64, col: u16, roots: &[String]) -> Option<Link> {
    let line = line_at(viewport, abs);
    let index = line.cells.iter().position(|&cell| cell == (abs, col))?;
    let found = assemble(&line.text, roots)
        .into_iter()
        .find(|found| index >= found.from && index < found.to)?;
    let mut spans: Vec<(i64, u16, u16)> = Vec::new();
    for &(row, col) in &line.cells[found.from..found.to.min(line.cells.len())] {
        match spans.last_mut() {
            Some(open) if open.0 == row => open.2 = open.2.max(col),
            _ => spans.push((row, col, col)),
        }
    }
    Some(Link {
        target: found.target,
        spans,
    })
}

fn assemble(text: &[char], roots: &[String]) -> Vec<Found> {
    let mut links = find_urls(text);
    let urls = links.len();
    for path in find_paths(text, roots) {
        let overlaps = links[..urls]
            .iter()
            .any(|url| path.from < url.to && url.from < path.to);
        if !overlaps {
            links.push(path);
        }
    }
    links.sort_by_key(|link| link.from);
    links
}

const URL_STOP: [char; 7] = ['<', '>', '"', '\'', '`', ')', ']'];
const TRAILING: [char; 6] = ['.', ',', ';', ':', '!', '?'];

fn starts_with_ignore_case(text: &[char], at: usize, prefix: &str) -> bool {
    let mut i = at;
    for p in prefix.chars() {
        match text.get(i) {
            Some(c) if c.eq_ignore_ascii_case(&p) => i += 1,
            _ => return false,
        }
    }
    true
}

fn find_urls(text: &[char]) -> Vec<Found> {
    let mut found = Vec::new();
    let mut at = 0;
    while at < text.len() {
        let scheme = if starts_with_ignore_case(text, at, "https://") {
            8
        } else if starts_with_ignore_case(text, at, "http://") {
            7
        } else {
            at += 1;
            continue;
        };
        let mut end = at + scheme;
        while end < text.len() && !text[end].is_whitespace() && !URL_STOP.contains(&text[end]) {
            end += 1;
        }
        let mut body_end = end;
        while body_end > at + scheme && TRAILING.contains(&text[body_end - 1]) {
            body_end -= 1;
        }
        let host = text.get(at + scheme).is_some_and(|c| *c != '/');
        if host && body_end > at + scheme {
            let raw: String = text[at..body_end].iter().collect();
            let url = format!("{}{}", raw[..scheme].to_ascii_lowercase(), &raw[scheme..]);
            found.push(Found {
                from: at,
                to: body_end,
                target: Target::Url(url),
            });
        }
        at = end.max(at + 1);
    }
    found
}

const SEPARATORS: [char; 21] = [
    ' ', '\t', '"', '\'', '`', '(', ')', '[', ']', '{', '}', '<', '>', ',', ';', '|', '&', '=',
    '*', '\u{a0}', '\0',
];
const PATH_TRAILING: [char; 8] = ['.', ',', ';', ':', '!', '?', '\'', '"'];

fn find_paths(text: &[char], roots: &[String]) -> Vec<Found> {
    let mut found = Vec::new();
    let mut at = 0;
    while at < text.len() {
        if SEPARATORS.contains(&text[at]) {
            at += 1;
            continue;
        }
        let mut end = at;
        while end < text.len() && !SEPARATORS.contains(&text[end]) {
            end += 1;
        }
        let token: String = text[at..end].iter().collect();
        if let Some((len, target)) = parse_token(&token, roots) {
            found.push(Found {
                from: at,
                to: at + len,
                target,
            });
        }
        at = end;
    }
    found
}

/// A token as a checkout-relative path; `len` is in characters, trailing
/// sentence punctuation excluded.
fn parse_token(raw: &str, roots: &[String]) -> Option<(usize, Target)> {
    let trimmed = raw.trim_end_matches(PATH_TRAILING);
    if trimmed.is_empty() || trimmed.starts_with('-') {
        return None;
    }
    let (body, line) = split_line(trimmed);
    if has_scheme(body) && !body.starts_with("file://") {
        return None;
    }
    let explicit = ["./", "/", "~/", "file://"]
        .iter()
        .any(|prefix| body.starts_with(prefix));
    let (root, path) = place(body, roots)?;
    if !explicit && !looks_like_file(&path) {
        return None;
    }
    Some((trimmed.chars().count(), Target::Path { root, path, line }))
}

/// `path#L12`, `path:12` and `path:12:5`; the column is dropped.
fn split_line(token: &str) -> (&str, Option<u32>) {
    if let Some(at) = token.rfind("#L").or_else(|| token.rfind("#l"))
        && at > 0
        && let Ok(line) = token[at + 2..].parse::<u32>()
    {
        return (&token[..at], Some(line));
    }
    let mut parts = token.rsplitn(3, ':');
    let last = parts.next();
    let middle = parts.next();
    let rest = parts.next();
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    match (rest, middle, last) {
        (Some(body), Some(line), Some(col)) if !body.is_empty() && digits(line) && digits(col) => {
            (body, line.parse().ok())
        }
        (_, Some(_), Some(line)) if digits(line) => {
            let body = &token[..token.len() - line.len() - 1];
            if body.is_empty() {
                (token, None)
            } else {
                (body, line.parse().ok())
            }
        }
        _ => (token, None),
    }
}

fn has_scheme(body: &str) -> bool {
    let Some(at) = body.find("://") else {
        return false;
    };
    let scheme = &body[..at];
    scheme
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic())
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-'))
}

/// Without this every `cargo check/clippy`, `@scope/package` and `1.4.1` in
/// a sentence would be a link; a slash has already shown its hand.
fn looks_like_file(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    let Some(dot) = name.rfind('.') else {
        return false;
    };
    let extension = &name[dot + 1..];
    let mut chars = extension.chars();
    let shaped = chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && extension.len() <= 10
        && chars.all(|c| c.is_ascii_alphanumeric());
    shaped && (path.contains('/') || extension.len() >= 2)
}

fn segments(path: &str) -> Vec<&str> {
    path.split('/').filter(|part| !part.is_empty()).collect()
}

fn clean(path: &str) -> Option<String> {
    let parts = segments(path);
    (!parts.is_empty() && !parts.contains(&"..")).then(|| parts.join("/"))
}

fn trim_root(root: &str) -> &str {
    if root.len() > 1 {
        root.trim_end_matches('/')
    } else {
        root
    }
}

/// A reference rewritten against the checkout it names: a relative path stays
/// on the first root, a `~/` or absolute one may name any root, longest first.
fn place(reference: &str, roots: &[String]) -> Option<(String, String)> {
    let mut value = reference.strip_prefix("file://").unwrap_or(reference);
    while let Some(rest) = value.strip_prefix("./") {
        value = rest;
    }
    if let Some(home) = value.strip_prefix("~/") {
        let parts = segments(home);
        let mut best: Option<(usize, &str, Option<String>)> = None;
        for root in roots {
            let root = trim_root(root);
            let root_parts = segments(root);
            for take in (1..=root_parts.len().min(parts.len())).rev() {
                if root_parts[root_parts.len() - take..] == parts[..take] {
                    if best.as_ref().is_none_or(|(score, ..)| take > *score) {
                        best = Some((take, root, clean(&parts[take..].join("/"))));
                    }
                    break;
                }
            }
        }
        let (_, root, path) = best?;
        return Some((root.to_string(), path?));
    }
    if value.starts_with('/') {
        let mut best: Option<(usize, &str, String)> = None;
        for root in roots {
            let root = trim_root(root);
            let Some(rest) = value
                .strip_prefix(root)
                .and_then(|rest| rest.strip_prefix('/'))
            else {
                continue;
            };
            let Some(path) = clean(rest) else {
                continue;
            };
            let score = segments(root).len();
            if best.as_ref().is_none_or(|(best, ..)| score > *best) {
                best = Some((score, root, path));
            }
        }
        return best.map(|(_, root, path)| (root.to_string(), path));
    }
    let root = trim_root(roots.first()?);
    Some((root.to_string(), clean(value)?))
}

/// A path as one shell word: single-quoted, refused with a control character.
#[must_use]
pub fn shell_quote(path: &str) -> Option<String> {
    if path.is_empty() || path.chars().any(char::is_control) {
        return None;
    }
    Some(format!("'{}'", path.replace('\'', "'\\''")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::pane::Viewport;
    use client::CellGrid;
    use domain::{Cell, PtySize, Row, TerminalSnapshot};
    use std::sync::Arc;

    fn chars(text: &str) -> Vec<char> {
        text.chars().collect()
    }

    fn roots() -> Vec<String> {
        vec![
            "/Users/x/dev/forge".to_string(),
            "/Users/x/dev/forge/.worktrees/feat".to_string(),
        ]
    }

    fn targets(text: &str) -> Vec<(String, Target)> {
        let text = chars(text);
        assemble(&text, &roots())
            .into_iter()
            .map(|found| (text[found.from..found.to].iter().collect(), found.target))
            .collect()
    }

    fn path(root: &str, path: &str, line: Option<u32>) -> Target {
        Target::Path {
            root: root.to_string(),
            path: path.to_string(),
            line,
        }
    }

    #[test]
    fn urls_drop_sentence_punctuation_and_win_over_paths() {
        assert_eq!(
            targets("see https://example.com/a?b=1. ok"),
            vec![(
                "https://example.com/a?b=1".to_string(),
                Target::Url("https://example.com/a?b=1".to_string())
            )]
        );
        assert_eq!(
            targets("HTTP://localhost:5353/"),
            vec![(
                "HTTP://localhost:5353/".to_string(),
                Target::Url("http://localhost:5353/".to_string())
            )]
        );
        assert!(targets("http:// nothing").is_empty(), "no host, no address");
        assert!(targets("(https://x.dev)")[0].0 == "https://x.dev");
    }

    #[test]
    fn paths_need_an_extension_unless_they_are_written_as_paths() {
        let root = "/Users/x/dev/forge";
        assert_eq!(
            targets("error at src/main.rs:12:5, fix it"),
            vec![(
                "src/main.rs:12:5".to_string(),
                path(root, "src/main.rs", Some(12))
            )]
        );
        assert_eq!(
            targets("README.md#L40"),
            vec![(
                "README.md#L40".to_string(),
                path(root, "README.md", Some(40))
            )]
        );
        assert!(targets("cargo check/clippy 1.4.1 e.g").is_empty());
        assert_eq!(
            targets("./scripts/dev")[0].1,
            path(root, "scripts/dev", None)
        );
        assert!(targets("../etc/passwd.txt").is_empty(), "never climbs out");
        assert!(targets("--flag.rs").is_empty());
    }

    #[test]
    fn absolute_and_home_paths_pick_the_longest_root() {
        assert_eq!(
            targets("/Users/x/dev/forge/.worktrees/feat/a/b.ts")[0].1,
            path("/Users/x/dev/forge/.worktrees/feat", "a/b.ts", None)
        );
        assert_eq!(
            targets("~/dev/forge/apps/ui/x.rs")[0].1,
            path("/Users/x/dev/forge", "apps/ui/x.rs", None)
        );
        assert!(
            targets("/etc/hosts.conf").is_empty(),
            "outside every checkout"
        );
    }

    #[test]
    fn a_shell_word_is_single_quoted() {
        assert_eq!(shell_quote("/a b/it's").unwrap(), "'/a b/it'\\''s'");
        assert_eq!(shell_quote("/a\nb"), None);
    }

    fn row(text: &str, cols: usize, wrapped: bool) -> Row {
        let mut cells = vec![Cell::default(); cols];
        for (i, c) in text.chars().take(cols).enumerate() {
            cells[i].text = c.to_string().into();
        }
        Row {
            cells: Arc::new(cells),
            wrapped,
        }
    }

    #[test]
    fn a_wrapped_path_is_one_link_across_two_rows() {
        let snapshot = TerminalSnapshot {
            seq: 1,
            size: PtySize {
                cols: 10,
                rows: 3,
                pixel_width: 0,
                pixel_height: 0,
            },
            visible: vec![
                row("$ cat src/", 10, true),
                row("main.rs ok", 10, false),
                row("", 10, false),
            ],
            scrollback_tail: Vec::new(),
            scrollback_len: 0,
            scrollback_generation: 1,
            cursor: Default::default(),
            modes: Default::default(),
            title: None,
        };
        let grid = CellGrid::from_snapshot(&snapshot);
        let viewport = Viewport {
            grid: &grid,
            scroll: 0,
        };
        let link = link_at(&viewport, 1, 2, &roots()).expect("a link under the cell");
        assert_eq!(link.target, path("/Users/x/dev/forge", "src/main.rs", None));
        assert_eq!(link.spans, vec![(0, 6, 9), (1, 0, 6)]);
        assert!(link_at(&viewport, 1, 9, &roots()).is_none());
    }
}
