//! Forge Review runs and findings. `target` and `coverage` are JSON columns,
//! the `project_actions.spec` reasoning: nothing queries inside them.

use std::collections::{HashMap, HashSet};

use domain::{
    Coverage, Finding, FindingReaction, Lineage, ProjectId, ReviewId, ReviewRequest, ReviewRun,
    ReviewStage, ReviewStatus, ReviewSuppression, ReviewTarget, SessionId, Severity,
    SuppressionKind, Tier, Timestamp, Verdict, WorkspaceId,
};
use rusqlite::{params, Connection, OptionalExtension, Row};

use crate::db::DbError;
use crate::repositories::{id_from_str, ts_from_opt, ts_from_str, ts_to_str};

// `ReviewRun::reviewers` is stored in the `swarm_size` column.
const RUN_COLUMNS: &str = "id, workspace_id, target, head_oid, base_oid, tier, provider, status, \
     stage, summary, score, verdict, error, session_id, coverage, created_at, finished_at, preset, branch, tree_oid, \
     diagrams, auto_approvable, swarm_size, schema_version, sandbox_run_id";

const FINDING_COLUMNS: &str = "id, fingerprint, path, line, end_line, severity, category, \
     rule_id, confidence, title, body, evidence, suggestion, lineage, hypothesis, kb_ref, \
     reachability, reachability_trigger";

pub fn insert_run(conn: &Connection, run: &ReviewRun) -> Result<(), DbError> {
    conn.prepare_cached(&format!(
        "INSERT INTO review_runs ({RUN_COLUMNS}, proposed) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, \
         ?21, ?22, ?23, ?24, ?25, ?26)"
    ))?
    .execute(params![
        run.id.to_string(),
        run.workspace_id.to_string(),
        json(&run.target)?,
        run.head_oid,
        run.base_oid,
        run.tier.name(),
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
        run.branch,
        run.tree_oid,
        json(&run.diagrams)?,
        run.auto_approvable,
        run.reviewers,
        schema_tag(run.schema_version),
        run.sandbox_run_id,
        run.coverage.proposed,
    ])?;
    Ok(())
}

/// [`insert_run`] with the request it came from, in one transaction.
///
/// # Errors
/// Returns [`DbError`] on a failed statement.
pub fn insert_requested_run(
    conn: &mut Connection,
    run: &ReviewRun,
    request: &ReviewRequest,
) -> Result<(), DbError> {
    let tx = conn.transaction()?;
    insert_run(&tx, run)?;
    tx.execute(
        "UPDATE review_runs SET request = ?2 WHERE id = ?1",
        params![run.id.to_string(), json(request)?],
    )?;
    tx.commit()?;
    Ok(())
}

/// `None` for an unknown run or one stored before requests were.
///
/// # Errors
/// Returns [`DbError`] on a failed statement or an unreadable request.
pub fn run_request(conn: &Connection, id: ReviewId) -> Result<Option<ReviewRequest>, DbError> {
    let text: Option<String> = conn
        .query_row(
            "SELECT request FROM review_runs WHERE id = ?1",
            params![id.to_string()],
            |row| row.get(0),
        )
        .optional()?
        .flatten();
    text.map(|text| from_json("ReviewRequest", &text))
        .transpose()
}

