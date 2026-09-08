//! Agent descriptors and the shared runtime types used to launch and detect
//! providers (§7.5, §7.6, §8.2).
//!
//! Note: the plan sketches `AgentDescriptor` with `&'static str` fields. We use
//! owned `String`/`Vec` here so descriptors can also travel over IPC
//! (`ListAgentProviders`) and support future custom agents. Built-ins are
//! constructed as owned values in the `agents` crate.

use crate::ids::{AgentProfileId, AgentProviderId, Timestamp, WorkspaceId};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// PTY dimensions, including pixel size for programs that query it (§7.6).
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
    /// Largest grid the daemon will allocate for a client-supplied size. A
    /// terminal is a viewport, not a document, so 1000×500 is already well past
    /// any real display.
    pub const MAX_COLS: u16 = 1000;
    /// See [`PtySize::MAX_COLS`].
    pub const MAX_ROWS: u16 = 500;

    /// Clamp `cols`/`rows` into the supported range before the geometry reaches
    /// the emulator. `cols`/`rows` arrive over the socket as attacker-controlled
    /// `u16`s, and the VT engine reserves `cols * rows` cells eagerly: an
    /// unclamped `65535×65535` asks for ~4.3e9 cells (~170 GB) and aborts the
    /// daemon with every live PTY inside it. A zero dimension is raised to 1
    /// because the engine divides by both.
    #[must_use]
    pub fn sanitized(self) -> Self {
        Self {
            cols: self.cols.clamp(1, Self::MAX_COLS),
            rows: self.rows.clamp(1, Self::MAX_ROWS),
            ..self
        }
    }
}

/// A fully resolved spawn specification. `env` is the *complete* environment,
/// not an incremental overlay (§7.6).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpawnSpec {
    /// Absolute, already-resolved program path.
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub env: Vec<(String, String)>,
}

/// Where a resolved environment came from (§12).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum EnvSource {
    /// Parsed from `$SHELL -l -c env` between sentinels.
    LoginShell,
    /// Fallback: daemon environment plus well-known PATH entries.
    ProcessFallback,
}

/// The login-shell environment resolved once and cached (§12).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedEnvironment {
    pub shell: PathBuf,
    pub vars: Vec<(String, String)>,
    pub path_entries: Vec<PathBuf>,
    pub resolved_at: Timestamp,
    pub source: EnvSource,
}

impl ResolvedEnvironment {
    /// Look up a variable by name.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&str> {
        self.vars
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }
}

/// A request to launch a provider inside a workspace (§7.6).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaunchAgentRequest {
    pub provider_id: AgentProviderId,
    pub cwd: PathBuf,
    /// Empty in the MVP.
    pub extra_args: Vec<String>,
    pub executable_override: Option<PathBuf>,
    /// A provider session id to re-enter instead of starting a fresh
    /// conversation (§13.5). The id is the provider's own — the one its
    /// transcript records — and only means something to the CLI that wrote it.
    pub resume_session_id: Option<String>,
    /// A prompt to hand the agent at launch instead of having the user type it
    /// (§16.8). Refused for a provider that declares no [`PromptStyle`].
    pub initial_prompt: Option<String>,
    /// Launch in the provider's own read-only mode (§16.9). Refused for a
    /// provider that declares no [`ReviewStyle`]: a review that can write is
    /// not the thing that was asked for.
    #[serde(default)]
    pub read_only: bool,
}

/// How a version probe resolved for a provider (§7.6, §13.1).
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

/// How to ask a provider's CLI what the account has used (§7.5).
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

/// One rolling allowance window a provider reports (§16.2).
///
/// The reading is a whole percent, not a float: the protocol stays `Eq` (no
/// float comparison in a wire type), and a meter has no use for the precision.
/// A window past its allowance reads as 100 — "full" is the whole message a
/// meter can carry.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageWindow {
    /// Percent of the allowance consumed, `0..=100`.
    pub used_percent: u8,
    /// Human label for the window — `"5h"`, `"week"`.
    pub window: String,
    /// When the window resets, if the provider says.
    pub resets_at: Option<Timestamp>,
}

/// What a provider reports having used, across one or more windows (§16.2).
///
/// A provider can expose several windows at once (a rolling session limit and a
/// weekly one, say); each becomes a [`UsageWindow`]. An empty `windows` means
/// the provider is known but reported nothing — rendered as no meter, never as
/// zero.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderUsage {
    pub provider_id: AgentProviderId,
    /// The windows the provider reports, in display order.
    pub windows: Vec<UsageWindow>,
    /// When the reading was taken, so a stale reading can be shown as stale.
    pub collected_at: Timestamp,
}

