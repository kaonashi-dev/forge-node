//! SQLite metadata store (ADR-009).
//!
//! Projects, workspaces, sessions, context, profiles, shares, ignores, and
//! opaque app state. Never the terminal stream, never a runtime `terminal_id`.
//! Encoding and schema: `docs/persistence.md`. Migrations are append-only.

pub mod db;
pub mod migrations;
pub mod repositories;

pub use db::{Db, DbError};
pub use repositories::{
    AgentProfileRepo, AppStateRepo, ContextRepo, OrchestrationRepo, ProjectGroupRepo, ProjectRepo,
    ProviderOverrideRepo, SessionRepo, WorkspaceRepo,
};

#[cfg(test)]
mod tests;
