//! Shell environment resolution (§12).
//!
//! A GUI launched from Finder/a launcher does not inherit an interactive
//! shell's `PATH`. We resolve the login-shell environment once by running
//! `<shell> -l -c 'printf BEGIN; env -0; printf END'` with a timeout, parsing
//! the NUL-separated variables between the sentinels (so multiline values are
//! unambiguous), and cache the result. On failure we fall back to the process
//! environment with a widened `PATH` and flag it for a `DaemonNotice`.

use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use domain::{EnvSource, ResolvedEnvironment, Timestamp};

const BEGIN: &str = "__FORGE_ENV_BEGIN__";
const END: &str = "__FORGE_ENV_END__";
const TIMEOUT: Duration = Duration::from_secs(5);

/// Well-known bin directories used to widen `PATH` in the fallback path (§12).
const FALLBACK_PATH_DIRS: &[&str] = &[
    "/usr/local/bin",
    "/opt/homebrew/bin",
    "/usr/bin",
    "/bin",
    "/usr/sbin",
    "/sbin",
];

/// Home-relative bin directories added to the fallback `PATH` (§12).
const FALLBACK_HOME_DIRS: &[&str] = &[".local/bin", ".cargo/bin", ".bun/bin", ".npm-global/bin"];

/// Resolves and caches the login-shell environment (§12).
pub struct ShellEnvironmentService {
    shell_override: Option<String>,
    cached: Option<ResolvedEnvironment>,
}

impl ShellEnvironmentService {
    /// Create the service. `shell_override` comes from `[sessions].shell`; empty
    /// means use `$SHELL` (§15.4).
    #[must_use]
    pub fn new(shell_override: Option<String>) -> Self {
        Self {
            shell_override: shell_override.filter(|s| !s.is_empty()),
            cached: None,
        }
    }

    /// The cached environment, resolving it on first use.
    pub fn get(&mut self) -> &ResolvedEnvironment {
        if self.cached.is_none() {
            self.cached = Some(self.resolve());
        }
        self.cached.as_ref().expect("just resolved")
    }

    /// Force a re-resolution (e.g. after `$SHELL` changed) and return the result.
    /// Reserved for the `RefreshAgentDetection` path when `$SHELL` changes.
    #[allow(dead_code)]
    pub fn refresh(&mut self) -> &ResolvedEnvironment {
        self.cached = Some(self.resolve());
        self.cached.as_ref().expect("just resolved")
    }

    /// Replace the cache with fixed variables; core tests must not inherit the
    /// shell the test runner was launched from. Marked `LoginShell` so
    /// `resolved_env` does not raise the process-fallback notice.
    #[cfg(test)]
    pub(crate) fn set_for_test(&mut self, shell: PathBuf, vars: Vec<(String, String)>) {
        self.cached = Some(Self::build(shell, vars, EnvSource::LoginShell));
    }

    /// Resolve the shell to invoke: override, else `$SHELL`, else the passwd
    /// entry, else `/bin/sh` (§12).
    fn resolve_shell(&self) -> PathBuf {
        if let Some(s) = &self.shell_override {
            return PathBuf::from(s);
        }
        if let Some(s) = std::env::var_os("SHELL") {
            if !s.is_empty() {
                return PathBuf::from(s);
            }
        }
        if let Ok(Some(user)) = nix::unistd::User::from_uid(nix::unistd::Uid::current()) {
            return user.shell;
        }
        PathBuf::from("/bin/sh")
    }

    /// Resolve the environment, using the login shell and falling back to the
    /// process environment on any failure (§12).
    fn resolve(&self) -> ResolvedEnvironment {
        // Only the shell path and the *count* of variables are ever recorded:
        // environment values never reach the log (§22, §23).
        let _span = tracing::info_span!("env.resolve").entered();
        let shell = self.resolve_shell();
        match self.resolve_via_login_shell(&shell) {
            Some(vars) if !vars.is_empty() => {
                tracing::info!(shell = %shell.display(), vars = vars.len(), "resolved login-shell environment");
                Self::build(shell, vars, EnvSource::LoginShell)
            }
            _ => {
                tracing::warn!(shell = %shell.display(), "login-shell env resolution failed; using process fallback");
                self.fallback(shell)
            }
        }
    }

