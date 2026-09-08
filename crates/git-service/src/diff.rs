//! Per-file working-tree patches for the GUI's diff view (§16.7).
//!
//! The sibling of [`crate::change`], and deliberately not the same thing:
//! `change_context` produces *one* bounded blob for Juva to read, while this
//! module produces one patch per file, with its own stats, for a human to
//! review. Both are read-only and local, so both stay on [`run_git`].
//!
//! The command set is the one a diff viewer actually needs, and it is chosen
//! to keep the subprocess count flat rather than linear in the number of
//! changed files: `status` names them, `numstat` counts them, and a **single**
//! `diff --patch` produces every tracked patch at once, split afterwards on
//! its own `diff --git ` markers. Only untracked files cost a process each,
//! because git has no batch form for them — `--no-index` takes one pair of
//! paths.
//!
//! Both entry points take an optional base. Against `HEAD` the file list comes
//! from `status`; against an older commit it has to come from
//! `diff --name-status`, because `status` cannot see a path that a commit
//! changed and left clean. [`change_summary`] is the patchless form, and it
//! stays flat even for untracked files: a panel that refreshes itself must not
//! cost a process per file the agent creates.

use std::collections::HashMap;
use std::path::Path;

use crate::command::{run_git, GitError, GitOutput};
use crate::repository::current_branch;

/// Context lines around each hunk.
///
/// Wider than git's own default of three: this feeds a window, not a terminal
/// pager, and a reviewer reading a hunk in a GUI has the room for it.
pub const DIFF_CONTEXT_LINES: u32 = 12;

/// Soft ceiling for the whole diff, across every file.
pub const MAX_DIFF_BYTES: usize = 2 * 1024 * 1024;

/// Soft ceiling for one file's patch.
pub const MAX_FILE_PATCH_BYTES: usize = 256 * 1024;

/// Soft ceiling for how many changed files are described.
pub const MAX_DIFF_FILES: usize = 500;

/// What happened to one path, as a diff viewer needs to label it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileChange {
    /// The path is new: untracked, or added to the index.
    Added,
    /// The path exists on both sides.
    Modified,
    /// The path is gone from the working tree.
    Deleted,
    /// The path is unmerged: a stopped rebase, merge or cherry-pick left both
    /// sides in it. It stops being one the moment it is staged, so nothing
    /// caches this — see [`crate::rebase`].
    Conflicted,
}

/// One changed path and the patch that describes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileDiff {
    /// Repository-relative path, exactly as git spells it.
    pub path: String,
    /// How the path changed.
    pub status: FileChange,
    /// Added lines.
    pub additions: u32,
    /// Removed lines.
    pub deletions: u32,
    /// Unified patch text, or empty when there is nothing to show.
    pub patch: String,
    /// Whether git reported the content as binary.
    pub binary: bool,
    /// Whether the patch was dropped for exceeding a budget. A patch is never
    /// cut in half: half a patch is not a patch, so it is dropped whole and
    /// the viewer says so.
    pub truncated: bool,
}

/// Everything uncommitted in one checkout.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkingTreeDiff {
    /// Current branch, when not detached.
    pub branch: Option<String>,
    /// Changed paths in git's own order, capped at [`MAX_DIFF_FILES`].
    pub files: Vec<FileDiff>,
    /// Whether files were dropped from the list, or a patch from a file.
    pub truncated: bool,
}