impl ProviderUsage {
    /// Whether two readings carry the same information, ignoring *when* each was
    /// taken. `collected_at` advances on every sweep, so comparing whole structs
    /// would always differ; the daemon uses this to broadcast only real changes.
    #[must_use]
    pub fn same_reading(&self, other: &Self) -> bool {
        self.provider_id == other.provider_id && self.windows == other.windows
    }
}

/// Where a provider's usage reading comes from (§16.2).
///
/// Every provider-specific fact lives in the `agents` crate; this only names the
/// mechanism. `Cli` keeps the original JSON-contract probe intact; the OAuth
/// variants read the provider's existing local credentials and call its usage
/// endpoint — no extra login.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum UsageSource {
    /// Run a CLI that prints the documented usage JSON on stdout.
    Cli(UsageProbe),
    /// Read `~/.codex/auth.json` and call the ChatGPT usage endpoint.
    CodexOAuth,
    /// Read Claude's local credentials and call the Anthropic usage endpoint.
    ClaudeOauth,
}

/// The result of detecting a single provider (§7.6).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DetectionResult {
    pub provider_id: AgentProviderId,
    pub status: DetectionStatus,
    pub checked_at: Timestamp,
}

/// How to verify a candidate binary is the intended provider (§7.5).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionProbe {
    /// Typically `["--version"]`.
    pub args: Vec<String>,
    /// e.g. `"cursor"` to reject an unrelated `agent` binary.
    pub expect_substring: Option<String>,
    pub timeout_ms: u32,
}

/// Informational capability flags (§7.5).
///
/// `supports_resume` mirrors whether the descriptor carries a [`ResumeStyle`],
/// and `supports_initial_prompt` mirrors its [`PromptStyle`] the same way:
/// neither is a second source of truth, both restate a spelling that either
/// exists on the descriptor or does not. `interactive_tui` is informative.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentCapabilities {
    pub interactive_tui: bool,
    pub supports_initial_prompt: bool,
    /// Whether the provider can run a task without a terminal and exit.
    #[serde(default)]
    pub supports_headless: bool,
    pub supports_resume: bool,
    /// Whether the provider has a mode of its own that reads without writing
    /// (§16.9). Restates the presence of a [`ReviewStyle`], like the flags
    /// above restate their own spellings.
    #[serde(default)]
    pub supports_review: bool,
}

/// How a provider takes a prompt at launch, rather than typed into its TUI
/// (§16.8).
///
/// The sibling of [`ResumeStyle`], and declared for the same reason: the
/// spelling belongs beside the descriptor that owns it, so nothing else
/// branches on a provider id (principle P2).
///
/// The prompt is passed as **one argument**, never interpolated into a command
/// line. It arrives at the child through `execve`, so a prompt containing
/// quotes, newlines or a `$(…)` is text and not a shell injection — which
/// matters here more than usual, because a prompt is assembled from a template,
/// a diff and whatever the user typed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum PromptStyle {
    /// The prompt is the last positional argument: `claude "<prompt>"`.
    Positional,
    /// The prompt follows a flag: `<cli> --prompt "<prompt>"`.
    Flag { flag: String },
}

/// How a provider is launched so that it reads and reasons but does not write
/// (§16.9).
///
/// The third sibling of [`ResumeStyle`] and [`PromptStyle`], declared for the
/// same reason: the spelling belongs beside the descriptor that owns it.
///
/// This is what makes an automatic pull-request review safe to start in a
/// checkout the user is working in. Every built-in has such a mode — Claude's
/// plan permission mode, Codex's read-only sandbox, OpenCode's `plan` agent,
/// Cursor's ask mode — and a provider that declares none may not be launched
/// for a review at all, rather than being launched able to edit.
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

/// How a provider re-enters one of its own earlier sessions (§13.5).
///
/// The two shapes are the two spellings the CLIs use for the same idea, and
/// both end as "these tokens, then the session id". Declaring it as data keeps
/// the provider-specific spelling next to the descriptor that owns it, so no
/// other crate branches on a provider id (principle P2).
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

/// How a provider is told to answer in a given JSON shape.
///
/// The two spellings are the two the CLIs use, and the difference is not
/// cosmetic: one takes the schema itself on the command line, the other takes
/// a path to a file holding it, so the caller has to write that file first.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum SchemaStyle {
    /// The schema follows the flag: `claude --json-schema '<json>'`.
    Inline { flag: String },
    /// A file holding the schema follows the flag:
    /// `codex --output-schema <path>`.
    File { flag: String },
}

