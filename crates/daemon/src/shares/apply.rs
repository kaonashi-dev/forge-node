//! Filesystem and subprocess effects for [`super::plan`], including cleanup
//! verification. Call outside the core lock; planning policy lives in `plan.rs`.

use std::io;
use std::path::{Component, Path};
use std::time::Duration;

use domain::{
    ShareAction, ShareCleanup, ShareRule, ShareState, ShareStatusEntry, ShareStrategy, ShareVerb,
};

use super::plan::{plan_rule, Observation};
use super::{
    Capabilities, EntryKind, ShareContext, MAX_COPY_BYTES, MAX_COPY_ENTRIES, MAX_MEASURE_ENTRIES,
};

/// Why a path may not be shared. Refused, never quietly rewritten: a rule is
/// user input, and `copy = ["../../.ssh/id_rsa"]` must not become a duplicated
/// secret.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InvalidPath(pub String);

impl std::fmt::Display for InvalidPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Check one rule's path and return it in normalized form.
///
/// # Errors
/// [`InvalidPath`] when the path is absolute, escapes the workspace, names the
/// git directory, or is empty.
pub fn validate(relative: &str) -> Result<String, InvalidPath> {
    let trimmed = relative.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return Err(InvalidPath("the path is empty".to_owned()));
    }
    let path = Path::new(trimmed);
    if path.is_absolute() {
        return Err(InvalidPath(
            "the path must be relative to the repository root".to_owned(),
        ));
    }
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => {
                let part = part.to_string_lossy();
                if part == ".git" {
                    return Err(InvalidPath(
                        "the git directory is not something to share".to_owned(),
                    ));
                }
                parts.push(part.into_owned());
            }
            Component::CurDir => {}
            Component::ParentDir => {
                return Err(InvalidPath("the path may not contain `..`".to_owned()));
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(InvalidPath(
                    "the path must be relative to the repository root".to_owned(),
                ));
            }
        }
    }
    if parts.is_empty() {
        return Err(InvalidPath("the path is empty".to_owned()));
    }
    Ok(parts.join("/"))
}

/// What the platform and these two directories can do.
#[must_use]
pub fn capabilities(source: &Path, target: &Path) -> Capabilities {
    Capabilities {
        supports_clone: cfg!(any(target_os = "macos", target_os = "linux")),
        same_filesystem: same_device(source, target),
    }
}

#[cfg(unix)]
fn same_device(a: &Path, b: &Path) -> bool {
    use std::os::unix::fs::MetadataExt as _;
    let (Ok(a), Ok(b)) = (std::fs::metadata(a), std::fs::metadata(b)) else {
        return false;
    };
    a.dev() == b.dev()
}

#[cfg(not(unix))]
fn same_device(_a: &Path, _b: &Path) -> bool {
    false
}

/// Read what one rule's three paths currently look like.
#[must_use]
pub fn observe(ctx: &ShareContext, rule: &ShareRule) -> Observation {
    let Ok(relative) = validate(&rule.path) else {
        return Observation::default();
    };
    let source = ctx.source.join(&relative);
    let target = ctx.target.join(&relative);
    let store = ctx.store_path(&relative);

    let source_kind = EntryKind::of(&source);
    let (bytes, entries) = match source_kind {
        // A clone does not read the tree, but the plan needs the number to
        // decide whether a *fallback* copy is affordable.
        EntryKind::File | EntryKind::Dir => measure(&source),
        _ => (None, None),
    };

    let target_kind = EntryKind::of(&target);
    let link_is_ours = target_kind == EntryKind::Symlink
        && store
            .as_ref()
            .is_some_and(|store| std::fs::read_link(&target).is_ok_and(|dest| &dest == store));

    Observation {
        source: source_kind,
        target: target_kind,
        store: store.as_deref().map_or(EntryKind::Missing, EntryKind::of),
        link_is_ours,
        bytes,
        entries,
    }
}

