//! `config.toml` loading (§15.4).
//!
//! The GUI reads the `[terminal]` keys; the daemon reads `[sessions]`,
//! `[worktrees]`, `[git]`, `[github]` and `[daemon]`. Changes require a restart
//! in the MVP (no hot reload). A missing file yields defaults.

use serde::{Deserialize, Serialize};
use std::path::Path;

/// Top-level config document (§15.4).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub terminal: TerminalConfig,
    pub sessions: SessionsConfig,
    pub worktrees: WorktreesConfig,
    pub git: GitConfig,
    pub github: GithubConfig,
    pub juva: JuvaConfig,
    pub daemon: DaemonConfig,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TerminalConfig {
    pub scrollback_lines: u32,
    pub font_family: String,
    pub font_size: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionsConfig {
    pub kill_grace_ms: u64,
    /// Empty = use `$SHELL`.
    pub shell: String,
    /// Run shell sessions as login shells (`-l`), the way Ghostty and
    /// Terminal.app do on macOS.
    ///
    /// On by default, so an in-app shell sources the same files as one opened
    /// in the user's own terminal: the login profile (`.zprofile`, `.bash_profile`)
    /// on top of the interactive rc file. Turn it off for a shell that rejects
    /// `-l`, which exits immediately and leaves the session dead on arrival.
    pub login_shell: bool,
    /// The `TERM` advertised to shell sessions (§13.3).
    ///
    /// Defaults to Ghostty's entry so an in-app shell identifies itself the way
    /// the user's terminal does. The daemon checks that the name has a terminfo
    /// entry and falls back to `xterm-256color` when it has none, so this can
    /// never leave a session with an unusable `TERM`. Note that the VT engine
    /// behind these sessions is our own, not the named terminal's.
    pub term: String,
    /// Empty = find the terminfo database for `term` automatically.
    ///
    /// Set this when the entry lives somewhere the search in
    /// [`crate::terminfo`] does not look; it is exported to sessions as
    /// `TERMINFO`.
    pub terminfo_dir: String,
    /// Keep the session history across daemon restarts.
    ///
    /// Off by default. A PTY never survives the daemon (§3.3), so every session
    /// row a restart finds is already dead; keeping them turns the tree into a
    /// growing pile of `Orphaned` entries. With this off the daemon drops the
    /// session rows on startup and the app opens on a clean tree; with it on
    /// they are reconciled to `Orphaned` instead and `RestartSession` can bring
    /// one back (§7.3, §15.3).
    pub persist_history: bool,

    /// Inactivity (no terminal output *and* no input) after which a live agent
    /// session is reported once, in seconds. `0` disables it.
    ///
    /// Defaults to 30 minutes. An agent that finished its task an hour ago
    /// still holds a PTY, a thread and a full scrollback grid; saying so once
    /// costs nothing and is the only rule on by default. Nothing is ever
    /// stopped on this threshold — see [`idle_stop_after_secs`](Self::idle_stop_after_secs).
    pub idle_warn_after_secs: u64,

    /// Inactivity after which a live session is *stopped*, in seconds. `0`
    /// disables it, which is the default.
    ///
    /// Off unless the user asks for it: ending someone's process is never
    /// something to infer from a timer. When set it must be at least
    /// `idle_warn_after_secs`, or the warning could never fire; the daemon
    /// clamps it rather than silently swallowing the warning.
    /// The stop goes through the normal kill path (SIGTERM to the process
    /// group, SIGKILL after `kill_grace_ms`), so the session ends up `Exited`
    /// and stays restartable.
    pub idle_stop_after_secs: u64,

    /// Allow the idle stop to end a session a client is attached to. Off by
    /// default: a terminal the user is looking at is in use by definition.
    pub idle_stop_attached: bool,

    /// Apply the idle rules to shell sessions as well as agents. Off by
    /// default — a shell sitting at a prompt is the resting state of a
    /// terminal, not a leak.
    pub idle_include_shells: bool,

    /// Age (time since creation, however busy) after which a live session is
    /// reported once, in seconds. `0` disables it, which is the default.
    ///
    /// This is the "open for too long" rule, and unlike inactivity it never
    /// stops anything: a session can be a week old and working perfectly.
    pub long_running_warn_after_secs: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WorktreesConfig {
    /// Empty = default of §14.2.
    pub root: String,

    /// Files to copy from the main checkout into every worktree Forge creates.
    ///
    /// A managed worktree lives under `worktrees.root`, not next to the
    /// repository, so it starts without the untracked files a project needs to
    /// actually run: `.env`, a local settings file, a certificate. An agent
    /// launched there fails on its first command and the failure looks like a
    /// bug in Forge rather than a missing file.
    ///
    /// Paths are relative to the repository root. A missing source is skipped
    /// silently — a list is a wish, not a requirement — and a path that tries
    /// to escape either tree is refused. Directories are **not** copied: this
    /// is for small secrets and settings, not for `node_modules`.
    pub copy: Vec<String>,

    /// A command run inside every worktree Forge creates, after `copy`.
    ///
    /// Empty = do nothing. Run through `sh -c` from the worktree root, with a
    /// timeout of [`WorktreesConfig::setup_timeout_secs`]. Its output goes to
    /// the daemon log and its failure is reported as a notice — it never
    /// aborts the worktree, which by then exists on disk and is usable.
    pub setup_script: String,

    /// How long [`WorktreesConfig::setup_script`] may run before it is killed.
    pub setup_timeout_secs: u64,

    /// Directories whose worktrees the rescan never adopts, for every project.
    ///
    /// A relative entry resolves against each project's `git_root`; an absolute
    /// one is used as it is. Subtree matching, so an entry covers every worktree
    /// under it. This is the machine-wide escape hatch; the GUI's per-project
    /// rules live in the database and apply without a restart.
    pub ignore: Vec<String>,
}

/// Git behaviour the user can tune (§14, branches plan §4).
///
/// Both knobs default to `0`, which reads as "use the built-in behaviour":
/// the `git-service` network timeout, and no automatic fetching.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GitConfig {
    /// Timeout for commands that talk to a remote, in seconds.
    ///
    /// Separate from the ADR-008 30 s budget, which is calibrated for local
    /// commands: for `rev-parse`, 30 s means something is broken; for the first
    /// `fetch` of a large monorepo it means the repository is large. `0`
    /// restores the `git-service` default of 120 s.
    pub fetch_timeout_secs: u64,

    /// Fetch automatically every N seconds. `0` (the default) disables it.
    ///
    /// Off deliberately. Background network traffic nobody asked for is a
    /// battery cost and, on a repository whose credentials have expired, a
    /// failure that repeats forever. The GUI fetches when the branch picker
    /// opens, which is when a fresh list is actually worth something.
    pub auto_fetch_secs: u64,
}

/// GitHub CLI behaviour for pull-request reads and creation.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GithubConfig {
    /// `gh` executable or path. Empty searches the login-shell `PATH` (§12)
    /// for `gh`, then falls back to the daemon's own.
    pub executable: String,
    /// Wall-clock budget for one `gh` invocation. `0` uses `GH_TIMEOUT`.
    pub timeout_secs: u64,
    /// Include personal searches outside repositories registered in Forge.
    pub include_all_repos: bool,
    /// Refresh every N seconds. `0` disables background GitHub traffic.
    pub refresh_secs: u64,
    /// Hosts explicitly known to be GitHub Enterprise installations.
    pub enterprise_hosts: Vec<String>,
}

