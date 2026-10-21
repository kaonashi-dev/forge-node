//! Universal Ctags, run on the few files that mention the symbol.
//!
//! Ctags is GPL and never bundled: the user installs it and the daemon probes
//! it. The probe requires Universal Ctags with JSON output and refuses
//! anything else — macOS ships a BSD `ctags` at `/usr/bin/ctags` that rejects
//! `--version`, and the classic tags format is not parsed here at all.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use fs_service::definition::{
    Answer, Candidate, Confidence, DefinitionProvider, DefinitionQuery, Target,
};
use fs_service::FsError;
use git_service::{run_git_prefix, GitError};
use serde::Deserialize;

use crate::process::{self, Outcome};
use crate::text;

/// A sensible budget for [`CtagsProvider::probe`].
pub const DEFAULT_PROBE_TIMEOUT: Duration = Duration::from_secs(3);

const PROBE_OUTPUT_BYTES: usize = 64 * 1024;

/// Most files handed to one ctags run.
const MAX_FILES: usize = 256;

/// Most bytes, by `metadata`, of the files handed to one ctags run.
const MAX_INPUT_BYTES: u64 = 16 * 1024 * 1024;

const MAX_GREP_OUTPUT_BYTES: usize = 1024 * 1024;

const MAX_TAGS_OUTPUT_BYTES: usize = 8 * 1024 * 1024;

/// Why an executable cannot serve as [`CtagsProvider`].
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ProbeError {
    #[error("could not run {}: {source}", executable.display())]
    Spawn {
        executable: PathBuf,
        source: std::io::Error,
    },
    #[error("{} did not answer within {timeout:?}", executable.display())]
    Timeout {
        executable: PathBuf,
        timeout: Duration,
    },
    /// Not Universal Ctags; `said` is the first line it printed.
    #[error("{} is not Universal Ctags: {said}", executable.display())]
    NotUniversal { executable: PathBuf, said: String },
    #[error("{} was built without JSON output", executable.display())]
    NoJson { executable: PathBuf },
}

/// Definitions from a user-installed Universal Ctags.
///
/// Grep first, tag second: `git grep -l -w -F` picks the files that mention
/// the symbol, the closest to the source are kept within the budgets, and one
/// ctags run tags them all. Ctags reads the disk copy; an unsaved buffer is
/// not written anywhere for it. It has no scope resolution, so an answer is
/// never authoritative.
#[derive(Debug)]
pub struct CtagsProvider {
    executable: PathBuf,
    version: String,
}

impl CtagsProvider {
    /// Run `executable --version` (a name is looked up on `PATH`) and keep it
    /// only if it is Universal Ctags with JSON output.
    ///
    /// # Errors
    /// [`ProbeError`] when it cannot run, is too slow, is another ctags, or
    /// cannot write JSON.
    pub fn probe(executable: impl AsRef<Path>, timeout: Duration) -> Result<Self, ProbeError> {
        let executable = executable.as_ref().to_path_buf();
        let deadline = Instant::now() + timeout;
        let version = probe_output(&executable, "--version", deadline, timeout)?;
        if !version.contains("Universal Ctags") {
            return Err(ProbeError::NotUniversal {
                said: first_line(&version),
                executable,
            });
        }
        // `--version` lists `+json` in its compiled features on current
        // releases; older ones only answer `--list-features`.
        if !version.contains("+json") {
            let features = probe_output(&executable, "--list-features", deadline, timeout)?;
            if !features
                .lines()
                .any(|row| row.split_whitespace().next() == Some("json"))
            {
                return Err(ProbeError::NoJson { executable });
            }
        }
        Ok(Self {
            version: first_line(&version),
            executable,
        })
    }

    #[must_use]
    pub fn executable(&self) -> &Path {
        &self.executable
    }

    /// First line of `--version`.
    #[must_use]
    pub fn version(&self) -> &str {
        &self.version
    }
}

