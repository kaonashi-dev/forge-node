//! What a pull request's description, or a review's instructions, declares out
//! of scope: reviewers are told not to report it.

use crate::clip;

pub const MAX_SCOPE_BYTES: usize = 2 * 1024;

/// Matched case-insensitively anywhere in a heading or a bullet.
const MARKERS: [&str; 7] = [
    "non-goal",
    "non goal",
    "nongoal",
    "out of scope",
    "out-of-scope",
    "no hacemos",
    "fuera de alcance",
];

/// Every section whose heading names a non-goal, and every bullet that does with
/// what is nested under it, in order, at most [`MAX_SCOPE_BYTES`] cut on a line.
/// `None` when the text declares nothing out of scope.
#[must_use]
pub fn acknowledged_scope(text: &str) -> Option<String> {
    let lines: Vec<&str> = text.lines().collect();
    let mut kept: Vec<&str> = Vec::new();
    let mut at = 0;
    while at < lines.len() {
        let line = lines[at];
        at += 1;
        if !marked(line) {
            continue;
        }
        if let Some(level) = heading_level(line) {
            kept.push(line.trim());
            while at < lines.len() && heading_level(lines[at]).is_none_or(|next| next > level) {
                kept.push(lines[at].trim_end());
                at += 1;
            }
        } else if let Some(indent) = bullet_indent(line) {
            kept.push(line.trim_end());
            while at < lines.len() && indent_of(lines[at]) > indent && !lines[at].trim().is_empty()
            {
                kept.push(lines[at].trim_end());
                at += 1;
            }
        } else if line.trim_end().ends_with(':') || line.trim().starts_with("**") {
            kept.push(line.trim());
            while at < lines.len()
                && heading_level(lines[at]).is_none()
                && (lines[at].trim().is_empty()
                    || bullet_indent(lines[at]).is_some()
                    || indent_of(lines[at]) > 0)
            {
                kept.push(lines[at].trim_end());
                at += 1;
            }
        }
        kept.push("");
    }
    let mut out = String::new();
    for line in kept {
        if line.is_empty() && (out.is_empty() || out.ends_with("\n\n")) {
            continue;
        }
        if out.len() + line.len() + 1 > MAX_SCOPE_BYTES {
            if out.is_empty() {
                out.push_str(clip(line, MAX_SCOPE_BYTES));
            }
            break;
        }
        out.push_str(line);
        out.push('\n');
    }
    let out = out.trim_end();
    (!out.is_empty()).then(|| out.to_owned())
}

fn marked(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    MARKERS.iter().any(|marker| lower.contains(marker))
}

fn heading_level(line: &str) -> Option<usize> {
    let hashes = line.bytes().take_while(|b| *b == b'#').count();
    ((1..=6).contains(&hashes) && line[hashes..].starts_with(' ')).then_some(hashes)
}

fn indent_of(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

fn bullet_indent(line: &str) -> Option<usize> {
    let rest = line.trim_start();
    let numbered = rest.split_once(". ").is_some_and(|(number, _)| {
        !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit())
    });
    (rest.starts_with("- ") || rest.starts_with("* ") || rest.starts_with("+ ") || numbered)
        .then(|| indent_of(line))
}
