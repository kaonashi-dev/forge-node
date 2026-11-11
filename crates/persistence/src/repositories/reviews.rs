//! Forge Review runs and findings. `target` and `coverage` are JSON columns,
//! the `project_actions.spec` reasoning: nothing queries inside them.

use std::collections::HashSet;

use domain::{
    Coverage, Finding, ReviewId, ReviewRun, ReviewStage, ReviewStatus, ReviewTarget, SessionId,
    Severity, Tier, Timestamp, Verdict, WorkspaceId,
};
use rusqlite::{params, Connection, OptionalExtension, Row};

use crate::db::DbError;
use crate::repositories::{id_from_str, ts_from_opt, ts_from_str, ts_to_str};

const RUN_COLUMNS: &str = "id, workspace_id, target, head_oid, base_oid, tier, provider, status, \
     stage, summary, score, verdict, error, session_id, coverage, created_at, finished_at, preset";

const FINDING_COLUMNS: &str = "id, fingerprint, path, line, end_line, severity, category, \
     rule_id, confidence, title, body, evidence, suggestion";

pub fn insert_run(conn: &Connection, run: &ReviewRun) -> Result<(), DbError> {
    conn.prepare_cached(&format!(
        "INSERT INTO review_runs ({RUN_COLUMNS}) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)"
    ))?
    .execute(params![
        run.id.to_string(),
        run.workspace_id.to_string(),
        json(&run.target)?,
        run.head_oid,
        run.base_oid,
        tier_tag(run.tier),
        run.provider,
        status_tag(run.status),
        run.stage.map(stage_tag),
        run.summary,
        run.score.map(f64::from),
        run.verdict.map(verdict_tag),
        run.error,
        run.session_id.map(|id| id.to_string()),
        json(&run.coverage)?,
        ts_to_str(&run.created_at),
        run.finished_at.as_ref().map(ts_to_str),
        run.preset,
    ])?;
    Ok(())
}

/// Rewrites every mutable column; a run removed meanwhile stays removed.
pub fn update_run(conn: &Connection, run: &ReviewRun) -> Result<(), DbError> {
    conn.prepare_cached(
        "UPDATE review_runs SET base_oid = ?2, status = ?3, stage = ?4, summary = ?5, \
         score = ?6, verdict = ?7, error = ?8, session_id = ?9, coverage = ?10, \
         finished_at = ?11 WHERE id = ?1",
    )?
    .execute(params![
        run.id.to_string(),
        run.base_oid,
        status_tag(run.status),
        run.stage.map(stage_tag),
        run.summary,
        run.score.map(f64::from),
        run.verdict.map(verdict_tag),
        run.error,
        run.session_id.map(|id| id.to_string()),
        json(&run.coverage)?,
        run.finished_at.as_ref().map(ts_to_str),
    ])?;
    Ok(())
}

pub fn insert_findings(
    conn: &Connection,
    review: ReviewId,
    findings: &[Finding],
) -> Result<(), DbError> {
    conn.execute("BEGIN IMMEDIATE", [])?;
    let result = (|| {
        let mut stmt = conn.prepare_cached(&format!(
            "INSERT INTO review_findings (review_id, {FINDING_COLUMNS}) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)"
        ))?;
        for f in findings {
            stmt.execute(params![
                review.to_string(),
                f.id,
                f.fingerprint,
                f.path,
                f.line,
                f.end_line,
                severity_tag(f.severity),
                f.category,
                f.rule_id,
                f64::from(f.confidence),
                f.title,
                f.body,
                f.evidence,
                f.suggestion,
            ])?;
        }
        Ok::<_, DbError>(())
    })();
    if result.is_ok() {
        conn.execute("COMMIT", [])?;
    } else {
        let _ = conn.execute("ROLLBACK", []);
    }
    result
}