/// Rewrites every mutable column; a run removed meanwhile stays removed.
pub fn update_run(conn: &Connection, run: &ReviewRun) -> Result<(), DbError> {
    conn.prepare_cached(
        "UPDATE review_runs SET base_oid = ?2, status = ?3, stage = ?4, summary = ?5, \
         score = ?6, verdict = ?7, error = ?8, session_id = ?9, coverage = ?10, \
         finished_at = ?11, head_oid = ?12, tree_oid = ?13, tier = ?14, diagrams = ?15, \
         auto_approvable = ?16, swarm_size = ?17, schema_version = ?18, sandbox_run_id = ?19, \
         proposed = ?20 WHERE id = ?1",
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
        run.head_oid,
        run.tree_oid,
        run.tier.name(),
        json(&run.diagrams)?,
        run.auto_approvable,
        run.reviewers,
        schema_tag(run.schema_version),
        run.sandbox_run_id,
        run.coverage.proposed,
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
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)"
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
                lineage_tag(f.lineage),
                f.hypothesis.map(domain::Hypothesis::name),
                f.kb_ref,
                f.reachability.as_ref().map(|reach| reach.kind.name()),
                f.reachability.as_ref().map(|reach| reach.trigger.as_str()),
            ])?;
        }
        Ok::<_, DbError>(())
    })();
    let result = result.and_then(|()| Ok(conn.execute_batch("COMMIT")?));
    if result.is_err() {
        let _ = conn.execute_batch("ROLLBACK");
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
    let mut findings = stmt
        .query_map(params![id.to_string()], finding_from_row)?
        .map(|row| row?)
        .collect::<Result<Vec<_>, _>>()?;
    let reactions = reactions(conn, id)?;
    for finding in &mut findings {
        finding.reaction = reactions.get(&finding.id).copied();
    }
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

/// Every agent session a run launched; `session_id` stays the first.
///
/// # Errors
/// Returns [`DbError`] on a failed statement.
pub fn set_agent_sessions(
    conn: &Connection,
    run: ReviewId,
    sessions: &[SessionId],
) -> Result<(), DbError> {
    let ids: Vec<String> = sessions.iter().map(ToString::to_string).collect();
    conn.execute(
        "UPDATE review_runs SET agent_sessions = ?2 WHERE id = ?1",
        params![run.to_string(), json(&ids)?],
    )?;
    Ok(())
}

/// Session ids named by review runs, every reviewer's included, for the
/// restart's read-only set.
pub fn reviewer_sessions(conn: &Connection) -> Result<HashSet<SessionId>, DbError> {
    let mut stmt = conn.prepare(
        "SELECT session_id FROM review_runs WHERE session_id IS NOT NULL \
         UNION SELECT j.value FROM review_runs, json_each(review_runs.agent_sessions) j",
    )?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
    let mut ids = HashSet::new();
    for row in rows {
        ids.insert(id_from_str("review reviewer session", &row?)?);
    }
    Ok(ids)
}

/// Keep a workspace's newest `keep` runs, and of those only the ones created
/// at or after `cutoff`. Findings go with their run. A run while any of its
/// reviewer session rows still exists is kept: that row is what makes the
/// restart read-only.
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
         AND (session_id IS NULL OR session_id NOT IN (SELECT id FROM sessions)) \
         AND NOT EXISTS (SELECT 1 FROM json_each(review_runs.agent_sessions) j \
           WHERE j.value IN (SELECT id FROM sessions))",
        params![workspace.to_string(), keep, ts_to_str(&cutoff)],
    )?)
}

const MAX_FEEDBACK_ROWS: u32 = 10_000;

pub fn set_feedback(
    conn: &Connection,
    project: ProjectId,
    review: ReviewId,
    finding: &Finding,
    reaction: FindingReaction,
) -> Result<(), DbError> {
    conn.prepare_cached(
        "INSERT INTO review_feedback (review_id, finding_id, project_id, fingerprint, rule_id, \
         category, severity, reaction, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9) \
         ON CONFLICT (review_id, finding_id) DO UPDATE SET reaction = ?8, created_at = ?9, counted = 1",
    )?
    .execute(params![
        review.to_string(),
        finding.id,
        project.to_string(),
        finding.fingerprint,
        finding.rule_id,
        finding.category,
        severity_tag(finding.severity),
        reaction_tag(reaction),
        ts_to_str(&Timestamp::now()),
    ])?;
    Ok(())
}

pub fn clear_feedback(conn: &Connection, review: ReviewId, finding: u32) -> Result<(), DbError> {
    conn.prepare_cached("DELETE FROM review_feedback WHERE review_id = ?1 AND finding_id = ?2")?
        .execute(params![review.to_string(), finding])?;
    Ok(())
}

/// The project's newest [`MAX_FEEDBACK_ROWS`] reactions.
pub fn feedback(conn: &Connection, project: ProjectId) -> Result<Vec<review::Feedback>, DbError> {
    let mut stmt = conn.prepare_cached(
        "SELECT fingerprint, rule_id, category, severity, reaction, created_at, counted \
         FROM review_feedback WHERE project_id = ?1 ORDER BY created_at DESC LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![project.to_string(), MAX_FEEDBACK_ROWS], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, Option<String>>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
            row.get::<_, String>(5)?,
            row.get::<_, bool>(6)?,
        ))
    })?;
    rows.map(|row| {
        let (fingerprint, rule_id, category, severity, reaction, created_at, counted) = row?;
        Ok(review::Feedback {
            fingerprint,
            rule_id,
            category,
            severity: parse_tag("Severity", &severity, SEVERITIES)?,
            reaction: parse_tag("FindingReaction", &reaction, REACTIONS)?,
            created_at: ts_from_str(&created_at)?.as_offset().unix_timestamp(),
            counted,
        })
    })
    .collect()
}

