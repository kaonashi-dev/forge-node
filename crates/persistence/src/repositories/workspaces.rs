//! Workspaces repository (§7.2, §15.2).

use std::path::PathBuf;

use domain::{ProjectId, Workspace, WorkspaceId, WorkspaceStatus};
use rusqlite::{params, Connection, OptionalExtension, Row};

use crate::db::DbError;
use crate::repositories::{
    id_from_str, path_to_str, ts_from_str, ts_to_str, workspace_kind_from_str,
    workspace_kind_to_str,
};

const COLUMNS: &str =
    "id, project_id, kind, path, branch, managed_by_app, created_at, display_name";

/// Repository over the `workspaces` table.
pub struct WorkspaceRepo<'a> {
    conn: &'a Connection,
}

impl<'a> WorkspaceRepo<'a> {
    #[must_use]
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }

    /// Insert a workspace, or update every column in place if its id already
    /// exists. `managed_by_app` is stored as an INTEGER (0/1).
    ///
    /// # Errors
    /// Returns [`DbError`] on a failed statement (e.g. UNIQUE `path` conflict, or
    /// a `project_id` that violates the foreign key).
    pub fn upsert(&self, workspace: &Workspace) -> Result<(), DbError> {
        self.conn.execute(
            "INSERT INTO workspaces \
             (id, project_id, kind, path, branch, managed_by_app, created_at, display_name) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) \
             ON CONFLICT(id) DO UPDATE SET \
               project_id = excluded.project_id, \
               kind = excluded.kind, \
               path = excluded.path, \
               branch = excluded.branch, \
               managed_by_app = excluded.managed_by_app, \
               created_at = excluded.created_at, \
               display_name = excluded.display_name",
            params![
                workspace.id.to_string(),
                workspace.project_id.to_string(),
                workspace_kind_to_str(workspace.kind)?,
                path_to_str(&workspace.path),
                workspace.branch,
                workspace.managed_by_app,
                ts_to_str(&workspace.created_at),
                workspace.display_name,
            ],
        )?;
        Ok(())
    }

    /// Fetch a workspace by id, or `None`.
    ///
    /// # Errors
    /// Returns [`DbError`] on a failed query or an undecodable row.
    pub fn get(&self, id: WorkspaceId) -> Result<Option<Workspace>, DbError> {
        let raw = self
            .conn
            .query_row(
                &format!("SELECT {COLUMNS} FROM workspaces WHERE id = ?1"),
                params![id.to_string()],
                RawWorkspace::from_row,
            )
            .optional()?;
        raw.map(RawWorkspace::into_domain).transpose()
    }

    /// List the workspaces of a project, oldest first (creation order).
    ///
    /// # Errors
    /// Returns [`DbError`] on a failed query or an undecodable row.
    pub fn list_by_project(&self, project_id: ProjectId) -> Result<Vec<Workspace>, DbError> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {COLUMNS} FROM workspaces WHERE project_id = ?1 ORDER BY created_at ASC"
        ))?;
        let rows = stmt.query_map(params![project_id.to_string()], RawWorkspace::from_row)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?.into_domain()?);
        }
        Ok(out)
    }

    /// Delete a workspace by id. Returns `true` if a row was removed.
    ///
    /// # Errors
    /// Returns [`DbError`] on a failed statement (e.g. a session still references it).
    pub fn delete(&self, id: WorkspaceId) -> Result<bool, DbError> {
        let n = self.conn.execute(
            "DELETE FROM workspaces WHERE id = ?1",
            params![id.to_string()],
        )?;
        Ok(n > 0)
    }
}

struct RawWorkspace {
    id: String,
    project_id: String,
    kind: String,
    path: String,
    branch: Option<String>,
    managed_by_app: bool,
    created_at: String,
    display_name: Option<String>,
}

impl RawWorkspace {
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            project_id: row.get(1)?,
            kind: row.get(2)?,
            path: row.get(3)?,
            branch: row.get(4)?,
            managed_by_app: row.get(5)?,
            created_at: row.get(6)?,
            display_name: row.get(7)?,
        })
    }

    fn into_domain(self) -> Result<Workspace, DbError> {
        Ok(Workspace {
            id: id_from_str::<WorkspaceId>("WorkspaceId", &self.id)?,
            project_id: id_from_str::<ProjectId>("ProjectId", &self.project_id)?,
            kind: workspace_kind_from_str(&self.kind)?,
            path: PathBuf::from(self.path),
            branch: self.branch,
            display_name: self.display_name,
            managed_by_app: self.managed_by_app,
            created_at: ts_from_str(&self.created_at)?,
            status: WorkspaceStatus::default(),
        })
    }
}