/// Collect the working-tree diff of `repo` with `context` lines around each
/// hunk.
///
/// "Working tree" means everything not committed: staged, unstaged and
/// untracked, against `HEAD`. A repository with no commits yet has no `HEAD`
/// to compare with, so every path is treated as new.
///
/// `base` widens that to "everything since a commit", which is what a session
/// asks for: its own commits count as its changes. It also changes where the
/// file list comes from — see [`changed_paths`].
///
/// # Errors
/// [`GitError::CommandFailed`] when git cannot describe the working tree at
/// all. A single file whose patch git refuses to produce is reported with an
/// empty patch instead of failing the whole read: one unreadable path must not
/// cost the user the other forty.
pub fn working_tree_diff(
    repo: &Path,
    base: Option<&str>,
    context: u32,
) -> Result<WorkingTreeDiff, GitError> {
    let branch = current_branch(repo).ok().flatten();
    // A recorded base is itself a commit, so it proves there is history to
    // diff against even where `HEAD` alone would not.
    let head = base.is_some() || has_head(repo);
    let entries = changed_paths(repo, base)?;
    let mut truncated = entries.len() > MAX_DIFF_FILES;
    let entries: Vec<Entry> = entries.into_iter().take(MAX_DIFF_FILES).collect();

    let stats = if head {
        numstat(repo, base)?
    } else {
        HashMap::new()
    };
    let mut patches = if head {
        batch_patches(repo, base, context)
    } else {
        HashMap::new()
    };

    let mut total = 0usize;
    let mut files = Vec::with_capacity(entries.len());
    for entry in entries {
        let patch = match (entry.untracked || !head, patches.remove(&entry.path)) {
            (true, _) => untracked_patch(repo, &entry.path, context),
            (false, Some(patch)) => patch,
            // git listed the path but the batch did not describe it: ask for
            // that one file rather than showing the user an empty diff.
            (false, None) => tracked_patch(repo, base, &entry.path, context),
        };

        let stat = stats.get(&entry.path).copied();
        // `numstat` is the authority when it saw the file; an untracked one it
        // never saw is judged by what git wrote in the patch itself.
        let binary = stat.map_or_else(|| is_binary_patch(&patch), |(_, _, binary)| binary);

        let over_budget =
            patch.len() > MAX_FILE_PATCH_BYTES || total + patch.len() > MAX_DIFF_BYTES;
        let (additions, deletions) = match stat {
            Some((additions, deletions, _)) => (additions, deletions),
            None => count_changed_lines(&patch),
        };
        if !over_budget {
            total += patch.len();
        } else {
            truncated = true;
        }

        files.push(FileDiff {
            path: entry.path,
            status: entry.status,
            additions,
            deletions,
            patch: if over_budget { String::new() } else { patch },
            binary,
            truncated: over_budget,
        });
    }

    tracing::debug!(target: "git", files = files.len(), truncated, "diff.working_tree");
    Ok(WorkingTreeDiff {
        branch,
        files,
        truncated,
    })
}

/// One line of `git status --porcelain`.
struct Entry {
    path: String,
    status: FileChange,
    untracked: bool,
}

fn has_head(repo: &Path) -> bool {
    const ARGS: [&str; 3] = ["rev-parse", "--verify", "HEAD"];
    run_git(Some(repo), &ARGS).is_ok_and(|out| out.success())
}

/// Every path git considers changed, untracked ones included, in git's order.
///
/// `-z` is what makes this safe: a path with a space, a quote or a newline is
/// delimited by a NUL instead of being shell-quoted, so no unescaping is
/// needed and no path is silently mangled. `--no-renames` keeps one record per
/// path, which is what a per-file viewer can display.
fn changed_paths(repo: &Path, base: Option<&str>) -> Result<Vec<Entry>, GitError> {
    let mut entries = match base {
        // `status` only ever describes the working tree against `HEAD`.
        // Against an older base it would miss every path a commit changed
        // and left clean, which is most of what a session did.
        Some(base) => named_paths(repo, base)?,
        None => Vec::new(),
    };
    let seen: std::collections::HashSet<String> =
        entries.iter().map(|entry| entry.path.clone()).collect();
    for entry in status_paths(repo)? {
        // With a base, `status` is only here for the untracked files git's
        // own `diff` cannot see; everything else it would say is already
        // covered, and covered against the right side.
        if base.is_some() && !entry.untracked {
            continue;
        }
        if seen.contains(&entry.path) {
            continue;
        }
        entries.push(entry);
    }
    Ok(entries)
}

