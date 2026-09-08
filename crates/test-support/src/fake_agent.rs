//! Fake agent executables for provider-detection tests (§21 "Determinismo",
//! §7.5/§7.6 detection).
//!
//! Detection probes a candidate binary by running it with `--version` and
//! matching a substring of its output. These helpers write a tiny `#!/bin/sh`
//! script that echoes a caller-chosen version string and mark it executable, so
//! a test can exercise the real probe path against a deterministic "agent"
//! without depending on any provider being installed.
//!
//! ```no_run
//! use test_support::fake_agent::fake_agent_in_tempdir;
//! use std::process::Command;
//!
//! let (_dir, path) = fake_agent_in_tempdir("cursor-agent", "cursor-agent 1.4.2");
//! let out = Command::new(&path).arg("--version").output().unwrap();
//! assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "cursor-agent 1.4.2");
//! ```
//!
//! Unix-only: the executable bit is set with
//! [`std::os::unix::fs::PermissionsExt`].

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use tempfile::TempDir;

/// Quote `s` as a single POSIX-shell single-quoted literal, safe to embed in a
/// generated script regardless of the characters it contains.
fn sh_single_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for ch in s.chars() {
        if ch == '\'' {
            // Close the quote, emit an escaped quote, reopen.
            out.push_str("'\\''");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}

/// Write `script` to `dir/name` and mark it executable (`0755`), returning its
/// path.
fn write_executable(dir: &Path, name: &str, script: &str) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, script).expect("write fake agent script");
    let mut perms = fs::metadata(&path)
        .expect("stat fake agent script")
        .permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&path, perms).expect("chmod fake agent script");
    path
}

/// Write an executable `#!/bin/sh` fake agent named `name` into `dir` that prints
/// `version_output` (followed by a newline) to stdout, and return its path.
///
/// Panics on I/O failure, which in a test almost always means a broken fixture.
#[must_use]
pub fn write_fake_agent(dir: &Path, name: &str, version_output: &str) -> PathBuf {
    let script = format!(
        "#!/bin/sh\nprintf '%s\\n' {}\n",
        sh_single_quote(version_output)
    );
    write_executable(dir, name, &script)
}

/// Like [`write_fake_agent`], but the script sleeps for `sleep` before printing,
/// for exercising probe-timeout handling (§7.5 `VersionProbe::timeout_ms`).
#[must_use]
pub fn write_slow_fake_agent(
    dir: &Path,
    name: &str,
    version_output: &str,
    sleep: Duration,
) -> PathBuf {
    let script = format!(
        "#!/bin/sh\nsleep {:.3}\nprintf '%s\\n' {}\n",
        sleep.as_secs_f64(),
        sh_single_quote(version_output)
    );
    write_executable(dir, name, &script)
}

/// Create a fresh temp directory containing a fake agent, returning the
/// [`TempDir`] (keep it alive to keep the file) and the executable's path.
#[must_use]
pub fn fake_agent_in_tempdir(name: &str, version_output: &str) -> (TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("create tempdir for fake agent");
    let path = write_fake_agent(dir.path(), name, version_output);
    (dir, path)
}

/// Builder for a fake agent executable, for tests that want to tweak behaviour
/// (e.g. add a startup delay) before writing.
///
/// ```no_run
/// use test_support::fake_agent::FakeAgent;
/// use std::time::Duration;
///
/// let (_dir, path) = FakeAgent::new("slow-agent", "slow-agent 0.1.0")
///     .with_sleep(Duration::from_millis(500))
///     .in_tempdir();
/// ```
#[derive(Clone, Debug)]
pub struct FakeAgent {
    name: String,
    version_output: String,
    sleep: Option<Duration>,
}

impl FakeAgent {
    /// Start describing a fake agent named `name` that reports `version_output`.
    #[must_use]
    pub fn new(name: impl Into<String>, version_output: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            version_output: version_output.into(),
            sleep: None,
        }
    }

    /// Make the script sleep for `sleep` before printing its version, to trigger
    /// probe timeouts.
    #[must_use]
    pub fn with_sleep(mut self, sleep: Duration) -> Self {
        self.sleep = Some(sleep);
        self
    }

    /// Write the agent into `dir`, returning its path.
    #[must_use]
    pub fn write_to(&self, dir: &Path) -> PathBuf {
        match self.sleep {
            Some(sleep) => write_slow_fake_agent(dir, &self.name, &self.version_output, sleep),
            None => write_fake_agent(dir, &self.name, &self.version_output),
        }
    }

    /// Write the agent into a fresh temp directory, returning it and the path.
    #[must_use]
    pub fn in_tempdir(&self) -> (TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("create tempdir for fake agent");
        let path = self.write_to(dir.path());
        (dir, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use std::time::Instant;

    #[test]
    fn fake_agent_is_executable_and_echoes_version() {
        let (_dir, path) = fake_agent_in_tempdir("cursor-agent", "cursor-agent 1.4.2");

        let out = Command::new(&path)
            .arg("--version")
            .output()
            .expect("run fake agent");
        assert!(out.status.success());
        assert_eq!(
            String::from_utf8_lossy(&out.stdout).trim(),
            "cursor-agent 1.4.2"
        );

        let mode = fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o111, 0o111, "expected the file to be executable");
    }

    #[test]
    fn version_output_with_quotes_is_preserved() {
        let (_dir, path) = fake_agent_in_tempdir("weird", "it's v1.0 (build 'x')");
        let out = Command::new(&path).output().expect("run fake agent");
        assert_eq!(
            String::from_utf8_lossy(&out.stdout).trim(),
            "it's v1.0 (build 'x')"
        );
    }

    #[test]
    fn slow_agent_actually_delays() {
        let (_dir, path) = FakeAgent::new("slow", "slow 0.1.0")
            .with_sleep(Duration::from_millis(300))
            .in_tempdir();
        let start = Instant::now();
        let out = Command::new(&path).output().expect("run slow fake agent");
        assert!(out.status.success());
        assert!(start.elapsed() >= Duration::from_millis(250));
    }
}
