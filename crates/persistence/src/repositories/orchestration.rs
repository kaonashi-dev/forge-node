//! Orchestration ledger. Enum tags are the snake_case names the CLI prints.

use domain::orchestration::{
    Attempt, AttemptPhase, AttemptReport, BoardEntry, LostReason, ReportOutcome, Run, RunStatus,
    Task, TaskStatus, WorkMode,
};
use domain::{AgentProviderId, RunId, TaskId, Timestamp};
use rusqlite::{params, Connection, OptionalExtension, Row};

use crate::db::DbError;
use crate::repositories::{id_from_opt, id_from_str, ts_from_opt, ts_from_str, ts_to_str};

pub struct OrchestrationRepo<'a> {
    conn: &'a Connection,
}

impl<'a> OrchestrationRepo<'a> {
    #[must_use]
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }

    pub fn insert_run(&self, run: &Run) -> Result<(), DbError> {
        self.conn.execute(
            "INSERT INTO runs (\
                id, project_id, objective, brief, brief_version, controller_session_id, \
                integration_workspace_id, base, parent_attempt_id, status, revision, \
                created_at, updated_at, closed_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            params![
                run.id.to_string(),
                run.project_id.to_string(),
                run.objective,
                run.brief,
                run.brief_version as i64,
                opt_id(run.controller_session_id),
                opt_id(run.integration_workspace_id),
                run.base,
                opt_id(run.parent_attempt_id),
                run_status_to_str(run.status)?,
                run.revision as i64,
                ts_to_str(&run.created_at),
                ts_to_str(&run.updated_at),
                run.closed_at.as_ref().map(ts_to_str),
            ],
        )?;
        Ok(())
    }

    pub fn update_run(&self, run: &Run) -> Result<(), DbError> {
        self.conn.execute(
            "UPDATE runs SET objective=?1, brief=?2, brief_version=?3, \
                controller_session_id=?4, integration_workspace_id=?5, base=?6, \
                parent_attempt_id=?7, status=?8, revision=?9, updated_at=?10, closed_at=?11 \
             WHERE id=?12",
            params![
                run.objective,
                run.brief,
                run.brief_version as i64,
                opt_id(run.controller_session_id),
                opt_id(run.integration_workspace_id),
                run.base,
                opt_id(run.parent_attempt_id),
                run_status_to_str(run.status)?,
                run.revision as i64,
                ts_to_str(&run.updated_at),
                run.closed_at.as_ref().map(ts_to_str),
                run.id.to_string(),
            ],
        )?;
        Ok(())
    }

    pub fn list_runs(&self) -> Result<Vec<Run>, DbError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, project_id, objective, brief, brief_version, controller_session_id, \
                    integration_workspace_id, base, parent_attempt_id, status, revision, \
                    created_at, updated_at, closed_at \
             FROM runs ORDER BY created_at ASC",
        )?;
        let rows = stmt.query_map([], run_from_row)?;
        rows.collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
    }

    pub fn insert_task(&self, task: &Task) -> Result<(), DbError> {
        self.conn.execute(
            "INSERT INTO tasks (\
                id, run_id, title, spec, acceptance, mode, allow_subruns, status, \
                attempts_used, revision, feedback, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                task.id.to_string(),
                task.run_id.to_string(),
                task.title,
                task.spec,
                task.acceptance,
                task_mode_to_str(task.mode)?,
                task.allow_subruns as i64,
                task_status_to_str(task.status)?,
                task.attempts_used as i64,
                task.revision as i64,
                task.feedback,
                ts_to_str(&task.created_at),
                ts_to_str(&task.updated_at),
            ],
        )?;
        let mut stmt = self
            .conn
            .prepare("INSERT INTO task_deps (task_id, after_task_id) VALUES (?1, ?2)")?;
        for after in &task.after {
            stmt.execute(params![task.id.to_string(), after.to_string()])?;
        }
        Ok(())
    }

    pub fn update_task(&self, task: &Task) -> Result<(), DbError> {
        self.conn.execute(
            "UPDATE tasks SET title=?1, spec=?2, acceptance=?3, mode=?4, allow_subruns=?5, \
                status=?6, attempts_used=?7, revision=?8, feedback=?9, updated_at=?10 \
             WHERE id=?11",
            params![
                task.title,
                task.spec,
                task.acceptance,
                task_mode_to_str(task.mode)?,
                task.allow_subruns as i64,
                task_status_to_str(task.status)?,
                task.attempts_used as i64,
                task.revision as i64,
                task.feedback,
                ts_to_str(&task.updated_at),
                task.id.to_string(),
            ],
        )?;
        self.conn.execute(
            "DELETE FROM task_deps WHERE task_id=?1",
            params![task.id.to_string()],
        )?;
        let mut stmt = self
            .conn
            .prepare("INSERT INTO task_deps (task_id, after_task_id) VALUES (?1, ?2)")?;
        for after in &task.after {
            stmt.execute(params![task.id.to_string(), after.to_string()])?;
        }
        Ok(())
    }

    pub fn list_tasks(&self) -> Result<Vec<Task>, DbError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, run_id, title, spec, acceptance, mode, allow_subruns, status, \
                    attempts_used, revision, feedback, created_at, updated_at \
             FROM tasks ORDER BY created_at ASC",
        )?;
        let rows = stmt.query_map([], task_from_row)?;
        let mut tasks = Vec::new();
        for row in rows {
            let mut task = row??;
            task.after = self.deps(task.id)?;
            tasks.push(task);
        }
        Ok(tasks)
    }

    pub fn deps(&self, task: TaskId) -> Result<Vec<TaskId>, DbError> {
        let mut stmt = self.conn.prepare(
            "SELECT after_task_id FROM task_deps WHERE task_id=?1 ORDER BY after_task_id ASC",
        )?;
        let rows = stmt.query_map(params![task.to_string()], |row| row.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(id_from_str("TaskId", &row?)?);
        }
        Ok(out)
    }

    pub fn insert_attempt(&self, attempt: &Attempt) -> Result<(), DbError> {
        let v = attempt_values(attempt)?;
        self.conn.execute(
            "INSERT INTO attempts (\
                id, task_id, n, session_id, workspace_id, branch, base_commit, provider_id, \
                profile_id, read_only, phase, outcome, lost_reason, summary, verification, \
                result_path, reported_head, dirty_at_report, files_changed_json, \
                integrated_commit, created_at, settled_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, \
                     ?16, ?17, ?18, ?19, ?20, ?21, ?22)",
            params![
                v.0, v.1, v.2, v.3, v.4, v.5, v.6, v.7, v.8, v.9, v.10, v.11, v.12, v.13, v.14,
                v.15, v.16, v.17, v.18, v.19, v.20, v.21
            ],
        )?;
        Ok(())
    }

    pub fn update_attempt(&self, attempt: &Attempt) -> Result<(), DbError> {
        let v = attempt_values(attempt)?;
        self.conn.execute(
            "UPDATE attempts SET session_id=?2, workspace_id=?3, branch=?4, base_commit=?5, \
                provider_id=?6, profile_id=?7, read_only=?8, phase=?9, outcome=?10, \
                lost_reason=?11, summary=?12, verification=?13, result_path=?14, \
                reported_head=?15, dirty_at_report=?16, files_changed_json=?17, \
                integrated_commit=?18, settled_at=?19 \
             WHERE id=?1",
            params![
                v.0, v.3, v.4, v.5, v.6, v.7, v.8, v.9, v.10, v.11, v.12, v.13, v.14, v.15, v.16,
                v.17, v.18, v.19, v.21
            ],
        )?;
        Ok(())
    }

    pub fn list_attempts(&self) -> Result<Vec<Attempt>, DbError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, task_id, n, session_id, workspace_id, branch, base_commit, provider_id, \
                    profile_id, read_only, phase, outcome, lost_reason, summary, verification, \
                    result_path, reported_head, dirty_at_report, files_changed_json, \
                    integrated_commit, created_at, settled_at \
             FROM attempts ORDER BY created_at ASC",
        )?;
        let rows = stmt.query_map([], attempt_from_row)?;
        rows.collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
    }

    pub fn list_board(&self, run: RunId) -> Result<Vec<BoardEntry>, DbError> {
        let mut stmt = self.conn.prepare(
            "SELECT run_id, key, value_json, version, updated_by, updated_at \
             FROM run_state WHERE run_id=?1 ORDER BY key ASC",
        )?;
        let rows = stmt.query_map(params![run.to_string()], board_from_row)?;
        rows.collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
    }

    pub fn upsert_board(&self, entry: &BoardEntry) -> Result<(), DbError> {
        self.conn.execute(
            "INSERT INTO run_state (run_id, key, value_json, version, updated_by, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6) \
             ON CONFLICT (run_id, key) DO UPDATE SET \
                value_json=excluded.value_json, version=excluded.version, \
                updated_by=excluded.updated_by, updated_at=excluded.updated_at",
            params![
                entry.run_id.to_string(),
                entry.key,
                entry.value_json,
                entry.version as i64,
                opt_id(entry.updated_by),
                ts_to_str(&entry.updated_at),
            ],
        )?;
        Ok(())
    }

    pub fn delete_board(&self, run: RunId, key: &str) -> Result<(), DbError> {
        self.conn.execute(
            "DELETE FROM run_state WHERE run_id=?1 AND key=?2",
            params![run.to_string(), key],
        )?;
        Ok(())
    }

    pub fn put_receipt(
        &self,
        request_id: &str,
        caller: &str,
        response: &[u8],
        at: Timestamp,
    ) -> Result<(), DbError> {
        self.conn.execute(
            "INSERT INTO orchestration_receipts (request_id, caller, response_msgpack, created_at) \
             VALUES (?1, ?2, ?3, ?4) \
             ON CONFLICT (request_id, caller) DO UPDATE SET \
                response_msgpack=excluded.response_msgpack, created_at=excluded.created_at",
            params![request_id, caller, response, ts_to_str(&at)],
        )?;
        Ok(())
    }

    pub fn get_receipt(
        &self,
        request_id: &str,
        caller: &str,
    ) -> Result<Option<(Vec<u8>, Timestamp)>, DbError> {
        let row = self
            .conn
            .query_row(
                "SELECT response_msgpack, created_at FROM orchestration_receipts \
                 WHERE request_id=?1 AND caller=?2",
                params![request_id, caller],
                |row| Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?;
        match row {
            Some((bytes, at)) => Ok(Some((bytes, ts_from_str(&at)?))),
            None => Ok(None),
        }
    }

    pub fn delete_run(&self, run: RunId) -> Result<(), DbError> {
        self.conn
            .execute("DELETE FROM runs WHERE id=?1", params![run.to_string()])?;
        Ok(())
    }
}

