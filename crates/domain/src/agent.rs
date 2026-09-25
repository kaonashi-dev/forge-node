//! Agent descriptors and the runtime types used to launch and detect them.
//!
//! Fields are owned so a descriptor can travel over IPC (`ListAgentProviders`).
//! Built-ins are constructed in the `agents` crate.

use crate::ids::{AgentProfileId, AgentProviderId, Timestamp, WorkspaceId};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PtySize {
    pub cols: u16,
    pub rows: u16,
    pub pixel_width: u16,
    pub pixel_height: u16,
}

impl Default for PtySize {
    fn default() -> Self {
        Self {
            cols: 80,
            rows: 24,
            pixel_width: 0,
            pixel_height: 0,
        }
    }
}

impl PtySize {
    /// Allocation ceiling for untrusted viewport dimensions.
    pub const MAX_COLS: u16 = 1000;
    /// See [`PtySize::MAX_COLS`].
    pub const MAX_ROWS: u16 = 500;

    /// Clamp before the emulator allocates `cols * rows` cells; zero would divide by zero.
    #[must_use]
    pub fn sanitized(self) -> Self {
        Self {
            cols: self.cols.clamp(1, Self::MAX_COLS),
            rows: self.rows.clamp(1, Self::MAX_ROWS),
            ..self
        }
    }
}

/// `env` is the complete environment, not an incremental overlay.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpawnSpec {
    /// Absolute, already-resolved program path.
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub env: Vec<(String, String)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum EnvSource {
    /// Parsed from `$SHELL -l -i -c env` between sentinels.
    LoginShell,
    /// Fallback: daemon environment plus well-known PATH entries.
    ProcessFallback,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedEnvironment {
    pub shell: PathBuf,
    pub vars: Vec<(String, String)>,
    pub path_entries: Vec<PathBuf>,
    pub resolved_at: Timestamp,
    pub source: EnvSource,
}

impl ResolvedEnvironment {
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&str> {
        self.vars
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaunchAgentRequest {
    pub provider_id: AgentProviderId,
    pub cwd: PathBuf,
    pub extra_args: Vec<String>,
    pub executable_override: Option<PathBuf>,
    /// Provider-owned transcript id, not a Forge session id.
    pub resume_session_id: Option<String>,
    /// Refused for a provider that declares no [`PromptStyle`].
    pub initial_prompt: Option<String>,
    /// Refused rather than downgraded if the provider declares no [`ReviewStyle`].
    #[serde(default)]
    pub read_only: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum DetectionStatus {
    Installed {
        executable: PathBuf,
        version: Option<String>,
    },
    NotFound,
    Rejected {
        candidate: String,
        reason: String,
    },
    ProbeTimeout,
}

impl DetectionStatus {
    #[must_use]
    pub fn is_installed(&self) -> bool {
        matches!(self, DetectionStatus::Installed { .. })
    }
}

/// The CLI must emit one JSON usage object on stdout.
///
/// Forge does not scrape provider-specific output. The probe's contract is one
/// JSON object on stdout:
///
/// ```json
/// { "used_fraction": 0.42, "window": "5h", "resets_at": "2026-08-22T18:00:00Z" }
/// ```
///
/// `used_fraction` is required, `window` defaults to `"session"` and
/// `resets_at` is optional. A CLI that prints anything else reports no usage
/// rather than a wrong number.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageProbe {
    /// Arguments that make the CLI print the usage document.
    pub args: Vec<String>,
    /// Hard timeout, like the version probe's.
    pub timeout_ms: u64,
}

/// Whole percentages preserve wire equality; usage beyond the allowance is capped at 100.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageWindow {
    /// Percent of the allowance consumed, `0..=100`.
    pub used_percent: u8,
    /// Human label for the window — `"5h"`, `"week"`.
    pub window: String,
    /// When the window resets, if the provider says.
    pub resets_at: Option<Timestamp>,
}

/// Empty `windows` means no reading, not zero usage.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderUsage {
    pub provider_id: AgentProviderId,
    /// Account whose allowance was read; `None` selects the provider's default.
    #[serde(default)]
    pub profile_id: Option<AgentProfileId>,
    /// The windows the provider reports, in display order.
    pub windows: Vec<UsageWindow>,
    /// When the reading was taken, so a stale reading can be shown as stale.
    pub collected_at: Timestamp,
}

impl ProviderUsage {
    /// Ignores `collected_at` so unchanged readings do not trigger broadcasts.
    #[must_use]
    pub fn same_reading(&self, other: &Self) -> bool {
        self.provider_id == other.provider_id
            && self.profile_id == other.profile_id
            && self.windows == other.windows
    }

    /// The account this reading belongs to: one provider can report several.
    #[must_use]
    pub fn account(&self) -> (&AgentProviderId, Option<AgentProfileId>) {
        (&self.provider_id, self.profile_id)
    }
}

/// Provider-specific usage collection is implemented in the `agents` crate.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum UsageSource {
    /// Run a CLI that prints the documented usage JSON on stdout.
    Cli(UsageProbe),
    /// Read `~/.codex/auth.json` and call the ChatGPT usage endpoint.
    CodexOAuth,
    /// Read Claude's local credentials and call the Anthropic usage endpoint.
    ClaudeOauth,
    /// Ask Grok over its own ACP entry ([`AcpSpec::args`]) rather than over
    /// HTTP: the account meter is a JSON-RPC extension method, not a URL.
    GrokAcp,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DetectionResult {
    pub provider_id: AgentProviderId,
    pub status: DetectionStatus,
    pub checked_at: Timestamp,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionProbe {
    /// Typically `["--version"]`.
    pub args: Vec<String>,
    /// e.g. `"cursor"` to reject an unrelated `agent` binary.
    pub expect_substring: Option<String>,
    pub timeout_ms: u32,
}

/// Informational flags derived from the descriptor's declared launch styles.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentCapabilities {
    pub interactive_tui: bool,
    pub supports_initial_prompt: bool,
    pub supports_resume: bool,
    #[serde(default)]
    pub supports_review: bool,
}

/// Prompts are passed as one argument, never interpolated into a shell command.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum PromptStyle {
    /// The prompt is the last positional argument: `claude "<prompt>"`.
    Positional,
    /// The prompt follows a flag: `<cli> --prompt "<prompt>"`.
    Flag { flag: String },
}

