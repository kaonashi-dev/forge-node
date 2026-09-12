//! Project worktree-ignore rules repository (§14.4, §15.2).
//!
//! One row per `(project_id, path)`: a rule has no identity of its own and the
//! rescan keys on the path, so the path is the primary key. `RemoveWorktree`
//! upserts an `Exact` tombstone here, the GUI replaces a project's whole
//! `Subtree` policy, and the rescan's GC deletes a tombstone whose worktree is
//! really gone.
//!
//! `path` is stored as written by the daemon, which resolves it when the rule is
//! written. The daemon's plan compares it against canonical git paths, so a
//! relative or `..` path must never reach this table.

use domain::{IgnoreScope, ProjectId, WorktreeIgnore};
use rusqlite::{params, Connection, Row};
use std::path::Path;

use crate::db::DbError;
use crate::repositories::{path_to_str, ts_from_str, ts_to_str};

const COLUMNS: &str = "project_id, path, scope, created_at";

/// Repository over the `worktree_ignores` table.
pub struct IgnoreRepo<'a> {
    conn: &'a Connection,
}

impl<'a> IgnoreRepo<'a> {
    #[must_use]
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }

    /// Every rule of every project, ordered by project then path.
    ///
    /// # Errors
    /// Returns [`DbError`] on a failed query or an undecodable row.
    pub fn list_all(&self) -> Result<Vec<WorktreeIgnore>, DbError> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {COLUMNS} FROM worktree_ignores ORDER BY project_id, path"
        ))?;
        let rows = stmt.query_map([], RawRule::from_row)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?.into_domain()?);
        }
        Ok(out)
    }

    /// Insert a rule or replace the one already at its path.
    ///
    /// # Errors
    /// Returns [`DbError`] on a failed statement or an unencodable scope.
    pub fn upsert(&self, rule: &WorktreeIgnore) -> Result<(), DbError> {
        self.conn.execute(
            "INSERT INTO worktree_ignores (project_id, path, scope, created_at) \
             VALUES (?1, ?2, ?3, ?4) \
             ON CONFLICT(project_id, path) DO UPDATE SET scope = excluded.scope",
            params![
                rule.project_id.to_string(),
                path_to_str(&rule.path),
                scope_to_str(rule.scope)?,
                ts_to_str(&rule.created_at),
            ],
        )?;
        Ok(())
    }

    /// Delete the rule at `path`, if any. Returns `true` if a row was removed.
    ///
    /// # Errors
    /// Returns [`DbError`] on a failed statement.
    pub fn delete(&self, project_id: ProjectId, path: &Path) -> Result<bool, DbError> {
        let n = self.conn.execute(
            "DELETE FROM worktree_ignores WHERE project_id = ?1 AND path = ?2",
            params![project_id.to_string(), path_to_str(path)],
        )?;
        Ok(n > 0)
    }

    /// Replace a project's whole rule set, in one transaction.
    ///
    /// The GUI edits the set as a list, so a half-applied replacement would be
    /// visible to a concurrent read. `created_at` is the caller's to preserve.
    ///
    /// # Errors
    /// Returns [`DbError`] on a failed statement or an unencodable scope.
    pub fn replace_for_project(
        &self,
        project_id: ProjectId,
        rules: &[WorktreeIgnore],
    ) -> Result<(), DbError> {
        self.conn.execute("BEGIN IMMEDIATE", [])?;
        let result = self.write_set(project_id, rules);
        if result.is_ok() {
            self.conn.execute("COMMIT", [])?;
        } else {
            // A failed set leaves the previous one: half a list is not a list.
            let _ = self.conn.execute("ROLLBACK", []);
        }
        result
    }

    fn write_set(&self, project_id: ProjectId, rules: &[WorktreeIgnore]) -> Result<(), DbError> {
        self.conn.execute(
            "DELETE FROM worktree_ignores WHERE project_id = ?1",
            params![project_id.to_string()],
        )?;
        for rule in rules {
            self.upsert(rule)?;
        }
        Ok(())
    }
}

fn scope_to_str(scope: IgnoreScope) -> Result<&'static str, DbError> {
    match scope {
        IgnoreScope::Exact => Ok("exact"),
        IgnoreScope::Subtree => Ok("subtree"),
        // `IgnoreScope` is `#[non_exhaustive]`: a variant added later would
        // compile here but have no column encoding. Refusing the write returns a
        // structured error instead of panicking inside the daemon's core lock.
        other => Err(DbError::Encode(format!(
            "unhandled IgnoreScope variant: {other:?}"
        ))),
    }
}

fn scope_from_str(s: &str) -> Result<IgnoreScope, DbError> {
    match s {
        "exact" => Ok(IgnoreScope::Exact),
        "subtree" => Ok(IgnoreScope::Subtree),
        _ => Err(DbError::decode_msg("IgnoreScope", s, "unknown scope tag")),
    }
}

struct RawRule {
    project_id: String,
    path: String,
    scope: String,
    created_at: String,
}

impl RawRule {
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            project_id: row.get(0)?,
            path: row.get(1)?,
            scope: row.get(2)?,
            created_at: row.get(3)?,
        })
    }

    fn into_domain(self) -> Result<WorktreeIgnore, DbError> {
        Ok(WorktreeIgnore {
            project_id: crate::repositories::id_from_str("ProjectId", &self.project_id)?,
            path: self.path.into(),
            scope: scope_from_str(&self.scope)?,
            created_at: ts_from_str(&self.created_at)?,
        })
    }
}
