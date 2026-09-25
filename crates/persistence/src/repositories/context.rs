//! Context envelopes repository.
//!
//! The two structured fields — `artifacts` and `git_context` — are stored as
//! JSON TEXT via `serde_json` (ADR-009 keeps them out of dedicated columns). The
//! `artifacts_json` column is NOT NULL and holds `"[]"` for an empty list;
//! `git_context_json` is NULL when there is no Git context.

use domain::{
    ContextArtifactRef, ContextEnvelope, ContextId, ContextKind, GitContextRef, RunId, SessionId,
    TaskId,
};
use rusqlite::{params, Connection, Row};

use crate::db::DbError;
use crate::repositories::{id_from_opt, id_from_str, ts_from_opt, ts_from_str, ts_to_str};

const COLUMNS: &str = "id, source_session_id, target_session_id, summary, instructions, \
     artifacts_json, git_context_json, created_at, run_id, task_id, kind, in_reply_to, acked_at";

/// Repository over the `context_envelopes` table.
pub struct ContextRepo<'a> {
    conn: &'a Connection,
}

impl<'a> ContextRepo<'a> {
    #[must_use]
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }

    /// Insert a context envelope. Envelopes are append-only, so there is
    /// no update path.
    ///
    /// # Errors
    /// Returns [`DbError`] on a failed statement, a foreign-key violation, or a
    /// JSON serialization failure.
    pub fn insert(&self, envelope: &ContextEnvelope) -> Result<(), DbError> {
        let artifacts_json = serde_json::to_string(&envelope.artifacts)?;
        let git_context_json = envelope
            .git_context
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?;
        self.conn.execute(
            "INSERT INTO context_envelopes (\
               id, source_session_id, target_session_id, summary, instructions, \
               artifacts_json, git_context_json, created_at, run_id, task_id, kind, \
               in_reply_to, acked_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                envelope.id.to_string(),
                envelope.source_session_id.map(|id| id.to_string()),
                envelope.target_session_id.map(|id| id.to_string()),
                envelope.summary,
                envelope.instructions,
                artifacts_json,
                git_context_json,
                ts_to_str(&envelope.created_at),
                envelope.run_id.map(|id| id.to_string()),
                envelope.task_id.map(|id| id.to_string()),
                envelope.kind.map(kind_to_str),
                envelope.in_reply_to.map(|id| id.to_string()),
                envelope.acked_at.as_ref().map(ts_to_str),
            ],
        )?;
        Ok(())
    }

    /// List the envelopes originating from a session, in creation order.
    ///
    /// # Errors
    /// Returns [`DbError`] on a failed query, an undecodable row, or a JSON
    /// deserialization failure.
    pub fn list_by_source(&self, source: SessionId) -> Result<Vec<ContextEnvelope>, DbError> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {COLUMNS} FROM context_envelopes WHERE source_session_id = ?1 \
             ORDER BY created_at ASC"
        ))?;
        let rows = stmt.query_map(params![source.to_string()], RawEnvelope::from_row)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?.into_domain()?);
        }
        Ok(out)
    }

    /// Envelopes where the session is the source or the target, newest first.
    ///
    /// # Errors
    /// Returns [`DbError`] on a failed query, an undecodable row, or a JSON
    /// deserialization failure.
    pub fn list_for_session(&self, session: SessionId) -> Result<Vec<ContextEnvelope>, DbError> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {COLUMNS} FROM context_envelopes \
             WHERE source_session_id = ?1 OR target_session_id = ?1 \
             ORDER BY created_at DESC"
        ))?;
        let rows = stmt.query_map(params![session.to_string()], RawEnvelope::from_row)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?.into_domain()?);
        }
        Ok(out)
    }

    pub fn list_for_run(&self, run: domain::RunId) -> Result<Vec<ContextEnvelope>, DbError> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {COLUMNS} FROM context_envelopes WHERE run_id = ?1 ORDER BY created_at ASC"
        ))?;
        let rows = stmt.query_map(params![run.to_string()], RawEnvelope::from_row)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?.into_domain()?);
        }
        Ok(out)
    }

    /// Mark envelopes acknowledged. Unknown ids are ignored.
    pub fn ack(&self, ids: &[ContextId], at: domain::Timestamp) -> Result<(), DbError> {
        let stamp = ts_to_str(&at);
        for id in ids {
            self.conn.execute(
                "UPDATE context_envelopes SET acked_at=?1 WHERE id=?2 AND acked_at IS NULL",
                params![stamp, id.to_string()],
            )?;
        }
        Ok(())
    }
}

fn kind_to_str(kind: ContextKind) -> &'static str {
    match kind {
        ContextKind::Message => "message",
        ContextKind::Question => "question",
        ContextKind::Answer => "answer",
        ContextKind::Feedback => "feedback",
        ContextKind::Pointer => "pointer",
        ContextKind::Unrecognized => "unrecognized",
        _ => "unrecognized",
    }
}

fn kind_from_str(value: &str) -> ContextKind {
    match value {
        "message" => ContextKind::Message,
        "question" => ContextKind::Question,
        "answer" => ContextKind::Answer,
        "feedback" => ContextKind::Feedback,
        "pointer" => ContextKind::Pointer,
        _ => ContextKind::Unrecognized,
    }
}

struct RawEnvelope {
    id: String,
    source_session_id: Option<String>,
    target_session_id: Option<String>,
    summary: Option<String>,
    instructions: Option<String>,
    artifacts_json: String,
    git_context_json: Option<String>,
    created_at: String,
    run_id: Option<String>,
    task_id: Option<String>,
    kind: Option<String>,
    in_reply_to: Option<String>,
    acked_at: Option<String>,
}

impl RawEnvelope {
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            source_session_id: row.get(1)?,
            target_session_id: row.get(2)?,
            summary: row.get(3)?,
            instructions: row.get(4)?,
            artifacts_json: row.get(5)?,
            git_context_json: row.get(6)?,
            created_at: row.get(7)?,
            run_id: row.get(8)?,
            task_id: row.get(9)?,
            kind: row.get(10)?,
            in_reply_to: row.get(11)?,
            acked_at: row.get(12)?,
        })
    }

    fn into_domain(self) -> Result<ContextEnvelope, DbError> {
        let artifacts: Vec<ContextArtifactRef> = serde_json::from_str(&self.artifacts_json)?;
        let git_context: Option<GitContextRef> = self
            .git_context_json
            .as_deref()
            .map(serde_json::from_str)
            .transpose()?;
        Ok(ContextEnvelope {
            id: id_from_str::<ContextId>("ContextId", &self.id)?,
            source_session_id: id_from_opt::<SessionId>("SessionId", self.source_session_id)?,
            target_session_id: id_from_opt::<SessionId>("SessionId", self.target_session_id)?,
            summary: self.summary,
            instructions: self.instructions,
            artifacts,
            git_context,
            created_at: ts_from_str(&self.created_at)?,
            run_id: id_from_opt::<RunId>("RunId", self.run_id)?,
            task_id: id_from_opt::<TaskId>("TaskId", self.task_id)?,
            kind: self.kind.as_deref().map(kind_from_str),
            in_reply_to: id_from_opt::<ContextId>("ContextId", self.in_reply_to)?,
            acked_at: ts_from_opt(self.acked_at)?,
        })
    }
}
