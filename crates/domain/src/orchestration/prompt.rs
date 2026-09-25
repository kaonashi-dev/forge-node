//! Launch prompts and the `forgectl context` digest.
//!
//! The fixed contract stays short. Everything else is truncated and marked,
//! and the worker is pointed at `forgectl context` for the rest.

use super::{clamp_text, Attempt, Task, MAX_CONTEXT_DIGEST_BYTES, MAX_PROMPT_BYTES};
use crate::ids::{AttemptId, RunId, TaskId};

const CONTRACT: &str = "\
## Contract — Forge reads these commands, not your terminal
- Commit your work on this branch. Do not push, open pull requests, or create worktrees.
- Decisions on scope, ownership or architecture go to the controller:
  forgectl ask --wait \"<question>\"
- Record a decision or claim others must see:
  forgectl state set <key> '<json>' --if-version <n>
- If you see \"Forge: N new message(s)\", run: forgectl inbox
- When finished or unable to finish, run exactly once, then stop and wait:
  forgectl report --done|--failed|--blocked --summary \"<what changed; how verified; what is left>\"
- Full contract: forgectl guide worker
";

/// Inputs already clamped by the caller, except the whole prompt which this
/// function fits to [`MAX_PROMPT_BYTES`].
#[derive(Clone, Debug)]
pub struct WorkerPromptInput<'a> {
    pub run_id: RunId,
    pub task_id: TaskId,
    pub attempt_n: u32,
    pub title: &'a str,
    pub objective: &'a str,
    pub spec: &'a str,
    pub acceptance: &'a str,
    pub brief: &'a str,
    pub brief_version: u64,
    pub workspace_path: &'a str,
    pub branch: &'a str,
    pub base_short: &'a str,
    pub dependencies: &'a [DependencySummary<'a>],
    pub feedback: Option<&'a str>,
    pub previous_report: Option<&'a str>,
}

#[derive(Clone, Debug)]
pub struct DependencySummary<'a> {
    pub task_id: TaskId,
    pub title: &'a str,
    pub summary: &'a str,
}

#[derive(Clone, Debug)]
pub struct ControllerPromptInput<'a> {
    pub run_id: RunId,
    pub objective: &'a str,
    pub brief: &'a str,
    pub brief_version: u64,
    pub base: &'a str,
    pub tasks: &'a [TaskLine<'a>],
    pub reports: &'a [ReportLine<'a>],
    pub questions: &'a [(&'a str, &'a str)],
    pub board_keys: &'a [&'a str],
    pub resume: bool,
}

#[derive(Clone, Debug)]
pub struct TaskLine<'a> {
    pub task_id: TaskId,
    pub title: &'a str,
    pub status: &'a str,
    pub spec: &'a str,
    pub acceptance: &'a str,
}

#[derive(Clone, Debug)]
pub struct ReportLine<'a> {
    pub task_id: TaskId,
    pub title: &'a str,
    pub outcome: &'a str,
    pub summary: &'a str,
}

#[derive(Clone, Debug)]
pub struct ContextDigestInput<'a> {
    pub objective: &'a str,
    pub brief: &'a str,
    pub brief_version: u64,
    pub spec: &'a str,
    pub acceptance: &'a str,
    pub dependencies: &'a [DependencySummary<'a>],
    pub board: &'a [(&'a str, Option<&'a str>)],
    pub unread: usize,
}

/// Worker launch prompt. Sections that do not fit are marked `[truncated]`.
#[must_use]
pub fn compose_worker_prompt(input: &WorkerPromptInput<'_>) -> String {
    let mut body = String::new();
    body.push_str("Forge task from your controller. Carry it out in this checkout.\n\n");
    push_section(
        &mut body,
        "Identity",
        &format!(
            "Task: {title} (run {run}, task {task}, attempt {n})\nObjective of the run: {objective}\nWorkspace: {path} on branch {branch}, branched from {base}.",
            title = input.title,
            run = input.run_id,
            task = input.task_id,
            n = input.attempt_n,
            objective = input.objective,
            path = input.workspace_path,
            branch = input.branch,
            base = input.base_short,
        ),
    );
    push_section(&mut body, "Task", input.spec);
    push_section(&mut body, "Acceptance", input.acceptance);
    let mut deps = String::new();
    if input.dependencies.is_empty() {
        deps.push_str("(none)");
    } else {
        for dep in input.dependencies {
            deps.push_str(&format!(
                "- {}: {} (full: forgectl report show {})\n",
                dep.title, dep.summary, dep.task_id
            ));
        }
    }
    push_section(&mut body, "Inputs from finished tasks", deps.trim_end());
    push_section(
        &mut body,
        &format!("Run brief (v{})", input.brief_version),
        if input.brief.is_empty() {
            "(none)"
        } else {
            input.brief
        },
    );
    if let Some(previous) = input.previous_report {
        push_section(&mut body, "Previous report", previous);
    }
    if let Some(feedback) = input.feedback {
        push_section(&mut body, "Feedback on the previous attempt", feedback);
    }
    body.push_str(CONTRACT);
    fit(&body, MAX_PROMPT_BYTES)
}

