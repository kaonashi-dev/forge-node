//! Shell environment resolution.
//!
//! A GUI launched from Finder/a launcher does not inherit an interactive
//! shell's `PATH`. We resolve that environment once by running
//! `<shell> -l -i -c 'printf BEGIN; env -0; printf END'` with a timeout, parsing
//! the NUL-separated variables between the sentinels (so multiline values are
//! unambiguous), and cache the result. `-i` is required: Homebrew prepends
//! itself in `.zprofile`, while native CLIs live in `~/.local/bin` via
//! `.zshrc`, and a login-only capture would keep detecting the older cask.
//! On failure we fall back to the process environment with a widened `PATH`
//! and flag it for a `DaemonNotice`.

use std::io::Read;
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use domain::{EnvSource, ResolvedEnvironment, Timestamp};

const BEGIN: &str = "__FORGE_ENV_BEGIN__";
const END: &str = "__FORGE_ENV_END__";
const TIMEOUT: Duration = Duration::from_secs(5);
const MAX_ENV_OUTPUT: u64 = 1024 * 1024;
const REAP_GRACE: Duration = Duration::from_millis(500);

/// Well-known bin directories used to widen `PATH` in the fallback path.
const FALLBACK_PATH_DIRS: &[&str] = &[
    "/usr/local/bin",
    "/opt/homebrew/bin",
    "/usr/bin",
    "/bin",
    "/usr/sbin",
    "/sbin",
];

/// Home-relative bin directories added to the fallback `PATH`.
const FALLBACK_HOME_DIRS: &[&str] = &[".local/bin", ".cargo/bin", ".bun/bin", ".npm-global/bin"];

#[derive(Clone)]
pub struct ShellEnvironmentService {
    shell_override: Option<String>,
    cached: Option<ResolvedEnvironment>,
}

impl ShellEnvironmentService {
    /// Create the service. `shell_override` comes from `[sessions].shell`; empty
    /// means use `$SHELL`.
    #[must_use]
    pub fn new(shell_override: Option<String>) -> Self {
        Self {
            shell_override: shell_override.filter(|s| !s.is_empty()),
            cached: None,
        }
    }

    #[must_use]
    pub fn is_resolved(&self) -> bool {
        self.cached.is_some()
    }

    /// The cached environment, resolving it on first use.
    pub fn get(&mut self) -> &ResolvedEnvironment {
        if self.cached.is_none() {
            self.cached = Some(self.resolve());
        }
        self.cached.as_ref().expect("just resolved")
    }

    /// Force a re-resolution after the shell's PATH or configuration changes.
    ///
    /// A failed capture never replaces a login-shell cache: a slow rc file or a
    /// transient timeout would otherwise swap a working `PATH` for the process
    /// fallback and make every agent undetectable until the next restart.
    pub fn refresh(&mut self) -> &ResolvedEnvironment {
        let next = self.resolve();
        let keep = next.source == EnvSource::ProcessFallback
            && self
                .cached
                .as_ref()
                .is_some_and(|env| env.source == EnvSource::LoginShell);
        if keep {
            tracing::warn!("login-shell refresh failed; keeping the previous environment");
        } else {
            self.cached = Some(next);
        }
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
    /// entry, else `/bin/sh`.
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
    /// process environment on any failure.
    fn resolve(&self) -> ResolvedEnvironment {
        // Only the shell path and the *count* of variables are ever recorded:
        // environment values never reach the log.
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
    fn resolve_via_login_shell(&self, shell: &Path) -> Option<Vec<(String, String)>> {
        let out = capture_login_shell(shell, TIMEOUT)?;
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

    /// Process environment plus a widened `PATH`.
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

fn capture_login_shell(shell: &Path, timeout: Duration) -> Option<Vec<u8>> {
    let script = format!("printf '%s' '{BEGIN}'; env -0; printf '%s' '{END}'");
    let mut command = Command::new(shell);
    command
        .args(["-l", "-i", "-c", &script])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    // A new session rather than `process_group(0)`: an interactive shell with a
    // controlling tty in a background group stops itself on SIGTTIN/SIGTTOU
    // when a daemon started from a terminal runs this, and sits until the
    // timeout. `setsid` still makes the child its own group leader, so the
    // negative-pid kill below reaches its descendants.
    // SAFETY: `setsid` is async-signal-safe and touches no parent memory.
    unsafe {
        command.pre_exec(|| {
            nix::unistd::setsid()
                .map(drop)
                .map_err(std::io::Error::from)
        });
    }
    let mut child = command.spawn().ok()?;
    let pid = child.id();
    let (tx, rx) = mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let mut out = Vec::new();
        let captured = child.stdout.take().is_some_and(|stdout| {
            stdout
                .take(MAX_ENV_OUTPUT + 1)
                .read_to_end(&mut out)
                .is_ok()
                && out.len() as u64 <= MAX_ENV_OUTPUT
        });
        if !captured {
            kill_shell_group(pid);
        }
        // EOF is not process exit; both must finish inside the caller's deadline.
        let reaped = child.wait().is_ok();
        let _ = tx.send((captured && reaped).then_some(out));
    });

    match rx.recv_timeout(timeout) {
        Ok(out) => out,
        Err(_) => {
            kill_shell_group(pid);
            // The worker retains reap ownership if a killed process cannot exit yet.
            let _ = rx.recv_timeout(REAP_GRACE);
            None
        }
    }
}

fn kill_shell_group(pid: u32) {
    let Ok(pid) = i32::try_from(pid) else {
        return;
    };
    let _ = nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(-pid),
        nix::sys::signal::Signal::SIGKILL,
    );
    // A shell can move itself out of the group before timing out.
    let _ = nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(pid),
        nix::sys::signal::Signal::SIGKILL,
    );
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
    use std::os::unix::fs::PermissionsExt as _;
    use std::time::Instant;

