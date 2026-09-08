//! Locating the terminfo entry for the `TERM` we advertise to sessions (§13.3).
//!
//! A `TERM` name is only useful if the child process can *find* its terminfo
//! entry: an unknown name leaves every ncurses program (`vim`, `htop`, `less`)
//! with no capabilities at all. That is a real risk for `[sessions].term`,
//! because terminal emulators that ship a non-standard entry usually keep it
//! inside their own app bundle and export `TERMINFO` themselves — Ghostty's
//! `xterm-ghostty` is the motivating case — so a daemon started from the GUI
//! never inherits the pointer to it.
//!
//! [`resolve`] therefore looks the entry up the way ncurses does and reports
//! the directory it came from, so the spawn can export `TERMINFO` next to
//! `TERM` when the entry lives outside the places ncurses searches on its own.
//! When nothing matches, the caller gets [`FALLBACK_TERM`] instead of a name no
//! child could resolve.

use std::path::{Path, PathBuf};

/// The `TERM` used when the requested one has no terminfo entry. Every system
/// terminfo database carries it, and `terminal-core`'s key encoding is written
/// against it (`terminal_core::input`).
pub const FALLBACK_TERM: &str = "xterm-256color";

/// Databases ncurses searches without being told to. An entry found here needs
/// no `TERMINFO` in the child environment.
const SYSTEM_DIRS: &[&str] = &[
    "/usr/share/terminfo",
    "/usr/lib/terminfo",
    "/etc/terminfo",
    "/opt/homebrew/share/terminfo",
];

/// Databases that ship *inside* a terminal emulator's app bundle rather than in
/// the system one. ncurses cannot find these on its own, so an entry found here
/// is exported as `TERMINFO`. Paths starting with `~/` are resolved against
/// `$HOME`.
const BUNDLED_DIRS: &[&str] = &[
    "/Applications/Ghostty.app/Contents/Resources/terminfo",
    "~/Applications/Ghostty.app/Contents/Resources/terminfo",
];

/// The `TERM` a session should advertise, plus the `TERMINFO` (if any) that
/// makes it resolvable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TermSelection {
    /// The value for `TERM`.
    pub term: String,
    /// `TERMINFO` to export alongside it, when the entry lives outside the
    /// directories ncurses searches by itself.
    pub terminfo_dir: Option<PathBuf>,
    /// The requested name, when it had no entry anywhere and `term` is
    /// [`FALLBACK_TERM`]. `None` means the request was honoured.
    pub fell_back_from: Option<String>,
}

impl TermSelection {
    fn found(term: &str, dir: Option<PathBuf>) -> Self {
        Self {
            term: term.to_owned(),
            terminfo_dir: dir,
            fell_back_from: None,
        }
    }
}

/// Where a candidate database came from, which decides whether the child needs
/// an explicit `TERMINFO` to find it again.
struct Candidate {
    dir: PathBuf,
    needs_export: bool,
}

/// Resolve `requested` against the terminfo databases visible to a session.
///
/// `configured_dir` is `[sessions].terminfo_dir` (empty = auto). `env` is the
/// environment the session will be spawned with, which is where `TERMINFO` and
/// `TERMINFO_DIRS` are read from — the daemon's own environment is irrelevant,
/// since [`SpawnSpec::env`](domain::SpawnSpec) replaces it wholesale.
#[must_use]
pub fn resolve(
    requested: &str,
    configured_dir: Option<&Path>,
    env: &[(String, String)],
) -> TermSelection {
    let requested = requested.trim();
    if requested.is_empty() {
        return TermSelection::found(FALLBACK_TERM, None);
    }
    for candidate in candidates(configured_dir, env) {
        if entry_exists(&candidate.dir, requested) {
            return TermSelection::found(
                requested,
                candidate.needs_export.then_some(candidate.dir),
            );
        }
    }
    if requested == FALLBACK_TERM {
        // Nothing to fall back to: a database this thin is a broken system, and
        // the caller is no better off being told twice.
        return TermSelection::found(FALLBACK_TERM, None);
    }
    TermSelection {
        term: FALLBACK_TERM.to_owned(),
        terminfo_dir: None,
        fell_back_from: Some(requested.to_owned()),
    }
}