/// Where Juva gets its prose, when it gets any.
///
/// Off by default and it stays useful off: every consumer falls back to the
/// module's own deterministic draft, so a missing key or a hung endpoint costs
/// the wording and never the answer.
///
/// The key is read from the environment variable named here and is **never**
/// stored in the config file or the database — the file is world-readable in
/// every sense that matters, and a key in a backup is a key in a backup.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct JuvaConfig {
    /// Whether to call the endpoint at all.
    pub enabled: bool,
    /// An OpenAI-compatible chat-completions URL.
    pub endpoint: String,
    /// The model to ask for.
    pub model: String,
    /// Name of the environment variable holding the API key.
    pub api_key_env: String,
    /// Wall-clock budget for one call. `0` uses the built-in default.
    pub timeout_secs: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DaemonConfig {
    pub log_level: String,
}

impl Default for TerminalConfig {
    fn default() -> Self {
        Self {
            scrollback_lines: 10_000,
            font_family: "JetBrains Mono".to_owned(),
            font_size: 13.0,
        }
    }
}

impl Default for SessionsConfig {
    fn default() -> Self {
        Self {
            kill_grace_ms: 3000,
            shell: String::new(),
            login_shell: true,
            term: "xterm-ghostty".to_owned(),
            terminfo_dir: String::new(),
            persist_history: false,
            idle_warn_after_secs: 30 * 60,
            idle_stop_after_secs: 0,
            idle_stop_attached: false,
            idle_include_shells: false,
            long_running_warn_after_secs: 0,
        }
    }
}