/// Bytes and entries under `path`, or `(None, None)` past the walk budget.
///
/// The budget is checked while walking, not after: the point of a bounded walk
/// is that a monorepo cannot make provisioning take minutes.
#[must_use]
pub fn measure(path: &Path) -> (Option<u64>, Option<u64>) {
    measure_with_limit(path, MAX_MEASURE_ENTRIES)
}

fn measure_with_limit(path: &Path, limit: u64) -> (Option<u64>, Option<u64>) {
    let mut bytes = 0_u64;
    let mut entries = 0_u64;
    let mut discovered = 1_u64;
    let mut stack = vec![path.to_path_buf()];
    while let Some(current) = stack.pop() {
        let Ok(meta) = std::fs::symlink_metadata(&current) else {
            continue;
        };
        entries += 1;
        if entries > limit {
            return (None, None);
        }
        if meta.is_symlink() {
            continue;
        }
        if meta.is_file() {
            bytes = bytes.saturating_add(meta.len());
            continue;
        }
        if meta.is_dir() {
            let Ok(read) = std::fs::read_dir(&current) else {
                continue;
            };
            for entry in read.flatten() {
                if discovered >= limit {
                    return (None, None);
                }
                discovered += 1;
                stack.push(entry.path());
            }
        }
    }
    (Some(bytes), Some(entries))
}

/// Apply a project's rules to one workspace, in order, `Run` rules last.
///
/// Never fatal: a rule that cannot be applied becomes a `Skip` with a note, and
/// the workspace stays usable. The checkout already exists by the time this
/// runs, and refusing to hand it over would leave the user worse off.
#[must_use]
pub fn apply_rules(ctx: &ShareContext, rules: &[ShareRule]) -> Vec<ShareAction> {
    let mut ordered: Vec<&ShareRule> = rules.iter().collect();
    ordered.sort_by_key(|rule| {
        (
            u8::from(matches!(rule.strategy, ShareStrategy::Run { .. })),
            rule.position,
        )
    });

    ordered
        .into_iter()
        .map(|rule| {
            let seen = observe(ctx, rule);
            let action = plan_rule(rule, seen, ctx.caps, ctx.explicit);
            perform(ctx, rule, action)
        })
        .collect()
}

/// What applying would do, without doing it.
#[must_use]
pub fn preview(ctx: &ShareContext, rules: &[ShareRule]) -> Vec<ShareAction> {
    rules
        .iter()
        .map(|rule| plan_rule(rule, observe(ctx, rule), ctx.caps, ctx.explicit))
        .collect()
}

fn perform(ctx: &ShareContext, rule: &ShareRule, mut action: ShareAction) -> ShareAction {
    let relative = match validate(&rule.path) {
        Ok(relative) => relative,
        Err(e) => {
            action.verb = ShareVerb::Skip;
            action.note = Some(e.0);
            return action;
        }
    };
    let target = ctx.target.join(&relative);

    let outcome = match action.verb {
        ShareVerb::Copy | ShareVerb::Clone => {
            let source = ctx.source.join(&relative);
            write_into(ctx, &source, &target, action.verb == ShareVerb::Clone)
        }
        ShareVerb::Link => ctx.store_path(&relative).map_or_else(
            || Err(io::Error::other("the project has no shared store")),
            |store| link_into(ctx, &store, &target),
        ),
        ShareVerb::Run => match &rule.strategy {
            ShareStrategy::Run {
                command,
                timeout_secs,
            } => run_in(
                &ctx.target,
                command,
                Duration::from_secs((*timeout_secs).clamp(1, 3600)),
            ),
            _ => Ok(()),
        },
        // `Backup`/`Remove` are produced by the cleanup path, which performs
        // its own effects; everything else is a no-op here.
        _ => Ok(()),
    };

    if let Err(e) = outcome {
        action.verb = ShareVerb::Skip;
        // A path, never a content: a failing `.env` copy must not print the
        // secret into the daemon log.
        action.note = Some(e.to_string());
    }
    action
}

