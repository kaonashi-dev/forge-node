//! Organizational project-group repository.

use domain::{ProjectGroup, ProjectGroupId};
use rusqlite::{params, Connection, OptionalExtension, Row};

use crate::db::DbError;
use crate::repositories::{id_from_str, ts_from_str, ts_to_str};

const COLUMNS: &str = "id, name, created_at";

/// Repository over the `project_groups` table.
pub struct ProjectGroupRepo<'a> {
    conn: &'a Connection,
}

impl<'a> ProjectGroupRepo<'a> {
    #[must_use]
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }

    /// Insert or update a project group.
    ///
    /// # Errors
    /// Returns [`DbError`] when the statement fails.
    pub fn upsert(&self, group: &ProjectGroup) -> Result<(), DbError> {
        self.conn.execute(
            "INSERT INTO project_groups (id, name, created_at) VALUES (?1, ?2, ?3) \
             ON CONFLICT(id) DO UPDATE SET name = excluded.name",
            params![
                group.id.to_string(),
                group.name,
                ts_to_str(&group.created_at),
            ],
        )?;
        Ok(())
    }

    /// Fetch one group by id.
    ///
    /// # Errors
    /// Returns [`DbError`] on a failed query or invalid row.
    pub fn get(&self, id: ProjectGroupId) -> Result<Option<ProjectGroup>, DbError> {
        let raw = self
            .conn
            .query_row(
                &format!("SELECT {COLUMNS} FROM project_groups WHERE id = ?1"),
                params![id.to_string()],
                RawProjectGroup::from_row,
            )
            .optional()?;
        raw.map(RawProjectGroup::into_domain).transpose()
    }

    /// List groups in creation order.
    ///
    /// # Errors
    /// Returns [`DbError`] on a failed query or invalid row.
    pub fn list(&self) -> Result<Vec<ProjectGroup>, DbError> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {COLUMNS} FROM project_groups ORDER BY created_at, id"
        ))?;
        let rows = stmt.query_map([], RawProjectGroup::from_row)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?.into_domain()?);
        }
        Ok(out)
    }

    /// Delete a group. Its projects move to General through `ON DELETE SET NULL`.
    ///
    /// # Errors
    /// Returns [`DbError`] when the statement fails.
    pub fn delete(&self, id: ProjectGroupId) -> Result<bool, DbError> {
        let changed = self.conn.execute(
            "DELETE FROM project_groups WHERE id = ?1",
            params![id.to_string()],
        )?;
        Ok(changed > 0)
    }
}

struct RawProjectGroup {
    id: String,
    name: String,
    created_at: String,
}

impl RawProjectGroup {
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            name: row.get(1)?,
            created_at: row.get(2)?,
        })
    }

    fn into_domain(self) -> Result<ProjectGroup, DbError> {
        Ok(ProjectGroup {
            id: id_from_str::<ProjectGroupId>("ProjectGroupId", &self.id)?,
            name: self.name,
            created_at: ts_from_str(&self.created_at)?,
        })
    }
}