fn probe_output(
    executable: &Path,
    flag: &str,
    deadline: Instant,
    timeout: Duration,
) -> Result<String, ProbeError> {
    let mut command = Command::new(executable);
    command.arg(flag);
    match process::run(command, None, PROBE_OUTPUT_BYTES, deadline, None) {
        Ok(Outcome::Finished(done)) => {
            let mut said = String::from_utf8_lossy(&done.stdout).into_owned();
            if said.trim().is_empty() {
                said = String::from_utf8_lossy(&done.stderr).into_owned();
            }
            Ok(said)
        }
        Ok(Outcome::Expired(_)) => Err(ProbeError::Timeout {
            executable: executable.to_path_buf(),
            timeout,
        }),
        Err(source) => Err(ProbeError::Spawn {
            executable: executable.to_path_buf(),
            source,
        }),
    }
}

fn first_line(text: &str) -> String {
    text::clamp(text.lines().next().unwrap_or("").trim())
}

impl DefinitionProvider for CtagsProvider {
    fn name(&self) -> &'static str {
        "ctags"
    }

    fn find(&self, query: &DefinitionQuery<'_>) -> Result<Answer, FsError> {
        let mut answer = Answer::default();
        if query.expired() || query.limit == 0 {
            answer.truncated = query.expired();
            return Ok(answer);
        }
        let Some((mut files, cut)) = files_mentioning(query.root, query.symbol)? else {
            return Ok(answer);
        };
        answer.truncated |= cut;
        files.sort_by_key(|path| closeness(path, query.source));
        let (files, over) = within_budget(query.root, files);
        answer.truncated |= over;
        if files.is_empty() {
            return Ok(answer);
        }
        if query.expired() {
            answer.truncated = true;
            return Ok(answer);
        }

        let mut list = Vec::new();
        for path in &files {
            list.extend_from_slice(path.as_bytes());
            list.push(b'\n');
        }
        let mut command = Command::new(&self.executable);
        command.current_dir(query.root).args([
            // First, so a user's `~/.ctags.d` cannot turn on reference tags or
            // another output format behind our back.
            "--options=NONE",
            "--output-format=json",
            "--fields=+nKZr",
            "--sort=no",
            "-f",
            "-",
            "-L",
            "-",
        ]);
        tracing::debug!(files = files.len(), symbol = query.symbol, "ctags run");
        let (stdout, cut) = match process::run(
            command,
            Some(list),
            MAX_TAGS_OUTPUT_BYTES,
            query.deadline,
            Some(query.cancel),
        )? {
            Outcome::Finished(done) => {
                if done.status != Some(0) && !done.cut {
                    tracing::debug!(
                        status = ?done.status,
                        stderr = %String::from_utf8_lossy(&done.stderr),
                        "ctags failed"
                    );
                }
                (done.stdout, done.cut)
            }
            Outcome::Expired(partial) => {
                answer.truncated = true;
                partial.map_or((Vec::new(), true), |done| (done.stdout, true))
            }
        };
        answer.truncated |= cut;

        let allowed: HashSet<&str> = files.iter().map(String::as_str).collect();
        let (tags, more) = parse_tags(
            process::whole_lines(&stdout, cut),
            query.symbol,
            &allowed,
            query.limit,
        );
        answer.truncated |= more;
        answer.candidates = candidates(query, tags);
        Ok(answer)
    }
}

