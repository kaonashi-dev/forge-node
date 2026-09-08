//! Projects repository (§7.1, §15.2).

use std::path::PathBuf;

use domain::{Project, ProjectGroupId, ProjectId, Timestamp};
use rusqlite::{params, Connection, OptionalExtension, Row};

use crate::db::DbError;
use crate::repositories::{id_from_str, path_to_str, ts_from_str, ts_to_str};

const COLUMNS: &str =
    "id, project_group_id, name, icon, root_path, git_root, created_at, last_opened_at";

/// Repository over the `projects` table.
pub struct ProjectRepo<'a> {
    conn: &'a Connection,
}

impl<'a> ProjectRepo<'a> {
    #[must_use]
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }

    /// Insert a project, or update every column in place if its id already
    /// exists. Upsert is keyed on the `id` primary key, so inserting a *different*
    /// project whose `root_path` matches an existing one violates the UNIQUE
    /// constraint and returns a [`DbError::Sqlite`].
    ///
    /// # Errors
    /// Returns [`DbError`] on a failed statement or UNIQUE conflict.
    pub fn upsert(&self, project: &Project) -> Result<(), DbError> {
        self.conn.execute(
            "INSERT INTO projects \
               (id, project_group_id, name, icon, root_path, git_root, created_at, \
                last_opened_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) \
             ON CONFLICT(id) DO UPDATE SET \
               project_group_id = excluded.project_group_id, \
               name = excluded.name, \
               icon = excluded.icon, \
               root_path = excluded.root_path, \
               git_root = excluded.git_root, \
               created_at = excluded.created_at, \
               last_opened_at = excluded.last_opened_at",
            params![
                project.id.to_string(),
                project.project_group_id.map(|id| id.to_string()),
                project.name,
                project.icon,
                path_to_str(&project.root_path),
                project.git_root.as_deref().map(path_to_str),
                ts_to_str(&project.created_at),
                ts_to_str(&project.last_opened_at),
            ],
        )?;
        Ok(())
    }

    /// Fetch a project by id, or `None` if it does not exist.
    ///
    /// # Errors
    /// Returns [`DbError`] on a failed query or an undecodable row.
    pub fn get(&self, id: ProjectId) -> Result<Option<Project>, DbError> {
        let raw = self
            .conn
            .query_row(
                &format!("SELECT {COLUMNS} FROM projects WHERE id = ?1"),
                params![id.to_string()],
                RawProject::from_row,
            )
            .optional()?;
        raw.map(RawProject::into_domain).transpose()
    }

    /// List all projects, most-recently-opened first.
    ///
    /// # Errors
    /// Returns [`DbError`] on a failed query or an undecodable row.
    pub fn list(&self) -> Result<Vec<Project>, DbError> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {COLUMNS} FROM projects ORDER BY last_opened_at DESC"
        ))?;
        let rows = stmt.query_map([], RawProject::from_row)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?.into_domain()?);
        }
        Ok(out)
    }

    /// Delete a project by id. Returns `true` if a row was removed.
    ///
    /// # Errors
    /// Returns [`DbError`] on a failed statement (e.g. a FK still references it).
    pub fn delete(&self, id: ProjectId) -> Result<bool, DbError> {
        let n = self.conn.execute(
            "DELETE FROM projects WHERE id = ?1",
            params![id.to_string()],
        )?;
        Ok(n > 0)
    }

    /// Update `last_opened_at` for a project. Returns `true` if it existed.
    ///
    /// # Errors
    /// Returns [`DbError`] on a failed statement.
    pub fn touch_last_opened(&self, id: ProjectId, at: Timestamp) -> Result<bool, DbError> {
        let n = self.conn.execute(
            "UPDATE projects SET last_opened_at = ?2 WHERE id = ?1",
            params![id.to_string(), ts_to_str(&at)],
        )?;
        Ok(n > 0)
    }
}

/// Raw column values, read inside the `rusqlite` closure and decoded afterwards
/// (decoding can fail with [`DbError`], which the closure cannot return).
struct RawProject {
    id: String,
    project_group_id: Option<String>,
    name: String,
    icon: Option<String>,
    root_path: String,
    git_root: Option<String>,
    created_at: String,
    last_opened_at: String,
}

impl RawProject {
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            project_group_id: row.get(1)?,
            name: row.get(2)?,
            icon: row.get(3)?,
            root_path: row.get(4)?,
            git_root: row.get(5)?,
            created_at: row.get(6)?,
            last_opened_at: row.get(7)?,
        })
    }

    fn into_domain(self) -> Result<Project, DbError> {
        Ok(Project {
            id: id_from_str::<ProjectId>("ProjectId", &self.id)?,
            project_group_id: self
                .project_group_id
                .as_deref()
                .map(|id| id_from_str::<ProjectGroupId>("ProjectGroupId", id))
                .transpose()?,
            name: self.name,
            icon: self.icon,
            root_path: PathBuf::from(self.root_path),
            git_root: self.git_root.map(PathBuf::from),
            created_at: ts_from_str(&self.created_at)?,
            last_opened_at: ts_from_str(&self.last_opened_at)?,
        })
    }
}