fn opt_id<T: std::fmt::Display>(id: Option<T>) -> Option<String> {
    id.map(|id| id.to_string())
}

fn run_from_row(row: &Row<'_>) -> rusqlite::Result<Result<Run, DbError>> {
    let status: String = row.get(9)?;
    let created: String = row.get(11)?;
    let updated: String = row.get(12)?;
    let closed: Option<String> = row.get(13)?;
    Ok((|| {
        Ok(Run {
            id: id_from_str("RunId", &row.get::<_, String>(0)?)?,
            project_id: id_from_str("ProjectId", &row.get::<_, String>(1)?)?,
            objective: row.get(2)?,
            brief: row.get(3)?,
            brief_version: u64::try_from(row.get::<_, i64>(4)?).unwrap_or(0),
            controller_session_id: id_from_opt("SessionId", row.get(5)?)?,
            integration_workspace_id: id_from_opt("WorkspaceId", row.get(6)?)?,
            base: row.get(7)?,
            parent_attempt_id: id_from_opt("AttemptId", row.get(8)?)?,
            status: run_status_from_str(&status)?,
            revision: u64::try_from(row.get::<_, i64>(10)?).unwrap_or(0),
            created_at: ts_from_str(&created)?,
            updated_at: ts_from_str(&updated)?,
            closed_at: ts_from_opt(closed)?,
        })
    })())
}

