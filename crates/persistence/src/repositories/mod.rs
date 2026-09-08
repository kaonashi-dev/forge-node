//! One repository per table (§15.2). Each repository is a thin, stateless
//! wrapper that borrows a [`rusqlite::Connection`] and maps every domain field
//! faithfully to and from its columns.
//!
//! Shared column encoders live here: domain enums are stored as small, explicit
//! and stable TEXT tags (never via serde) so column values stay readable and do
//! not drift if a serde representation changes.

pub mod agent_profiles;
pub mod app_state;
pub mod context;
pub mod project_groups;
pub mod projects;
pub mod provider_overrides;
pub mod sessions;
pub mod shares;
pub mod workspaces;

pub use agent_profiles::AgentProfileRepo;
pub use app_state::AppStateRepo;
pub use context::ContextRepo;
pub use project_groups::ProjectGroupRepo;
pub use projects::ProjectRepo;
pub use provider_overrides::ProviderOverrideRepo;
pub use sessions::SessionRepo;
pub use shares::ShareRepo;
pub use workspaces::WorkspaceRepo;

use std::path::Path;
use std::str::FromStr;

use domain::{SessionKind, SessionRole, SessionState, Timestamp, WorkspaceKind};

use crate::db::DbError;

/// Reason substituted for `SessionState::Failed` on load: the original reason is
/// not a column (§15.2), so it cannot survive a restart. This is acceptable —
/// any session that was live becomes `Orphaned` on reconciliation anyway (§15.3).
pub(crate) const FAILED_REASON_PLACEHOLDER: &str = "reason not persisted across restart";

// ---- path <-> TEXT --------------------------------------------------------

/// Render a path for storage. Non-UTF-8 paths are stored lossily; the MVP only
/// ever stores canonicalized project/workspace paths.
pub(crate) fn path_to_str(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

// ---- Timestamp <-> RFC-3339 TEXT ------------------------------------------

pub(crate) fn ts_to_str(ts: &Timestamp) -> String {
    ts.to_rfc3339()
}

pub(crate) fn ts_from_str(s: &str) -> Result<Timestamp, DbError> {
    Timestamp::parse_rfc3339(s).map_err(|e| DbError::decode("Timestamp", s, e))
}

/// Decode an optional RFC-3339 timestamp column.
pub(crate) fn ts_from_opt(s: Option<String>) -> Result<Option<Timestamp>, DbError> {
    s.as_deref().map(ts_from_str).transpose()
}

// ---- typed ids <-> TEXT ----------------------------------------------------

/// Decode a UUID-backed id column. Works for any id newtype whose `FromStr`
/// error is `Display`.
pub(crate) fn id_from_str<T>(what: &str, s: &str) -> Result<T, DbError>
where
    T: FromStr,
    T::Err: std::fmt::Display,
{
    T::from_str(s).map_err(|e| DbError::decode(what, s, e))
}

/// Decode an optional UUID-backed id column.
pub(crate) fn id_from_opt<T>(what: &str, s: Option<String>) -> Result<Option<T>, DbError>
where
    T: FromStr,
    T::Err: std::fmt::Display,
{
    s.as_deref().map(|v| id_from_str(what, v)).transpose()
}

// ---- WorkspaceKind <-> TEXT ------------------------------------------------

pub(crate) fn workspace_kind_to_str(kind: WorkspaceKind) -> Result<&'static str, DbError> {
    match kind {
        WorkspaceKind::Main => Ok("Main"),
        WorkspaceKind::GitWorktree => Ok("GitWorktree"),
        // `WorkspaceKind` is `#[non_exhaustive]`: a variant added later would
        // compile here but have no column encoding. Refusing the write returns a
        // structured error instead of panicking inside the daemon's core lock,
        // which would poison it for every other request.
        other => Err(DbError::Encode(format!(
            "unhandled WorkspaceKind variant: {other:?}"
        ))),
    }
}