/// Controller launch prompt, also used when `run resume` seeds a new session.
#[must_use]
pub fn compose_controller_prompt(input: &ControllerPromptInput<'_>) -> String {
    let mut body = String::new();
    if input.resume {
        body.push_str(
            "You are resuming an interrupted Forge run. The ledger is the memory. Workers were not revived.\n\n",
        );
    } else {
        body.push_str(
            "You are the controller of a Forge run. You decide; the daemon only keeps the ledger and the rails.\n\n",
        );
    }
    push_section(
        &mut body,
        "Objective",
        &format!(
            "Run {}\nBase: {}\n\n{}",
            input.run_id, input.base, input.objective
        ),
    );
    let mut tasks = String::new();
    if input.tasks.is_empty() {
        tasks.push_str("(none yet — add the first task)");
    }
    for task in input.tasks {
        tasks.push_str(&format!(
            "- {} [{}] {}\n  spec: {}\n  acceptance: {}\n",
            task.task_id, task.status, task.title, task.spec, task.acceptance
        ));
    }
    push_section(&mut body, "Tasks", tasks.trim_end());
    let mut reports = String::new();
    if input.reports.is_empty() {
        reports.push_str("(none)");
    }
    for report in input.reports {
        reports.push_str(&format!(
            "- {} ({}) {}: {}\n",
            report.title, report.outcome, report.task_id, report.summary
        ));
    }
    push_section(
        &mut body,
        "Accepted dependency summaries",
        reports.trim_end(),
    );
    push_section(
        &mut body,
        &format!("Run brief (v{})", input.brief_version),
        if input.brief.is_empty() {
            "(none)"
        } else {
            input.brief
        },
    );
    let mut questions = String::new();
    if input.questions.is_empty() {
        questions.push_str("(none)");
    }
    for (task, text) in input.questions {
        questions.push_str(&format!("- {task}: {text}\n"));
    }
    push_section(&mut body, "Unanswered questions", questions.trim_end());
    let keys = if input.board_keys.is_empty() {
        "(none)".to_owned()
    } else {
        input.board_keys.join("\n")
    };
    push_section(&mut body, "Board keys", &keys);
    body.push_str(
        "\nLoop: task add → task start → run wait --for attention,reported,question → act. Never poll with sleep.\nFull contract: forgectl guide controller\n",
    );
    fit(&body, MAX_PROMPT_BYTES)
}

/// What `forgectl context` prints. Current brief and newly accepted deps.
#[must_use]
pub fn compose_context_digest(input: &ContextDigestInput<'_>) -> String {
    let mut body = String::new();
    push_section(&mut body, "Objective", input.objective);
    push_section(
        &mut body,
        &format!("Run brief (v{})", input.brief_version),
        if input.brief.is_empty() {
            "(none)"
        } else {
            input.brief
        },
    );
    push_section(&mut body, "Task", input.spec);
    push_section(&mut body, "Acceptance", input.acceptance);
    let mut deps = String::new();
    if input.dependencies.is_empty() {
        deps.push_str("(none)");
    }
    for dep in input.dependencies {
        deps.push_str(&format!("- {}: {}\n", dep.title, dep.summary));
    }
    push_section(&mut body, "Accepted dependency summaries", deps.trim_end());
    let mut board = String::new();
    if input.board.is_empty() {
        board.push_str("(none)");
    }
    for (key, value) in input.board {
        match value {
            Some(value) => board.push_str(&format!("{key}={value}\n")),
            None => board.push_str(&format!("{key}\n")),
        }
    }
    push_section(&mut body, "Board", board.trim_end());
    body.push_str(&format!("\nUnread messages: {}\n", input.unread));
    fit(&body, MAX_CONTEXT_DIGEST_BYTES)
}

