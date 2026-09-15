//! The standalone adapter: bytes in, bytes out, nothing else.
//!
//! The core owns no filesystem, so loading and saving live here. Both halves
//! are bounded before they allocate — the cap decides whether to read, not
//! what to do with what was already read — and a save writes an exclusively
//! created temp file and renames it, so a crash cannot leave half a file.

use std::fs::{File, OpenOptions};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use editor_core::limits::MAX_DOCUMENT_BYTES;

pub struct Loaded {
    pub bytes: Vec<u8>,
    pub revision: String,
}

#[derive(Debug, thiserror::Error)]
pub enum DiskError {
    #[error("file is larger than {limit} bytes")]
    TooLarge { limit: usize },
    #[error("{0}")]
    Io(#[from] std::io::Error),
}

/// Read `path`, refusing anything over the document budget without reading it.
///
/// `metadata` alone is not the cap: the file can grow between the check and the
/// read, so the reader is capped too and a result at the limit is refused.
pub fn load(path: &Path) -> Result<Loaded, DiskError> {
    let file = File::open(path)?;
    let size = file.metadata()?.len();
    if size > MAX_DOCUMENT_BYTES as u64 {
        return Err(DiskError::TooLarge {
            limit: MAX_DOCUMENT_BYTES,
        });
    }
    let mut bytes = Vec::with_capacity(size as usize + 1);
    file.take(MAX_DOCUMENT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_DOCUMENT_BYTES {
        return Err(DiskError::TooLarge {
            limit: MAX_DOCUMENT_BYTES,
        });
    }
    let revision = revision_of(&bytes);
    Ok(Loaded { bytes, revision })
}

/// The revision of what is on disk now, or `None` when the file is gone.
pub fn revision(path: &Path) -> Option<String> {
    load(path).ok().map(|loaded| loaded.revision)
}

/// Write `text` atomically, returning the revision now on disk.
pub fn save(path: &Path, text: &str) -> Result<String, DiskError> {
    let temp = temp_path(path)?;
    let guard = TempFile(temp);
    let mode = path.metadata().ok().map(|meta| meta.permissions());
    {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&guard.0)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
    }
    if let Some(mode) = mode {
        let _ = std::fs::set_permissions(&guard.0, mode);
    }
    std::fs::rename(&guard.0, path)?;
    std::mem::forget(guard);
    Ok(revision_of(text.as_bytes()))
}

/// Removes the temp file on every path out, including a failed write.
struct TempFile(PathBuf);

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// A name no concurrent save can pick.
///
/// `with_extension` is not enough: it maps `a.rs` and `a.md` to the same temp
/// path, so two saves in one directory would race for one file.
fn temp_path(path: &Path) -> Result<PathBuf, DiskError> {
    let directory = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("forge-editor");
    let pid = std::process::id();
    for attempt in 0..64 {
        let candidate = directory.join(format!(".{name}.forge-editor.{pid}.{attempt}"));
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    Err(DiskError::Io(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "no free temporary name next to the target",
    )))
}

/// Same shape as `fs-service::revision_of`, so the two adapters agree on what
/// "the disk has changed" means.
fn revision_of(bytes: &[u8]) -> String {
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_over_the_budget_is_refused_before_it_is_read() {
        let directory = tempfile::tempdir().expect("temp dir");
        let path = directory.path().join("big.txt");
        std::fs::write(&path, vec![b'a'; MAX_DOCUMENT_BYTES + 1]).expect("fixture");
        assert!(matches!(load(&path), Err(DiskError::TooLarge { .. })));
    }

    #[test]
    fn a_file_at_the_budget_still_loads() {
        let directory = tempfile::tempdir().expect("temp dir");
        let path = directory.path().join("edge.txt");
        std::fs::write(&path, vec![b'a'; MAX_DOCUMENT_BYTES]).expect("fixture");
        assert_eq!(load(&path).expect("loads").bytes.len(), MAX_DOCUMENT_BYTES);
    }

    #[test]
    fn saving_leaves_no_temp_file_and_changes_the_revision() {
        let directory = tempfile::tempdir().expect("temp dir");
        let path = directory.path().join("note.txt");
        std::fs::write(&path, "old").expect("fixture");
        let before = load(&path).expect("load").revision;

        let after = save(&path, "new").expect("save");
        assert_ne!(before, after);
        assert_eq!(std::fs::read_to_string(&path).expect("read back"), "new");
        assert_eq!(load(&path).expect("reload").revision, after);

        let leftovers: Vec<_> = std::fs::read_dir(directory.path())
            .expect("read dir")
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.contains("forge-editor"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "temp files left behind: {leftovers:?}"
        );
    }

    #[test]
    fn two_names_that_share_a_stem_do_not_share_a_temp_file() {
        let directory = tempfile::tempdir().expect("temp dir");
        let rust = directory.path().join("a.rs");
        let markdown = directory.path().join("a.md");
        assert_ne!(
            temp_path(&rust).expect("name"),
            temp_path(&markdown).expect("name")
        );
    }

    #[test]
    fn a_failed_write_removes_its_temp_file() {
        let directory = tempfile::tempdir().expect("temp dir");
        let path = directory.path().join("sub").join("note.txt");
        // The parent does not exist, so creating the temp file fails.
        assert!(save(&path, "x").is_err());
        assert!(!directory.path().join("sub").exists());
    }
}