pub(crate) fn workspace_kind_from_str(s: &str) -> Result<WorkspaceKind, DbError> {
    match s {
        "Main" => Ok(WorkspaceKind::Main),
        "GitWorktree" => Ok(WorkspaceKind::GitWorktree),
        _ => Err(DbError::decode_msg("WorkspaceKind", s, "unknown kind tag")),
    }
}

// ---- SessionKind <-> TEXT --------------------------------------------------

pub(crate) fn session_kind_to_str(kind: SessionKind) -> Result<&'static str, DbError> {
    match kind {
        SessionKind::Shell => Ok("Shell"),
        SessionKind::Agent => Ok("Agent"),
        other => Err(DbError::Encode(format!(
            "unhandled SessionKind variant: {other:?}"
        ))),
    }
}

pub(crate) fn session_kind_from_str(s: &str) -> Result<SessionKind, DbError> {
    match s {
        "Shell" => Ok(SessionKind::Shell),
        "Agent" => Ok(SessionKind::Agent),
        _ => Err(DbError::decode_msg("SessionKind", s, "unknown kind tag")),
    }
}

// ---- SessionRole <-> TEXT --------------------------------------------------
//
// Known roles are stored as their plain tag ("Generic", "Planner", ...).
// `Custom(name)` is stored as `"custom:<name>"` so it round-trips even when the
// custom name collides with a known tag (stored as `"custom:Generic"`) or
// contains colons. No known tag starts with `custom:`, so the two spaces never
// overlap.

pub(crate) fn role_to_str(role: &SessionRole) -> Result<String, DbError> {
    match role {
        SessionRole::Generic => Ok("Generic".to_owned()),
        SessionRole::Orchestrator => Ok("Orchestrator".to_owned()),
        SessionRole::Planner => Ok("Planner".to_owned()),
        SessionRole::Researcher => Ok("Researcher".to_owned()),
        SessionRole::Executor => Ok("Executor".to_owned()),
        SessionRole::Reviewer => Ok("Reviewer".to_owned()),
        SessionRole::Tester => Ok("Tester".to_owned()),
        SessionRole::Custom(name) => Ok(format!("custom:{name}")),
        other => Err(DbError::Encode(format!(
            "unhandled SessionRole variant: {other:?}"
        ))),
    }
}

pub(crate) fn role_from_str(s: &str) -> Result<SessionRole, DbError> {
    if let Some(name) = s.strip_prefix("custom:") {
        return Ok(SessionRole::Custom(name.to_owned()));
    }
    match s {
        "Generic" => Ok(SessionRole::Generic),
        "Orchestrator" => Ok(SessionRole::Orchestrator),
        "Planner" => Ok(SessionRole::Planner),
        "Researcher" => Ok(SessionRole::Researcher),
        "Executor" => Ok(SessionRole::Executor),
        "Reviewer" => Ok(SessionRole::Reviewer),
        "Tester" => Ok(SessionRole::Tester),
        _ => Err(DbError::decode_msg("SessionRole", s, "unknown role tag")),
    }
}

// ---- SessionState <-> (last_state TEXT, last_exit_code INTEGER) -------------
//
// Only the discriminant is stored in `last_state`; the exit code goes to its own
// column. `signal` has no column, so it is lost across a restart (§15.2) — this
// is acceptable because a `Running` session is reconciled to `Orphaned` anyway
// (§15.3). `Failed { reason }` likewise loses its reason and is reconstructed
// with `FAILED_REASON_PLACEHOLDER`.

pub(crate) fn state_discriminant(state: &SessionState) -> Result<&'static str, DbError> {
    match state {
        SessionState::Starting => Ok("Starting"),
        SessionState::Running => Ok("Running"),
        SessionState::Exited { .. } => Ok("Exited"),
        SessionState::Failed { .. } => Ok("Failed"),
        SessionState::Orphaned => Ok("Orphaned"),
        other => Err(DbError::Encode(format!(
            "unhandled SessionState variant: {other:?}"
        ))),
    }
}

