//! Per-project file-sharing rules repository (§14.2, §15.2).
//!
//! A rule says how one path reaches every workspace of a project. The strategy
//! is stored as JSON for the same reason `agent_profiles.args_json` is: nothing
//! ever queries inside it, and a `Run` command is a shape, not a column.
//!
//! The GUI edits a *set*, so the write path here is
//! [`ShareRepo::replace_for_project`] in one transaction. Row-at-a-time upserts
//! would leave a half-applied list visible to a concurrent read.

use domain::{ProjectId, ShareRule, ShareRuleId, ShareStrategy};
use rusqlite::{params, Connection, Row};

use crate::db::DbError;
use crate::repositories::{id_from_str, ts_from_str, ts_to_str};

const COLUMNS: &str = "id, project_id, path, strategy, enabled, position, created_at";

/// Repository over the `worktree_shares` table.
pub struct ShareRepo<'a> {
    conn: &'a Connection,
}

impl<'a> ShareRepo<'a> {
    #[must_use]
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }

    /// Every rule of every project, in project then application order.
    ///
    /// # Errors
    /// Returns [`DbError`] on a failed query or an undecodable row.
    pub fn list_all(&self) -> Result<Vec<ShareRule>, DbError> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {COLUMNS} FROM worktree_shares ORDER BY project_id ASC, position ASC"
        ))?;
        let rows = stmt.query_map([], RawRule::from_row)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?.into_domain()?);
        }
        Ok(out)
    }

    /// One project's rules, in application order.
    ///
    /// # Errors
    /// Returns [`DbError`] on a failed query or an undecodable row.
    pub fn list_for_project(&self, project_id: ProjectId) -> Result<Vec<ShareRule>, DbError> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {COLUMNS} FROM worktree_shares WHERE project_id = ?1 ORDER BY position ASC"
        ))?;
        let rows = stmt.query_map(params![project_id.to_string()], RawRule::from_row)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?.into_domain()?);
        }
        Ok(out)
    }

    /// Replace a project's whole rule set, in one transaction.
    ///
    /// `position` is rewritten from the slice order, so the caller orders the
    /// list and the table records that order rather than a client's opinion of
    /// it.
    ///
    /// # Errors
    /// Returns [`DbError`] on a failed statement, including the unique index on
    /// `(project_id, path)`, which the daemon turns into a `Conflict`.
    pub fn replace_for_project(
        &self,
        project_id: ProjectId,
        rules: &[ShareRule],
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

    fn write_set(&self, project_id: ProjectId, rules: &[ShareRule]) -> Result<(), DbError> {
        self.conn.execute(
            "DELETE FROM worktree_shares WHERE project_id = ?1",
            params![project_id.to_string()],
        )?;
        for (position, rule) in rules.iter().enumerate() {
            let strategy = serde_json::to_string(&rule.strategy)
                .map_err(|e| DbError::Encode(format!("share strategy: {e}")))?;
            self.conn.execute(
                "INSERT INTO worktree_shares (\
                   id, project_id, path, strategy, enabled, position, created_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    rule.id.to_string(),
                    project_id.to_string(),
                    rule.path,
                    strategy,
                    i64::from(rule.enabled),
                    i64::try_from(position).unwrap_or(i64::MAX),
                    ts_to_str(&rule.created_at),
                ],
            )?;
        }
        Ok(())
    }

    /// Delete one rule by id. Returns `true` if a row was removed.
    ///
    /// # Errors
    /// Returns [`DbError`] on a failed statement.
    pub fn delete(&self, id: ShareRuleId) -> Result<bool, DbError> {
        let n = self.conn.execute(
            "DELETE FROM worktree_shares WHERE id = ?1",
            params![id.to_string()],
        )?;
        Ok(n > 0)
    }
}

struct RawRule {
    id: String,
    project_id: String,
    path: String,
    strategy: String,
    enabled: i64,
    position: i64,
    created_at: String,
}

impl RawRule {
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            project_id: row.get(1)?,
            path: row.get(2)?,
            strategy: row.get(3)?,
            enabled: row.get(4)?,
            position: row.get(5)?,
            created_at: row.get(6)?,
        })
    }

    fn into_domain(self) -> Result<ShareRule, DbError> {
        let strategy: ShareStrategy = serde_json::from_str(&self.strategy)
            .map_err(|e| DbError::decode("share strategy", &self.strategy, e))?;
        Ok(ShareRule {
            id: id_from_str("ShareRuleId", &self.id)?,
            project_id: id_from_str("ProjectId", &self.project_id)?,
            path: self.path,
            strategy,
            enabled: self.enabled != 0,
            position: u32::try_from(self.position).unwrap_or(u32::MAX),
            created_at: ts_from_str(&self.created_at)?,
        })
    }
}
