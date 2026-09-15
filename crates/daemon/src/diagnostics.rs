//! Run a checker and turn its output into gutter marks.
//!
//! Not a language server and deliberately not one: a person asks, a command
//! runs once in the checkout, and what it says about the open file becomes
//! marks. `cargo check --message-format=short` and `oxlint` both print
//! `path:line:col: level: message`, which is the whole grammar here — anything
//! that speaks it is a producer, and anything that does not simply yields
//! nothing rather than a wrong mark.
//!
//! Off unless `[editor] diagnostics_command` names one. A checker that runs by
//! itself is a subprocess per keystroke, which is a cost this repository prices
//! rather than assumes.

use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use editor_control::WireDiagnostic;

/// How long a checker has. Longer than a `git` command (ADR-008's 30 s) because
/// a cold `cargo check` is a build, and shorter than forever because a person
/// is waiting for it.
pub const TIMEOUT: Duration = Duration::from_secs(120);

/// How long to wait for the worker to reap a killed child.
const KILL_REAP_GRACE: Duration = Duration::from_secs(5);

/// Most marks one answer carries.
///
/// A file with more errors than this has one error; the list is capped so a
/// broken build cannot put a megabyte through a socket sized for a buffer.
pub const MAX_DIAGNOSTICS: usize = 500;

/// Most output bytes read from a checker.
///
/// Clamped before the parse, not after: a build that prints a gigabyte is a
/// build that prints a gigabyte, and `wait_with_output` would hold all of it.
pub const MAX_OUTPUT_BYTES: usize = 4 * 1024 * 1024;

/// Run `command` in `root` and keep what it said about `relative`.
///
/// Returns `None` when there is no command configured — which is different
/// from "it found nothing", and is why the editor is told which.
pub fn run(root: &Path, command: &[String], relative: &str) -> Option<Vec<WireDiagnostic>> {
    let (program, args) = command.split_first()?;
    if program.is_empty() {
        return None;
    }
    let output = capture(root, program, args)?;
    Some(parse(&output, relative))
}

/// Spawn, drain and reap, with the whole process group killed on a timeout.
///
/// The same shape as `git_service::run_git`, and for the same reason: a
/// checker forks (cargo forks rustc, a script forks a linter), and killing the
/// direct child alone leaves a grandchild holding the pipes — a blocked thread,
/// two leaked fds and the process that was hanging still running.
fn capture(root: &Path, program: &str, args: &[String]) -> Option<String> {
    use std::os::unix::process::CommandExt as _;

    let mut cmd = Command::new(program);
    let child = cmd
        .args(args)
        .current_dir(root)
        // Stable, machine-parseable output regardless of the user's locale.
        .env("LC_ALL", "C")
        // A checker that asks a question gets EOF, not a wait forever.
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .ok()?;
    let pid = child.id();

    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(child.wait_with_output());
    });

    match rx.recv_timeout(TIMEOUT) {
        Ok(Ok(output)) => {
            // Both streams: cargo writes diagnostics to stderr and oxlint to
            // stdout, and guessing wrong means finding nothing at all.
            let mut body = String::new();
            for stream in [&output.stdout, &output.stderr] {
                let room = MAX_OUTPUT_BYTES.saturating_sub(body.len());
                if room == 0 {
                    break;
                }
                let take = stream.len().min(room);
                body.push_str(&String::from_utf8_lossy(&stream[..take]));
            }
            Some(body)
        }
        Ok(Err(_)) => None,
        Err(mpsc::RecvTimeoutError::Timeout) => {
            kill_group(pid);
            let _ = rx.recv_timeout(KILL_REAP_GRACE);
            None
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => None,
    }
}

fn kill_group(pid: u32) {
    use nix::sys::signal::Signal::SIGKILL;
    #[allow(clippy::cast_possible_wrap)]
    let raw = pid as i32;
    // The group first — that is what reaches the descendants holding the pipes
    // — then the bare pid, for a child that moved itself out of the group.
    crate::core::signal_group(raw, SIGKILL);
    let _ = nix::sys::signal::kill(nix::unistd::Pid::from_raw(raw), SIGKILL);
}

/// `path:line:col: level: message` lines that name `relative`.
///
/// Pure, so the grammar is testable without running anything. A line that does
/// not parse is not a diagnostic: this is a heuristic over ordinary compiler
/// output, and a wrong mark is worse than a missing one.
#[must_use]
pub fn parse(output: &str, relative: &str) -> Vec<WireDiagnostic> {
    let mut found = Vec::new();
    for row in output.lines() {
        if found.len() == MAX_DIAGNOSTICS {
            break;
        }
        let Some(item) = parse_line(row, relative) else {
            continue;
        };
        found.push(item);
    }
    found.sort_by_key(|item| item.line);
    found.dedup_by(|a, b| a.line == b.line && a.message == b.message);
    found
}