/// Accepted reports of `after`, in dependency order. Missing reports are skipped.
#[must_use]
pub fn accepted_dependency_summaries<'a>(
    after: &[TaskId],
    tasks: &'a [Task],
    attempts: &'a [Attempt],
) -> Vec<DependencySummary<'a>> {
    let mut out = Vec::new();
    for id in after {
        let Some(task) = tasks.iter().find(|task| task.id == *id) else {
            continue;
        };
        if !matches!(
            task.status,
            super::TaskStatus::Accepted | super::TaskStatus::Integrated
        ) {
            continue;
        }
        let Some(summary) = attempts.iter().rev().find_map(|attempt| {
            if attempt.task_id != *id {
                return None;
            }
            attempt
                .report
                .as_ref()
                .map(|report| report.summary.as_str())
        }) else {
            continue;
        };
        out.push(DependencySummary {
            task_id: task.id,
            title: task.title.as_str(),
            summary,
        });
    }
    out
}

#[must_use]
pub fn attempt_id_of(attempts: &[Attempt], task: TaskId) -> Option<AttemptId> {
    attempts
        .iter()
        .filter(|attempt| attempt.task_id == task)
        .max_by_key(|attempt| attempt.n)
        .map(|attempt| attempt.id)
}

fn push_section(out: &mut String, title: &str, body: &str) {
    out.push_str("## ");
    out.push_str(title);
    out.push('\n');
    out.push_str(body);
    if !body.ends_with('\n') {
        out.push('\n');
    }
    out.push('\n');
}