fn task_from_row(row: &Row<'_>) -> rusqlite::Result<Result<Task, DbError>> {
    let mode: String = row.get(5)?;
    let status: String = row.get(7)?;
    let created: String = row.get(11)?;
    let updated: String = row.get(12)?;
    Ok((|| {
        Ok(Task {
            id: id_from_str("TaskId", &row.get::<_, String>(0)?)?,
            run_id: id_from_str("RunId", &row.get::<_, String>(1)?)?,
            title: row.get(2)?,
            spec: row.get(3)?,
            acceptance: row.get(4)?,
            after: Vec::new(),
            mode: task_mode_from_str(&mode)?,
            allow_subruns: row.get::<_, i64>(6)? != 0,
            status: task_status_from_str(&status)?,
            attempts_used: u32::try_from(row.get::<_, i64>(8)?).unwrap_or(0),
            revision: u64::try_from(row.get::<_, i64>(9)?).unwrap_or(0),
            feedback: row.get(10)?,
            created_at: ts_from_str(&created)?,
            updated_at: ts_from_str(&updated)?,
        })
    })())
}

type AttemptRow = (
    String,
    String,
    i64,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    String,
    Option<String>,
    i64,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<i64>,
    Option<String>,
    Option<String>,
    String,
    Option<String>,
);