impl Default for WorktreesConfig {
    fn default() -> Self {
        Self {
            root: String::new(),
            copy: Vec::new(),
            setup_script: String::new(),
            setup_timeout_secs: 120,
            ignore: Vec::new(),
        }
    }
}

impl GitConfig {
    /// The configured network timeout, or the `git-service` default.
    #[must_use]
    pub fn fetch_timeout(&self) -> std::time::Duration {
        if self.fetch_timeout_secs == 0 {
            git_service::GIT_NETWORK_TIMEOUT
        } else {
            std::time::Duration::from_secs(self.fetch_timeout_secs)
        }
    }
}

impl GithubConfig {
    /// Resolve the executable, timeout, and search path passed to `git-service`.
    ///
    /// `path_entries` is the login-shell `PATH` (§12). A GUI launched from
    /// Finder does not inherit the user's shell `PATH`, so an unconfigured bare
    /// `gh` is unlaunchable there even though it works in every terminal on the
    /// same machine; handing the resolved directories to `git-service` is what
    /// makes the two agree.
    #[must_use]
    pub fn cli(&self, path_entries: Vec<std::path::PathBuf>) -> git_service::GitHubCli {
        git_service::GitHubCli {
            executable: if self.executable.trim().is_empty() {
                "gh".into()
            } else {
                self.executable.trim().into()
            },
            timeout: if self.timeout_secs == 0 {
                git_service::GH_TIMEOUT
            } else {
                std::time::Duration::from_secs(self.timeout_secs)
            },
            path_entries,
        }
    }
}

impl JuvaConfig {
    /// The key, or `None` when Juva is off, unconfigured, or the variable is
    /// unset. Callers treat every `None` the same way: draft locally.
    #[must_use]
    pub fn api_key(&self) -> Option<String> {
        if !self.enabled || self.endpoint.trim().is_empty() || self.model.trim().is_empty() {
            return None;
        }
        let var = self.api_key_env.trim();
        if var.is_empty() {
            return None;
        }
        std::env::var(var).ok().filter(|key| !key.trim().is_empty())
    }

    /// The per-call budget, never zero: a zero would abort every request.
    #[must_use]
    pub fn timeout(&self) -> std::time::Duration {
        std::time::Duration::from_secs(if self.timeout_secs == 0 {
            30
        } else {
            self.timeout_secs
        })
    }
}

impl WorktreesConfig {
    /// The setup-script timeout, never zero (a zero would kill it instantly).
    #[must_use]
    pub fn setup_timeout(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.setup_timeout_secs.max(1))
    }
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            log_level: "info".to_owned(),
        }
    }
}