/// How a provider runs one task without a terminal and then exits.
///
/// The interactive spelling (`prompt`, `resume`) says how to *open* a session;
/// this says how to run one and get an answer back. Both CLIs support it
/// today and both spell it differently — `claude -p --output-format
/// stream-json --verbose <prompt>` against `codex exec --json <prompt>` — so
/// the difference lives here as data and the daemon spawns them identically.
///
/// Deliberately **not** part of this: anything about credentials. A headless
/// run is the same binary started the same way as an interactive one and
/// reads the same subscription login from the provider's own config. (For
/// Claude Code specifically, that is why `--bare` must never appear in
/// `args`: bare mode skips the OAuth credentials and demands an API key.)
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeadlessSpec {
    /// The arguments that select non-interactive mode, first on the command
    /// line: `["-p"]`, `["exec"]`.
    pub mode_args: Vec<String>,
    /// The arguments that make it emit machine-readable events on stdout, one
    /// JSON object per line.
    pub stream_args: Vec<String>,
    /// How it re-enters one of its own headless runs. `None` means every job
    /// starts a fresh conversation.
    pub resume: Option<ResumeStyle>,
    /// How the task itself is passed.
    pub prompt: PromptStyle,
    /// How it is asked to answer in a given JSON shape, when it can be.
    ///
    /// What turns a reviewer's verdict from a line of prose somebody greps for
    /// into a value: `{"verdict":"APPROVED"}` either parses or it does not.
    /// `None` means the provider offers no such flag and the caller has to
    /// read the answer out of whatever the agent wrote on disk.
    pub schema: Option<SchemaStyle>,
    /// The field names that may carry the provider's *own* session id in the
    /// event stream, tried in order. Read once, so a follow-up job can resume
    /// the conversation this one started.
    ///
    /// Names only: parsing the stream is the daemon's job, because this crate
    /// depends on `serde` alone and knows nothing of JSON documents (§17).
    pub session_id_fields: Vec<String>,
    /// Flag granting one more writable directory on the headless command
    /// line, for providers that sandbox file writes to their cwd.
    ///
    /// Data, not a branch: the daemon appends the flag once per directory and
    /// a provider without one ignores the directories. Codex spells it
    /// `--add-dir`; a step running in a worktree reaches the repository's
    /// shared harness state through it.
    pub extra_writable_dir_flag: Option<String>,
}

impl HeadlessSpec {
    /// The whole command line after the executable.
    ///
    /// Order is fixed by the CLIs, not by taste: the mode selector leads
    /// (`codex exec`), a resume follows it as its own subcommand or flag
    /// (`codex exec resume <id>`, `claude -p --resume <id>`), and the prompt
    /// is last because both spell it positionally.
    #[must_use]
    pub fn command_args(&self, prompt: &str, resume_from: Option<&str>) -> Vec<String> {
        self.command_args_with(prompt, resume_from, None)
    }

    /// The command line including the schema argument, when one is asked for.
    ///
    /// `schema` is the value the flag takes: the schema document itself for
    /// [`SchemaStyle::Inline`], the path of a file holding it for
    /// [`SchemaStyle::File`]. A schema asked of a provider that declares none
    /// is dropped rather than guessed at — the answer is then prose, which is
    /// what it would have been anyway.
    #[must_use]
    pub fn command_args_with(
        &self,
        prompt: &str,
        resume_from: Option<&str>,
        schema: Option<&str>,
    ) -> Vec<String> {
        self.command_args_with_dirs(prompt, resume_from, schema, &[])
    }

    /// The command line granting `writable_dirs` as extra writable
    /// directories, for providers that declare
    /// [`HeadlessSpec::extra_writable_dir_flag`].
    ///
    /// Placed with the other options, before the positional prompt: anything
    /// appended after it would be read as part of the prompt.
    #[must_use]
    pub fn command_args_with_dirs(
        &self,
        prompt: &str,
        resume_from: Option<&str>,
        schema: Option<&str>,
        writable_dirs: &[std::path::PathBuf],
    ) -> Vec<String> {
        let mut args = self.mode_args.clone();
        if let (Some(style), Some(id)) = (self.resume.as_ref(), resume_from) {
            args.extend(style.args(id));
        }
        args.extend(self.stream_args.iter().cloned());
        if let Some(flag) = self.extra_writable_dir_flag.as_ref() {
            for dir in writable_dirs {
                args.push(flag.clone());
                args.push(dir.display().to_string());
            }
        }
        if let (Some(style), Some(schema)) = (self.schema.as_ref(), schema) {
            let flag = match style {
                SchemaStyle::Inline { flag } | SchemaStyle::File { flag } => flag,
            };
            args.push(flag.clone());
            args.push(schema.to_owned());
        }
        args.extend(self.prompt.args(prompt));
        args
    }
}

