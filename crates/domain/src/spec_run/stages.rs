//! The prompts Forge hands each spec-run agent once the gate before it has passed.

use super::brief::{coder_rules, Tools, REPORT_FORMAT};
use super::model::{CheckRecord, Route, MAX_REASON_BYTES};
use super::principles::{ARTIFACT_RULES, POWER_OF_THE_SIMPLE, PRINCIPLES, QUESTIONS};
use super::schema::{PLAN_SCHEMA, REPORT_SCHEMA};
use crate::orchestration::ToolSurface;
use crate::review::{findings_prompt, Finding, FindingReaction, ReviewStatus};

/// What every stage prompt of one run shares.
#[derive(Clone, Debug)]
pub struct StageContext<'a> {
    pub id: &'a str,
    pub name: &'a str,
    /// The run's directory, absolute.
    pub dir: &'a str,
    pub tools: &'a Tools,
}

impl StageContext<'_> {
    fn path(&self, file: &str) -> String {
        format!("{}/{file}", self.dir)
    }

    fn save(&self, kind: &str) -> String {
        match self.tools.surface {
            ToolSurface::Mcp => format!(
                "`spec_save` (run {}, kind {kind}, file: the path you wrote)",
                self.id
            ),
            ToolSurface::Cli => format!(
                "`{} --json spec save {} --kind {kind} --file <{kind} file>`",
                self.tools.ctl, self.id
            ),
        }
    }

    fn report(&self) -> String {
        match self.tools.surface {
            ToolSurface::Mcp => format!(
                "`spec_report` (run {}, file: the report you wrote, artifact_url when you published one)",
                self.id
            ),
            ToolSurface::Cli => format!(
                "`{} --json spec report {} --file <report file> [--artifact-url <url>]`",
                self.tools.ctl, self.id
            ),
        }
    }

    fn round(&self) -> String {
        match self.tools.surface {
            ToolSurface::Mcp => format!(
                "`spec_round` (run {}, file: your numbered findings), or with accept true to accept \
the current task",
                self.id
            ),
            ToolSurface::Cli => format!(
                "`{ctl} --json spec round {id} --file <findings file>`, or `{ctl} --json spec round \
{id} --accept` to accept the current task",
                ctl = self.tools.ctl,
                id = self.id
            ),
        }
    }

    fn inbox(&self) -> String {
        self.tools.tool("inbox", "inbox")
    }

    fn answer(&self) -> String {
        match self.tools.surface {
            ToolSurface::Mcp => "`send_message` (to session:<coder id>)".to_owned(),
            ToolSurface::Cli => format!(
                "`{} --json send --to session:<coder id> '...'`",
                self.tools.ctl
            ),
        }
    }
}