/// Everything that differs from `base`, committed or not, in git's order.
fn named_paths(repo: &Path, base: &str) -> Result<Vec<Entry>, GitError> {
    let args = [
        "diff",
        "--no-ext-diff",
        "--no-renames",
        "--name-status",
        "-z",
        base,
        "--",
        ".",
    ];
    let out = run_git(Some(repo), &args)?;
    if !out.success() {
        // An unreachable base is the caller's problem to report, not a
        // reason to fail the read: it falls back and says which base it used.
        return Ok(Vec::new());
    }

    // With `-z` the status letter and the path are two separate NUL fields,
    // which is what makes a path with a newline in it survive the trip.
    let mut entries = Vec::new();
    let mut fields = out.stdout.split('\0');
    while let (Some(code), Some(path)) = (fields.next(), fields.next()) {
        if code.is_empty() || path.is_empty() {
            continue;
        }
        entries.push(Entry {
            path: path.to_string(),
            status: classify_name_status(code),
            untracked: false,
        });
    }
    Ok(entries)
}

/// One letter of `diff --name-status`, in the viewer's vocabulary.
fn classify_name_status(code: &str) -> FileChange {
    match code.chars().next() {
        Some('A') => FileChange::Added,
        Some('D') => FileChange::Deleted,
        Some('U') => FileChange::Conflicted,
        _ => FileChange::Modified,
    }
}

/// Every path git's `status` considers changed, untracked ones included.
fn status_paths(repo: &Path) -> Result<Vec<Entry>, GitError> {
    const ARGS: [&str; 7] = [
        "status",
        "--porcelain=v1",
        "--untracked-files=all",
        "--no-renames",
        "-z",
        "--",
        ".",
    ];
    let out = run_git(Some(repo), &ARGS)?.ok(&ARGS)?;

    let mut entries = Vec::new();
    for record in out.stdout.split('\0') {
        // `XY <path>`: two status columns, a space, then the path.
        if record.len() < 4 {
            continue;
        }
        let (code, path) = record.split_at(3);
        let path = path.to_string();
        if path.is_empty() {
            continue;
        }
        let untracked = code.starts_with("??");
        entries.push(Entry {
            path,
            status: classify(code),
            untracked,
        });
    }
    Ok(entries)
}

/// Map a porcelain `XY` pair onto what the viewer shows.
///
/// The working tree is what is on screen, so its column decides: a path staged
/// as added and then deleted on disk (`AD`) is gone, and calling it "added"
/// would offer the user a file that is not there.
fn classify(code: &str) -> FileChange {
    let mut chars = code.chars();
    let index = chars.next().unwrap_or(' ');
    let worktree = chars.next().unwrap_or(' ');
    // Unmerged first: `DU`, `UD` and `AA` all read as an ordinary delete or add
    // on one column, and calling a live conflict "deleted" offers the reviewer
    // a resolution that would throw one side away.
    if index == 'U'
        || worktree == 'U'
        || (index == 'A' && worktree == 'A')
        || (index == 'D' && worktree == 'D')
    {
        return FileChange::Conflicted;
    }
    if worktree == 'D' || (worktree == ' ' && index == 'D') {
        return FileChange::Deleted;
    }
    if index == 'A' || index == '?' || worktree == '?' {
        return FileChange::Added;
    }
    FileChange::Modified
}

