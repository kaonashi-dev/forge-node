//! `spec_runs`: a spec run's stage and metadata. Never its Markdown or logs.

use domain::spec_run::model::SpecRunView;
use domain::SpecRunId;
use rusqlite::{params, Connection};

use crate::db::DbError;
use crate::repositories::ts_to_str;

/// # Errors
/// Returns [`DbError`] on a failed statement or an unencodable view.
pub fn upsert(conn: &Connection, run: &SpecRunView) -> Result<(), DbError> {
    let meta = serde_json::to_string(run).map_err(|e| DbError::Encode(format!("spec run: {e}")))?;
    conn.execute(
        "INSERT INTO spec_runs (id, project_id, stage, meta, created_at, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6) \
         ON CONFLICT(id) DO UPDATE SET stage = excluded.stage, meta = excluded.meta, \
         updated_at = excluded.updated_at",
        params![
            run.id.to_string(),
            run.project_id.to_string(),
            run.stage.as_str(),
            meta,
            ts_to_str(&run.created_at),
            ts_to_str(&run.updated_at),
        ],
    )?;
    Ok(())
}

/// Every spec run, oldest first. A row that no longer decodes is skipped rather than failing
/// startup.
///
/// # Errors
/// Returns [`DbError`] on a failed query.
pub fn list(conn: &Connection) -> Result<Vec<SpecRunView>, DbError> {
    let mut stmt = conn.prepare("SELECT meta FROM spec_runs ORDER BY created_at")?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
    let mut out = Vec::new();
    for row in rows {
        if let Ok(run) = serde_json::from_str::<SpecRunView>(&row?) {
            out.push(run);
        }
    }
    Ok(out)
}

/// # Errors
/// Returns [`DbError`] on a failed statement.
pub fn delete(conn: &Connection, id: SpecRunId) -> Result<bool, DbError> {
    Ok(conn.execute(
        "DELETE FROM spec_runs WHERE id = ?1",
        params![id.to_string()],
    )? > 0)
}