const PLAN_BLOCK: &str = "\
```json
{\"schema\": \"forge.spec_plan/v1\",
 \"steps\": [{\"id\": \"S1\", \"criteria\": [\"AC-1\"], \"files\": [\"src/x.rs\"],
             \"check\": {\"command\": \"cargo test -p x name -- --exact\", \"expect\": \"exit 0\"}},
            {\"id\": \"S2\", \"criteria\": [\"AC-2\"], \"manual\": \"what the person does and sees\"}],
 \"gate\": {\"command\": \"scripts/dev check\"},
 \"tasks\": []}
```";

/// Stage 2, for the planner after G1.
#[must_use]
pub fn plan_stage_prompt(ctx: &StageContext, gate_action: Option<&str>, route: Route) -> String {
    let gate = match gate_action {
        Some(command) => format!(
            "the project's gate action, `{command}`. Forge runs it; leave `gate` out of the block."
        ),
        None => "propose one as `gate`, deduced from AGENTS.md, the Makefile or the CI, with the \
exact command. The person approves it at the plan gate."
            .to_owned(),
    };
    let tasks = if route == Route::Full {
        "\n- This is the full route: split the plan into `tasks`, one per capability, each with \
`id`, `title`, the `criteria` it covers and the earlier task ids it waits for in `after`. Forge \
runs them in order, each with its own coder rounds."
    } else {
        ""
    };
    format!(
        "You are the planner of spec run {id} ({name}) in Forge. You own the plan and the review; a \
coder Forge launches writes the code. Until the person approves your plan you only read: do not \
edit files and do not launch agents, Forge refuses it.

{POWER_OF_THE_SIMPLE}
{PRINCIPLES}
## 2. Research and plan

The approved spec is {spec}. Read all of it.
- Research first: facts with file:line, no proposals. Save it with {research}.
- For each function a criterion touches, ask the project's codebase graph who calls it and what it \
calls with {graph}, and note every caller with file:line: those are the places the change can break.
- Then the plan, in vertical slices: each step names the criteria (AC-n) it serves, the files it \
changes, its automatic check (exact command and expected result) and a manual check only when \
nothing can be automated.
- Before saving, build the table AC → step → check. Do not save while a criterion has no step or \
a step serves no criterion; Forge repeats that cross and refuses the plan otherwise.
- Write \"What we are not doing\".
- The repository gate is the last check: {gate}{tasks}
- {QUESTIONS}

The plan is Markdown for people and ends with this block, which Forge reads; the checks it names \
are what Forge runs after every coder report:

{PLAN_BLOCK}

Save the plan with {plan} (schema `{PLAN_SCHEMA}`), then stop. The person approves the plan, asks \
for changes or rejects it in Forge; their text reaches you as a prompt.
",
        id = ctx.id,
        name = ctx.name,
        spec = ctx.path("spec.md"),
        research = ctx.save("research"),
        graph = ctx.tools.graph(),
        plan = ctx.save("plan"),
    )
}

/// After G2: the coder runs and the planner only answers it.
#[must_use]
pub fn watch_stage_prompt(ctx: &StageContext, coder_session: &str, workspace: &str) -> String {
    format!(
        "Spec run {id}: the person approved the plan and Forge started the coder, session \
{coder_session}, in {workspace}. The approved spec is {spec} and the plan {plan}.

- When you see \"Forge: N new message(s)\", read them with {inbox} and answer what the spec and \
the plan answer with {answer}. Take anything else to the person with your question tool.
- Never approve a permission, a command or a destructive action for the coder.
- Do not edit its files. Forge runs the plan's checks after its report and then sends you the \
review.
",
        id = ctx.id,
        inbox = ctx.inbox(),
        answer = ctx.answer(),
        spec = ctx.path("spec.md"),
        plan = ctx.path("plan.md"),
    )
}

#[derive(Clone, Debug)]
pub struct ReviewInput<'a> {
    pub ctx: &'a StageContext<'a>,
    pub round: u32,
    pub rounds: u32,
    pub coder_session: &'a str,
    pub workspace: &'a str,
    pub base_commit: &'a str,
    /// Already modified when the coder started; not part of its change.
    pub dirty: &'a [String],
    pub report_summary: &'a str,
    pub report_file: Option<&'a str>,
    pub checks: &'a [CheckRecord],
    /// Criteria whose check failed this round and the one before.
    pub repeated: &'a [String],
    pub deviations: &'a [String],
    pub publish_report: bool,
    /// `None` outside the full route; otherwise the task and whether another follows.
    pub task: Option<(&'a str, bool)>,
}

fn check_line(check: &CheckRecord) -> String {
    let result = match (&check.not_verified, check.timed_out, check.exit_code) {
        (Some(reason), _, _) => format!("not run ({reason})"),
        (None, true, _) => "timed out".to_owned(),
        (None, false, Some(code)) => format!("exit {code}"),
        (None, false, None) => "no exit code".to_owned(),
    };
    let sandbox = check
        .not_sandboxed
        .as_ref()
        .map(|why| format!(" (not sandboxed: {why})"))
        .unwrap_or_default();
    format!(
        "- {}: `{}` → {result}{sandbox}; log {}",
        check.criterion, check.command, check.log
    )
}

/// Stages 5 and 6, after Forge ran the checks of a coder report.
#[must_use]
pub fn review_stage_prompt(input: &ReviewInput) -> String {
    let ctx = input.ctx;
    let checks = if input.checks.is_empty() {
        "None".to_owned()
    } else {
        input
            .checks
            .iter()
            .map(check_line)
            .collect::<Vec<_>>()
            .join("\n")
    };
    let dirty = if input.dirty.is_empty() {
        String::new()
    } else {
        format!(
            " These paths were already modified before the coder started and are not its change: \
{}.",
            input.dirty.join(", ")
        )
    };
    let repeated = if input.repeated.is_empty() {
        String::new()
    } else {
        format!(
            "\nThe check of {} failed this round and the one before: state the premise both \
rounds share in one sentence and take it to the person before anything else; one option is to fix \
the plan and relaunch instead of patching the diff.\n",
            input.repeated.join(", ")
        )
    };
    let deviations = if input.deviations.is_empty() {
        String::new()
    } else {
        format!(
            "\nForge found deviations from the delivery chosen at the plan gate; report them: {}.\n",
            input.deviations.join("; ")
        )
    };
    let report_file = input
        .report_file
        .map(|file| format!(" Its full report is {file}."))
        .unwrap_or_default();
    let next = match input.task {
        Some((title, true)) => format!(
            "This round covered task \"{title}\"; another task follows. When this task needs no \
more changes, accept it with {} and Forge starts the next one.",
            ctx.round()
        ),
        Some((title, false)) => format!("This round covered the last task, \"{title}\"."),
        None => String::new(),
    };
    let publish = if input.publish_report {
        format!(
            "\nThe person asked for the report as a Claude artifact as well.\n\n{ARTIFACT_RULES}"
        )
    } else {
        String::new()
    };
    let rounds_left = input.rounds.saturating_sub(input.round);
    format!(
        "Spec run {id}: the coder (session {coder}) reported round {round} of {rounds}, and Forge \
ran the plan's checks in {workspace}.

## 5. Review

The coder's summary is a hint, never evidence: \"{summary}\".{report_file}
Checks Forge ran this round:
{checks}

Read the spec {spec} and the plan {plan}, and the diff with `git -C {workspace} diff {base}` plus \
its untracked files.{dirty}
- First look for missing criteria and scope drift, then for quality.
- Every finding cites file:line and an excerpt. A finding you cannot cite is not a finding.
- Before you classify, read the project's learned rules and false positives with {kb}; do not \
raise what it marks as a false positive.
- Classify each one: patch (a clear fix, it goes to the coder), defer (only when the problem \
existed before this change; a problem the change caused is never deferred) or decision (it goes to \
the person with options).
{repeated}{deviations}
## Next

{next}
- Patches and {rounds_left} round(s) left: send numbered findings with {round_tool}. Forge starts \
the next coder round with them.
- Nothing left to patch, or no round left: write the report and save it once with {report_tool}.

{REPORT_FORMAT}
It ends with this block, which Forge checks against the approved spec, the diff and this round's \
checks: a met needs a check of this round that passed.

```json
{{\"schema\": \"{REPORT_SCHEMA}\",
 \"criteria\": [{{\"id\": \"AC-1\", \"state\": \"met\", \"paths\": [\"src/x.rs:12\"],
                \"evidence\": {{\"command\": \"cargo test -p x name -- --exact\", \"exit_code\": 0, \"excerpt\": \"1 passed\"}}}},
               {{\"id\": \"AC-2\", \"state\": \"not_verified\", \"reason\": \"why, and the manual check\"}}],
 \"gate\": {{\"id\": \"gate\", \"state\": \"met\", \"evidence\": {{\"command\": \"scripts/dev check\", \"exit_code\": 0, \"excerpt\": \"ok\"}}}},
 \"changed\": [\"src/x.rs:12\"], \"deferred\": []}}
```
{publish}",
        id = ctx.id,
        coder = input.coder_session,
        round = input.round,
        rounds = input.rounds,
        workspace = input.workspace,
        summary = input.report_summary.trim(),
        spec = ctx.path("spec.md"),
        plan = ctx.path("plan.md"),
        base = input.base_commit,
        kb = ctx.tools.kb(),
        round_tool = ctx.round(),
        report_tool = ctx.report(),
    )
}

#[derive(Clone, Debug)]
pub struct CoderInput<'a> {
    pub ctx: &'a StageContext<'a>,
    pub round: u32,
    pub task: &'a str,
    pub criteria: &'a [String],
    /// Commits on its branch (delivery "commit"), otherwise none.
    pub commit: bool,
    pub findings: Option<&'a str>,
}

/// Stage 3, the coder's launch prompt for one round.
#[must_use]
pub fn coder_prompt(input: &CoderInput) -> String {
    let ctx = input.ctx;
    let report = match ctx.tools.surface {
        ToolSurface::Mcp => "When done, or blocked, call `report` exactly once (outcome done, \
failed or blocked; summary: your report; result_file: a file with the full report), then stop. \
Questions go to the planner with `ask` and wait true."
            .to_owned(),
        ToolSurface::Cli => format!(
            "When done, or blocked, run exactly once `{ctl} report --done|--failed|--blocked \
--summary \"<short report>\" --result-file <report file>`, then stop. Questions go to the planner \
with `{ctl} ask --wait \"<question>\"`.",
            ctl = ctx.tools.ctl
        ),
    };
    let findings = input
        .findings
        .map(|text| {
            format!(
                "\n## Findings from the planner's review\n\nFix these first; the rest of your \
earlier work stays as it is in this checkout.\n\n{text}\n"
            )
        })
        .unwrap_or_default();
    format!(
        "You are the coder of spec run {id} ({name}), round {round}. Your task: {task}, covering \
{criteria}.

Read the approved spec {spec} and the plan {plan} completely before you change anything. Forge \
runs the plan's checks after your report, so a claim without them is only a hint.
{findings}
{rules}",
        id = ctx.id,
        name = ctx.name,
        round = input.round,
        task = input.task,
        criteria = input.criteria.join(", "),
        spec = ctx.path("spec.md"),
        plan = ctx.path("plan.md"),
        rules = coder_rules(&report, input.commit),
    )
}

/// A gate decision's text as it reaches the agent, verbatim.
#[must_use]
pub fn reason_prompt(id: &str, what: &str, reason: &str, then: &str) -> String {
    format!("Forge, spec run {id}: {what}.\n\n{reason}\n\n{then}")
}

/// Kept free in the reason for the note on what was left out.
const LEFT_OUT_NOTE_BYTES: usize = 160;

/// A completed Forge Review's standing findings as the text of another round, within a gate
/// reason's cap, and how many were left out; `None` while there is nothing to send.
#[must_use]
pub fn review_round_text(status: ReviewStatus, findings: &[Finding]) -> Option<(String, usize)> {
    if status != ReviewStatus::Completed {
        return None;
    }
    let standing: Vec<&Finding> = findings
        .iter()
        .filter(|finding| finding.reaction != Some(FindingReaction::Dismiss))
        .collect();
    if standing.is_empty() {
        return None;
    }
    let (mut text, left_out) = findings_prompt(&standing, MAX_REASON_BYTES - LEFT_OUT_NOTE_BYTES);
    if left_out > 0 {
        text.push_str(&format!(
            "{left_out} more of Forge Review's findings did not fit here and were left out.\n"
        ));
    }
    Some((text.trim_end().to_owned(), left_out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec_run::MAX_STAGE_INSTRUCTIONS;

    fn ctx(tools: &Tools) -> StageContext<'_> {
        StageContext {
            id: "r1",
            name: "cache",
            dir: "/home/me/.forge/spec-runs/r1",
            tools,
        }
    }

    fn count(text: &str) -> usize {
        text.lines()
            .filter(|l| {
                let t = l.trim_start();
                t.starts_with("- ")
                    || t.split_once(". ").is_some_and(|(n, _)| {
                        !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())
                    })
            })
            .count()
    }

    #[test]
    fn stage_prompts_name_their_tools_by_surface_and_stay_short() {
        for surface in [ToolSurface::Mcp, ToolSurface::Cli] {
            let tools = Tools::new(surface, std::path::Path::new("/bin/forgectl"));
            let ctx = ctx(&tools);
            let unsandboxed = CheckRecord {
                criterion: "AC-1".into(),
                task: 0,
                round: 1,
                command: "test -f done.txt".into(),
                exit_code: Some(0),
                timed_out: false,
                not_verified: None,
                not_sandboxed: Some("Forge sandboxes checks only on macOS".into()),
                duration_ms: 1,
                log: "checks/AC-1-t1-r1-1.log".into(),
            };
            let plan = plan_stage_prompt(&ctx, None, Route::Full);
            let review = review_stage_prompt(&ReviewInput {
                ctx: &ctx,
                round: 1,
                rounds: 3,
                coder_session: "c1",
                workspace: "/w",
                base_commit: "abc",
                dirty: &["notes.md".into()],
                report_summary: "done",
                report_file: None,
                checks: std::slice::from_ref(&unsandboxed),
                repeated: &["AC-1".into()],
                deviations: &[],
                publish_report: false,
                task: Some(("cache", true)),
            });
            let coder = coder_prompt(&CoderInput {
                ctx: &ctx,
                round: 2,
                task: "cache",
                criteria: &["AC-1".into()],
                commit: false,
                findings: Some("1. src/x.rs:3 off by one"),
            });
            for text in [&plan, &review, &coder] {
                assert!(
                    text.contains(POWER_OF_THE_SIMPLE) || text == &review,
                    "{text}"
                );
                for section in text.split("\n## ") {
                    assert!(count(section) < MAX_STAGE_INSTRUCTIONS, "{section}");
                }
            }
            match surface {
                ToolSurface::Mcp => {
                    assert!(plan.contains("`spec_save` (run r1, kind plan"));
                    assert!(review.contains("`spec_round` (run r1"));
                    assert!(review.contains("`spec_report` (run r1"));
                    assert!(coder.contains("call `report` exactly once"));
                    assert!(!coder.contains("forgectl"));
                    assert!(plan.contains("`review_graph_query` (query callers, callees"));
                    assert!(review.contains("`review_kb_get`"));
                    assert!(!plan.contains("forgectl") && !review.contains("forgectl"));
                }
                ToolSurface::Cli => {
                    assert!(plan.contains(
                        "`/bin/forgectl --json review graph query --callers|--callees|--similar|--imports <function>`"
                    ));
                    assert!(review.contains("`/bin/forgectl --json review kb show`"));
                    assert!(plan.contains("/bin/forgectl --json spec save r1 --kind plan"));
                    assert!(review.contains("spec round r1 --accept"));
                    assert!(coder.contains("/bin/forgectl report --done|--failed|--blocked"));
                }
            }
            assert!(plan.contains("note every caller with file:line"));
            assert!(review.contains("do not raise what it marks as a false positive"));
            assert!(review.contains("exit 0 (not sandboxed: Forge sandboxes checks only on macOS)"));
            assert!(plan.contains("split the plan into `tasks`"));
            assert!(review.contains("notes.md"));
            assert!(review.contains("failed this round and the one before"));
            assert!(coder.contains("Do not commit"));
            assert!(coder.contains("off by one"));
            assert!(!coder.contains("send parent"));
        }
    }

    fn finding(id: u32, title: &str, body: &str) -> Finding {
        Finding {
            id,
            fingerprint: format!("f{id}"),
            path: "src/loader.rs".into(),
            line: Some(12),
            end_line: None,
            severity: crate::review::Severity::P1,
            category: "bug".into(),
            rule_id: None,
            confidence: 0.9,
            title: title.into(),
            body: body.into(),
            evidence: "let x = 1;".into(),
            suggestion: Some("let x = 2;".into()),
            lineage: crate::review::Lineage::New,
            reaction: None,
            hypothesis: None,
            kb_ref: None,
            reachability: None,
        }
    }

    #[test]
    fn a_completed_reviews_standing_findings_become_another_round_within_the_reason_cap() {
        let mut dismissed = finding(2, "Not a problem", "b");
        dismissed.reaction = Some(FindingReaction::Dismiss);
        let findings = [
            finding(1, "Off by one", "The loop skips the last entry."),
            dismissed,
        ];
        assert_eq!(review_round_text(ReviewStatus::Running, &findings), None);
        assert_eq!(
            review_round_text(ReviewStatus::Completed, &findings[1..]),
            None
        );
        let (text, left_out) =
            review_round_text(ReviewStatus::Completed, &findings).expect("one standing");
        assert_eq!(left_out, 0);
        assert!(text.contains("src/loader.rs:12 — Off by one"), "{text}");
        assert!(text.contains("let x = 2;"), "{text}");
        assert!(!text.contains("Not a problem"), "{text}");

        let many: Vec<Finding> = (0..40)
            .map(|id| finding(id, "Unchecked index", &"x".repeat(200)))
            .collect();
        let (text, left_out) = review_round_text(ReviewStatus::Completed, &many).expect("many");
        assert!(left_out > 0);
        assert!(text.len() <= MAX_REASON_BYTES, "{}", text.len());
        assert!(
            text.ends_with(&format!(
                "{left_out} more of Forge Review's findings did not fit here and were left out."
            )),
            "{text}"
        );
    }
}