/// `path -> (additions, deletions, binary)` for everything tracked.
fn numstat(repo: &Path, base: Option<&str>) -> Result<HashMap<String, (u32, u32, bool)>, GitError> {
    let args = [
        "diff",
        "--no-ext-diff",
        "--no-renames",
        "--numstat",
        "-z",
        base.unwrap_or("HEAD"),
        "--",
        ".",
    ];
    let out = run_git(Some(repo), &args)?;
    if !out.success() {
        return Ok(HashMap::new());
    }

    let mut stats = HashMap::new();
    for record in out.stdout.split('\0') {
        // `<added>\t<deleted>\t<path>`, and `-` in place of a count is git's
        // way of saying "binary, there are no lines to count".
        let mut fields = record.splitn(3, '\t');
        let (Some(added), Some(deleted), Some(path)) =
            (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        if path.is_empty() {
            continue;
        }
        let binary = added == "-" || deleted == "-";
        stats.insert(
            path.to_string(),
            (
                added.parse().unwrap_or(0),
                deleted.parse().unwrap_or(0),
                binary,
            ),
        );
    }
    Ok(stats)
}

/// Every tracked patch in one subprocess, keyed by path.
///
/// Best effort on purpose: when the batch fails the caller falls back to one
/// command per file, which is slower but still correct.
fn batch_patches(repo: &Path, base: Option<&str>, context: u32) -> HashMap<String, String> {
    let unified = format!("--unified={context}");
    let args = [
        "diff",
        "--patch",
        "--no-ext-diff",
        "--no-renames",
        &unified,
        base.unwrap_or("HEAD"),
        "--",
        ".",
    ];
    let Ok(out) = run_git(Some(repo), &args) else {
        return HashMap::new();
    };
    if !out.success() {
        return HashMap::new();
    }
    split_patch(&out.stdout)
}

/// Split a multi-file patch on its `diff --git ` markers.
fn split_patch(text: &str) -> HashMap<String, String> {
    let mut chunks: Vec<(usize, usize)> = Vec::new();
    let mut starts: Vec<usize> = Vec::new();
    for (index, line) in line_offsets(text) {
        if line.starts_with("diff --git ") {
            starts.push(index);
        }
    }
    for (position, start) in starts.iter().enumerate() {
        let end = starts.get(position + 1).copied().unwrap_or(text.len());
        chunks.push((*start, end));
    }

    let mut patches: HashMap<String, String> = HashMap::new();
    for (start, end) in chunks {
        let chunk = &text[start..end];
        let Some(path) = path_from_chunk(chunk) else {
            continue;
        };
        patches.entry(path).or_default().push_str(chunk);
    }
    patches
}

/// `(byte offset, line)` for every line of `text`.
fn line_offsets(text: &str) -> impl Iterator<Item = (usize, &str)> {
    let mut offset = 0;
    text.split_inclusive('\n').map(move |line| {
        let start = offset;
        offset += line.len();
        (start, line.trim_end_matches('\n'))
    })
}

/// The path one chunk of a multi-file patch describes.
///
/// `+++ b/<path>` first, because it is the side that still exists and it is
/// never abbreviated; `--- a/<path>` for a deletion, where the `+++` line is
/// `/dev/null`. The `diff --git` header is the last resort: for a path with a
/// space it is genuinely ambiguous, which is why git puts the same information
/// on two unambiguous lines below it.
fn path_from_chunk(chunk: &str) -> Option<String> {
    let mut fallback = None;
    for (_, line) in line_offsets(chunk) {
        if let Some(rest) = line.strip_prefix("+++ ") {
            if rest != "/dev/null" {
                return Some(strip_prefix(rest));
            }
        } else if let Some(rest) = line.strip_prefix("--- ") {
            if rest != "/dev/null" {
                fallback = Some(strip_prefix(rest));
            }
        } else if line.starts_with("@@") {
            break;
        }
    }
    fallback.or_else(|| {
        let header = chunk.lines().next()?.strip_prefix("diff --git ")?;
        let separator = header.find(" b/")?;
        Some(strip_prefix(&header[separator + 1..]))
    })
}

/// Drop git's `a/` / `b/` diff prefix.
fn strip_prefix(path: &str) -> String {
    path.strip_prefix("a/")
        .or_else(|| path.strip_prefix("b/"))
        .unwrap_or(path)
        .to_string()
}

/// One tracked file's patch, for when the batch did not cover it.
fn tracked_patch(repo: &Path, base: Option<&str>, path: &str, context: u32) -> String {
    let unified = format!("--unified={context}");
    let args = [
        "diff",
        "--patch",
        "--no-ext-diff",
        "--no-renames",
        &unified,
        base.unwrap_or("HEAD"),
        "--",
        path,
    ];
    match run_git(Some(repo), &args) {
        Ok(out) if out.success() => out.stdout,
        _ => String::new(),
    }
}

/// An untracked file's patch, produced against `/dev/null`.
///
/// `--no-index` exits **1** when the two inputs differ, which for a new file is
/// always. That is the success case here, so only a status above 1 — git's own
/// error range — is treated as a failure.
fn untracked_patch(repo: &Path, path: &str, context: u32) -> String {
    let unified = format!("--unified={context}");
    let args = [
        "diff",
        "--no-index",
        "--patch",
        "--no-ext-diff",
        "--no-renames",
        &unified,
        "--",
        "/dev/null",
        path,
    ];
    match run_git(Some(repo), &args) {
        Ok(GitOutput { stdout, status, .. }) if status == 0 || status == 1 => stdout,
        _ => String::new(),
    }
}

fn is_binary_patch(patch: &str) -> bool {
    patch
        .lines()
        .any(|line| line.starts_with("Binary files ") || line.starts_with("GIT binary patch"))
}

/// `(additions, deletions)` read off a patch, for the files `numstat` never
/// covers — the untracked ones.
fn count_changed_lines(patch: &str) -> (u32, u32) {
    let mut additions = 0;
    let mut deletions = 0;
    let mut in_hunk = false;
    for line in patch.lines() {
        if line.starts_with("@@") {
            in_hunk = true;
            continue;
        }
        if !in_hunk {
            continue;
        }
        match line.as_bytes().first() {
            Some(b'+') => additions += 1,
            Some(b'-') => deletions += 1,
            _ => {}
        }
    }
    (additions, deletions)
}

// ---------------------------------------------------------------- summary ---

/// How many commits a summary lists before it says "and more".
pub const MAX_SUMMARY_COMMITS: usize = 50;

/// How many bytes of an untracked file are read to count its lines.
///
/// A summary needs a number, not the file. Reading a capped prefix keeps the
/// subprocess count flat — the alternative is one `--no-index` per untracked
/// path — and a file bigger than this reports what it counted and says so.
const MAX_UNTRACKED_SCAN_BYTES: u64 = 256 * 1024;

/// One changed path, without its patch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChangeSummaryLine {
    pub path: String,
    pub status: FileChange,
    pub additions: u32,
    pub deletions: u32,
    pub binary: bool,
}