/// The value for the `last_exit_code` column: the exit code for `Exited`, else
/// `None`. Infallible and correct for every present or future variant.
pub(crate) fn state_exit_code(state: &SessionState) -> Option<i32> {
    match state {
        SessionState::Exited { code, .. } => *code,
        _ => None,
    }
}

pub(crate) fn state_from_parts(
    discriminant: &str,
    exit_code: Option<i32>,
) -> Result<SessionState, DbError> {
    match discriminant {
        "Starting" => Ok(SessionState::Starting),
        "Running" => Ok(SessionState::Running),
        // signal is not a column (§15.2), so it is reconstructed as None.
        "Exited" => Ok(SessionState::Exited {
            code: exit_code,
            signal: None,
        }),
        // reason is not a column (§15.2); reconstruct a generic one.
        "Failed" => Ok(SessionState::Failed {
            reason: FAILED_REASON_PLACEHOLDER.to_owned(),
        }),
        "Orphaned" => Ok(SessionState::Orphaned),
        _ => Err(DbError::decode_msg(
            "SessionState",
            discriminant,
            "unknown state discriminant",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use domain::SessionId;

    /// Assert that a decode failed as a [`DbError::Decode`] naming the value,
    /// rather than panicking inside a request handler.
    fn assert_decode_error<T: std::fmt::Debug>(result: Result<T, DbError>, value: &str) {
        match result {
            Err(DbError::Decode(msg)) => assert!(
                msg.contains(value),
                "decode error should name the offending value, got {msg:?}"
            ),
            other => panic!("expected a Decode error for {value:?}, got {other:?}"),
        }
    }

    #[test]
    fn workspace_and_session_kind_tags_round_trip() {
        for kind in [WorkspaceKind::Main, WorkspaceKind::GitWorktree] {
            let tag = workspace_kind_to_str(kind).unwrap();
            assert_eq!(workspace_kind_from_str(tag).unwrap(), kind);
        }
        for kind in [SessionKind::Shell, SessionKind::Agent] {
            let tag = session_kind_to_str(kind).unwrap();
            assert_eq!(session_kind_from_str(tag).unwrap(), kind);
        }
    }

    #[test]
    fn every_known_role_round_trips() {
        for role in [
            SessionRole::Generic,
            SessionRole::Orchestrator,
            SessionRole::Planner,
            SessionRole::Researcher,
            SessionRole::Executor,
            SessionRole::Reviewer,
            SessionRole::Tester,
        ] {
            let tag = role_to_str(&role).unwrap();
            assert!(
                !tag.starts_with("custom:"),
                "a known role must not use the custom namespace"
            );
            assert_eq!(role_from_str(&tag).unwrap(), role);
        }
    }

    /// The `custom:` prefix keeps user-named roles in their own namespace, so a
    /// custom role may safely be spelled like a built-in one or contain colons.
    #[test]
    fn a_custom_role_round_trips_even_when_it_shadows_a_known_tag() {
        for name in ["Reviewer", "a:b:c", "custom:nested", "", "  spaced  "] {
            let role = SessionRole::Custom(name.to_owned());
            let tag = role_to_str(&role).unwrap();
            assert_eq!(tag, format!("custom:{name}"));
            assert_eq!(role_from_str(&tag).unwrap(), role);
        }
    }

    #[test]
    fn an_unknown_tag_is_a_decode_error_not_a_panic() {
        assert_decode_error(workspace_kind_from_str("Submodule"), "Submodule");
        assert_decode_error(session_kind_from_str("Daemon"), "Daemon");
        assert_decode_error(role_from_str("Archivist"), "Archivist");
        assert_decode_error(state_from_parts("Zombie", None), "Zombie");
        // Case matters: the tags are exact, not normalized.
        assert_decode_error(workspace_kind_from_str("main"), "main");
        assert_decode_error(role_from_str(""), "");
    }

    #[test]
    fn a_corrupt_id_or_timestamp_column_is_a_decode_error() {
        assert_decode_error(
            id_from_str::<SessionId>("SessionId", "not-a-uuid"),
            "not-a-uuid",
        );
        assert_decode_error(ts_from_str("yesterday"), "yesterday");
        // A well-formed value still decodes.
        let id = SessionId::new();
        assert_eq!(
            id_from_str::<SessionId>("SessionId", &id.to_string()).unwrap(),
            id
        );
        let ts = Timestamp::now();
        assert_eq!(ts_from_str(&ts_to_str(&ts)).unwrap(), ts);
    }

    #[test]
    fn optional_columns_decode_none_without_touching_the_parser() {
        assert_eq!(ts_from_opt(None).unwrap(), None);
        assert_eq!(id_from_opt::<SessionId>("SessionId", None).unwrap(), None);

        let id = SessionId::new();
        assert_eq!(
            id_from_opt::<SessionId>("SessionId", Some(id.to_string())).unwrap(),
            Some(id)
        );
        assert_decode_error(
            id_from_opt::<SessionId>("SessionId", Some("bogus".to_owned())),
            "bogus",
        );
    }

    /// The exit code lives in its own column, so the discriminant must never
    /// smuggle it — and no state other than `Exited` may fill that column.
    #[test]
    fn the_exit_code_column_is_filled_only_by_an_exited_state() {
        assert_eq!(
            state_exit_code(&SessionState::Exited {
                code: Some(3),
                signal: None
            }),
            Some(3)
        );
        assert_eq!(
            state_exit_code(&SessionState::Exited {
                code: None,
                signal: Some(9)
            }),
            None
        );
        assert_eq!(state_exit_code(&SessionState::Running), None);
        assert_eq!(
            state_exit_code(&SessionState::Failed {
                reason: "boom".to_owned()
            }),
            None
        );
    }

    /// §15.2: `signal` and `reason` have no columns. They must come back as a
    /// documented placeholder rather than silently pretending to be the original.
    #[test]
    fn a_state_is_rebuilt_from_its_discriminant_and_exit_code() {
        assert_eq!(
            state_from_parts("Starting", None).unwrap(),
            SessionState::Starting
        );
        assert_eq!(
            state_from_parts("Running", None).unwrap(),
            SessionState::Running
        );
        assert_eq!(
            state_from_parts("Orphaned", None).unwrap(),
            SessionState::Orphaned
        );
        assert_eq!(
            state_from_parts("Exited", Some(130)).unwrap(),
            SessionState::Exited {
                code: Some(130),
                signal: None
            }
        );
        assert_eq!(
            state_from_parts("Failed", None).unwrap(),
            SessionState::Failed {
                reason: FAILED_REASON_PLACEHOLDER.to_owned()
            }
        );

        // A signalled exit loses only its signal; the code column still rules.
        let signalled = SessionState::Exited {
            code: None,
            signal: Some(9),
        };
        let discriminant = state_discriminant(&signalled).unwrap();
        assert_eq!(
            state_from_parts(discriminant, state_exit_code(&signalled)).unwrap(),
            SessionState::Exited {
                code: None,
                signal: None
            }
        );
    }

    #[test]
    fn each_state_has_a_stable_discriminant() {
        for (state, tag) in [
            (SessionState::Starting, "Starting"),
            (SessionState::Running, "Running"),
            (
                SessionState::Exited {
                    code: Some(0),
                    signal: None,
                },
                "Exited",
            ),
            (
                SessionState::Failed {
                    reason: "x".to_owned(),
                },
                "Failed",
            ),
            (SessionState::Orphaned, "Orphaned"),
        ] {
            assert_eq!(state_discriminant(&state).unwrap(), tag);
        }
    }

    #[test]
    fn a_path_is_stored_as_its_display_form() {
        assert_eq!(path_to_str(Path::new("/tmp/forge demo")), "/tmp/forge demo");
        assert_eq!(path_to_str(Path::new("")), "");
    }
}
