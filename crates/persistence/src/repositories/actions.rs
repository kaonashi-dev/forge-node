//! A project's actions. Everything a run reads besides the label, command and
//! kind is one JSON `spec` column, the `worktree_shares.strategy` reasoning:
//! nothing queries inside it. Written as a whole set in one transaction.

use domain::{
    ActionKind, ActionPort, ActionReady, ActionSource, ProjectAction, ProjectActionId, ProjectId,
};
use rusqlite::{params, Connection, Row};
use serde::{Deserialize, Serialize};

use crate::db::DbError;
use crate::repositories::{id_from_str, ts_from_str, ts_to_str};

const COLUMNS: &str = "id, project_id, label, command, kind, spec, pinned, position, created_at";

/// Repository over the `project_actions` table.
pub struct ActionRepo<'a> {
    conn: &'a Connection,
}

#[derive(Serialize, Deserialize)]
struct Spec {
    #[serde(default)]
    cwd: Option<String>,
    single_instance: bool,
    #[serde(default)]
    ready: ActionReady,
    #[serde(default)]
    port: Option<ActionPort>,
    #[serde(default)]
    depends_on: Vec<ProjectActionId>,
    source: ActionSource,
}

impl<'a> ActionRepo<'a> {
    #[must_use]
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }

    /// Every action of every project, in project then position order.
    ///
    /// # Errors
    /// Returns [`DbError`] on a failed query or an undecodable row.
    pub fn list_all(&self) -> Result<Vec<ProjectAction>, DbError> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {COLUMNS} FROM project_actions ORDER BY project_id ASC, position ASC"
        ))?;
        let rows = stmt.query_map([], RawAction::from_row)?;
        rows.map(|row| row?.into_domain()).collect()
    }

    /// One project's actions, in position order.
    ///
    /// # Errors
    /// Returns [`DbError`] on a failed query or an undecodable row.
    pub fn list_for_project(&self, project_id: ProjectId) -> Result<Vec<ProjectAction>, DbError> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {COLUMNS} FROM project_actions WHERE project_id = ?1 ORDER BY position ASC"
        ))?;
        let rows = stmt.query_map(params![project_id.to_string()], RawAction::from_row)?;
        rows.map(|row| row?.into_domain()).collect()
    }

    /// Replace a project's whole action set in one transaction; `position` is
    /// rewritten from the slice order.
    ///
    /// # Errors
    /// Returns [`DbError`] on a failed statement, including the unique index on
    /// `(project_id, label)` ignoring case.
    pub fn replace_for_project(
        &self,
        project_id: ProjectId,
        actions: &[ProjectAction],
    ) -> Result<(), DbError> {
        self.conn.execute("BEGIN IMMEDIATE", [])?;
        let result = self.write_set(project_id, actions);
        if result.is_ok() {
            self.conn.execute("COMMIT", [])?;
        } else {
            let _ = self.conn.execute("ROLLBACK", []);
        }
        result
    }

    fn write_set(&self, project_id: ProjectId, actions: &[ProjectAction]) -> Result<(), DbError> {
        self.conn.execute(
            "DELETE FROM project_actions WHERE project_id = ?1",
            params![project_id.to_string()],
        )?;
        for (position, action) in actions.iter().enumerate() {
            let spec = serde_json::to_string(&Spec {
                cwd: action.cwd.clone(),
                single_instance: action.single_instance,
                ready: action.ready.clone(),
                port: action.port,
                depends_on: action.depends_on.clone(),
                source: action.source.clone(),
            })
            .map_err(|e| DbError::Encode(format!("action spec: {e}")))?;
            self.conn.execute(
                "INSERT INTO project_actions (\
                   id, project_id, label, command, kind, spec, pinned, position, created_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    action.id.to_string(),
                    project_id.to_string(),
                    action.label,
                    action.command,
                    kind_to_str(action.kind),
                    spec,
                    i64::from(action.pinned),
                    i64::try_from(position).unwrap_or(i64::MAX),
                    ts_to_str(&action.created_at),
                ],
            )?;
        }
        Ok(())
    }
}

fn kind_to_str(kind: ActionKind) -> &'static str {
    match kind {
        ActionKind::Dev => "dev",
        ActionKind::Test => "test",
        ActionKind::Lint => "lint",
        ActionKind::Build => "build",
        _ => "custom",
    }
}

fn kind_from_str(tag: &str) -> ActionKind {
    match tag {
        "dev" => ActionKind::Dev,
        "test" => ActionKind::Test,
        "lint" => ActionKind::Lint,
        "build" => ActionKind::Build,
        _ => ActionKind::Custom,
    }
}

struct RawAction {
    id: String,
    project_id: String,
    label: String,
    command: String,
    kind: String,
    spec: String,
    pinned: i64,
    position: i64,
    created_at: String,
}

impl RawAction {
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            project_id: row.get(1)?,
            label: row.get(2)?,
            command: row.get(3)?,
            kind: row.get(4)?,
            spec: row.get(5)?,
            pinned: row.get(6)?,
            position: row.get(7)?,
            created_at: row.get(8)?,
        })
    }

    fn into_domain(self) -> Result<ProjectAction, DbError> {
        let spec: Spec = serde_json::from_str(&self.spec)
            .map_err(|e| DbError::decode("action spec", &self.spec, e))?;
        Ok(ProjectAction {
            id: id_from_str("ProjectActionId", &self.id)?,
            project_id: id_from_str("ProjectId", &self.project_id)?,
            label: self.label,
            command: self.command,
            kind: kind_from_str(&self.kind),
            cwd: spec.cwd,
            single_instance: spec.single_instance,
            pinned: self.pinned != 0,
            ready: spec.ready,
            port: spec.port,
            depends_on: spec.depends_on,
            source: spec.source,
            position: u32::try_from(self.position).unwrap_or(u32::MAX),
            created_at: ts_from_str(&self.created_at)?,
        })
    }
}
