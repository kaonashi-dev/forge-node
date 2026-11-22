//! The macOS sandbox a spec run's check runs under: Forge's home and socket out of reach, writes
//! only to the coder's checkout, the run's `checks/`, the temp directory and the user's tool caches.

use std::path::{Path, PathBuf};

use crate::core::Daemon;
use crate::sandbox::canonical;

/// `configured` with `~` expanded against `home`, canonical, without what does not exist.
pub(crate) fn write_paths(configured: &[String], home: Option<&Path>) -> Vec<PathBuf> {
    configured
        .iter()
        .filter_map(|raw| {
            let path = match raw.strip_prefix('~') {
                Some("") => home?.to_path_buf(),
                Some(rest) => home?.join(rest.strip_prefix('/')?),
                None => PathBuf::from(raw),
            };
            path.is_absolute()
                .then(|| std::fs::canonicalize(&path).ok())
                .flatten()
        })
        .collect()
}

impl Daemon {
    /// The SBPL profile for one check, or why it runs unsandboxed.
    pub(crate) fn check_profile(
        &self,
        checkout: &Path,
        checks_dir: &Path,
        offline: bool,
    ) -> Result<String, String> {
        if !cfg!(target_os = "macos") {
            return Err("Forge sandboxes checks only on macOS".to_owned());
        }
        if !Path::new(runtime_sandbox::SANDBOX_EXEC).exists() {
            return Err(format!("{} is missing", runtime_sandbox::SANDBOX_EXEC));
        }
        let home = self
            .launch_var("HOME")
            .or_else(|| std::env::var("HOME").ok())
            .map(PathBuf::from);
        let (denied_reads, socket) = self.forge_denied();
        Ok(runtime_sandbox::profile(&runtime_sandbox::Policy {
            run_dir: canonical(checkout),
            cache_dir: canonical(checks_dir),
            tmp_dir: canonical(&std::env::temp_dir()),
            extra_writes: write_paths(&self.config.spec_runs.check_write_paths, home.as_deref()),
            denied_reads,
            network: !offline,
            socket,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_paths_expand_the_home_and_drop_what_is_missing_or_relative() {
        let dir = tempfile::tempdir().expect("tempdir");
        let home = dir.path().canonicalize().expect("canonical");
        std::fs::create_dir_all(home.join(".cargo")).expect("fixture");
        std::fs::create_dir_all(home.join("abs")).expect("fixture");
        let configured = [
            "~/.cargo".to_owned(),
            "~/.rustup".to_owned(),
            "~".to_owned(),
            "~other/.cargo".to_owned(),
            "relative/dir".to_owned(),
            home.join("abs").to_string_lossy().into_owned(),
        ];
        assert_eq!(
            write_paths(&configured, Some(&home)),
            [home.join(".cargo"), home.clone(), home.join("abs")]
        );
        assert_eq!(
            write_paths(&configured[..1], None),
            Vec::<PathBuf>::new(),
            "without a home, `~` names nothing"
        );
    }
}