fn attempt_values(attempt: &Attempt) -> Result<AttemptRow, DbError> {
    let report = attempt.report.as_ref();
    let files = report
        .map(|report| serde_json::to_string(&report.files_changed))
        .transpose()?;
    Ok((
        attempt.id.to_string(),
        attempt.task_id.to_string(),
        i64::from(attempt.n),
        opt_id(attempt.session_id),
        opt_id(attempt.workspace_id),
        attempt.branch.clone(),
        attempt.base_commit.clone(),
        attempt.provider_id.to_string(),
        opt_id(attempt.profile_id),
        i64::from(attempt.read_only),
        attempt_phase_to_str(attempt.phase)?.to_owned(),
        attempt
            .outcome
            .map(report_outcome_to_str)
            .transpose()?
            .map(str::to_owned),
        attempt
            .lost_reason
            .map(lost_reason_to_str)
            .transpose()?
            .map(str::to_owned),
        report.map(|report| report.summary.clone()),
        report.and_then(|report| report.verification.clone()),
        report.and_then(|report| report.result_path.clone()),
        report.and_then(|report| report.reported_head.clone()),
        report.map(|report| i64::from(report.dirty_at_report)),
        files,
        attempt.integrated_commit.clone(),
        ts_to_str(&attempt.created_at),
        attempt.settled_at.as_ref().map(ts_to_str),
    ))
}