/// Copy or clone `source` onto `target`, backing up whatever is in the way.
fn write_into(ctx: &ShareContext, source: &Path, target: &Path, cloning: bool) -> io::Result<()> {
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if EntryKind::of(target).exists() {
        backup(ctx, target, false)?;
        remove_any(target)?;
    }
    if cloning && clone_path(source, target).is_ok() {
        return Ok(());
    }
    copy_tree(source, target, &mut Budget::default())
}

/// Replace `target` with a symlink to `store`.
fn link_into(ctx: &ShareContext, store: &Path, target: &Path) -> io::Result<()> {
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if EntryKind::of(target).exists() {
        backup(ctx, target, false)?;
        remove_any(target)?;
    }
    symlink(store, target)
}

#[cfg(unix)]
fn symlink(source: &Path, target: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(source, target)
}

#[cfg(not(unix))]
fn symlink(_source: &Path, _target: &Path) -> io::Result<()> {
    Err(io::Error::other(
        "symlinks are not supported on this platform",
    ))
}

/// Copy-on-write clone, where the platform has one.
///
/// macOS `clonefile(2)` clones a whole tree in one call; Linux's `FICLONE`
/// ioctl is per file, so a directory falls back to the recursive copy that
/// `write_into` performs when this returns an error.
#[cfg(target_os = "macos")]
fn clone_path(source: &Path, target: &Path) -> io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt as _;

    let src = CString::new(source.as_os_str().as_bytes())
        .map_err(|_| io::Error::other("path contains a NUL byte"))?;
    let dst = CString::new(target.as_os_str().as_bytes())
        .map_err(|_| io::Error::other("path contains a NUL byte"))?;
    // SAFETY: both pointers are valid, NUL-terminated C strings that outlive
    // the call, and `clonefile` neither retains nor frees them.
    let rc = unsafe { nix::libc::clonefile(src.as_ptr(), dst.as_ptr(), 0) };
    if rc == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(target_os = "linux")]
