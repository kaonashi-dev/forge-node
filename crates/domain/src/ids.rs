//! Typed identifiers and the shared `Timestamp` type (§7).
//!
//! All UUID-based IDs are newtypes over `uuid::Uuid` v7 (time-ordered).
//! Timestamps are `time::OffsetDateTime` in UTC, serialized as RFC-3339.

use serde::{Deserialize, Serialize};
use std::fmt;
use time::OffsetDateTime;
use uuid::Uuid;

/// Defines a newtype wrapper over `uuid::Uuid` with v7 generation and
/// string round-tripping. IDs are ordered by their inner UUID (time-ordered
/// for v7), which keeps sidebar/session ordering stable and chronological.
macro_rules! uuid_id {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(
            Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
        )]
        pub struct $name(pub Uuid);

        impl $name {
            /// Generate a fresh, time-ordered v7 identifier.
            #[must_use]
            pub fn new() -> Self {
                Self(Uuid::now_v7())
            }

            /// Wrap an existing UUID.
            #[must_use]
            pub const fn from_uuid(id: Uuid) -> Self {
                Self(id)
            }

            /// The inner UUID.
            #[must_use]
            pub const fn as_uuid(&self) -> Uuid {
                self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt::Display::fmt(&self.0, f)
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}({})", stringify!($name), self.0)
            }
        }

        impl std::str::FromStr for $name {
            type Err = uuid::Error;
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Ok(Self(Uuid::parse_str(s)?))
            }
        }

        impl From<Uuid> for $name {
            fn from(id: Uuid) -> Self {
                Self(id)
            }
        }
    };
}

uuid_id!(
    /// Identifies a user-added project directory.
    ProjectId
);
uuid_id!(
    /// Identifies an organizational group of projects.
    ProjectGroupId
);
uuid_id!(
    /// Identifies a workspace (main checkout or worktree) within a project.
    WorkspaceId
);
uuid_id!(
    /// Identifies a persistent session (domain node of the session graph).
    SessionId
);
uuid_id!(
    /// Identifies a live terminal runtime. Runtime-only, never persisted;
    /// regenerated on every spawn/restart (§15.2).
    TerminalId
);
uuid_id!(
    /// Identifies a connected IPC client.
    ClientId
);
uuid_id!(
    /// Identifies a context envelope (§8.3).
    ContextId
);
uuid_id!(
    /// Identifies a saved launch profile for an agent provider (§13.4).
    AgentProfileId
);
uuid_id!(
    /// Identifies one sharing rule of a project (§14.2). Declared here rather
    /// than in `share`: `uuid_id!` is a `macro_rules!` of this module with no
    /// `#[macro_export]`, so it is not reachable from a sibling.
    ShareRuleId
);
uuid_id!(
    /// Identifies one headless agent run (`crate::job::Job`). Runtime-only,
    /// like [`TerminalId`]: a job is a process, and no process outlives the
    /// daemon that started it.
    JobId
);

/// Identifies an agent provider, e.g. `"claude"`, `"codex"`, `"opencode"`,
/// `"cursor"`. Unlike the UUID ids this is a stable, human-readable string
/// (§7.5) so descriptors and overrides can key on it.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct AgentProviderId(pub String);

impl AgentProviderId {
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for AgentProviderId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for AgentProviderId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "AgentProviderId({})", self.0)
    }
}

impl From<&str> for AgentProviderId {
    fn from(s: &str) -> Self {
        Self(s.to_owned())
    }
}

/// A UTC timestamp, serialized as an RFC-3339 string (§7).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Timestamp(#[serde(with = "time::serde::rfc3339")] pub OffsetDateTime);

impl Timestamp {
    /// The current instant in UTC.
    #[must_use]
    pub fn now() -> Self {
        Self(OffsetDateTime::now_utc())
    }

    #[must_use]
    pub const fn from_offset(dt: OffsetDateTime) -> Self {
        Self(dt)
    }

    #[must_use]
    pub const fn as_offset(&self) -> OffsetDateTime {
        self.0
    }

    /// RFC-3339 rendering, used for SQLite persistence (§15).
    #[must_use]
    pub fn to_rfc3339(&self) -> String {
        self.0
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_default()
    }

    /// Parse an RFC-3339 string back into a timestamp.
    ///
    /// # Errors
    /// Returns a `time::error::Parse` if the string is not valid RFC-3339.
    pub fn parse_rfc3339(s: &str) -> Result<Self, time::error::Parse> {
        Ok(Self(OffsetDateTime::parse(
            s,
            &time::format_description::well_known::Rfc3339,
        )?))
    }

    /// Build from a Unix epoch second count (Codex's `reset_at`, etc.).
    #[must_use]
    pub fn from_unix_secs(secs: i64) -> Option<Self> {
        OffsetDateTime::from_unix_timestamp(secs).ok().map(Self)
    }
}

impl fmt::Debug for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Timestamp({})", self.to_rfc3339())
    }
}

impl fmt::Display for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_rfc3339())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn uuid_ids_are_time_ordered() {
        let a = SessionId::new();
        let b = SessionId::new();
        // v7 is time-ordered; a created earlier sorts before b.
        assert!(a < b, "v7 ids should be monotonically increasing");
    }

    #[test]
    fn uuid_id_roundtrips_through_string() {
        let id = ProjectId::new();
        let s = id.to_string();
        let back = ProjectId::from_str(&s).unwrap();
        assert_eq!(id, back);
    }

    #[test]
    fn uuid_id_roundtrips_through_json() {
        let id = WorkspaceId::new();
        let json = serde_json::to_string(&id).unwrap();
        let back: WorkspaceId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, back);
    }

    #[test]
    fn provider_id_is_a_plain_string() {
        let id = AgentProviderId::from("claude");
        assert_eq!(id.as_str(), "claude");
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, "\"claude\"");
    }

    #[test]
    fn timestamp_roundtrips_rfc3339() {
        let ts = Timestamp::now();
        let s = ts.to_rfc3339();
        let back = Timestamp::parse_rfc3339(&s).unwrap();
        assert_eq!(ts, back);
    }

    #[test]
    fn timestamp_from_unix_secs() {
        let ts = Timestamp::from_unix_secs(1_787_707_211).expect("valid epoch");
        assert!(ts.to_rfc3339().starts_with("2026-"));
        assert!(Timestamp::from_unix_secs(i64::MIN).is_none());
    }
}
