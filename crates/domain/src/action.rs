//! One-click commands a project runs in its workspaces.
//!
//! [`ProjectAction`] is persisted per project and shared by every workspace of
//! it; [`ActionRun`] is runtime state on a `Task` session, like
//! [`crate::Session::terminal_id`]. Details: `docs/actions.md`.

use crate::ids::{ProjectActionId, ProjectId, Timestamp};
use serde::{Deserialize, Serialize};

pub const MAX_PROJECT_ACTIONS: usize = 32;
pub const MAX_ACTION_LABEL_BYTES: usize = 64;
pub const MAX_ACTION_COMMAND_BYTES: usize = 4096;
pub const MAX_ACTION_DEPENDENCIES: usize = 8;
pub const MAX_READY_PATTERN_BYTES: usize = 256;
pub const MAX_READY_TIMEOUT_SECS: u64 = 600;
/// How many ports past [`ActionPort::base`] a per-worktree port may land on.
pub const ACTION_PORT_SPAN: u16 = 50;
/// The placeholder [`ProjectAction::command`] may carry for the resolved port.
pub const PORT_PLACEHOLDER: &str = "{port}";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectAction {
    pub id: ProjectActionId,
    pub project_id: ProjectId,
    /// Unique within the project, ignoring case.
    pub label: String,
    /// Run as `$SHELL -l -c` from [`Self::cwd`]; `{port}` is replaced by the
    /// port resolved at launch.
    pub command: String,
    pub kind: ActionKind,
    /// Relative to the workspace root, validated like a share path; `None` is
    /// the root.
    #[serde(default)]
    pub cwd: Option<String>,
    /// At most one live session per (workspace, action); a second run focuses it.
    pub single_instance: bool,
    /// Shown on the workspace row without hovering.
    pub pinned: bool,
    #[serde(default)]
    pub ready: ActionReady,
    #[serde(default)]
    pub port: Option<ActionPort>,
    /// Actions of the same project this one waits for: a probed or `Dev`
    /// action until it serves, any other until it exits 0.
    #[serde(default)]
    pub depends_on: Vec<ProjectActionId>,
    pub source: ActionSource,
    pub position: u32,
    pub created_at: Timestamp,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ActionKind {
    Dev,
    Test,
    Lint,
    Build,
    /// Also what a kind this build does not recognize reads as.
    #[serde(other)]
    Custom,
}

/// How the daemon learns that a long-running action is serving.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ActionReady {
    /// A TCP connect to `127.0.0.1` on the resolved [`ProjectAction::port`].
    Port { timeout_secs: u64 },
    /// A case-insensitive literal substring of the output, ANSI stripped. Not
    /// a regex: a repository-supplied pattern must not be able to cost more
    /// than a scan.
    Output { pattern: String, timeout_secs: u64 },
    /// Running is all there is to know. Also an unrecognized probe.
    #[default]
    #[serde(other)]
    None,
}

impl ActionReady {
    #[must_use]
    pub const fn timeout_secs(&self) -> Option<u64> {
        match self {
            Self::None => None,
            Self::Port { timeout_secs } | Self::Output { timeout_secs, .. } => Some(*timeout_secs),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionPort {
    pub base: u16,
    /// Take the first free port in `base..base + ACTION_PORT_SPAN`, so a
    /// worktree's server does not collide with main's; otherwise exactly `base`.
    pub per_worktree: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ActionSource {
    /// Suggested from the workspace's manifests and accepted in the dialog.
    Detected,
    Manual,
    /// Imported from `.forge/actions.toml`. `file_hash` is the SHA-256 (hex)
    /// of the file the user approved; the saved copy is what runs, so a later
    /// edit of the file is drift to review, never a new command.
    Repo {
        file_hash: String,
    },
}

/// Runtime state of a `Task` session. Never a column; `None` after a restart.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionRun {
    pub action_id: ProjectActionId,
    /// Copied at launch so a deleted or renamed action still names its run.
    pub label: String,
    pub port: Option<u16>,
    pub readiness: ActionReadiness,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ActionReadiness {
    /// Dependencies are not ready; nothing is spawned yet.
    Waiting {
        on: Vec<ProjectActionId>,
    },
    Probing,
    Ready {
        url: Option<String>,
    },
    /// The probe's deadline passed; the process may still be serving.
    TimedOut,
    /// A fixed port was already taken at launch. It launched anyway.
    PortInUse {
        port: u16,
    },
    /// The action has no probe. Also an unrecognized state.
    #[serde(other)]
    Unprobed,
}

impl ActionReadiness {
    /// Whether a dependent may start on the strength of this dependency.
    #[must_use]
    pub const fn satisfies_dependents(&self) -> bool {
        matches!(self, Self::Ready { .. } | Self::Unprobed)
    }
}

/// An action the daemon would propose from a workspace's files. Becomes a
/// [`ProjectAction`] only when the dialog saves it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionSuggestion {
    pub label: String,
    pub command: String,
    pub kind: ActionKind,
    pub pinned: bool,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default = "single_instance_default")]
    pub single_instance: bool,
    #[serde(default)]
    pub ready: ActionReady,
    #[serde(default)]
    pub port: Option<ActionPort>,
    /// Labels of other suggestions from the same source, resolved to ids when
    /// the dialog saves them.
    #[serde(default)]
    pub depends_on: Vec<String>,
    pub source: ActionSource,
    /// The file the suggestion was read from, such as `package.json`.
    pub detected_from: String,
    /// The saved action with the same label, ignoring case.
    pub saved_as: Option<ProjectActionId>,
}

const fn single_instance_default() -> bool {
    true
}

/// What `.forge/actions.toml` in the workspace says, offered for import.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoActionsFile {
    /// SHA-256 (hex) of the bytes read; what an import records.
    pub file_hash: String,
    pub actions: Vec<ActionSuggestion>,
    /// Why the file could not be read or parsed; `actions` is empty then.
    pub error: Option<String>,
    /// Saved actions imported from an earlier version of the file.
    pub drift: Vec<ProjectActionId>,
}

/// `command` with every `{port}` replaced; unchanged without a port.
#[must_use]
pub fn resolve_command(command: &str, port: Option<u16>) -> String {
    match port {
        Some(port) => command.replace(PORT_PLACEHOLDER, &port.to_string()),
        None => command.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_kind_and_probe_read_as_the_safe_default() {
        let kind: ActionKind = serde_json::from_str("\"deploy\"").unwrap();
        assert_eq!(kind, ActionKind::Custom);
        let ready: ActionReady = serde_json::from_str(r#"{"kind":"http","url":"x"}"#).unwrap();
        assert_eq!(ready, ActionReady::None);
        let readiness: ActionReadiness = serde_json::from_str(r#"{"state":"healthy"}"#).unwrap();
        assert_eq!(readiness, ActionReadiness::Unprobed);
    }

    #[test]
    fn ready_round_trips_with_its_tag() {
        let ready = ActionReady::Output {
            pattern: "localhost:".into(),
            timeout_secs: 60,
        };
        let json = serde_json::to_string(&ready).unwrap();
        assert_eq!(
            json,
            r#"{"kind":"output","pattern":"localhost:","timeout_secs":60}"#
        );
        assert_eq!(serde_json::from_str::<ActionReady>(&json).unwrap(), ready);
    }

    #[test]
    fn the_port_placeholder_is_replaced_everywhere() {
        assert_eq!(
            resolve_command("vite --port {port} # {port}", Some(5174)),
            "vite --port 5174 # 5174"
        );
        assert_eq!(
            resolve_command("vite --port {port}", None),
            "vite --port {port}"
        );
    }
}