/// Lifts `suppression`: a fingerprint's dismissals are deleted at every
/// severity, so the finding comes back; a rule's or a category's stop counting
/// toward it, and each finding they dismissed stays dismissed. Returns the
/// runs whose findings are no longer dismissed, each once.
pub fn clear_suppression(
    conn: &Connection,
    project: ProjectId,
    suppression: &ReviewSuppression,
) -> Result<Vec<ReviewId>, DbError> {
    let column = match suppression.kind {
        SuppressionKind::Rule => "rule_id",
        SuppressionKind::Category => "category",
        SuppressionKind::Fingerprint => "fingerprint",
    };
    let filter = format!("project_id = ?1 AND reaction = ?2 AND {column} = ?3");
    let params = params![
        project.to_string(),
        reaction_tag(FindingReaction::Dismiss),
        suppression.value
    ];
    if suppression.kind != SuppressionKind::Fingerprint {
        conn.execute(
            &format!("UPDATE review_feedback SET counted = 0 WHERE {filter} AND counted = 1"),
            params,
        )?;
        return Ok(Vec::new());
    }
    let mut stmt = conn.prepare(&format!(
        "DELETE FROM review_feedback WHERE {filter} RETURNING review_id"
    ))?;
    let rows = stmt.query_map(params, |row| row.get::<_, String>(0))?;
    let mut ids = Vec::new();
    for row in rows {
        let id = id_from_str("review feedback run", &row?)?;
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    Ok(ids)
}

fn reactions(
    conn: &Connection,
    review: ReviewId,
) -> Result<HashMap<u32, FindingReaction>, DbError> {
    let mut stmt = conn
        .prepare_cached("SELECT finding_id, reaction FROM review_feedback WHERE review_id = ?1")?;
    let rows = stmt.query_map(params![review.to_string()], |row| {
        Ok((row.get::<_, u32>(0)?, row.get::<_, String>(1)?))
    })?;
    rows.map(|row| {
        let (id, reaction) = row?;
        Ok((id, parse_tag("FindingReaction", &reaction, REACTIONS)?))
    })
    .collect()
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
    let branch: Option<String> = row.get(18)?;
    let tree_oid: Option<String> = row.get(19)?;
    let diagrams: String = row.get(20)?;
    let auto_approvable: bool = row.get(21)?;
    let reviewers: u8 = row.get(22)?;
    let schema_version: String = row.get(23)?;
    let sandbox_run_id: Option<String> = row.get(24)?;
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
            tier: parse_tier(&tier)?,
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
            branch,
            tree_oid,
            diagrams: from_json("diagrams", &diagrams)?,
            auto_approvable,
            reviewers,
            schema_version: parse_tag("SchemaVersion", &schema_version, SCHEMAS)?,
            sandbox_run_id,
        })
    })())
}

fn finding_from_row(row: &Row<'_>) -> rusqlite::Result<Result<Finding, DbError>> {
    let severity: String = row.get(5)?;
    let lineage: String = row.get(13)?;
    let hypothesis: Option<String> = row.get(14)?;
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
        lineage: domain::Lineage::New,
        reaction: None,
        hypothesis: hypothesis
            .as_deref()
            .and_then(domain::Hypothesis::from_name),
        kb_ref: row.get(15)?,
        reachability: None,
    };
    let reachability: Option<String> = row.get(16)?;
    let trigger: Option<String> = row.get(17)?;
    Ok((|| {
        Ok(Finding {
            severity: parse_tag("Severity", &severity, SEVERITIES)?,
            lineage: parse_tag("Lineage", &lineage, LINEAGES)?,
            reachability: reachability
                .map(|kind| {
                    domain::ReachabilityKind::from_name(&kind)
                        .map(|kind| domain::Reachability {
                            kind,
                            trigger: trigger.unwrap_or_default(),
                        })
                        .ok_or_else(|| DbError::decode("ReachabilityKind", &kind, "unknown tag"))
                })
                .transpose()?,
            ..finding
        })
    })())
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