/// Errors loading config.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("failed to read config file: {0}")]
    Io(#[from] std::io::Error),
    #[error("failed to parse config.toml: {0}")]
    Parse(#[from] toml::de::Error),
}

impl Config {
    /// Load `config.toml`, returning defaults if the file does not exist.
    pub fn load(path: &Path) -> Result<Config, ConfigError> {
        match std::fs::read_to_string(path) {
            Ok(text) => Ok(toml::from_str(&text)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
            Err(e) => Err(ConfigError::Io(e)),
        }
    }

    /// Clamp `scrollback_lines` to the documented maximum of 100_000 (§11.4).
    #[must_use]
    pub fn effective_scrollback(&self) -> u32 {
        self.terminal.scrollback_lines.min(100_000)
    }

    /// The configured kill grace period (§11.3).
    #[must_use]
    pub fn kill_grace(&self) -> std::time::Duration {
        std::time::Duration::from_millis(self.sessions.kill_grace_ms)
    }

    /// The configured terminfo database, or `None` to discover it (§13.3).
    #[must_use]
    pub fn terminfo_dir(&self) -> Option<&Path> {
        let dir = self.sessions.terminfo_dir.trim();
        (!dir.is_empty()).then(|| Path::new(dir))
    }

    /// The effective idle policy for live sessions.
    ///
    /// Each `0` becomes `None` ("off") so the TOML needs no sentinel. A
    /// `idle_stop_after_secs` below `idle_warn_after_secs` is raised to it: the
    /// two thresholds describe one escalation, and a stop that fires before its
    /// own warning would make the warning unreachable.
    #[must_use]
    pub fn idle_policy(&self) -> crate::idle::IdlePolicy {
        let s = &self.sessions;
        let warn_after = opt_secs(s.idle_warn_after_secs);
        let stop_after = opt_secs(s.idle_stop_after_secs).map(|stop| match warn_after {
            Some(warn) => stop.max(warn),
            None => stop,
        });
        crate::idle::IdlePolicy {
            warn_after,
            stop_after,
            long_running_after: opt_secs(s.long_running_warn_after_secs),
            stop_attached: s.idle_stop_attached,
            include_shells: s.idle_include_shells,
        }
    }
}

/// `0` means "off" for every duration in `[sessions]`.
fn opt_secs(secs: u64) -> Option<std::time::Duration> {
    (secs > 0).then(|| std::time::Duration::from_secs(secs))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn defaults_match_the_plan() {
        let c = Config::default();
        assert_eq!(c.terminal.scrollback_lines, 10_000);
        assert_eq!(c.terminal.font_family, "JetBrains Mono");
        assert_eq!(c.sessions.kill_grace_ms, 3000);
        assert!(c.sessions.shell.is_empty());
        assert!(
            c.sessions.login_shell,
            "shell sessions default to login shells, like the user's own terminal"
        );
        assert_eq!(c.sessions.term, "xterm-ghostty");
        assert!(c.sessions.terminfo_dir.is_empty());
        assert!(
            !c.sessions.persist_history,
            "a fresh start is the default: the daemon drops dead session rows"
        );
        assert_eq!(c.sessions.idle_warn_after_secs, 1800);
        assert_eq!(
            c.sessions.idle_stop_after_secs, 0,
            "stopping a session is opt-in, never a default"
        );
        assert!(!c.sessions.idle_stop_attached);
        assert!(!c.sessions.idle_include_shells);
        assert_eq!(c.sessions.long_running_warn_after_secs, 0);
        assert!(c.worktrees.root.is_empty());
        assert!(
            c.worktrees.copy.is_empty(),
            "copying files into a worktree is opt-in"
        );
        assert!(c.worktrees.setup_script.is_empty());
        assert_eq!(c.worktrees.setup_timeout_secs, 120);
        assert!(
            c.worktrees.ignore.is_empty(),
            "ignoring a folder is opt-in, never a default"
        );
        assert_eq!(
            c.git.fetch_timeout_secs, 0,
            "0 defers to the git-service network default"
        );
        assert_eq!(
            c.git.auto_fetch_secs, 0,
            "background network traffic is never a default"
        );
        assert!(c.github.executable.is_empty());
        assert_eq!(c.github.cli(Vec::new()).executable, Path::new("gh"));
        assert_eq!(c.github.timeout_secs, 0);
        assert_eq!(c.github.cli(Vec::new()).timeout, git_service::GH_TIMEOUT);
        assert!(!c.github.include_all_repos);
        assert_eq!(
            c.github.refresh_secs, 0,
            "background GitHub traffic is never a default"
        );
        assert!(c.github.enterprise_hosts.is_empty());
        assert_eq!(c.daemon.log_level, "info");
    }

    #[test]
    fn the_network_timeout_falls_back_to_the_git_service_default() {
        let mut c = Config::default();
        assert_eq!(c.git.fetch_timeout(), git_service::GIT_NETWORK_TIMEOUT);
        c.git.fetch_timeout_secs = 45;
        assert_eq!(c.git.fetch_timeout(), std::time::Duration::from_secs(45));
    }

    #[test]
    fn github_cli_resolves_configured_executable_and_timeout() {
        let mut c = Config::default();
        c.github.executable = " /opt/homebrew/bin/gh ".into();
        c.github.timeout_secs = 15;
        assert_eq!(
            c.github.cli(vec![PathBuf::from("/opt/homebrew/bin")]),
            git_service::GitHubCli {
                executable: "/opt/homebrew/bin/gh".into(),
                timeout: std::time::Duration::from_secs(15),
                path_entries: vec![PathBuf::from("/opt/homebrew/bin")],
            }
        );
    }

    #[test]
    fn a_zero_setup_timeout_never_kills_the_script_instantly() {
        // `0` reads as "off" everywhere else in this file; here it would mean
        // "SIGKILL immediately", which is a trap rather than a setting.
        let mut c = Config::default();
        c.worktrees.setup_timeout_secs = 0;
        assert!(c.worktrees.setup_timeout() >= std::time::Duration::from_secs(1));
    }

    /// The shipped example must parse into the struct it documents; a key
    /// renamed in code and not in the file would otherwise be silently ignored
    /// by `#[serde(default)]` and read as "this setting does nothing".
    #[test]
    fn the_example_config_parses_and_matches_what_it_claims() {
        let text = include_str!("../../../docs/config.example.toml");
        let parsed: Config = toml::from_str(text).expect("docs/config.example.toml must parse");
        // Every value in the example is meant to be a default, so parsing it
        // has to be indistinguishable from parsing nothing at all.
        assert_eq!(parsed, Config::default());
    }

    #[test]
    fn partial_toml_fills_in_defaults() {
        let text = r#"
            [terminal]
            font_size = 15.0

            [daemon]
            log_level = "debug"
        "#;
        let c: Config = toml::from_str(text).unwrap();
        assert_eq!(c.terminal.font_size, 15.0);
        assert_eq!(c.terminal.scrollback_lines, 10_000); // default preserved
        assert_eq!(c.daemon.log_level, "debug");
    }

    #[test]
    fn missing_file_yields_defaults() {
        let c = Config::load(Path::new("/nonexistent/forge/config.toml")).unwrap();
        assert_eq!(c, Config::default());
    }

    #[test]
    fn the_default_policy_only_warns_about_idle_agents() {
        let p = Config::default().idle_policy();
        assert_eq!(p.warn_after, Some(std::time::Duration::from_secs(1800)));
        assert_eq!(p.stop_after, None);
        assert_eq!(p.long_running_after, None);
        assert!(!p.include_shells);
        assert!(p.is_enabled());
    }

    #[test]
    fn zero_disables_a_threshold() {
        let mut c = Config::default();
        c.sessions.idle_warn_after_secs = 0;
        let p = c.idle_policy();
        assert_eq!(p.warn_after, None);
        assert!(!p.is_enabled(), "every rule off means no sweeper at all");
    }

    #[test]
    fn a_stop_threshold_below_its_warning_is_raised_to_it() {
        let mut c = Config::default();
        c.sessions.idle_warn_after_secs = 1800;
        c.sessions.idle_stop_after_secs = 60;
        let p = c.idle_policy();
        assert_eq!(
            p.stop_after,
            Some(std::time::Duration::from_secs(1800)),
            "a stop that fires first would make the warning unreachable"
        );
    }

    #[test]
    fn a_stop_threshold_stands_alone_when_warnings_are_off() {
        let mut c = Config::default();
        c.sessions.idle_warn_after_secs = 0;
        c.sessions.idle_stop_after_secs = 60;
        assert_eq!(
            c.idle_policy().stop_after,
            Some(std::time::Duration::from_secs(60))
        );
    }

    #[test]
    fn scrollback_is_clamped() {
        let mut c = Config::default();
        c.terminal.scrollback_lines = 5_000_000;
        assert_eq!(c.effective_scrollback(), 100_000);
    }
}