fn fit(body: &str, max_bytes: usize) -> String {
    let (text, truncated) = clamp_text(body, max_bytes);
    if truncated && !text.contains("forgectl context") && max_bytes > 80 {
        let note = "\n[truncated] Full text: forgectl context\n";
        let (again, _) = clamp_text(&format!("{text}{note}"), max_bytes);
        again
    } else {
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{AgentProviderId, AttemptId, RunId, TaskId, WorkspaceId};
    use crate::orchestration::{
        AttemptPhase, AttemptReport, ReportOutcome, TaskStatus, WorkMode, MAX_PROMPT_BYTES,
    };
    use crate::Timestamp;

    fn ids() -> (RunId, TaskId) {
        (RunId::new(), TaskId::new())
    }

    #[test]
    fn worker_and_controller_text_include_the_required_sections() {
        let (run_id, task_id) = ids();
        let dep_id = TaskId::new();
        let deps = [DependencySummary {
            task_id: dep_id,
            title: "API",
            summary: "Added GET /export",
        }];
        let worker = compose_worker_prompt(&WorkerPromptInput {
            run_id,
            task_id,
            attempt_n: 2,
            title: "UI button",
            objective: "Ship export",
            spec: "Add the button",
            acceptance: "Button calls /export",
            brief: "Stream, do not paginate",
            brief_version: 3,
            workspace_path: "/tmp/wt",
            branch: "forge/task-ui",
            base_short: "abc1234",
            dependencies: &deps,
            feedback: Some("Name the button Export"),
            previous_report: Some("First attempt missed the label"),
        });
        assert!(worker.contains("Ship export"));
        assert!(worker.contains("Add the button"));
        assert!(worker.contains("Button calls /export"));
        assert!(worker.contains("Stream, do not paginate"));
        assert!(worker.contains("Run brief (v3)"));
        assert!(worker.contains("Added GET /export"));
        assert!(worker.contains("Accepted") || worker.contains("Inputs from finished tasks"));
        assert!(worker.contains("Name the button Export"));
        assert!(worker.contains(&task_id.to_string()));
        assert!(worker.contains("forgectl report"));

        let tasks = [TaskLine {
            task_id,
            title: "UI button",
            status: "ready",
            spec: "Add the button",
            acceptance: "Button calls /export",
        }];
        let reports = [ReportLine {
            task_id: dep_id,
            title: "API",
            outcome: "done",
            summary: "Added GET /export",
        }];
        let controller = compose_controller_prompt(&ControllerPromptInput {
            run_id,
            objective: "Ship export",
            brief: "Stream, do not paginate",
            brief_version: 3,
            base: "main",
            tasks: &tasks,
            reports: &reports,
            questions: &[("UI button", "Which icon?")],
            board_keys: &["decision/export-format"],
            resume: true,
        });
        assert!(controller.contains("Ship export"));
        assert!(controller.contains("Add the button"));
        assert!(controller.contains("Button calls /export"));
        assert!(controller.contains("Stream, do not paginate"));
        assert!(controller.contains("Added GET /export"));
        assert!(controller.contains("interrupted"));
        assert!(controller.contains("decision/export-format"));
        assert!(controller.contains("Which icon?"));
    }

    #[test]
    fn oversized_sections_are_marked_truncated() {
        let (run_id, task_id) = ids();
        let huge = "z".repeat(MAX_PROMPT_BYTES);
        let worker = compose_worker_prompt(&WorkerPromptInput {
            run_id,
            task_id,
            attempt_n: 1,
            title: "big",
            objective: "obj",
            spec: &huge,
            acceptance: "acc",
            brief: "brief text",
            brief_version: 1,
            workspace_path: "/tmp/wt",
            branch: "forge/task-big",
            base_short: "abc",
            dependencies: &[],
            feedback: None,
            previous_report: None,
        });
        assert!(worker.len() <= MAX_PROMPT_BYTES);
        assert!(worker.contains("[truncated]"));
        assert!(worker.contains("obj"));
    }

    #[test]
    fn context_digest_carries_current_brief_deps_and_unread() {
        let dep = TaskId::new();
        let digest = compose_context_digest(&ContextDigestInput {
            objective: "Ship export",
            brief: "v2 brief",
            brief_version: 2,
            spec: "Add the button",
            acceptance: "It posts",
            dependencies: &[DependencySummary {
                task_id: dep,
                title: "API",
                summary: "streaming",
            }],
            board: &[("decision/export-format", Some(r#"{"mode":"stream"}"#))],
            unread: 2,
        });
        assert!(digest.contains("Ship export"));
        assert!(digest.contains("v2 brief"));
        assert!(digest.contains("Add the button"));
        assert!(digest.contains("It posts"));
        assert!(digest.contains("streaming"));
        assert!(digest.contains("decision/export-format"));
        assert!(digest.contains("Unread messages: 2"));
    }

    #[test]
    fn accepted_dependency_summaries_follow_after() {
        let now = Timestamp::now();
        let dep_id = TaskId::new();
        let mut dep = Task {
            id: dep_id,
            run_id: RunId::new(),
            title: "API".into(),
            spec: "spec".into(),
            acceptance: "ok".into(),
            after: vec![],
            mode: WorkMode::Write,
            allow_subruns: false,
            status: TaskStatus::Accepted,
            attempts_used: 1,
            revision: 1,
            created_at: now,
            updated_at: now,
            feedback: None,
        };
        let attempt = Attempt {
            id: AttemptId::new(),
            task_id: dep_id,
            n: 1,
            session_id: None,
            workspace_id: Some(WorkspaceId::new()),
            branch: None,
            base_commit: None,
            provider_id: AgentProviderId::new("codex"),
            profile_id: None,
            read_only: false,
            phase: AttemptPhase::Reported,
            outcome: Some(ReportOutcome::Done),
            lost_reason: None,
            report: Some(AttemptReport {
                outcome: ReportOutcome::Done,
                summary: "streaming ndjson".into(),
                verification: None,
                result_path: None,
                reported_head: None,
                dirty_at_report: false,
                files_changed: vec![],
            }),
            integrated_commit: None,
            created_at: now,
            settled_at: None,
        };
        dep.status = TaskStatus::Review;
        let tasks = [dep.clone()];
        let attempts = [attempt.clone()];
        let none = accepted_dependency_summaries(&[dep_id], &tasks, &attempts);
        assert!(none.is_empty());
        dep.status = TaskStatus::Accepted;
        let tasks = [dep];
        let attempts = [attempt];
        let found = accepted_dependency_summaries(&[dep_id], &tasks, &attempts);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].summary, "streaming ndjson");
        assert_eq!(found[0].title, "API");
    }
}