fn clone_path(source: &Path, target: &Path) -> io::Result<()> {
    use std::os::fd::AsRawFd as _;
    use std::os::unix::fs::OpenOptionsExt as _;

    // `FICLONE` is file-to-file; a directory is left to the recursive copy.
    if !source.is_file() {
        return Err(io::Error::other(
            "only files can be cloned on this platform",
        ));
    }
    const FICLONE: nix::libc::c_ulong = 0x4004_9409;
    let src = std::fs::File::open(source)?;
    let permissions = src.metadata()?.permissions();
    let dst = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(target)?;
    // SAFETY: both descriptors are open for the duration of the call and
    // `FICLONE` takes the source fd by value, writing nothing to userspace.
    let rc = unsafe { nix::libc::ioctl(dst.as_raw_fd(), FICLONE, src.as_raw_fd()) };
    if rc == 0 {
        dst.set_permissions(permissions)
    } else {
        let err = io::Error::last_os_error();
        let _ = std::fs::remove_file(target);
        Err(err)
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn clone_path(_source: &Path, _target: &Path) -> io::Result<()> {
    Err(io::Error::other("this platform cannot clone"))
}

/// Bytes and entries a single copy may still write.
struct Budget {
    bytes: u64,
    entries: u64,
}

impl Default for Budget {
    fn default() -> Self {
        Self {
            bytes: MAX_COPY_BYTES,
            entries: MAX_COPY_ENTRIES,
        }
    }
}

/// Recursive copy that preserves modes and **recreates symlinks as symlinks**.
///
/// Following them would turn `node_modules/.bin/tsc` into a duplicate of the
/// binary it points at, and a `.env` that is itself a link into a copy of the
/// secret. The budget is spent as the walk goes, so an oversized tree stops
/// early instead of filling the disk and then being reported.
fn copy_tree(source: &Path, target: &Path, budget: &mut Budget) -> io::Result<()> {
    let meta = std::fs::symlink_metadata(source)?;
    budget.entries = budget
        .entries
        .checked_sub(1)
        .ok_or_else(|| io::Error::other("over the copy budget of entries"))?;

    if meta.is_symlink() {
        let dest = std::fs::read_link(source)?;
        return symlink(&dest, target);
    }
    if meta.is_file() {
        budget.bytes = budget
            .bytes
            .checked_sub(meta.len())
            .ok_or_else(|| io::Error::other("over the copy budget of bytes"))?;
        std::fs::copy(source, target)?;
        // A copied `.env` carries credentials; it keeps the source's mode.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = meta.permissions().mode();
            let _ = std::fs::set_permissions(target, std::fs::Permissions::from_mode(mode));
        }
        return Ok(());
    }
    if meta.is_dir() {
        std::fs::create_dir_all(target)?;
        for entry in std::fs::read_dir(source)? {
            let entry = entry?;
            copy_tree(&entry.path(), &target.join(entry.file_name()), budget)?;
        }
        return Ok(());
    }
    Err(io::Error::other("not a file, directory or symlink"))
}

/// Adoption still needs its source; replacements may move theirs aside.
fn backup(ctx: &ShareContext, path: &Path, preserve_source: bool) -> io::Result<()> {
    let Some(root) = ctx.backups.as_ref() else {
        return Ok(());
    };
    let stamp = domain::Timestamp::now()
        .to_rfc3339()
        .replace([':', '.'], "-");
    let name = path
        .file_name()
        .map_or_else(|| "entry".to_owned(), |n| n.to_string_lossy().into_owned());
    let dir = root.join(stamp);
    std::fs::create_dir_all(&dir)?;
    let into = dir.join(name);
    if !preserve_source && std::fs::rename(path, &into).is_ok() {
        return Ok(());
    }
    if preserve_source && clone_path(path, &into).is_ok() {
        return Ok(());
    }
    copy_tree(path, &into, &mut Budget::default())
}

fn remove_any(path: &Path) -> io::Result<()> {
    match EntryKind::of(path) {
        EntryKind::Dir => std::fs::remove_dir_all(path),
        EntryKind::Missing => Ok(()),
        _ => std::fs::remove_file(path),
    }
}

/// `sh -c` from the workspace root, in its own process group.
///
/// `wait_with_output` drains both pipes before it waits, so killing only the
/// direct child would leave a grandchild holding the write ends: the kill goes
/// to the **negative** pgid first, with the direct pid as a fallback.
fn run_in(workspace: &Path, command: &str, timeout: Duration) -> io::Result<()> {
    use std::os::unix::process::CommandExt as _;
    use std::process::{Command, Stdio};

    let child = Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(workspace)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()?;
    let pid = child.id();

    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(child.wait_with_output());
    });

    match rx.recv_timeout(timeout) {
        Ok(Ok(output)) if output.status.success() => {
            tracing::debug!(
                target: "git",
                stdout = %String::from_utf8_lossy(&output.stdout).trim(),
                "shares.run"
            );
            Ok(())
        }
        Ok(Ok(output)) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let tail = stderr.trim().lines().last().unwrap_or("").to_owned();
            Err(io::Error::other(format!(
                "the command failed (status {}): {tail}",
                output.status.code().unwrap_or(-1)
            )))
        }
        Ok(Err(e)) => Err(e),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            #[allow(clippy::cast_possible_wrap)]
            let raw = pid as i32;
            let _ = nix::sys::signal::kill(
                nix::unistd::Pid::from_raw(-raw),
                nix::sys::signal::Signal::SIGKILL,
            );
            let _ = nix::sys::signal::kill(
                nix::unistd::Pid::from_raw(raw),
                nix::sys::signal::Signal::SIGKILL,
            );
            let _ = rx.recv_timeout(Duration::from_secs(5));
            Err(io::Error::other(format!(
                "the command timed out after {}s and was killed",
                timeout.as_secs()
            )))
        }
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            Err(io::Error::other("the command's worker thread disconnected"))
        }
    }
}

