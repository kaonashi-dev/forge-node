//! SQLite metadata store (ADR-009). Never the terminal stream, never a runtime `terminal_id`.
//!
//! Encoding and schema: `docs/persistence.md`. Migrations are append-only.

pub mod db;
pub mod migrations;
pub mod repositories;

pub use db::{managed_worktree_paths, vacuum_into, Db, DbError, ReviewStore, SpecRunStore};
pub use repositories::{
    AgentProfileRepo, AppStateRepo, ContextRepo, OrchestrationRepo, ProjectGroupRepo, ProjectRepo,
    ProviderOverrideRepo, SessionRepo, WorkspaceRepo,
};

#[cfg(test)]
mod tests;