const SCHEMAS: &[(domain::SchemaVersion, &str)] = &[
    (domain::SchemaVersion::V1, "v1"),
    (domain::SchemaVersion::V2, "v2"),
];

fn schema_tag(version: domain::SchemaVersion) -> &'static str {
    tag(version, SCHEMAS)
}

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
const LINEAGES: &[(Lineage, &str)] = &[(Lineage::New, "new"), (Lineage::Persisting, "persisting")];
const REACTIONS: &[(FindingReaction, &str)] = &[
    (FindingReaction::Up, "up"),
    (FindingReaction::Down, "down"),
    (FindingReaction::Dismiss, "dismiss"),
];

fn parse_tier(text: &str) -> Result<Tier, DbError> {
    Tier::parse(text).ok_or_else(|| DbError::decode("Tier", text, "unknown tag"))
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
fn lineage_tag(lineage: Lineage) -> &'static str {
    tag(lineage, LINEAGES)
}
fn reaction_tag(reaction: FindingReaction) -> &'static str {
    tag(reaction, REACTIONS)
}

/// One completed run, as `GetReviewStats` counts it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatsRow {
    pub id: String,
    pub tier: Tier,
    /// Wall-clock seconds from start to finish.
    pub seconds: i64,
    pub reviewers: u8,
    pub hypotheses_found: u32,
    pub cites_kb: bool,
    /// Findings the reviewers proposed; `0` for a run stored before it was counted.
    pub proposed: u32,
    /// Kept findings that say how they are reached.
    pub reachable: u32,
}

/// Completed runs, of one workspace or all, newest first.
///
/// # Errors
/// Returns [`DbError`] on a failed statement or an undecodable row.
pub fn stats_rows(
    conn: &Connection,
    workspace: Option<WorkspaceId>,
) -> Result<Vec<StatsRow>, DbError> {
    let mut statement = conn.prepare_cached(
        "SELECT r.id, r.tier, r.created_at, r.finished_at, r.swarm_size, \
         (SELECT COUNT(DISTINCT f.hypothesis) FROM review_findings f WHERE f.review_id = r.id AND f.hypothesis IS NOT NULL), \
         EXISTS (SELECT 1 FROM review_findings f WHERE f.review_id = r.id AND f.kb_ref IS NOT NULL), \
         r.proposed, \
         (SELECT COUNT(*) FROM review_findings f WHERE f.review_id = r.id AND f.reachability IS NOT NULL) \
         FROM review_runs r WHERE r.status = 'Completed' AND r.finished_at IS NOT NULL \
         AND (?1 IS NULL OR r.workspace_id = ?1) ORDER BY r.id DESC LIMIT 10000",
    )?;
    let rows = statement.query_map(params![workspace.map(|w| w.to_string())], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, u8>(4)?,
            row.get::<_, u32>(5)?,
            row.get::<_, bool>(6)?,
            row.get::<_, u32>(7)?,
            row.get::<_, u32>(8)?,
        ))
    })?;
    rows.map(|row| {
        let (
            id,
            tier,
            created,
            finished,
            reviewers,
            hypotheses_found,
            cites_kb,
            proposed,
            reachable,
        ) = row?;
        let seconds = ts_from_str(&finished)?.as_offset().unix_timestamp()
            - ts_from_str(&created)?.as_offset().unix_timestamp();
        Ok(StatsRow {
            id,
            tier: parse_tier(&tier)?,
            seconds,
            reviewers,
            hypotheses_found,
            cites_kb,
            proposed,
            reachable,
        })
    })
    .collect()
}

/// Bytes the knowledge base holds, for one project or all.
///
/// # Errors
/// Returns [`DbError`] on a failed statement.
pub fn kb_bytes(conn: &Connection, project: Option<ProjectId>) -> Result<u64, DbError> {
    let bytes: i64 = conn.query_row(
        "SELECT COALESCE(SUM(LENGTH(content)), 0) FROM kb_entries WHERE ?1 IS NULL OR project_id = ?1",
        params![project.map(|p| p.to_string())],
        |row| row.get(0),
    )?;
    Ok(u64::try_from(bytes).unwrap_or(0))
}