/// Declarative description of an agent provider (§7.5).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentDescriptor {
    pub id: AgentProviderId,
    pub display_name: String,
    /// Candidate binary names in order of preference.
    pub binary_candidates: Vec<String>,
    pub default_args: Vec<String>,
    pub version_probe: VersionProbe,
    /// Where to read account usage, when the provider exposes it (§16.2).
    /// `None` means no usage is reported for this provider.
    pub usage_source: Option<UsageSource>,
    /// How the provider re-enters an earlier session of its own (§13.5).
    /// `None` means it offers no way to, so its history is read-only.
    pub resume: Option<ResumeStyle>,
    /// How the provider takes a prompt at launch (§16.8). `None` means it only
    /// takes one typed into its TUI, so nothing may be launched *for* it.
    pub prompt: Option<PromptStyle>,
    /// How the provider runs a task headless. `None` means it has no such
    /// mode, so it can only ever be driven through a terminal.
    pub headless: Option<HeadlessSpec>,
    /// How the provider is put in a read-only posture (§16.9). `None` means it
    /// has none, so nothing may launch it for a review.
    #[serde(default)]
    pub review: Option<ReviewStyle>,
    pub capabilities: AgentCapabilities,
    /// The fields a profile editor offers for this provider (§13.4). Empty is
    /// fine: the generic argument and environment editors always work.
    pub profile_fields: Vec<ProfileField>,
}

/// A field the profile editor offers for one provider (§13.4).
///
/// Pure data, declared next to the descriptor in the `agents` crate, so the
/// UI can render a provider-aware form and the daemon can create a
/// directory-valued variable without any crate branching on a provider id
/// (principle P2).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileField {
    /// What the form calls it — `"Config directory"`.
    pub label: String,
    /// One line of help under the label.
    pub help: String,
    /// What filling it in does to the launch.
    pub effect: ProfileFieldEffect,
}

/// What a [`ProfileField`] contributes to a launch (§13.4).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum ProfileFieldEffect {
    /// Set an environment variable. A directory-valued one is created before
    /// the launch, which is what a hand-written shell wrapper did with
    /// `mkdir -p`.
    Env { name: String, is_directory: bool },
    /// Append this flag, followed by the value the user typed, to the args.
    Flag { flag: String },
}

/// A named way to launch a provider: its own command, arguments and
/// environment (§13.4).
///
/// A profile is not a provider. It borrows the provider's descriptor — icon,
/// detection, binary candidates, usage source — and overrides only how the
/// process starts. Unlike [`SpawnSpec::env`], which is a *complete*
/// environment, [`AgentProfile::env`] is an overlay applied on top of the
/// resolved login-shell environment.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentProfile {
    pub id: AgentProfileId,
    /// The provider this profile launches.
    pub provider_id: AgentProviderId,
    /// What the user calls it — `"Personal"`, `"Work"`.
    pub name: String,
    /// Program to run instead of the detected binary; `None` inherits it.
    pub executable: Option<PathBuf>,
    /// Appended after the descriptor's `default_args`.
    pub args: Vec<String>,
    /// Applied over the resolved environment, in order.
    pub env: Vec<(String, String)>,
    pub created_at: Timestamp,
}

/// Environment variables a profile may never set: Forge owns the terminal
/// contract with the child process (§13.3), and a profile that redefined these
/// would break the emulator rather than configure the agent.
pub const RESERVED_PROFILE_VARS: [&str; 5] = [
    "TERM",
    "TERMINFO",
    "COLORTERM",
    "FORGE_SESSION_ID",
    "FORGE_WORKSPACE",
];

impl AgentProfile {
    /// Whether `name` is a variable this profile is allowed to set.
    #[must_use]
    pub fn is_reserved_var(name: &str) -> bool {
        RESERVED_PROFILE_VARS.contains(&name)
    }

    /// Whether `name` is a syntactically valid environment variable name.
    #[must_use]
    pub fn is_valid_var_name(name: &str) -> bool {
        let mut chars = name.chars();
        match chars.next() {
            Some(first) if first.is_ascii_alphabetic() || first == '_' => {}
            _ => return false,
        }
        chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
    }
}

/// Where a child session should run relative to its parent (§8.2).
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
            args: vec!["--model".to_owned(), "opus".to_owned()],
            env: vec![(
                "CLAUDE_CONFIG_DIR".to_owned(),
                "/home/me/.claude-work".to_owned(),
            )],
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
    fn variable_names_follow_the_posix_shape() {
        for good in ["PATH", "_HIDDEN", "CLAUDE_CONFIG_DIR", "A1"] {
            assert!(AgentProfile::is_valid_var_name(good), "{good}");
        }
        for bad in ["", "1PATH", "WITH-DASH", "WITH SPACE", "WITH=EQ", "é"] {
            assert!(!AgentProfile::is_valid_var_name(bad), "{bad:?}");
        }
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