fn parse_line(row: &str, relative: &str) -> Option<WireDiagnostic> {
    let row = row.trim();
    // `path:line:col: rest` — the path may itself contain colons, so the split
    // walks from the left and takes the first pair of numbers that follows a
    // path matching the open file.
    let (path, rest) = split_path(row, relative)?;
    let _ = path;
    let mut parts = rest.splitn(3, ':');
    let line: u32 = parts.next()?.trim().parse().ok()?;
    let _column: u32 = parts.next()?.trim().parse().ok()?;
    let tail = parts.next()?.trim();
    let (level, message) = tail
        .split_once(':')
        .map_or(("warning", tail), |(head, body)| (head.trim(), body.trim()));
    if message.is_empty() {
        return None;
    }
    Some(WireDiagnostic {
        line,
        severity: severity(level),
        message: message.to_string(),
    })
}

/// Whether `row` starts with a path naming `relative`, and what follows it.
///
/// A checker may print the path absolute, workspace-relative or
/// crate-relative, so the match is on the tail: `src/main.rs` is named by
/// `/home/x/repo/src/main.rs` and by `src/main.rs`, and not by `other.rs`.
fn split_path<'a>(row: &'a str, relative: &str) -> Option<(&'a str, &'a str)> {
    let at = row.find(&format!("{relative}:"))?;
    let before = &row[..at];
    // Only at a boundary: `a/lib.rs` must not be found inside `xlib.rs`.
    if !before.is_empty() && !before.ends_with('/') && !before.ends_with(' ') {
        return None;
    }
    let start = at + relative.len() + 1;
    Some((&row[at..start], &row[start..]))
}

fn severity(level: &str) -> editor_control::WireSeverity {
    use editor_control::WireSeverity;
    match level.to_ascii_lowercase().as_str() {
        "error" | "fatal" | "fatal error" => WireSeverity::Error,
        "note" | "help" | "info" => WireSeverity::Info,
        _ => WireSeverity::Warning,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use editor_control::WireSeverity;

    #[test]
    fn a_compilers_short_output_becomes_marks() {
        let output = "\
src/main.rs:12:9: error: cannot find value `x` in this scope
src/main.rs:20:1: warning: unused import: `std::io`
other.rs:3:1: error: not this file
";
        let found = parse(output, "src/main.rs");
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].line, 12);
        assert_eq!(found[0].severity, WireSeverity::Error);
        assert!(found[0].message.contains("cannot find value"));
        assert_eq!(found[1].severity, WireSeverity::Warning);
    }

    /// A checker may print the path any of three ways; all of them name the
    /// same file, and a different file names none of them.
    #[test]
    fn a_path_is_matched_on_its_tail_and_at_a_boundary() {
        for row in [
            "src/lib.rs:1:1: error: boom",
            "/home/dev/repo/src/lib.rs:1:1: error: boom",
            "  src/lib.rs:1:1: error: boom",
        ] {
            assert_eq!(parse(row, "src/lib.rs").len(), 1, "{row}");
        }
        // `xsrc/lib.rs` is a different file that ends with the same text.
        assert!(parse("xsrc/lib.rs:1:1: error: boom", "src/lib.rs").is_empty());
    }

    /// This is a heuristic over ordinary output; a wrong mark is worse than a
    /// missing one, so anything that does not parse is not a diagnostic.
    #[test]
    fn prose_is_not_a_diagnostic() {
        let output = "\
    Checking forge v0.1.0
warning: 1 warning emitted
src/main.rs: something without a line
src/main.rs:notanumber:1: error: nope
";
        assert!(parse(output, "src/main.rs").is_empty());
    }

    #[test]
    fn the_list_is_capped_and_deduplicated() {
        let mut output = String::new();
        for _ in 0..3 {
            output.push_str("a.rs:1:1: error: same\n");
        }
        assert_eq!(parse(&output, "a.rs").len(), 1);

        let mut many = String::new();
        for n in 0..MAX_DIAGNOSTICS * 2 {
            many.push_str(&format!("a.rs:{}:1: error: e{n}\n", n + 1));
        }
        assert_eq!(parse(&many, "a.rs").len(), MAX_DIAGNOSTICS);
    }

    #[test]
    fn no_command_is_not_the_same_as_no_findings() {
        assert!(run(Path::new("/"), &[], "a.rs").is_none());
        assert!(run(Path::new("/"), &[String::new()], "a.rs").is_none());
    }
}
