//! `kb_entries`: a project's stored knowledge-base files. `source` is
//! `daemon` or `user`; a daemon write never replaces a person's.

use domain::{ProjectId, Severity};
use knowledge_base::FalsePositive;
use rusqlite::{params, Connection};

use crate::db::DbError;

pub const DAEMON: &str = "daemon";
pub const USER: &str = "user";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KbEntry {
    pub file: String,
    pub content: String,
    pub source: String,
    /// Unix seconds.
    pub created_at: i64,
    pub expires_at: Option<i64>,
}

/// The live entries of a project, oldest first.
///
/// # Errors
/// Returns [`DbError`] on a failed statement.
pub fn entries(conn: &Connection, project: ProjectId, now: i64) -> Result<Vec<KbEntry>, DbError> {
    let mut statement = conn.prepare_cached(
        "SELECT file, content, source, created_at, expires_at FROM kb_entries \
         WHERE project_id = ?1 AND (expires_at IS NULL OR expires_at > ?2) ORDER BY id",
    )?;
    let rows = statement.query_map(params![project.to_string(), now], |row| {
        Ok(KbEntry {
            file: row.get(0)?,
            content: row.get(1)?,
            source: row.get(2)?,
            created_at: row.get(3)?,
            expires_at: row.get(4)?,
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// `file`'s entry from `source` replaced by `content`.
///
/// # Errors
/// Returns [`DbError`] on a failed statement.
pub fn set(
    conn: &mut Connection,
    project: ProjectId,
    file: &str,
    content: &str,
    source: &str,
    now: i64,
) -> Result<(), DbError> {
    let tx = conn.transaction()?;
    tx.execute(
        "DELETE FROM kb_entries WHERE project_id = ?1 AND file = ?2 AND source = ?3",
        params![project.to_string(), file, source],
    )?;
    tx.execute(
        "INSERT INTO kb_entries (project_id, file, content, source, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![project.to_string(), file, content, source, now],
    )?;
    tx.commit()?;
    Ok(())
}

/// Every row of `file` replaced by one per item, from `source`.
///
/// # Errors
/// Returns [`DbError`] on a failed statement.
pub fn replace_items(
    conn: &mut Connection,
    project: ProjectId,
    file: &str,
    items: &[String],
    source: &str,
    now: i64,
) -> Result<(), DbError> {
    let tx = conn.transaction()?;
    tx.execute(
        "DELETE FROM kb_entries WHERE project_id = ?1 AND file = ?2",
        params![project.to_string(), file],
    )?;
    for item in items {
        tx.execute(
            "INSERT INTO kb_entries (project_id, file, content, source, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![project.to_string(), file, item, source, now],
        )?;
    }
    tx.commit()?;
    Ok(())
}

/// Appends the learned `patterns.json` items that
/// [`knowledge_base::admit_patterns`] lets in beside the project's live rows;
/// returns how many were added.
///
/// # Errors
/// Returns [`DbError`] on a failed statement.
pub fn add_patterns(
    conn: &mut Connection,
    project: ProjectId,
    items: &[String],
    now: i64,
    expires_at: i64,
) -> Result<usize, DbError> {
    let tx = conn.transaction()?;
    let (mut stored, mut others) = (Vec::new(), 0usize);
    for entry in entries(&tx, project, now)? {
        if entry.file == knowledge_base::PATTERNS {
            stored.push(entry.content);
        } else {
            others = others.saturating_add(entry.content.len());
        }
    }
    let admitted = knowledge_base::admit_patterns(&stored, others, items);
    for item in &admitted {
        tx.execute(
            "INSERT INTO kb_entries (project_id, file, content, source, created_at, expires_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                project.to_string(),
                knowledge_base::PATTERNS,
                item,
                DAEMON,
                now,
                expires_at
            ],
        )?;
    }
    tx.commit()?;
    Ok(admitted.len())
}

/// # Errors
/// Returns [`DbError`] on a failed statement.
pub fn clear(conn: &Connection, project: ProjectId, file: &str) -> Result<usize, DbError> {
    Ok(conn.execute(
        "DELETE FROM kb_entries WHERE project_id = ?1 AND file = ?2",
        params![project.to_string(), file],
    )?)
}

/// Expired rows, and the daemon's rows written before `cutoff`.
///
/// # Errors
/// Returns [`DbError`] on a failed statement.
pub fn prune(conn: &Connection, now: i64, cutoff: i64) -> Result<usize, DbError> {
    Ok(conn.execute(
        "DELETE FROM kb_entries WHERE (expires_at IS NOT NULL AND expires_at <= ?1) \
         OR (source = ?3 AND created_at < ?2)",
        params![now, cutoff, DAEMON],
    )?)
}

/// The project's 👎 findings, newest first, with their path while the run that
/// reported them is kept.
///
/// # Errors
/// Returns [`DbError`] on a failed statement or an unknown severity.
pub fn false_positives(
    conn: &Connection,
    project: ProjectId,
) -> Result<Vec<FalsePositive>, DbError> {
    let mut statement = conn.prepare_cached(
        "SELECT f.fingerprint, rf.path, f.rule_id, f.severity FROM review_feedback f \
         LEFT JOIN review_findings rf ON rf.review_id = f.review_id AND rf.id = f.finding_id \
         WHERE f.project_id = ?1 AND f.reaction = 'down' ORDER BY f.created_at DESC LIMIT 1000",
    )?;
    let rows = statement.query_map(params![project.to_string()], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, Option<String>>(1)?,
            row.get::<_, Option<String>>(2)?,
            row.get::<_, String>(3)?,
        ))
    })?;
    rows.map(|row| {
        let (fingerprint, path, rule, severity) = row?;
        let severity = match severity.as_str() {
            "P0" => Severity::P0,
            "P1" => Severity::P1,
            "P2" => Severity::P2,
            other => return Err(DbError::decode_msg("Severity", other, "unknown severity")),
        };
        Ok(FalsePositive {
            fingerprint,
            path,
            rule_id: rule,
            severity,
        })
    })
    .collect()
}