    /// Run the login shell and parse the variables between the sentinels.
    fn resolve_via_login_shell(&self, shell: &PathBuf) -> Option<Vec<(String, String)>> {
        let script = format!("printf '%s' '{BEGIN}'; env -0; printf '%s' '{END}'");
        let mut child = Command::new(shell)
            .arg("-l")
            .arg("-c")
            .arg(&script)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;

        // Read stdout on a worker thread so a hung shell cannot block us.
        let mut stdout = child.stdout.take()?;
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = stdout.read_to_end(&mut buf);
            let _ = tx.send(buf);
        });

        let out = match rx.recv_timeout(TIMEOUT) {
            Ok(buf) => {
                let _ = child.wait();
                buf
            }
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        };

        Self::parse_between_sentinels(&out)
    }

    /// Extract `KEY=VALUE` pairs from the NUL-separated `env -0` output that sits
    /// between the two sentinels; everything else (rc-file banners) is ignored.
    fn parse_between_sentinels(out: &[u8]) -> Option<Vec<(String, String)>> {
        let text_start = find_sub(out, BEGIN.as_bytes())? + BEGIN.len();
        let text_end = find_sub(&out[text_start..], END.as_bytes())? + text_start;
        let body = &out[text_start..text_end];

        let mut vars = Vec::new();
        for entry in body.split(|&b| b == 0) {
            if entry.is_empty() {
                continue;
            }
            // `env -0` emits `KEY=VALUE`; VALUE may itself contain newlines.
            if let Some(eq) = entry.iter().position(|&b| b == b'=') {
                let key = String::from_utf8_lossy(&entry[..eq]).into_owned();
                let value = String::from_utf8_lossy(&entry[eq + 1..]).into_owned();
                if !key.is_empty() {
                    vars.push((key, value));
                }
            }
        }
        Some(vars)
    }

    /// Build a [`ResolvedEnvironment`] from variables, deriving `path_entries`
    /// from the `PATH` variable.
    fn build(
        shell: PathBuf,
        vars: Vec<(String, String)>,
        source: EnvSource,
    ) -> ResolvedEnvironment {
        let path_entries = vars
            .iter()
            .find(|(k, _)| k == "PATH")
            .map(|(_, v)| std::env::split_paths(v).collect())
            .unwrap_or_default();
        ResolvedEnvironment {
            shell,
            vars,
            path_entries,
            resolved_at: Timestamp::now(),
            source,
        }
    }

    /// Process environment plus a widened `PATH` (§12).
    fn fallback(&self, shell: PathBuf) -> ResolvedEnvironment {
        let mut vars: Vec<(String, String)> = std::env::vars().collect();

        let home = std::env::var("HOME").ok();
        let mut extra: Vec<PathBuf> = FALLBACK_PATH_DIRS.iter().map(PathBuf::from).collect();
        if let Some(home) = &home {
            for suffix in FALLBACK_HOME_DIRS {
                extra.push(PathBuf::from(home).join(suffix));
            }
        }

        // Merge existing PATH with the well-known dirs, de-duplicated, existing first.
        let mut path_dirs: Vec<PathBuf> = vars
            .iter()
            .find(|(k, _)| k == "PATH")
            .map(|(_, v)| std::env::split_paths(v).collect())
            .unwrap_or_default();
        for dir in extra {
            if !path_dirs.contains(&dir) {
                path_dirs.push(dir);
            }
        }
        let joined = std::env::join_paths(&path_dirs)
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        match vars.iter_mut().find(|(k, _)| k == "PATH") {
            Some((_, v)) => *v = joined,
            None => vars.push(("PATH".to_string(), joined)),
        }

        ResolvedEnvironment {
            shell,
            vars,
            path_entries: path_dirs,
            resolved_at: Timestamp::now(),
            source: EnvSource::ProcessFallback,
        }
    }
}

/// Find the first index of `needle` in `haystack`.
fn find_sub(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_env0_between_sentinels_with_multiline_values() {
        let mut out = Vec::new();
        out.extend_from_slice(b"shell banner ignored\n");
        out.extend_from_slice(BEGIN.as_bytes());
        out.extend_from_slice(b"PATH=/usr/bin:/bin\0");
        out.extend_from_slice(b"MULTI=line1\nline2\0");
        out.extend_from_slice(b"EMPTY=\0");
        out.extend_from_slice(END.as_bytes());
        out.extend_from_slice(b"trailing ignored");

        let vars = ShellEnvironmentService::parse_between_sentinels(&out).unwrap();
        assert_eq!(
            vars.iter().find(|(k, _)| k == "PATH").unwrap().1,
            "/usr/bin:/bin"
        );
        assert_eq!(
            vars.iter().find(|(k, _)| k == "MULTI").unwrap().1,
            "line1\nline2"
        );
        assert_eq!(vars.iter().find(|(k, _)| k == "EMPTY").unwrap().1, "");
    }

    #[test]
    fn missing_sentinels_yields_none() {
        assert!(ShellEnvironmentService::parse_between_sentinels(b"no markers here").is_none());
    }

    #[test]
    fn fallback_has_widened_path_and_is_marked() {
        let svc = ShellEnvironmentService::new(None);
        let env = svc.fallback(PathBuf::from("/bin/sh"));
        assert_eq!(env.source, EnvSource::ProcessFallback);
        assert!(env.path_entries.iter().any(|p| p.ends_with("bin")));
    }

    #[test]
    fn real_login_shell_resolves_or_falls_back() {
        // Integration-ish: on a dev machine this should resolve a PATH either
        // way. Just assert we get a non-empty environment with a PATH.
        let mut svc = ShellEnvironmentService::new(None);
        let env = svc.get();
        assert!(env.get("PATH").is_some(), "resolved env should carry PATH");
    }
}