/// One commit between a base and `HEAD`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitEntry {
    pub short_id: String,
    pub subject: String,
}

/// What changed since a base, counted rather than rendered.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SummaryOfChanges {
    pub branch: Option<String>,
    pub files: Vec<ChangeSummaryLine>,
    pub commits: Vec<CommitEntry>,
    pub commit_count: u32,
    pub truncated: bool,
}

/// Everything that changed since `base`, with no patches.
///
/// Four subprocesses whatever the number of changed files — `name-status`,
/// `status`, `numstat` and `log` — because this is what a panel beside a live
/// terminal refreshes on its own, and a per-file cost there would scale with
/// the work the agent is doing. Untracked additions are counted from a capped
/// read rather than a fifth process per file.
///
/// # Errors
/// [`GitError::CommandFailed`] when git cannot describe the working tree.
pub fn change_summary(repo: &Path, base: Option<&str>) -> Result<SummaryOfChanges, GitError> {
    let branch = current_branch(repo).ok().flatten();
    let head = base.is_some() || has_head(repo);
    let entries = changed_paths(repo, base)?;
    let mut truncated = entries.len() > MAX_DIFF_FILES;
    let entries: Vec<Entry> = entries.into_iter().take(MAX_DIFF_FILES).collect();

    let stats = if head {
        numstat(repo, base)?
    } else {
        HashMap::new()
    };

    let mut files = Vec::with_capacity(entries.len());
    for entry in entries {
        // `numstat` is the authority wherever it saw the path. It never sees an
        // untracked one, which is what the capped read below is for.
        let (additions, deletions, binary) = match stats.get(&entry.path) {
            Some(&stat) => stat,
            None if entry.untracked => {
                let (lines, capped) = untracked_lines(repo, &entry.path);
                truncated |= capped;
                (lines, 0, false)
            }
            None => (0, 0, false),
        };
        files.push(ChangeSummaryLine {
            path: entry.path,
            status: entry.status,
            additions,
            deletions,
            binary,
        });
    }

    let (commits, commit_count) = match base {
        Some(base) if head => commits_since(repo, base),
        _ => (Vec::new(), 0),
    };
    truncated |= commits.len() < commit_count as usize;

    tracing::debug!(target: "git", files = files.len(), commit_count, truncated, "diff.summary");
    Ok(SummaryOfChanges {
        branch,
        files,
        commits,
        commit_count,
        truncated,
    })
}