pub fn get(conn: &Connection, id: ReviewId) -> Result<Option<(ReviewRun, Vec<Finding>)>, DbError> {
    let run = conn
        .prepare_cached(&format!(
            "SELECT {RUN_COLUMNS} FROM review_runs WHERE id = ?1"
        ))?
        .query_row(params![id.to_string()], run_from_row)
        .optional()?;
    let Some(run) = run else {
        return Ok(None);
    };
    let run = run?;
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT {FINDING_COLUMNS} FROM review_findings WHERE review_id = ?1 ORDER BY id ASC"
    ))?;
    let findings = stmt
        .query_map(params![id.to_string()], finding_from_row)?
        .map(|row| row?)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Some((run, findings)))
}

/// Newest first; ids are UUIDv7, so id order is creation order.
pub fn list(
    conn: &Connection,
    workspace: WorkspaceId,
    limit: u32,
) -> Result<Vec<ReviewRun>, DbError> {
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT {RUN_COLUMNS} FROM review_runs WHERE workspace_id = ?1 ORDER BY id DESC LIMIT ?2"
    ))?;
    let rows = stmt.query_map(params![workspace.to_string(), limit], run_from_row)?;
    rows.map(|row| row?).collect()
}

pub fn interrupt_unfinished(conn: &Connection) -> Result<usize, DbError> {
    Ok(conn.execute(
        "UPDATE review_runs SET status = 'Interrupted', finished_at = ?1 \
         WHERE status IN ('Queued', 'Running')",
        params![ts_to_str(&Timestamp::now())],
    )?)
}

/// Session ids named by review runs, for the restart's read-only set.
pub fn reviewer_sessions(conn: &Connection) -> Result<HashSet<SessionId>, DbError> {
    let mut stmt =
        conn.prepare("SELECT DISTINCT session_id FROM review_runs WHERE session_id IS NOT NULL")?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
    let mut ids = HashSet::new();
    for row in rows {
        ids.insert(id_from_str("review reviewer session", &row?)?);
    }
    Ok(ids)
}

/// Keep a workspace's newest `keep` runs, and of those only the ones created
/// at or after `cutoff`. Findings go with their run. A run whose reviewer
/// session row still exists is kept: that row is what makes the restart
/// read-only.
pub fn prune(
    conn: &Connection,
    workspace: WorkspaceId,
    keep: u32,
    cutoff: Timestamp,
) -> Result<usize, DbError> {
    Ok(conn.execute(
        "DELETE FROM review_runs WHERE workspace_id = ?1 \
         AND (created_at < ?3 OR id NOT IN \
           (SELECT id FROM review_runs WHERE workspace_id = ?1 ORDER BY id DESC LIMIT ?2)) \
         AND (session_id IS NULL OR session_id NOT IN (SELECT id FROM sessions))",
        params![workspace.to_string(), keep, ts_to_str(&cutoff)],
    )?)
}

fn json<T: serde::Serialize>(value: &T) -> Result<String, DbError> {
    serde_json::to_string(value).map_err(|e| DbError::decode("review json", "", e))
}

fn from_json<T: serde::de::DeserializeOwned>(what: &str, text: &str) -> Result<T, DbError> {
    serde_json::from_str(text).map_err(|e| DbError::decode(what, text, e))
}

fn run_from_row(row: &Row<'_>) -> rusqlite::Result<Result<ReviewRun, DbError>> {
    let id: String = row.get(0)?;
    let workspace: String = row.get(1)?;
    let target: String = row.get(2)?;
    let tier: String = row.get(5)?;
    let status: String = row.get(7)?;
    let stage: Option<String> = row.get(8)?;
    let score: Option<f64> = row.get(10)?;
    let verdict: Option<String> = row.get(11)?;
    let session: Option<String> = row.get(13)?;
    let coverage: String = row.get(14)?;
    let created_at: String = row.get(15)?;
    let finished_at: Option<String> = row.get(16)?;
    let head_oid: String = row.get(3)?;
    let base_oid: Option<String> = row.get(4)?;
    let provider: String = row.get(6)?;
    let summary: String = row.get(9)?;
    let error: Option<String> = row.get(12)?;
    let preset: String = row.get(17)?;
    Ok((|| {
        #[allow(
            clippy::cast_possible_truncation,
            reason = "stored from an f32 score in 0..=5"
        )]
        let score = score.map(|score| score as f32);
        Ok(ReviewRun {
            id: id_from_str("ReviewId", &id)?,
            workspace_id: id_from_str("WorkspaceId", &workspace)?,
            target: from_json::<ReviewTarget>("ReviewTarget", &target)?,
            head_oid,
            base_oid,
            tier: parse_tag("Tier", &tier, TIERS)?,
            preset,
            provider,
            status: parse_tag("ReviewStatus", &status, STATUSES)?,
            stage: stage
                .map(|stage| parse_tag("ReviewStage", &stage, STAGES))
                .transpose()?,
            summary,
            score,
            verdict: verdict
                .map(|verdict| parse_tag("Verdict", &verdict, VERDICTS))
                .transpose()?,
            error,
            session_id: session
                .map(|id| id_from_str("SessionId", &id))
                .transpose()?,
            coverage: from_json::<Coverage>("Coverage", &coverage)?,
            created_at: ts_from_str(&created_at)?,
            finished_at: ts_from_opt(finished_at)?,
        })
    })())
}