/// What each rule looks like in this workspace right now.
#[must_use]
pub fn status(ctx: &ShareContext, rules: &[ShareRule]) -> Vec<ShareStatusEntry> {
    rules
        .iter()
        .map(|rule| ShareStatusEntry {
            rule_id: rule.id,
            path: rule.path.clone(),
            state: state_of(ctx, rule),
        })
        .collect()
}

fn state_of(ctx: &ShareContext, rule: &ShareRule) -> ShareState {
    let relative = match validate(&rule.path) {
        Ok(relative) => relative,
        Err(e) => return ShareState::Skipped { reason: e.0 },
    };
    let target = ctx.target.join(&relative);
    let seen = EntryKind::of(&target);

    match &rule.strategy {
        ShareStrategy::Link => match seen {
            EntryKind::Missing => ShareState::Missing,
            EntryKind::Symlink => {
                let ours = ctx.store_path(&relative).is_some_and(|store| {
                    std::fs::read_link(&target).is_ok_and(|dest| dest == store)
                });
                if ours {
                    ShareState::Applied
                } else {
                    ShareState::Skipped {
                        reason: "a symlink Forge did not write".to_owned(),
                    }
                }
            }
            // The failure this status exists for: an editor's `rename()` left a
            // regular file where the link was, and sharing stopped in silence.
            _ => ShareState::Severed,
        },
        ShareStrategy::Copy | ShareStrategy::Clone => {
            if !seen.exists() {
                return ShareState::Missing;
            }
            let source = ctx.source.join(&relative);
            if diverged(&source, &target) {
                ShareState::Diverged
            } else {
                ShareState::Applied
            }
        }
        ShareStrategy::Run { .. } => {
            if seen.exists() {
                ShareState::Applied
            } else {
                ShareState::Missing
            }
        }
        _ => ShareState::Skipped {
            reason: "strategy not supported by this build".to_owned(),
        },
    }
}

// Status is a cheap heuristic; destructive cleanup needs verified_copy instead.
fn diverged(source: &Path, target: &Path) -> bool {
    const CONTENT_COMPARE_MAX: u64 = 1024 * 1024;

    let (Ok(a), Ok(b)) = (
        std::fs::symlink_metadata(source),
        std::fs::symlink_metadata(target),
    ) else {
        return false;
    };
    if a.is_dir() || b.is_dir() {
        return false;
    }
    if a.len() != b.len() {
        return true;
    }
    if a.len() > CONTENT_COMPARE_MAX {
        return false;
    }
    match (std::fs::read(source), std::fs::read(target)) {
        (Ok(a), Ok(b)) => a != b,
        _ => false,
    }
}

