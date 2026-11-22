//! `sandbox_runs`: what sandbox checks ran and how they ended.

use domain::{ProjectId, SandboxRun, SandboxTarget, Timestamp};
use rusqlite::{params, Connection, OptionalExtension};

use crate::db::DbError;
use crate::repositories::{id_from_str, ts_from_opt, ts_from_str, ts_to_str};

/// # Errors
/// Returns [`DbError`] on a failed statement.
pub fn upsert(conn: &Connection, run: &SandboxRun) -> Result<(), DbError> {
    let failed: Vec<&SandboxTarget> = run.targets.iter().filter(|t| !t.passed()).collect();
    let first = failed.first();
    let targets =
        serde_json::to_string(&run.targets).map_err(|e| DbError::Encode(e.to_string()))?;
    let names = serde_json::to_string(&failed.iter().map(|t| &t.name).collect::<Vec<_>>())
        .map_err(|e| DbError::Encode(e.to_string()))?;
    conn.execute(
        "INSERT INTO sandbox_runs (id, review_run_id, workspace_id, targets_run, targets_failed, \
         stdout_excerpt, stderr_excerpt, exit_code, duration_ms, started_at, finished_at, error, \
         unapproved_hash) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13) \
         ON CONFLICT(id) DO UPDATE SET targets_run = excluded.targets_run, \
         targets_failed = excluded.targets_failed, stdout_excerpt = excluded.stdout_excerpt, \
         stderr_excerpt = excluded.stderr_excerpt, exit_code = excluded.exit_code, \
         duration_ms = excluded.duration_ms, finished_at = excluded.finished_at, error = excluded.error, \
         unapproved_hash = excluded.unapproved_hash",
        params![
            run.id,
            run.review_id.map(|id| id.to_string()),
            run.workspace_id.to_string(),
            targets,
            names,
            first.map(|t| t.stdout.clone()),
            first.map(|t| t.stderr.clone()),
            first.map_or(0, |t| t.exit_code.unwrap_or(-1)),
            i64::try_from(run.targets.iter().map(|t| t.duration_ms).sum::<u64>()).unwrap_or(i64::MAX),
            ts_to_str(&run.started_at),
            run.finished_at.as_ref().map(ts_to_str),
            run.error,
            run.unapproved_hash,
        ],
    )?;
    Ok(())
}

/// # Errors
/// Returns [`DbError`] on a failed statement or an undecodable row.
pub fn get(conn: &Connection, id: &str) -> Result<Option<SandboxRun>, DbError> {
    let row = conn
        .query_row(
            "SELECT id, review_run_id, workspace_id, targets_run, started_at, finished_at, error, \
             unapproved_hash FROM sandbox_runs WHERE id = ?1",
            [id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, Option<String>>(7)?,
                ))
            },
        )
        .optional()?;
    let Some((id, review, workspace, targets, started_at, finished_at, error, unapproved_hash)) =
        row
    else {
        return Ok(None);
    };
    Ok(Some(SandboxRun {
        id,
        workspace_id: id_from_str("WorkspaceId", &workspace)?,
        review_id: review.map(|r| id_from_str("ReviewId", &r)).transpose()?,
        targets: serde_json::from_str(&targets)
            .map_err(|e| DbError::decode("sandbox targets", &targets, e))?,
        error,
        unapproved_hash,
        started_at: ts_from_str(&started_at)?,
        finished_at: ts_from_opt(finished_at)?,
    }))
}

/// `(review id, any target failed)` of every run linked to a review.
///
/// # Errors
/// Returns [`DbError`] on a failed statement.
pub fn linked(conn: &Connection) -> Result<Vec<(String, bool)>, DbError> {
    let mut statement = conn.prepare_cached(
        "SELECT review_run_id, targets_failed, error FROM sandbox_runs WHERE review_run_id IS NOT NULL",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)? != "[]" || row.get::<_, Option<String>>(2)?.is_some(),
        ))
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// # Errors
/// Returns [`DbError`] on a failed statement.
pub fn approve(conn: &Connection, project_id: ProjectId, file_hash: &str) -> Result<(), DbError> {
    conn.execute(
        "INSERT INTO sandbox_approvals (project_id, file_hash, approved_at) VALUES (?1, ?2, ?3) \
         ON CONFLICT(project_id) DO UPDATE SET file_hash = excluded.file_hash, \
         approved_at = excluded.approved_at",
        params![
            project_id.to_string(),
            file_hash,
            ts_to_str(&Timestamp::now())
        ],
    )?;
    Ok(())
}

/// # Errors
/// Returns [`DbError`] on a failed statement.
pub fn approved(conn: &Connection, project_id: ProjectId) -> Result<Option<String>, DbError> {
    Ok(conn
        .query_row(
            "SELECT file_hash FROM sandbox_approvals WHERE project_id = ?1",
            [project_id.to_string()],
            |row| row.get(0),
        )
        .optional()?)
}