fn finding_from_row(row: &Row<'_>) -> rusqlite::Result<Result<Finding, DbError>> {
    let severity: String = row.get(5)?;
    let confidence: f64 = row.get(8)?;
    #[allow(
        clippy::cast_possible_truncation,
        reason = "stored from an f32 confidence in 0..=1"
    )]
    let confidence = confidence as f32;
    let finding = Finding {
        id: row.get(0)?,
        fingerprint: row.get(1)?,
        path: row.get(2)?,
        line: row.get(3)?,
        end_line: row.get(4)?,
        severity: Severity::P2,
        category: row.get(6)?,
        rule_id: row.get(7)?,
        confidence,
        title: row.get(9)?,
        body: row.get(10)?,
        evidence: row.get(11)?,
        suggestion: row.get(12)?,
    };
    Ok(
        parse_tag("Severity", &severity, SEVERITIES).map(|severity| Finding {
            severity,
            ..finding
        }),
    )
}

fn parse_tag<T: Copy>(what: &str, text: &str, table: &[(T, &str)]) -> Result<T, DbError> {
    table
        .iter()
        .find(|(_, tag)| *tag == text)
        .map(|(value, _)| *value)
        .ok_or_else(|| DbError::decode(what, text, "unknown tag"))
}

fn tag<T: Copy + PartialEq>(value: T, table: &[(T, &'static str)]) -> &'static str {
    table
        .iter()
        .find(|(candidate, _)| *candidate == value)
        .map_or("", |(_, tag)| tag)
}

const TIERS: &[(Tier, &str)] = &[
    (Tier::Base, "base"),
    (Tier::Plus, "plus"),
    (Tier::Apex, "apex"),
];
const STATUSES: &[(ReviewStatus, &str)] = &[
    (ReviewStatus::Queued, "Queued"),
    (ReviewStatus::Running, "Running"),
    (ReviewStatus::Completed, "Completed"),
    (ReviewStatus::Failed, "Failed"),
    (ReviewStatus::Cancelled, "Cancelled"),
    (ReviewStatus::Interrupted, "Interrupted"),
];
const STAGES: &[(ReviewStage, &str)] = &[
    (ReviewStage::Diff, "Diff"),
    (ReviewStage::Pack, "Pack"),
    (ReviewStage::Model, "Model"),
    (ReviewStage::Validate, "Validate"),
];
const VERDICTS: &[(Verdict, &str)] = &[
    (Verdict::Block, "Block"),
    (Verdict::Review, "Review"),
    (Verdict::Ok, "Ok"),
];
const SEVERITIES: &[(Severity, &str)] = &[
    (Severity::P0, "P0"),
    (Severity::P1, "P1"),
    (Severity::P2, "P2"),
];

fn tier_tag(tier: Tier) -> &'static str {
    tag(tier, TIERS)
}
fn status_tag(status: ReviewStatus) -> &'static str {
    tag(status, STATUSES)
}
fn stage_tag(stage: ReviewStage) -> &'static str {
    tag(stage, STAGES)
}
fn verdict_tag(verdict: Verdict) -> &'static str {
    tag(verdict, VERDICTS)
}
fn severity_tag(severity: Severity) -> &'static str {
    tag(severity, SEVERITIES)
}