/// Provider-owned read-only flags; an absent declaration forbids review launches.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewStyle {
    /// The provider's own flags for a read-only session, in order.
    pub args: Vec<String>,
    /// What the provider calls that mode, for a menu that has to say which of
    /// four different postures the user is picking.
    pub label: String,
}

impl PromptStyle {
    /// The arguments that carry `prompt`, in the order they must appear.
    #[must_use]
    pub fn args(&self, prompt: &str) -> Vec<String> {
        match self {
            Self::Positional => vec![prompt.to_owned()],
            Self::Flag { flag } => vec![flag.clone(), prompt.to_owned()],
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum ResumeStyle {
    /// A flag carrying the id: `claude --resume <id>`, `opencode --session <id>`.
    Flag { flag: String },
    /// A subcommand carrying the id: `codex resume <id>`. It has to lead the
    /// command line, which is why it is not just a flag with a funny name.
    Subcommand { command: String },
}

impl ResumeStyle {
    /// The arguments that re-enter `session_id`, in the order they must appear.
    #[must_use]
    pub fn args(&self, session_id: &str) -> Vec<String> {
        match self {
            Self::Flag { flag } => vec![flag.clone(), session_id.to_owned()],
            Self::Subcommand { command } => vec![command.clone(), session_id.to_owned()],
        }
    }
}

/// How a provider speaks [Agent Client Protocol](https://agentclientprotocol.com)
/// over stdio.
///
/// `None` on a descriptor means the provider has no ACP entry. Grok's usage
/// reading spawns `args` rather than spelling the subcommand a second time.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcpSpec {
    /// Extra args after the executable when spawning as an ACP agent.
    #[serde(default)]
    pub args: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentDescriptor {
    pub id: AgentProviderId,
    pub display_name: String,
    /// Candidate binary names in order of preference.
    pub binary_candidates: Vec<String>,
    pub default_args: Vec<String>,
    pub version_probe: VersionProbe,
    /// `None` means no usage is reported for this provider.
    pub usage_source: Option<UsageSource>,
    /// `None` means it offers no way to, so its history is read-only.
    pub resume: Option<ResumeStyle>,
    /// How the provider takes a prompt at launch. `None` means it only
    /// takes one typed into its TUI, so nothing may be launched *for* it.
    pub prompt: Option<PromptStyle>,
    /// How the provider speaks ACP over stdio. `None` means it has no ACP entry.
    #[serde(default)]
    pub acp: Option<AcpSpec>,
    /// How the provider is put in a read-only posture. `None` means it
    /// has none, so nothing may launch it for a review.
    #[serde(default)]
    pub review: Option<ReviewStyle>,
    pub capabilities: AgentCapabilities,
    /// `None` forbids profile-specific config directories.
    pub config_dir: Option<ConfigDirSpec>,
}

/// All declared variables point to the same profile directory.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigDirSpec {
    /// Every variable set to the profile's directory, all to the same path.
    pub vars: Vec<String>,
    /// One line of help under the field.
    pub help: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentProfile {
    pub id: AgentProfileId,
    /// The provider this profile launches.
    pub provider_id: AgentProviderId,
    /// What the user calls it — `"Personal"`, `"Work"`.
    pub name: String,
    /// Program to run instead of the detected binary; `None` inherits it.
    ///
    /// A bare name (`claude-personal`) is looked up on the resolved login-shell
    /// PATH, which is where a wrapper script standing in for a shell alias
    /// lives. An alias itself is not a program and cannot be launched.
    pub executable: Option<PathBuf>,
    /// The account this profile logs into, as
    /// [`ConfigDirSpec::vars`] for its provider; `None` shares the provider's
    /// default account.
    ///
    /// Stored as the user typed it. It is resolved against `$HOME` at launch —
    /// see [`AgentProfile::resolve_config_dir`] — so `.claude-personal` names a
    /// directory in the home directory and not one in whatever the daemon's
    /// working directory happens to be.
    pub config_dir: Option<PathBuf>,
    /// Appended after the descriptor's `default_args`.
    pub args: Vec<String>,
    pub created_at: Timestamp,
}

/// Environment variables a profile may never set: Forge owns the terminal
/// contract with the child process, and a provider whose config
/// directory was spelled with one of these would break the emulator rather
/// than switch accounts.
pub const RESERVED_PROFILE_VARS: [&str; 5] = [
    "TERM",
    "TERMINFO",
    "COLORTERM",
    "FORGE_SESSION_ID",
    "FORGE_WORKSPACE",
];

impl AgentProfile {
    /// Whether `name` is a variable a profile is allowed to set.
    #[must_use]
    pub fn is_reserved_var(name: &str) -> bool {
        RESERVED_PROFILE_VARS.contains(&name)
    }

    /// The absolute directory `dir` names for a user whose home is `home`.
    ///
    /// The daemon's working directory is not the user's: it is `/` under
    /// launchd, so a relative `.claude-personal` created there fails on a
    /// read-only file system instead of landing in the home directory the user
    /// meant. `~` is expanded for the same reason — the form takes text, and no
    /// shell ever sees it.
    #[must_use]
    pub fn resolve_config_dir(dir: &std::path::Path, home: &std::path::Path) -> PathBuf {
        let text = dir.to_string_lossy();
        match text.strip_prefix('~') {
            Some("") => home.to_path_buf(),
            Some(rest) if rest.starts_with('/') => home.join(rest.trim_start_matches('/')),
            _ if dir.is_absolute() => dir.to_path_buf(),
            _ => home.join(dir),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum ChildWorkspacePolicy {
    SameWorkspace,
    NewManagedWorktree {
        branch_hint: Option<String>,
        base: Option<String>,
    },
    ExistingWorkspace(WorkspaceId),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile() -> AgentProfile {
        AgentProfile {
            id: AgentProfileId::new(),
            provider_id: AgentProviderId::new("claude"),
            name: "Work".to_owned(),
            executable: None,
            config_dir: Some(PathBuf::from(".claude-work")),
            args: vec!["--model".to_owned(), "opus".to_owned()],
            created_at: Timestamp::now(),
        }
    }

    #[test]
    fn a_profile_round_trips_through_json() {
        let profile = profile();
        let json = serde_json::to_string(&profile).unwrap();
        let back: AgentProfile = serde_json::from_str(&json).unwrap();
        assert_eq!(profile, back);
    }

    #[test]
    fn a_config_directory_is_resolved_against_the_home_directory() {
        let home = std::path::Path::new("/home/me");
        let resolve = |dir: &str| AgentProfile::resolve_config_dir(std::path::Path::new(dir), home);

        // The case in the screenshot: a bare dotted name is a directory in the
        // home directory, not in the daemon's cwd (`/` under launchd).
        assert_eq!(resolve(".claude-personal"), home.join(".claude-personal"));
        assert_eq!(
            resolve("accounts/claude-personal"),
            home.join("accounts/claude-personal")
        );
        assert_eq!(resolve("~/.claude-personal"), home.join(".claude-personal"));
        assert_eq!(resolve("~"), home);
        // An absolute path is somewhere else entirely, and stays there.
        assert_eq!(
            resolve("/Volumes/work/.claude"),
            PathBuf::from("/Volumes/work/.claude")
        );
    }

    #[test]
    fn the_terminal_contract_is_reserved() {
        for reserved in RESERVED_PROFILE_VARS {
            assert!(AgentProfile::is_reserved_var(reserved));
        }
        assert!(!AgentProfile::is_reserved_var("CLAUDE_CONFIG_DIR"));
    }

    #[test]
    fn pty_size_is_clamped_into_the_supported_range() {
        // The adversarial case the clamp exists for.
        let huge = PtySize {
            cols: u16::MAX,
            rows: u16::MAX,
            pixel_width: 4,
            pixel_height: 8,
        }
        .sanitized();
        assert_eq!(huge.cols, PtySize::MAX_COLS);
        assert_eq!(huge.rows, PtySize::MAX_ROWS);
        // Pixel hints pass through untouched — they don't drive allocation.
        assert_eq!((huge.pixel_width, huge.pixel_height), (4, 8));

        // A zero dimension is raised to 1: the engine divides by both.
        let zero = PtySize {
            cols: 0,
            rows: 0,
            pixel_width: 0,
            pixel_height: 0,
        }
        .sanitized();
        assert_eq!((zero.cols, zero.rows), (1, 1));

        // A normal geometry is left exactly as-is.
        let ok = PtySize {
            cols: 200,
            rows: 50,
            pixel_width: 0,
            pixel_height: 0,
        };
        assert_eq!(ok.sanitized(), ok);
    }
}