    fn shell_fixture(body: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let shell = dir.path().join("shell");
        std::fs::write(&shell, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o755)).unwrap();
        (dir, shell)
    }

    #[test]
    fn environment_capture_rejects_oversized_output() {
        let (_dir, shell) =
            shell_fixture("exec /bin/dd if=/dev/zero bs=1024 count=1025 2>/dev/null");
        assert!(capture_login_shell(&shell, Duration::from_secs(5)).is_none());
    }

    #[test]
    fn environment_deadline_includes_exit_after_stdout_closes() {
        let (_dir, shell) = shell_fixture(&format!(
            "printf '%s' '{BEGIN}PATH=/bin{END}'\nexec 1>&-\nexec /bin/sleep 30"
        ));
        let started = Instant::now();
        assert!(capture_login_shell(&shell, Duration::from_millis(100)).is_none());
        assert!(started.elapsed() < Duration::from_secs(3));
    }

    #[test]
    fn environment_timeout_kills_descendants_holding_stdout() {
        let (dir, shell) = shell_fixture("");
        let survived = dir.path().join("survived");
        std::fs::write(
            &shell,
            format!(
                "#!/bin/sh\n(/bin/sleep 1; printf survived > '{}') &\nwait\n",
                survived.display()
            ),
        )
        .unwrap();
        assert!(capture_login_shell(&shell, Duration::from_millis(100)).is_none());
        // Wait past the descendant's write to distinguish group kill from direct-child kill.
        std::thread::sleep(Duration::from_millis(1200));
        assert!(!survived.exists());
    }

    #[test]
    fn environment_capture_preserves_complete_output_at_the_limit() {
        let (_dir, shell) =
            shell_fixture("exec /bin/dd if=/dev/zero bs=1024 count=1024 2>/dev/null");
        let out = capture_login_shell(&shell, Duration::from_secs(5)).unwrap();
        assert_eq!(out.len() as u64, MAX_ENV_OUTPUT);
    }

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

    #[test]
    fn a_failed_refresh_keeps_the_login_shell_environment() {
        let (_dir, shell) = shell_fixture("exit 1");
        let mut svc = ShellEnvironmentService::new(Some(shell.display().to_string()));
        svc.set_for_test(shell, vec![("PATH".into(), "/good/bin".into())]);
        let env = svc.refresh();
        assert_eq!(env.source, EnvSource::LoginShell);
        assert_eq!(env.get("PATH"), Some("/good/bin"));
    }

    #[test]
    fn the_capture_runs_without_a_controlling_terminal() {
        let (_dir, shell) = shell_fixture(
            r#"if (exec 3</dev/tty) 2>/dev/null; then FORGE_TTY=held; else FORGE_TTY=none; fi
export FORGE_TTY
while [ "$1" != "-c" ] && [ $# -gt 0 ]; do shift; done
shift
eval "$1""#,
        );
        let out = capture_login_shell(&shell, Duration::from_secs(5)).unwrap();
        let vars = ShellEnvironmentService::parse_between_sentinels(&out).unwrap();
        let tty = &vars.iter().find(|(key, _)| key == "FORGE_TTY").unwrap().1;
        assert_eq!(tty, "none");
    }

    #[test]
    fn interactive_path_wins_over_the_login_profile() {
        let (_dir, shell) = shell_fixture(
            r#"PATH=/login/bin:/usr/bin:/bin
for arg; do
  [ "$arg" = "-i" ] && PATH=/interactive/bin:$PATH
done
while [ "$1" != "-c" ] && [ $# -gt 0 ]; do shift; done
shift
eval "$1""#,
        );
        let out = capture_login_shell(&shell, Duration::from_secs(5)).unwrap();
        let vars = ShellEnvironmentService::parse_between_sentinels(&out).unwrap();
        let path = &vars.iter().find(|(key, _)| key == "PATH").unwrap().1;
        assert!(
            path.starts_with("/interactive/bin"),
            "expected the interactive PATH to lead, got {path}"
        );
    }
}
