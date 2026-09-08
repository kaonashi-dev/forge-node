//! Test builders for [`domain::ResolvedEnvironment`] (§12).
//!
//! The daemon resolves a login-shell environment once and caches it; agents and
//! daemon tests need a plausible value without shelling out. These helpers build
//! one deterministically.

use std::path::PathBuf;

use domain::{EnvSource, ResolvedEnvironment, Timestamp};

/// A [`ResolvedEnvironment`] with the given PATH entries, an empty variable set,
/// and [`EnvSource::ProcessFallback`] — enough for tests that only care about
/// PATH lookup.
#[must_use]
pub fn resolved_env(path_entries: Vec<PathBuf>) -> ResolvedEnvironment {
    ResolvedEnvironment {
        shell: PathBuf::from("/bin/sh"),
        vars: Vec::new(),
        path_entries,
        resolved_at: Timestamp::now(),
        source: EnvSource::ProcessFallback,
    }
}

/// A [`ResolvedEnvironment`] with explicit variables and PATH entries, for tests
/// that need to assert variable lookup as well.
#[must_use]
pub fn resolved_env_with(
    shell: PathBuf,
    vars: Vec<(String, String)>,
    path_entries: Vec<PathBuf>,
) -> ResolvedEnvironment {
    ResolvedEnvironment {
        shell,
        vars,
        path_entries,
        resolved_at: Timestamp::now(),
        source: EnvSource::ProcessFallback,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolved_env_carries_path_entries() {
        let env = resolved_env(vec![
            PathBuf::from("/usr/local/bin"),
            PathBuf::from("/usr/bin"),
        ]);
        assert_eq!(env.source, EnvSource::ProcessFallback);
        assert_eq!(env.path_entries.len(), 2);
        assert_eq!(env.path_entries[0], PathBuf::from("/usr/local/bin"));
    }

    #[test]
    fn resolved_env_with_supports_lookup() {
        let env = resolved_env_with(
            PathBuf::from("/bin/zsh"),
            vec![("FOO".to_owned(), "bar".to_owned())],
            vec![PathBuf::from("/usr/bin")],
        );
        assert_eq!(env.get("FOO"), Some("bar"));
        assert_eq!(env.get("MISSING"), None);
    }
}
