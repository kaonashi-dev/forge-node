//! # persistence
//!
//! SQLite-backed persistence of Forge (ForgeNode) domain metadata (§15).
//!
//! Per **ADR-009**, SQLite stores metadata only — projects, workspaces,
//! sessions and their graph, context envelopes, provider overrides and opaque
//! app/layout state. It **never** stores the terminal stream: scrollback lives
//! in daemon memory, bounded. Runtime-only identifiers such as `terminal_id` are
//! likewise never persisted (§15.2).
//!
//! ## Layout
//! - [`db`]: the [`Db`] handle, connection setup (WAL + `foreign_keys = ON`),
//!   the [`DbError`] type, and daemon-startup reconciliation
//!   ([`Db::reconcile_orphaned`], §15.3).
//! - [`migrations`]: the ordered schema migrations (§15.2), versioned through
//!   SQLite's `user_version`.
//! - [`repositories`]: one repository per table, mapping every domain field to
//!   and from its columns. Domain enums are stored as explicit, stable TEXT tags
//!   (never via serde).
//!
//! ## Encoding conventions (§15.2)
//! - IDs are stored as TEXT (`id.to_string()` / `Id::from_str`).
//! - Timestamps are RFC-3339 TEXT (`Timestamp::to_rfc3339` / `parse_rfc3339`).
//! - Enums are TEXT: [`domain::SessionRole::Custom`] round-trips as
//!   `"custom:<name>"`; [`domain::SessionState`] stores its discriminant in
//!   `last_state` and the exit code in `last_exit_code`. A session's `signal`
//!   and a `Failed` reason have no column and are lost across a restart, which is
//!   acceptable because live sessions are reconciled to `Orphaned` anyway
//!   (§15.3).

pub mod db;
pub mod migrations;
pub mod repositories;

pub use db::{Db, DbError};
pub use repositories::{
    AgentProfileRepo, AppStateRepo, ContextRepo, ProjectGroupRepo, ProjectRepo,
    ProviderOverrideRepo, SessionRepo, WorkspaceRepo,
};

#[cfg(test)]
mod tests;