/// The databases to search, in ncurses' order: the explicit config first, then
/// what the session's own environment points at, then the system databases,
/// then the app bundles ncurses knows nothing about.
fn candidates(configured_dir: Option<&Path>, env: &[(String, String)]) -> Vec<Candidate> {
    let var = |key: &str| {
        env.iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
            .filter(|v| !v.is_empty())
    };
    let home = var("HOME").map(PathBuf::from);
    let mut out = Vec::new();

    if let Some(dir) = configured_dir {
        // The user named this one, so the child is told about it explicitly
        // rather than being trusted to have the same view we do.
        out.push(Candidate {
            dir: dir.to_path_buf(),
            needs_export: true,
        });
    }
    // Already in the session environment, so finding an entry here needs no
    // further help.
    if let Some(dir) = var("TERMINFO") {
        out.push(Candidate {
            dir: PathBuf::from(dir),
            needs_export: false,
        });
    }
    if let Some(home) = &home {
        out.push(Candidate {
            dir: home.join(".terminfo"),
            needs_export: false,
        });
    }
    if let Some(dirs) = var("TERMINFO_DIRS") {
        for dir in dirs.split(':') {
            // An empty entry means "the system database" (`terminfo(5)`).
            let dir = if dir.is_empty() { SYSTEM_DIRS[0] } else { dir };
            out.push(Candidate {
                dir: PathBuf::from(dir),
                needs_export: false,
            });
        }
    }
    out.extend(SYSTEM_DIRS.iter().map(|d| Candidate {
        dir: PathBuf::from(d),
        needs_export: false,
    }));
    for dir in BUNDLED_DIRS {
        let dir = match dir.strip_prefix("~/") {
            Some(rest) => match &home {
                Some(home) => home.join(rest),
                None => continue,
            },
            None => PathBuf::from(dir),
        };
        out.push(Candidate {
            dir,
            needs_export: true,
        });
    }
    out
}

/// Whether `dir` holds a compiled entry for `name`.
///
/// ncurses files entries under a directory named for the first character of the
/// entry; macOS uses the two-digit hex of that byte instead. Both layouts are
/// live on the machines we target, so both are checked.
fn entry_exists(dir: &Path, name: &str) -> bool {
    let Some(first) = name.as_bytes().first().copied() else {
        return false;
    };
    if dir.join(format!("{first:02x}")).join(name).exists() {
        return true;
    }
    // Only single-byte directory names exist in the character layout, so a
    // non-ASCII first byte can only be the hex one.
    first.is_ascii() && dir.join((first as char).to_string()).join(name).exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Create `<root>/<layout dir>/<name>` and return `root`.
    fn db_with(root: &Path, name: &str, hex_layout: bool) -> PathBuf {
        let first = name.as_bytes()[0];
        let sub = if hex_layout {
            format!("{first:02x}")
        } else {
            (first as char).to_string()
        };
        let dir = root.join(&sub);
        std::fs::create_dir_all(&dir).expect("create layout dir");
        std::fs::write(dir.join(name), b"compiled entry").expect("write entry");
        root.to_path_buf()
    }

    fn env(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    #[test]
    fn an_entry_in_terminfo_needs_no_export() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let db = db_with(tmp.path(), "xterm-ghostty", true);
        let sel = resolve(
            "xterm-ghostty",
            None,
            &env(&[("TERMINFO", &db.to_string_lossy())]),
        );
        // The child already carries `TERMINFO`, so nothing has to be injected.
        assert_eq!(sel.term, "xterm-ghostty");
        assert_eq!(sel.terminfo_dir, None);
        assert_eq!(sel.fell_back_from, None);
    }

    #[test]
    fn a_configured_directory_is_exported_to_the_child() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let db = db_with(tmp.path(), "xterm-ghostty", false);
        let sel = resolve("xterm-ghostty", Some(&db), &[]);
        assert_eq!(sel.term, "xterm-ghostty");
        assert_eq!(sel.terminfo_dir.as_deref(), Some(db.as_path()));
    }

    #[test]
    fn both_directory_layouts_are_recognised() {
        for hex in [true, false] {
            let tmp = tempfile::tempdir().expect("tempdir");
            let db = db_with(tmp.path(), "xterm-ghostty", hex);
            assert!(
                entry_exists(&db, "xterm-ghostty"),
                "hex layout {hex} should be found"
            );
        }
    }

    #[test]
    fn an_unknown_term_falls_back_instead_of_breaking_ncurses() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let sel = resolve(
            "xterm-nonesuch",
            Some(tmp.path()),
            &env(&[("HOME", &tmp.path().to_string_lossy())]),
        );
        assert_eq!(sel.term, FALLBACK_TERM);
        assert_eq!(sel.terminfo_dir, None);
        assert_eq!(sel.fell_back_from.as_deref(), Some("xterm-nonesuch"));
    }

    #[test]
    fn an_empty_request_is_the_fallback_without_a_notice() {
        let sel = resolve("  ", None, &[]);
        assert_eq!(sel.term, FALLBACK_TERM);
        assert_eq!(sel.fell_back_from, None);
    }

    #[test]
    fn the_fallback_never_reports_falling_back_to_itself() {
        // On a machine with no terminfo database at all we still advertise the
        // fallback, but there is nothing to warn the user about.
        let tmp = tempfile::tempdir().expect("tempdir");
        let sel = resolve(
            FALLBACK_TERM,
            Some(tmp.path()),
            &env(&[("HOME", &tmp.path().to_string_lossy())]),
        );
        assert_eq!(sel.term, FALLBACK_TERM);
        assert_eq!(sel.fell_back_from, None);
    }
}