/// `(lines, capped)` for an untracked file, read no further than the cap.
///
/// `metadata()` before the read, and a `take` on the reader: the size is a
/// number off the filesystem and is clamped before anything is allocated, not
/// after a 4 GB file is already resident.
fn untracked_lines(repo: &Path, path: &str) -> (u32, bool) {
    use std::io::Read;

    let full = repo.join(path);
    let Ok(meta) = std::fs::metadata(&full) else {
        return (0, false);
    };
    if !meta.is_file() {
        return (0, false);
    }
    let capped = meta.len() > MAX_UNTRACKED_SCAN_BYTES;
    let Ok(file) = std::fs::File::open(&full) else {
        return (0, false);
    };
    let mut buffer = Vec::with_capacity(meta.len().min(MAX_UNTRACKED_SCAN_BYTES) as usize);
    if file
        .take(MAX_UNTRACKED_SCAN_BYTES)
        .read_to_end(&mut buffer)
        .is_err()
    {
        return (0, capped);
    }
    let newlines = u32::try_from(bytecount(&buffer, b'\n')).unwrap_or(u32::MAX);
    // A last line with no trailing newline is still a line.
    let trailing = u32::from(!buffer.is_empty() && buffer.last() != Some(&b'\n'));
    (newlines.saturating_add(trailing), capped)
}

fn bytecount(haystack: &[u8], needle: u8) -> usize {
    haystack.iter().filter(|byte| **byte == needle).count()
}

/// `(the newest commits, how many there are)` between `base` and `HEAD`.
///
/// The list is capped at [`MAX_SUMMARY_COMMITS`]; the count is not, so a
/// caller can say "and 40 more" without holding all of them.
#[must_use]
pub fn commits_since(repo: &Path, base: &str) -> (Vec<CommitEntry>, u32) {
    let range = format!("{base}..HEAD");
    let limit = format!("-n{}", MAX_SUMMARY_COMMITS + 1);
    let args = [
        "log",
        "--no-color",
        "--format=%h%x00%s",
        "-z",
        &limit,
        &range,
    ];
    let Ok(out) = run_git(Some(repo), &args) else {
        return (Vec::new(), 0);
    };
    if !out.success() {
        return (Vec::new(), 0);
    }

    let mut entries = Vec::new();
    for record in out.stdout.split('\0') {
        // `-z` separates records, and `%x00` separates the two fields inside
        // one, so a record is `<short id>` and the next is `<subject>`.
        if record.is_empty() {
            continue;
        }
        entries.push(record.to_string());
    }

    let mut commits = Vec::new();
    let mut fields = entries.into_iter();
    while let (Some(short_id), Some(subject)) = (fields.next(), fields.next()) {
        commits.push(CommitEntry {
            short_id,
            subject: subject.trim_start_matches('\n').to_string(),
        });
    }

    let count = u32::try_from(commits.len()).unwrap_or(u32::MAX);
    commits.truncate(MAX_SUMMARY_COMMITS);
    (commits, count)
}