/// Paths (relative to `root`) that mention `symbol` as a word, and whether the
/// listing was cut. `None` when `root` is not a Git checkout.
fn files_mentioning(root: &Path, symbol: &str) -> Result<Option<(Vec<String>, bool)>, FsError> {
    let args = [
        "grep",
        "-l",
        "-z",
        "-w",
        "-F",
        "-I",
        "--untracked",
        "--no-color",
        "-e",
        symbol,
        "--",
        ".",
    ];
    let (out, cut) = run_git_prefix(Some(root), &args, MAX_GREP_OUTPUT_BYTES)?;
    // Exit 1 is "no match", which is success; a cut grep was killed, so its
    // status says nothing.
    if !cut && out.status == 1 {
        return Ok(Some((Vec::new(), false)));
    }
    if !cut && out.status != 0 {
        if out.stderr.contains("not a git repository") {
            return Ok(None);
        }
        return Err(FsError::Git(GitError::CommandFailed {
            args: args.iter().map(|arg| (*arg).to_owned()).collect(),
            stderr: out.stderr,
            status: out.status,
        }));
    }
    let mut names: Vec<&str> = out.stdout.split('\0').collect();
    // Without a cut the last piece is the empty tail after the final NUL;
    // with one it is a clipped name.
    names.pop();
    Ok(Some((
        names
            .into_iter()
            .filter(|name| !name.is_empty())
            .map(str::to_owned)
            .collect(),
        cut,
    )))
}

fn parent(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(dir, _)| dir)
}

/// 0 for the source, 1 beside it, 2 in its language family, 3 elsewhere.
fn closeness(path: &str, source: &str) -> u8 {
    if path == source {
        0
    } else if parent(path) == parent(source) {
        1
    } else if languages::same_family(languages::Registry::builtin(), path, source) {
        2
    } else {
        3
    }
}

/// The leading `files` that fit [`MAX_FILES`] and [`MAX_INPUT_BYTES`], and
/// whether any was left out. A file `-L` cannot name (a newline in it) or that
/// is not a regular file is skipped without counting as a cut.
fn within_budget(root: &Path, files: Vec<String>) -> (Vec<String>, bool) {
    let mut kept = Vec::new();
    let mut bytes = 0u64;
    let mut over = false;
    for path in files {
        if kept.len() == MAX_FILES {
            over = true;
            break;
        }
        if path.contains('\n') || !text::is_plain_relative(&path) {
            continue;
        }
        let Ok(meta) = std::fs::symlink_metadata(root.join(&path)) else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        match bytes.checked_add(meta.len()) {
            Some(total) if total <= MAX_INPUT_BYTES => {
                bytes = total;
                kept.push(path);
            }
            _ => over = true,
        }
    }
    (kept, over)
}

#[derive(Debug, PartialEq, Eq)]
struct Tag {
    path: String,
    line: u32,
    pattern: Option<String>,
}

#[derive(Deserialize)]
struct RawTag {
    #[serde(rename = "_type")]
    record: String,
    name: String,
    path: String,
    line: Option<u32>,
    pattern: Option<String>,
    roles: Option<String>,
}

/// Definition tags named `symbol` in files we asked about, at most `limit`,
/// and whether more were left.
fn parse_tags(
    stdout: &[u8],
    symbol: &str,
    allowed: &HashSet<&str>,
    limit: usize,
) -> (Vec<Tag>, bool) {
    let mut tags = Vec::new();
    for row in stdout.split(|&b| b == b'\n') {
        let Ok(row) = std::str::from_utf8(row) else {
            continue;
        };
        if !row.contains(symbol) {
            continue;
        }
        let Ok(raw) = serde_json::from_str::<RawTag>(row) else {
            continue;
        };
        if raw.record != "tag" || raw.name != symbol || !allowed.contains(raw.path.as_str()) {
            continue;
        }
        if raw
            .roles
            .as_deref()
            .is_some_and(|roles| !roles.split(',').any(|role| role == "def"))
        {
            continue;
        }
        let Some(line) = raw.line.filter(|&line| line > 0) else {
            continue;
        };
        let tag = Tag {
            path: raw.path,
            line,
            pattern: raw.pattern,
        };
        if tags.contains(&tag) {
            continue;
        }
        if tags.len() == limit {
            return (tags, true);
        }
        tags.push(tag);
    }
    (tags, false)
}

