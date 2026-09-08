//! Shared helpers for this crate's unit tests: fake executable shell scripts
//! and a minimal [`ResolvedEnvironment`] whose PATH points at a temp directory.

use std::path::{Path, PathBuf};

use domain::{EnvSource, ResolvedEnvironment, Timestamp};

/// Write `body` to `dir/name`, mark it executable, and return the full path.
pub(crate) fn write_script(dir: &Path, name: &str, body: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, body).expect("write fake script");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).unwrap();
    }
    path
}

/// A `/bin/sh` script that prints `line` to stdout and exits 0.
pub(crate) fn echo_script(line: &str) -> String {
    format!("#!/bin/sh\necho '{line}'\n")
}

/// A [`ResolvedEnvironment`] whose PATH is exactly `dirs` and whose var set is
/// empty (detection then synthesizes PATH from `path_entries`).
pub(crate) fn env_with_path(dirs: Vec<PathBuf>) -> ResolvedEnvironment {
    ResolvedEnvironment {
        shell: PathBuf::from("/bin/sh"),
        vars: Vec::new(),
        path_entries: dirs,
        resolved_at: Timestamp::now(),
        source: EnvSource::ProcessFallback,
    }
}