/// How many commits `base..HEAD` holds, without listing them.
///
/// One subprocess per call, so a caller with many bases caps how many it
/// asks about rather than letting the count scale with the session list.
#[must_use]
pub fn commit_count(repo: &Path, base: &str) -> u32 {
    let range = format!("{base}..HEAD");
    let args = ["rev-list", "--count", &range];
    let Ok(out) = run_git(Some(repo), &args) else {
        return 0;
    };
    if !out.success() {
        return 0;
    }
    out.stdout_trimmed().parse().unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use tempfile::tempdir;

    fn git(repo: &Path, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(repo)
            .status()
            .expect("git");
        assert!(status.success(), "git {args:?}");
    }

    fn repo_with_commit(repo: &Path) {
        git(repo, &["init"]);
        git(repo, &["config", "user.email", "t@example.com"]);
        git(repo, &["config", "user.name", "t"]);
        std::fs::write(repo.join("a.txt"), "one\ntwo\nthree\n").unwrap();
        git(repo, &["add", "a.txt"]);
        git(repo, &["commit", "-m", "init"]);
    }

    #[test]
    fn a_modified_file_carries_its_own_patch_and_counts() {
        let dir = tempdir().unwrap();
        let repo = dir.path();
        repo_with_commit(repo);
        std::fs::write(repo.join("a.txt"), "one\nTWO\nthree\n").unwrap();

        let diff = working_tree_diff(repo, None, DIFF_CONTEXT_LINES).unwrap();
        assert_eq!(diff.files.len(), 1);
        let file = &diff.files[0];
        assert_eq!(file.path, "a.txt");
        assert_eq!(file.status, FileChange::Modified);
        assert_eq!((file.additions, file.deletions), (1, 1));
        assert!(file.patch.contains("+TWO"));
        assert!(file.patch.contains("-two"));
        assert!(!diff.truncated);
    }

    #[test]
    fn staged_unstaged_and_untracked_all_appear() {
        let dir = tempdir().unwrap();
        let repo = dir.path();
        repo_with_commit(repo);
        std::fs::write(repo.join("staged.txt"), "s\n").unwrap();
        git(repo, &["add", "staged.txt"]);
        std::fs::write(repo.join("a.txt"), "one\ntwo\nfour\n").unwrap();
        std::fs::write(repo.join("new.txt"), "fresh\n").unwrap();

        let diff = working_tree_diff(repo, None, DIFF_CONTEXT_LINES).unwrap();
        let paths: Vec<&str> = diff.files.iter().map(|f| f.path.as_str()).collect();
        assert!(paths.contains(&"staged.txt"), "{paths:?}");
        assert!(paths.contains(&"a.txt"), "{paths:?}");
        assert!(paths.contains(&"new.txt"), "{paths:?}");

        // An untracked file has no `numstat`, so its counts come off the patch.
        let new = diff.files.iter().find(|f| f.path == "new.txt").unwrap();
        assert_eq!(new.status, FileChange::Added);
        assert_eq!((new.additions, new.deletions), (1, 0));
        assert!(new.patch.contains("+fresh"));
    }

    #[test]
    fn a_deleted_file_is_reported_as_deleted() {
        let dir = tempdir().unwrap();
        let repo = dir.path();
        repo_with_commit(repo);
        std::fs::remove_file(repo.join("a.txt")).unwrap();

        let diff = working_tree_diff(repo, None, DIFF_CONTEXT_LINES).unwrap();
        assert_eq!(diff.files[0].status, FileChange::Deleted);
        assert_eq!(diff.files[0].deletions, 3);
    }

    #[test]
    fn a_repository_without_commits_treats_everything_as_new() {
        let dir = tempdir().unwrap();
        let repo = dir.path();
        git(repo, &["init"]);
        git(repo, &["config", "user.email", "t@example.com"]);
        git(repo, &["config", "user.name", "t"]);
        std::fs::write(repo.join("a.txt"), "one\n").unwrap();

        let diff = working_tree_diff(repo, None, DIFF_CONTEXT_LINES).unwrap();
        assert_eq!(diff.files.len(), 1);
        assert_eq!(diff.files[0].status, FileChange::Added);
        assert!(diff.files[0].patch.contains("+one"));
    }

    #[test]
    fn a_path_with_a_space_survives_the_round_trip() {
        let dir = tempdir().unwrap();
        let repo = dir.path();
        repo_with_commit(repo);
        std::fs::write(repo.join("two words.txt"), "hello\n").unwrap();
        git(repo, &["add", "two words.txt"]);
        git(repo, &["commit", "-m", "add"]);
        std::fs::write(repo.join("two words.txt"), "goodbye\n").unwrap();

        let diff = working_tree_diff(repo, None, DIFF_CONTEXT_LINES).unwrap();
        assert_eq!(diff.files[0].path, "two words.txt");
        assert!(diff.files[0].patch.contains("+goodbye"));
    }

    #[test]
    fn a_multi_file_patch_splits_by_path() {
        let text = "diff --git a/one.rs b/one.rs\n\
                    index 111..222 100644\n\
                    --- a/one.rs\n\
                    +++ b/one.rs\n\
                    @@ -1 +1 @@\n\
                    -a\n\
                    +b\n\
                    diff --git a/two.rs b/two.rs\n\
                    deleted file mode 100644\n\
                    --- a/two.rs\n\
                    +++ /dev/null\n\
                    @@ -1 +0,0 @@\n\
                    -gone\n";
        let patches = split_patch(text);
        assert_eq!(patches.len(), 2);
        assert!(patches["one.rs"].contains("+b"));
        assert!(patches["two.rs"].contains("-gone"));
    }

    #[test]
    fn the_worktree_column_decides_what_a_status_pair_means() {
        assert_eq!(classify("?? "), FileChange::Added);
        assert_eq!(classify("A  "), FileChange::Added);
        assert_eq!(classify("AD "), FileChange::Deleted);
        assert_eq!(classify("D  "), FileChange::Deleted);
        assert_eq!(classify(" M "), FileChange::Modified);
        assert_eq!(classify("MM "), FileChange::Modified);
    }

    /// The trap this whole `base` path exists for: `status` cannot see a file
    /// that a commit changed and left clean, so against a base the file list
    /// has to come from `diff --name-status` or the session's own commits
    /// vanish from its diff.
    #[test]
    fn a_base_sees_what_a_commit_changed_and_left_clean() {
        let dir = tempdir().unwrap();
        let repo = dir.path();
        repo_with_commit(repo);
        let base = crate::repository::head_commit(repo).unwrap();

        std::fs::write(repo.join("a.txt"), "one\nTWO\nthree\n").unwrap();
        git(repo, &["commit", "-am", "change a"]);

        // Nothing is dirty, so the working-tree read is empty…
        let clean = working_tree_diff(repo, None, DIFF_CONTEXT_LINES).unwrap();
        assert!(clean.files.is_empty());

        // …but the change is still what this session did.
        let since = working_tree_diff(repo, Some(&base), DIFF_CONTEXT_LINES).unwrap();
        assert_eq!(since.files.len(), 1);
        assert_eq!(since.files[0].path, "a.txt");
        assert_eq!(since.files[0].status, FileChange::Modified);
        assert!(since.files[0].patch.contains("+TWO"));
    }

    #[test]
    fn a_summary_counts_commits_and_untracked_lines_without_patches() {
        let dir = tempdir().unwrap();
        let repo = dir.path();
        repo_with_commit(repo);
        let base = crate::repository::head_commit(repo).unwrap();

        std::fs::write(repo.join("a.txt"), "one\nTWO\nthree\n").unwrap();
        git(repo, &["commit", "-am", "change a"]);
        std::fs::write(repo.join("new.txt"), "x\ny\n").unwrap();

        let summary = change_summary(repo, Some(&base)).unwrap();
        assert_eq!(summary.commit_count, 1);
        assert_eq!(summary.commits[0].subject, "change a");

        let mut paths: Vec<&str> = summary.files.iter().map(|f| f.path.as_str()).collect();
        paths.sort_unstable();
        assert_eq!(paths, ["a.txt", "new.txt"]);

        let tracked = summary.files.iter().find(|f| f.path == "a.txt").unwrap();
        assert_eq!((tracked.additions, tracked.deletions), (1, 1));
        // Untracked files are never in `numstat`; the capped read is what counts
        // them, and it counts a last line with no trailing newline too.
        let untracked = summary.files.iter().find(|f| f.path == "new.txt").unwrap();
        assert_eq!(untracked.additions, 2);
        assert_eq!(untracked.status, FileChange::Added);
        assert!(!summary.truncated);
    }

    #[test]
    fn a_summary_with_no_base_reports_the_working_tree_and_no_commits() {
        let dir = tempdir().unwrap();
        let repo = dir.path();
        repo_with_commit(repo);
        std::fs::write(repo.join("a.txt"), "one\nTWO\nthree\n").unwrap();

        let summary = change_summary(repo, None).unwrap();
        assert_eq!(summary.commit_count, 0);
        assert!(summary.commits.is_empty());
        assert_eq!(summary.files.len(), 1);
        assert_eq!(
            (summary.files[0].additions, summary.files[0].deletions),
            (1, 1)
        );
    }
}