/// Ctags' search pattern without its `/^…$/` and escapes: the text when the
/// file could not be read.
fn pattern_text(pattern: &str) -> String {
    let body = pattern.strip_prefix("/^").unwrap_or(pattern);
    let body = body
        .strip_suffix("$/")
        .or_else(|| body.strip_suffix('/'))
        .unwrap_or(body);
    let mut out = String::with_capacity(body.len());
    let mut chars = body.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(next) = chars.next() {
                out.push(next);
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn candidates(query: &DefinitionQuery<'_>, tags: Vec<Tag>) -> Vec<Candidate> {
    let mut by_file: HashMap<&str, Vec<u32>> = HashMap::new();
    for tag in &tags {
        by_file.entry(&tag.path).or_default().push(tag.line);
    }
    let mut lines: HashMap<(&str, u32), String> = HashMap::new();
    for (path, wanted) in &by_file {
        let read = text::read_lines(&query.root.join(path), wanted);
        for (line, found) in wanted.iter().zip(read) {
            if let Some(found) = found {
                lines.insert((*path, *line), found);
            }
        }
    }
    tags.iter()
        .map(|tag| {
            let line = lines
                .get(&(tag.path.as_str(), tag.line))
                .cloned()
                .or_else(|| tag.pattern.as_deref().map(pattern_text))
                .unwrap_or_default();
            Candidate {
                target: Target::Workspace(tag.path.clone()),
                line: tag.line,
                column: text::word_column(&line, query.symbol),
                text: text::clamp(&line),
                confidence: Confidence::Indexed,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::AtomicBool;

    fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    const UNIVERSAL: &str = r#"case "$1" in
--version) echo 'Universal Ctags 6.1.0, Copyright (C) 2015-2023 Universal Ctags Team'
           echo '  Optional compiled features: +wildcards, +regex, +iconv, +json' ;;
*) exit 2 ;;
esac"#;

    /// A freshly written script's first `exec` can be held for many seconds
    /// by macOS's executable scan, so every budget a test expects to be met
    /// is far above what the code needs.
    const SLOW_EXEC: Duration = Duration::from_secs(120);

    fn has_git() -> bool {
        Command::new("git").arg("--version").output().is_ok()
    }

    fn checkout(files: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let status = Command::new("git")
            .args(["init", "-q"])
            .current_dir(dir.path())
            .status()
            .unwrap();
        assert!(status.success());
        for (path, body) in files {
            let path = dir.path().join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, body).unwrap();
        }
        dir
    }

    fn query<'a>(
        root: &'a Path,
        source: &'a str,
        symbol: &'a str,
        cancel: &'a AtomicBool,
        budget: Duration,
    ) -> DefinitionQuery<'a> {
        DefinitionQuery {
            root,
            source,
            symbol,
            position: None,
            buffer: None,
            limit: 10,
            deadline: Instant::now() + budget,
            cancel,
        }
    }

    #[test]
    fn the_probe_refuses_bsd_ctags() {
        let dir = tempfile::tempdir().unwrap();
        let bsd = script(
            dir.path(),
            "ctags",
            "echo 'ctags: illegal option -- -' >&2\necho 'usage: ctags [-BFTaduwvx] [-f tagsfile] file ...' >&2\nexit 1",
        );
        let error = CtagsProvider::probe(&bsd, SLOW_EXEC).unwrap_err();
        let ProbeError::NotUniversal { said, .. } = error else {
            panic!("expected a refusal, got {error:?}");
        };
        assert!(said.contains("illegal option"));
    }

    #[test]
    fn the_probe_accepts_universal_ctags_with_json() {
        let dir = tempfile::tempdir().unwrap();
        let ctags = script(dir.path(), "ctags", UNIVERSAL);
        let provider = CtagsProvider::probe(&ctags, SLOW_EXEC).unwrap();
        assert!(provider.version().starts_with("Universal Ctags 6.1.0"));

        let listed = script(
            dir.path(),
            "listed",
            r#"case "$1" in
--version) echo 'Universal Ctags 5.9.0' ;;
--list-features) printf '#NAME DESCRIPTION\nregex  can use regex\njson   supports json format output\n' ;;
esac"#,
        );
        assert!(CtagsProvider::probe(&listed, SLOW_EXEC).is_ok());

        let plain = script(
            dir.path(),
            "plain",
            r#"case "$1" in
--version) echo 'Universal Ctags 5.9.0' ;;
--list-features) printf '#NAME DESCRIPTION\nregex  can use regex\n' ;;
esac"#,
        );
        assert!(matches!(
            CtagsProvider::probe(&plain, SLOW_EXEC),
            Err(ProbeError::NoJson { .. })
        ));
        assert!(matches!(
            CtagsProvider::probe(dir.path().join("absent"), DEFAULT_PROBE_TIMEOUT),
            Err(ProbeError::Spawn { .. })
        ));
    }

    #[test]
    fn a_probe_that_hangs_times_out() {
        let dir = tempfile::tempdir().unwrap();
        let slow = script(dir.path(), "ctags", "sleep 30");
        let started = Instant::now();
        assert!(matches!(
            CtagsProvider::probe(&slow, Duration::from_millis(300)),
            Err(ProbeError::Timeout { .. })
        ));
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn the_parser_keeps_definitions_of_the_name_in_asked_files() {
        let out = br#"{"_type": "tag", "name": "widget", "path": "src/a.rs", "pattern": "/^fn widget() {$/", "line": 3, "kind": "function", "roles": "def"}
{"_type": "tag", "name": "widget", "path": "src/a.rs", "line": 9, "kind": "function", "roles": "ref"}
{"_type": "tag", "name": "widgets", "path": "src/a.rs", "line": 4}
{"_type": "tag", "name": "widget", "path": "elsewhere.rs", "line": 1}
{"_type": "ptag", "name": "widget", "path": "src/a.rs", "line": 1}
not json widget
{"_type": "tag", "name": "widget", "path": "src/b.rs", "line": 7, "kind": "struct"}
{"_type": "tag", "name": "widget", "path": "src/b.rs", "line": 7, "kind": "struct"}
{"_type": "tag", "name": "widget", "path": "src/c.rs", "line": 2}
"#;
        let allowed: HashSet<&str> = ["src/a.rs", "src/b.rs", "src/c.rs"].into_iter().collect();
        let (tags, more) = parse_tags(out, "widget", &allowed, 2);
        assert!(more, "src/c.rs is past the limit");
        assert_eq!(
            tags,
            [
                Tag {
                    path: "src/a.rs".into(),
                    line: 3,
                    pattern: Some("/^fn widget() {$/".into()),
                },
                Tag {
                    path: "src/b.rs".into(),
                    line: 7,
                    pattern: None,
                },
            ]
        );
        assert_eq!(pattern_text(r"/^  a \/ b \\ widget$/"), r"  a / b \ widget");
        assert_eq!(
            process::whole_lines(b"{\"a\"}\n{\"trunc", true),
            b"{\"a\"}\n"
        );
    }

    #[test]
    fn files_are_ordered_by_closeness_to_the_source() {
        let mut files = vec![
            "z/far.py".to_owned(),
            "lib/other.rs".to_owned(),
            "src/near.rs".to_owned(),
            "src/a.rs".to_owned(),
        ];
        files.sort_by_key(|path| closeness(path, "src/a.rs"));
        assert_eq!(
            files,
            ["src/a.rs", "src/near.rs", "lib/other.rs", "z/far.py"]
        );
    }

    #[test]
    fn a_fake_ctags_answers_through_the_whole_pipeline() {
        if !has_git() {
            eprintln!("git not found; skipping the ctags pipeline test");
            return;
        }
        let repo = checkout(&[
            (
                "src/a.rs",
                "use crate::b::widget;\nfn main() { widget(); }\n",
            ),
            (
                "src/b.rs",
                "// widget maker\npub fn widget() -> u8 {\n    1\n}\n",
            ),
            ("notes.txt", "nothing here\n"),
        ]);
        let root = repo.path().canonicalize().unwrap();
        let tools = tempfile::tempdir().unwrap();
        let seen = tools.path().join("seen");
        let ctags = script(
            tools.path(),
            "ctags",
            &format!(
                r#"case "$1" in
--version) echo 'Universal Ctags 6.1.0'; echo '+json' ; exit 0 ;;
esac
cat > '{}'
echo '{{"_type": "tag", "name": "widget", "path": "src/b.rs", "pattern": "/^pub fn widget() -> u8 {{$/", "line": 2, "kind": "function"}}'"#,
                seen.display()
            ),
        );
        let provider = CtagsProvider::probe(&ctags, SLOW_EXEC).unwrap();
        let cancel = AtomicBool::new(false);
        let answer = provider
            .find(&query(&root, "src/a.rs", "widget", &cancel, SLOW_EXEC))
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(&seen).unwrap(),
            "src/a.rs\nsrc/b.rs\n",
            "the source first, and only files that mention the word"
        );
        assert!(!answer.authoritative);
        assert!(!answer.truncated);
        assert_eq!(
            answer.candidates,
            [Candidate {
                target: Target::Workspace("src/b.rs".into()),
                line: 2,
                column: 8,
                text: "pub fn widget() -> u8 {".into(),
                confidence: Confidence::Indexed,
            }]
        );

        let none = provider
            .find(&query(&root, "src/a.rs", "absent", &cancel, SLOW_EXEC))
            .unwrap();
        assert_eq!(none, Answer::default());

        let outside = tempfile::tempdir().unwrap();
        let outside = outside.path().canonicalize().unwrap();
        std::fs::write(outside.join("a.rs"), "fn widget() {}\n").unwrap();
        let not_git = provider
            .find(&query(&outside, "a.rs", "widget", &cancel, SLOW_EXEC))
            .unwrap();
        assert_eq!(
            not_git,
            Answer::default(),
            "outside a checkout there is nothing to grep"
        );
    }

    #[test]
    fn a_ctags_run_past_the_deadline_is_killed_and_reported_truncated() {
        if !has_git() {
            eprintln!("git not found; skipping the ctags deadline test");
            return;
        }
        let repo = checkout(&[("a.rs", "fn widget() {}\n")]);
        let root = repo.path().canonicalize().unwrap();
        let tools = tempfile::tempdir().unwrap();
        let ctags = script(
            tools.path(),
            "ctags",
            "case \"$1\" in --version) echo 'Universal Ctags 6.1.0 +json'; exit 0 ;; esac\nsleep 30",
        );
        let provider = CtagsProvider::probe(&ctags, SLOW_EXEC).unwrap();
        let cancel = AtomicBool::new(false);
        let started = Instant::now();
        let answer = provider
            .find(&query(
                &root,
                "a.rs",
                "widget",
                &cancel,
                Duration::from_secs(2),
            ))
            .unwrap();
        assert!(answer.truncated);
        assert!(answer.candidates.is_empty());
        assert!(started.elapsed() < Duration::from_secs(60));
    }

    #[test]
    fn a_real_universal_ctags_finds_a_rust_function() {
        let Ok(provider) = CtagsProvider::probe("ctags", DEFAULT_PROBE_TIMEOUT) else {
            eprintln!("Universal Ctags not on PATH; skipping the real ctags test");
            return;
        };
        if !has_git() {
            eprintln!("git not found; skipping the real ctags test");
            return;
        }
        let repo = checkout(&[
            ("src/a.rs", "fn main() { widget(); }\n"),
            ("src/b.rs", "pub fn widget() -> u8 {\n    1\n}\n"),
        ]);
        let root = repo.path().canonicalize().unwrap();
        let cancel = AtomicBool::new(false);
        let answer = provider
            .find(&query(&root, "src/a.rs", "widget", &cancel, SLOW_EXEC))
            .unwrap();
        assert!(answer.candidates.iter().any(|candidate| {
            candidate.target == Target::Workspace("src/b.rs".into())
                && candidate.line == 1
                && candidate.column == 8
        }));
    }
}