fn attempt_from_row(row: &Row<'_>) -> rusqlite::Result<Result<Attempt, DbError>> {
    Ok((|| {
        let phase: String = row.get(10)?;
        let outcome: Option<String> = row.get(11)?;
        let lost: Option<String> = row.get(12)?;
        let summary: Option<String> = row.get(13)?;
        let files: Option<String> = row.get(18)?;
        let verification: Option<String> = row.get(14)?;
        let result_path: Option<String> = row.get(15)?;
        let reported_head: Option<String> = row.get(16)?;
        let dirty = row.get::<_, Option<i64>>(17)?.unwrap_or(0) != 0;
        let files_changed: Vec<String> = files
            .as_deref()
            .map(serde_json::from_str)
            .transpose()?
            .unwrap_or_default();
        let parsed_outcome = outcome
            .as_deref()
            .map(report_outcome_from_str)
            .transpose()?;
        let report = summary.map(|summary| AttemptReport {
            outcome: parsed_outcome.unwrap_or(ReportOutcome::Done),
            summary,
            verification,
            result_path,
            reported_head,
            dirty_at_report: dirty,
            files_changed,
        });
        let created: String = row.get(20)?;
        let settled: Option<String> = row.get(21)?;
        Ok(Attempt {
            id: id_from_str("AttemptId", &row.get::<_, String>(0)?)?,
            task_id: id_from_str("TaskId", &row.get::<_, String>(1)?)?,
            n: u32::try_from(row.get::<_, i64>(2)?).unwrap_or(0),
            session_id: id_from_opt("SessionId", row.get(3)?)?,
            workspace_id: id_from_opt("WorkspaceId", row.get(4)?)?,
            branch: row.get(5)?,
            base_commit: row.get(6)?,
            provider_id: AgentProviderId::new(row.get::<_, String>(7)?),
            profile_id: id_from_opt("AgentProfileId", row.get(8)?)?,
            read_only: row.get::<_, i64>(9)? != 0,
            phase: attempt_phase_from_str(&phase)?,
            outcome: outcome
                .as_deref()
                .map(report_outcome_from_str)
                .transpose()?,
            lost_reason: lost.as_deref().map(lost_reason_from_str).transpose()?,
            report,
            integrated_commit: row.get(19)?,
            created_at: ts_from_str(&created)?,
            settled_at: ts_from_opt(settled)?,
        })
    })())
}

fn board_from_row(row: &Row<'_>) -> rusqlite::Result<Result<BoardEntry, DbError>> {
    let updated: String = row.get(5)?;
    Ok((|| {
        Ok(BoardEntry {
            run_id: id_from_str("RunId", &row.get::<_, String>(0)?)?,
            key: row.get(1)?,
            value_json: row.get(2)?,
            version: u64::try_from(row.get::<_, i64>(3)?).unwrap_or(0),
            updated_by: id_from_opt("SessionId", row.get(4)?)?,
            updated_at: ts_from_str(&updated)?,
        })
    })())
}

fn run_status_to_str(status: RunStatus) -> Result<&'static str, DbError> {
    match status {
        RunStatus::Active => Ok("active"),
        RunStatus::Completed => Ok("completed"),
        RunStatus::Cancelled => Ok("cancelled"),
        RunStatus::Interrupted => Ok("interrupted"),
        other => Err(DbError::Encode(format!("unhandled RunStatus: {other:?}"))),
    }
}

fn run_status_from_str(value: &str) -> Result<RunStatus, DbError> {
    match value {
        "active" => Ok(RunStatus::Active),
        "completed" => Ok(RunStatus::Completed),
        "cancelled" => Ok(RunStatus::Cancelled),
        "interrupted" => Ok(RunStatus::Interrupted),
        _ => Err(DbError::decode_msg("RunStatus", value, "unknown tag")),
    }
}

fn task_status_to_str(status: TaskStatus) -> Result<&'static str, DbError> {
    match status {
        TaskStatus::Pending => Ok("pending"),
        TaskStatus::Ready => Ok("ready"),
        TaskStatus::Active => Ok("active"),
        TaskStatus::Review => Ok("review"),
        TaskStatus::Accepted => Ok("accepted"),
        TaskStatus::Integrated => Ok("integrated"),
        TaskStatus::Blocked => Ok("blocked"),
        TaskStatus::Cancelled => Ok("cancelled"),
        TaskStatus::Failed => Ok("failed"),
        other => Err(DbError::Encode(format!("unhandled TaskStatus: {other:?}"))),
    }
}