fn verified_copy(source: &Path, target: &Path) -> bool {
    use std::io::Read as _;

    const MAX_COMPARE_BYTES: u64 = 1024 * 1024;
    let read = |path: &Path| -> io::Result<Vec<u8>> {
        let meta = std::fs::symlink_metadata(path)?;
        if !meta.is_file() || meta.len() > MAX_COMPARE_BYTES {
            return Err(io::Error::other(
                "copy cannot be verified within the cleanup budget",
            ));
        }
        let mut bytes = Vec::new();
        std::fs::File::open(path)?
            .take(MAX_COMPARE_BYTES + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_COMPARE_BYTES {
            return Err(io::Error::other("copy grew beyond the cleanup budget"));
        }
        Ok(bytes)
    };
    matches!((read(source), read(target)), (Ok(a), Ok(b)) if a == b)
}

/// Move a real file into the project's shared store and leave a link.
///
/// The one operation that changes the user's primary checkout, which is why it
/// is explicit, backs the original up first, and refuses rather than guesses.
///
/// # Errors
/// [`io::Error`] when the project has no store, the path is invalid, the source
/// is not a plain file, or the move fails.
pub fn adopt_into_store(ctx: &ShareContext, relative: &str) -> io::Result<()> {
    let relative = validate(relative).map_err(|e| io::Error::other(e.0))?;
    let store = ctx
        .store_path(&relative)
        .ok_or_else(|| io::Error::other("the project has no shared store"))?;
    let source = ctx.source.join(&relative);

    if store.exists() {
        return Err(io::Error::other("already in the shared store"));
    }
    match EntryKind::of(&source) {
        EntryKind::File | EntryKind::Dir => {}
        EntryKind::Symlink => {
            return Err(io::Error::other("already a link"));
        }
        _ => return Err(io::Error::other("not present in the source workspace")),
    }

    if let Some(parent) = store.parent() {
        std::fs::create_dir_all(parent)?;
    }
    backup(ctx, &source, true)?;
    if std::fs::rename(&source, &store).is_err() {
        copy_tree(&source, &store, &mut Budget::default())?;
        remove_any(&source)?;
    }
    symlink(&store, &source)
}

/// Copy a store file back into a workspace as a real file.
///
/// # Errors
/// [`io::Error`] when the store has no such file or the copy fails.
pub fn materialize(ctx: &ShareContext, relative: &str) -> io::Result<()> {
    let relative = validate(relative).map_err(|e| io::Error::other(e.0))?;
    let store = ctx
        .store_path(&relative)
        .ok_or_else(|| io::Error::other("the project has no shared store"))?;
    if !store.exists() {
        return Err(io::Error::other("not in the shared store"));
    }
    let target = ctx.target.join(&relative);
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if EntryKind::of(&target) == EntryKind::Symlink {
        std::fs::remove_file(&target)?;
    } else if EntryKind::of(&target).exists() {
        return Err(io::Error::other("a real file is already there"));
    }
    copy_tree(&store, &target, &mut Budget::default())
}

/// Undo one rule in one workspace, as far as the cleanup mode allows.
///
/// `RemoveInjected` deletes only what can still be recognized as Forge's: a
/// link into the store, or a copy that has not been touched since. A modified
/// file is somebody's work and is reported, not removed.
#[must_use]
pub fn clean_up(ctx: &ShareContext, rule: &ShareRule, mode: ShareCleanup) -> ShareAction {
    let mut action = ShareAction {
        rule_id: rule.id,
        path: rule.path.clone(),
        verb: ShareVerb::Skip,
        bytes: None,
        fallback: false,
        note: None,
    };
    let relative = match validate(&rule.path) {
        Ok(relative) => relative,
        Err(e) => {
            action.note = Some(e.0);
            return action;
        }
    };
    let target = ctx.target.join(&relative);

    // The source workspace's file is the original, not an injection: taking it
    // away would delete the only copy of the user's `.env` and leave every
    // other workspace pointing at nothing.
    if ctx.target_is_source() && mode == ShareCleanup::RemoveInjected {
        action.note = Some("the source workspace keeps its own file".to_owned());
        return action;
    }

    match mode {
        ShareCleanup::Leave => action.note = Some("left as it is".to_owned()),
        ShareCleanup::Materialize => match materialize(ctx, &relative) {
            Ok(()) => {
                action.verb = ShareVerb::Copy;
                action.note = Some("kept as a real file".to_owned());
            }
            Err(e) => action.note = Some(e.to_string()),
        },
        ShareCleanup::RemoveInjected => match state_of(ctx, rule) {
            ShareState::Applied
                if match rule.strategy {
                    ShareStrategy::Link => true,
                    ShareStrategy::Copy | ShareStrategy::Clone => {
                        verified_copy(&ctx.source.join(&relative), &target)
                    }
                    _ => false,
                } =>
            {
                match remove_any(&target) {
                    Ok(()) => action.verb = ShareVerb::Remove,
                    Err(e) => action.note = Some(e.to_string()),
                }
            }
            ShareState::Missing => action.note = Some("nothing there".to_owned()),
            // Severed, diverged or foreign: somebody's file now.
            _ => {
                action.note =
                    Some("changed or could not verify Forge's copy — left alone".to_owned());
            }
        },
        _ => action.note = Some("unknown cleanup mode".to_owned()),
    }
    action
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context(root: &Path) -> ShareContext {
        let source = root.join("source");
        let target = root.join("target");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::create_dir_all(&target).unwrap();
        ShareContext {
            source,
            target,
            store: Some(root.join("store")),
            backups: Some(root.join("backups")),
            caps: Capabilities {
                supports_clone: false,
                same_filesystem: true,
            },
            explicit: true,
        }
    }

    #[test]
    fn adoption_preserves_source_and_backup_on_the_same_filesystem() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context(dir.path());
        std::fs::write(ctx.source.join(".env"), "secret").unwrap();
        adopt_into_store(&ctx, ".env").unwrap();
        assert_eq!(std::fs::read(ctx.source.join(".env")).unwrap(), b"secret");
        assert_eq!(
            std::fs::read(ctx.store_path(".env").unwrap()).unwrap(),
            b"secret"
        );
        assert!(ctx.source.join(".env").is_symlink());
        let backup_dir = std::fs::read_dir(ctx.backups.unwrap())
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        assert_eq!(std::fs::read(backup_dir.join(".env")).unwrap(), b"secret");
    }

    #[test]
    fn cleanup_preserves_unverified_copies_and_run_outputs() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context(dir.path());
        let rule = ShareRule {
            id: domain::ShareRuleId::new(),
            project_id: domain::ProjectId::new(),
            path: "entry".into(),
            strategy: ShareStrategy::Copy,
            enabled: true,
            position: 0,
            created_at: domain::Timestamp::now(),
        };
        let source = ctx.source.join("entry");
        let target = ctx.target.join("entry");
        std::fs::create_dir(&source).unwrap();
        std::fs::create_dir(&target).unwrap();
        std::fs::write(target.join("user-work"), "keep").unwrap();
        assert_eq!(
            clean_up(&ctx, &rule, ShareCleanup::RemoveInjected).verb,
            ShareVerb::Skip
        );
        assert_eq!(std::fs::read(target.join("user-work")).unwrap(), b"keep");

        let rule = ShareRule {
            path: "large".into(),
            ..rule
        };
        let source = ctx.source.join("large");
        let target = ctx.target.join("large");
        std::fs::write(&source, vec![b'a'; 1024 * 1024 + 1]).unwrap();
        std::fs::write(&target, vec![b'b'; 1024 * 1024 + 1]).unwrap();
        assert_eq!(
            clean_up(&ctx, &rule, ShareCleanup::RemoveInjected).verb,
            ShareVerb::Skip
        );
        assert!(target.exists());
        std::fs::remove_file(&source).unwrap();
        assert_eq!(
            clean_up(&ctx, &rule, ShareCleanup::RemoveInjected).verb,
            ShareVerb::Skip
        );
        assert!(target.exists());

        std::fs::write(&source, "same").unwrap();
        std::fs::write(&target, "same").unwrap();
        let run = ShareRule {
            strategy: ShareStrategy::Run {
                command: "true".into(),
                timeout_secs: 1,
            },
            ..rule.clone()
        };
        assert_eq!(
            clean_up(&ctx, &run, ShareCleanup::RemoveInjected).verb,
            ShareVerb::Skip
        );
        assert!(target.exists());
        assert_eq!(
            clean_up(&ctx, &rule, ShareCleanup::RemoveInjected).verb,
            ShareVerb::Remove
        );
        assert!(!target.exists());
    }

    #[test]
    fn measurement_stops_at_discovery_budget() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..5 {
            std::fs::write(dir.path().join(i.to_string()), "x").unwrap();
        }
        assert_eq!(measure_with_limit(dir.path(), 5), (None, None));
        assert_eq!(measure_with_limit(dir.path(), 6), (Some(5), Some(6)));
    }
}