fn task_status_from_str(value: &str) -> Result<TaskStatus, DbError> {
    match value {
        "pending" => Ok(TaskStatus::Pending),
        "ready" => Ok(TaskStatus::Ready),
        "active" => Ok(TaskStatus::Active),
        "review" => Ok(TaskStatus::Review),
        "accepted" => Ok(TaskStatus::Accepted),
        "integrated" => Ok(TaskStatus::Integrated),
        "blocked" => Ok(TaskStatus::Blocked),
        "cancelled" => Ok(TaskStatus::Cancelled),
        "failed" => Ok(TaskStatus::Failed),
        _ => Err(DbError::decode_msg("TaskStatus", value, "unknown tag")),
    }
}

fn task_mode_to_str(mode: WorkMode) -> Result<&'static str, DbError> {
    match mode {
        WorkMode::Write => Ok("write"),
        WorkMode::ReadOnly => Ok("read_only"),
        other => Err(DbError::Encode(format!("unhandled WorkMode: {other:?}"))),
    }
}

fn task_mode_from_str(value: &str) -> Result<WorkMode, DbError> {
    match value {
        "write" => Ok(WorkMode::Write),
        "read_only" => Ok(WorkMode::ReadOnly),
        _ => Err(DbError::decode_msg("WorkMode", value, "unknown tag")),
    }
}

fn attempt_phase_to_str(phase: AttemptPhase) -> Result<&'static str, DbError> {
    match phase {
        AttemptPhase::Starting => Ok("starting"),
        AttemptPhase::Running => Ok("running"),
        AttemptPhase::Reported => Ok("reported"),
        AttemptPhase::Rejected => Ok("rejected"),
        AttemptPhase::Lost => Ok("lost"),
        other => Err(DbError::Encode(format!(
            "unhandled AttemptPhase: {other:?}"
        ))),
    }
}

fn attempt_phase_from_str(value: &str) -> Result<AttemptPhase, DbError> {
    match value {
        "starting" => Ok(AttemptPhase::Starting),
        "running" => Ok(AttemptPhase::Running),
        "reported" => Ok(AttemptPhase::Reported),
        "rejected" => Ok(AttemptPhase::Rejected),
        "lost" => Ok(AttemptPhase::Lost),
        _ => Err(DbError::decode_msg("AttemptPhase", value, "unknown tag")),
    }
}

fn report_outcome_to_str(outcome: ReportOutcome) -> Result<&'static str, DbError> {
    match outcome {
        ReportOutcome::Done => Ok("done"),
        ReportOutcome::Failed => Ok("failed"),
        ReportOutcome::Blocked => Ok("blocked"),
        other => Err(DbError::Encode(format!(
            "unhandled ReportOutcome: {other:?}"
        ))),
    }
}

fn report_outcome_from_str(value: &str) -> Result<ReportOutcome, DbError> {
    match value {
        "done" => Ok(ReportOutcome::Done),
        "failed" => Ok(ReportOutcome::Failed),
        "blocked" => Ok(ReportOutcome::Blocked),
        _ => Err(DbError::decode_msg("ReportOutcome", value, "unknown tag")),
    }
}

fn lost_reason_to_str(reason: LostReason) -> Result<&'static str, DbError> {
    match reason {
        LostReason::ExitedWithoutReport => Ok("exited_without_report"),
        LostReason::DaemonRestarted => Ok("daemon_restarted"),
        LostReason::Cancelled => Ok("cancelled"),
        LostReason::SpawnFailed => Ok("spawn_failed"),
        other => Err(DbError::Encode(format!("unhandled LostReason: {other:?}"))),
    }
}

fn lost_reason_from_str(value: &str) -> Result<LostReason, DbError> {
    match value {
        "exited_without_report" => Ok(LostReason::ExitedWithoutReport),
        "daemon_restarted" => Ok(LostReason::DaemonRestarted),
        "cancelled" => Ok(LostReason::Cancelled),
        "spawn_failed" => Ok(LostReason::SpawnFailed),
        _ => Err(DbError::decode_msg("LostReason", value, "unknown tag")),
    }
}
