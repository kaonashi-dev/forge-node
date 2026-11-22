//! The planner brief, the coder rules, the drafter prompt and the guide, composed from stage
//! sections that each stay under [`MAX_STAGE_INSTRUCTIONS`].

use std::sync::LazyLock;

use super::principles::{
    ARTIFACT_RULES, POWER_OF_THE_SIMPLE, PRINCIPLES, QUESTIONS, READINESS, RISKS, SPEC_BLOCK,
};
use super::{quote, source_section, SpecRun};
use crate::orchestration::ToolSurface;

/// An overloaded prompt silently skips its interactive steps; each stage stays below this many
/// bullet or numbered lines.
pub const MAX_STAGE_INSTRUCTIONS: usize = 40;

/// How a spec-run agent reaches Forge, as the `forge` MCP tools or as `forgectl` commands.
#[derive(Clone, Debug)]
pub struct Tools {
    pub surface: ToolSurface,
    /// Absolute: a session's `PATH` need not contain `forgectl`.
    pub ctl: String,
}

impl Tools {
    #[must_use]
    pub fn new(surface: ToolSurface, forgectl: &std::path::Path) -> Self {
        Self {
            surface,
            ctl: quote(&forgectl.to_string_lossy()),
        }
    }

    fn mcp(&self) -> bool {
        matches!(self.surface, ToolSurface::Mcp)
    }

    #[must_use]
    pub fn wait(&self, who: &str) -> String {
        if self.mcp() {
            format!("`wait_session` (session {who}, for idle,waiting,exit, timeout 600)")
        } else {
            format!(
                "`{} --json session wait {who} --for idle,waiting,exit --timeout 15m`",
                self.ctl
            )
        }
    }

    #[must_use]
    pub fn read(&self, who: &str) -> String {
        if self.mcp() {
            format!("`read_session` (session {who}, lines 80)")
        } else {
            format!("`{} --json session read {who} --lines 80`", self.ctl)
        }
    }

    /// A queued send of a file's contents (CLI) or of its text (MCP).
    #[must_use]
    pub fn send_queued(&self, who: &str, what: &str) -> String {
        if self.mcp() {
            format!("`send_prompt` (session {who}, queue true, text: the {what})")
        } else {
            format!(
                "`{} --json session send {who} --queue --file <{what} file>`",
                self.ctl
            )
        }
    }

    #[must_use]
    pub fn changes(&self, who: &str) -> String {
        if self.mcp() {
            format!("`session_changes` (session {who})")
        } else {
            format!("`{} --json session changes {who}`", self.ctl)
        }
    }

    #[must_use]
    pub fn show(&self, who: &str) -> String {
        if self.mcp() {
            format!("`show_session` (session {who})")
        } else {
            format!("`{} --json session show {who}`", self.ctl)
        }
    }

    #[must_use]
    pub fn graph(&self) -> String {
        if self.mcp() {
            "`review_graph_query` (query callers, callees, similar or imports; function: its name)"
                .to_owned()
        } else {
            format!(
                "`{} --json review graph query --callers|--callees|--similar|--imports <function>`",
                self.ctl
            )
        }
    }

    #[must_use]
    pub fn kb(&self) -> String {
        self.tool("review_kb_get", "review kb show")
    }

    #[must_use]
    pub fn tool(&self, mcp: &str, cli: &str) -> String {
        if self.mcp() {
            format!("`{mcp}`")
        } else {
            format!("`{} --json {cli}`", self.ctl)
        }
    }
}

/// Where the coder sends its report when it is not a ledger attempt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReportTo {
    /// The planner is the Forge session that launched the coder.
    Parent,
    /// No parent session: the coder writes the report to a file the planner reads on idle.
    File,
}

/// What the coder must hand back, the same for a parent, a file or a ledger report.
const CODER_REPORT: &str = "\
Your report, per criterion: its state (`done`, `done_with_concerns`, `needs_context` or `blocked`), \
what changed with file:line, and the exact command that proves it with its exit code and an excerpt \
of its output; then the commands you ran, deviations from the plan, and \"Noticed, not touched\". \
Write None for an empty list.";

/// The rules a coder follows, shared by the plan the planner writes and by Forge's own coder
/// prompt; `report` says how this coder reports.
#[must_use]
pub fn coder_rules(report: &str, commit: bool) -> String {
    let commit = if commit {
        "- Commit on this branch when a step passes. Do not push or open pull requests."
    } else {
        "- Do not commit, push or open pull requests."
    };
    format!(
        "{POWER_OF_THE_SIMPLE}
## Coder rules

- For each step write the test that fails for the expected reason first and keep that output. \
Then make the smallest change that passes it; the test uses the code the way its user does.
- If no honest test is possible, say so and describe the manual check instead.
- When the plan and the code disagree, stop and report blocked with Expected / Found / Why it \
matters / Proposal. Do not work around it.
- Questions go to the planner, never to the person.
{commit}
- {report}

{CODER_REPORT}
"
    )
}

fn coder_report_line(to: ReportTo, tools: &Tools) -> String {
    match to {
        ReportTo::Parent => format!(
            "When done, or when you need a decision, send your report to the planner with {}.",
            tools.send_queued("parent", "report")
        ),
        ReportTo::File => "When done, or when you need a decision, write your report to \
<report file> (the path the planner gives in this plan), then stop: the planner reads it when \
you are idle."
            .to_owned(),
    }
}

/// Where the resolved spec goes: only a spec saved through Forge carries the block it checks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpecFor {
    /// `forgectl spec brief`: the planner keeps the spec in its own session.
    Brief,
    /// A spec run's drafter, whose spec `spec_save` validates.
    Forge,
}

/// Stage 1 text, shared by a Run Spec planner and a Draft Spec drafter.
#[must_use]
pub fn resolve_spec_section(questions_with: &str, tools: &Tools, spec_for: SpecFor) -> String {
    let (readiness_in, ending) = match spec_for {
        SpecFor::Brief => ("a table", String::new()),
        SpecFor::Forge => (
            "the block's `readiness`",
            format!(" It ends with the block below, filled in.\n\n{SPEC_BLOCK}"),
        ),
    };
    format!(
        "## 1. Resolve the spec

1. Understand. Write your hypothesis in one sentence with your confidence (for example 70 %). \
Below 70 %, say what is missing. What the code answers is not asked.
2. Investigate without opinion. Describe what exists, with file:line, proposing nothing yet. Ask \
the codebase graph who calls each function the change would touch and what it calls ({graph}), \
and read the project's architecture notes and learned rules ({kb}). Send wide searches to \
subagents to protect your context.
3. Reproduce, if it is a bug. Find a command or test that shows the failure, then check whether the \
behaviour is intended (docs, comments, git blame, earlier issues). The verdict is bug, intended or \
unclear; in doubt it stays unclear. Record how the failure is reached: automatic (a command or \
test alone shows it), sequence (a person's steps in order) or adversarial (only crafted input \
does), with its trigger and the command.
4. Weigh the risks. {RISKS}
5. Ask what is left. {questions_with}
6. Write the resolved spec: every criterion gets a stable id (AC-1, AC-2…) and a validation, \
preferably a command with its expected result; a manual check only when nothing can be automated.

Before asking for approval, rate each category Clear, Partial or Missing in {readiness_in}: \
{READINESS}

The resolved spec has these sections:
- Problem. For a bug: actual → expected → the mechanism, with file:line, then the reproduction.
- Acceptance criteria, declarative EARS: `AC-n: WHEN <event> THE SYSTEM SHALL <behaviour>`, never \
\"might\", \"could\" or \"should consider\"; then `Automatic validation: <command> → <expected>` or \
`Manual validation: <steps>`.
- Risks: each family you weighed, with the criteria that cover it or why it is out of scope.
- Unchanged behaviour.
- Out of scope, under a heading that reads exactly `## Out of scope`. It excuses a feature left \
out, never a defect: a defect in what the change touches is a criterion.
- Boundaries (Always / Ask first / Never); Assumptions; Clarifications (Q → A).
- Context: where the idea came from. When the source names another product, the spec names each \
capability by what it does and that product's name appears only here.
- Code context: facts with file:line. Every path and document you cite must exist; open it first.

Its header names the route: short (at most 3 criteria, no protocol or schema change), standard, or \
full (several capabilities tested apart).{ending}

Ending without a spec is a valid result, not a failure: needs_details (only the reporter has the \
information), intended_behavior (say why it is not a bug), not_actionable, or too_big (propose how \
to split it). Each one is reported with its evidence.
",
        graph = tools.graph(),
        kb = tools.kb(),
    )
}

fn plan_section(gate: &str) -> String {
    format!(
        "## 2. Plan

Research first, then the plan. Research states facts with file:line and proposes nothing. The plan \
is a series of vertical slices; each step leaves something that works and is tested:
- the criteria (AC-n) it serves;
- the files it changes;
- its automatic check: the exact command and the expected result;
- a manual check only when nothing can be automated.

Then:
- Build the table AC → step → check. Do not go on while a criterion has no step or a step serves no \
criterion.
- Write \"What we are not doing\".
- End with the repository gate as the last, mandatory check: {gate}
"
    )
}

const DEFAULT_GATE: &str = "the command AGENTS.md, the Makefile or the CI names as the project's \
own check (for example `scripts/dev check`), written out exactly.";

fn review_section(rounds: u32) -> String {
    format!(
        "## 5. Review

The coder's summary is a hint, never evidence. Rerun every check yourself and read the diff \
against the spec, criterion by criterion.
- First look for missing criteria and scope drift, then for quality.
- Every finding cites file:line and an excerpt. A finding you cannot cite is not a finding.
- Classify each one: patch (a clear fix, it goes to the coder), defer (only when the problem \
existed before this change; a problem the change caused is never deferred) or decision (it goes to \
the person with options).
- If the same check fails two rounds in a row, state the premise both rounds share in one \
sentence and take it to the person. One option is to fix the plan and relaunch instead of \
patching the diff.
- Stop after {rounds} review round(s) and hand what remains to the person.
"
    )
}

/// The persisted spec-run report: people read the top, `forge.spec_run/v1` closes it.
pub const REPORT_FORMAT: &str = "\
## Report

Each criterion ends in exactly one state, and the evidence decides it, not you:
- met: the agreed validation ran and gave the expected result (command, exit code and an excerpt; \
or file:line for a text change);
- not_met: it ran and failed, or the diff contradicts the criterion (command and the failing excerpt);
- not_verified: it could not run (why, and the manual check you propose);
- needs_decision: only the person can decide (the question, its options, your recommendation).

The report is Markdown: `# <name> · <state>` and one or two sentences; then the sections What was \
done (three to seven lines with file:line), Criteria (a table: ID, criterion, state, how it was \
proven, result, with a `gate` row), Tests added (say whether each failed without the change), \
Deviations from the plan, Noticed, not touched, Assumptions, Review (rounds and findings) and Next \
action. An empty section says None; it is never left out. What needs the person goes before the \
detail. The overall state is done when every criterion is met and the gate passed, \
done_with_concerns with any not_verified or deferred item, otherwise needs_decision or blocked.
";

#[must_use]
pub fn compose_planner_prompt(run: &SpecRun) -> String {
    let tools = Tools::new(run.surface, &run.forgectl);
    let report_to = if run.planner_is_session {
        ReportTo::Parent
    } else {
        ReportTo::File
    };
    let spawn = spawn_line(run, &tools);
    let place = if run.worktree.is_some() {
        "The coder works in the new worktree the result reports as `workspace.path`; review it there."
    } else {
        "The coder works in your checkout: do not edit files while it runs."
    };
    let rules = coder_rules(&coder_report_line(report_to, &tools), false);
    let follow = match report_to {
        ReportTo::Parent => {
            "The coder's report reaches you as a queued prompt once you are idle.".to_owned()
        }
        ReportTo::File => {
            "You are not a Forge session, so the coder has no parent to report to: name a report \
file in the plan and read it when the coder is idle."
                .to_owned()
        }
    };
    let questions =
        format!("{QUESTIONS} Launch nothing until every criterion has an id and a validation.");
    format!(
        "You are the planner of a spec run in Forge. You own the spec, the plan and the review; \
another agent session, the coder, writes the code. Do not write or edit the implementation yourself.

{POWER_OF_THE_SIMPLE}
{PRINCIPLES}
## The spec

{source}

{resolve}
{plan}
Put these rules in the plan, word for word, for the coder:

<<<CODER RULES
{rules}CODER RULES>>>

Keep the plan under 60 KiB and save it outside the checkout, for example in your temporary directory.

## 3. Launch the coder

{spawn}

Keep the returned `session_id` and use it, never the title. {place} {follow}

## 4. Wait

Wait with {wait}. A timeout is not proof the coder is busy: some agents never report their \
activity, so read {read} and treat a coder that awaits input as idle.
- waiting: read {read}. Answer what the spec or the plan answers with {send}; take anything else \
to the person. Never approve a permission, a command or a destructive action for it.
- exit: the coder ended. Tell the person; relaunch only if they ask.
- idle: review.

Send to the coder only queued: a queued prompt is submitted when the coder is idle. The queue \
holds one prompt, so a send refused with Conflict means the coder has not consumed the previous \
one yet: wait for it to go idle again, then send once more. Never switch to typing the prompt in.

{review}
Read {changes} and the git diff from its base_commit ({show}). Send numbered findings with {send}, \
at most 8 KiB (for more, a short prompt naming a file), then wait again.

{report}
{artifact}
Finish with the report. Do not commit, push, merge or close the coder's session unless the person \
asks.
",
        source = source_section(&run.source),
        resolve = resolve_spec_section(&questions, &tools, SpecFor::Brief),
        plan = plan_section(DEFAULT_GATE),
        wait = tools.wait("<coder id>"),
        read = tools.read("<coder id>"),
        send = tools.send_queued("<coder id>", "review"),
        changes = tools.changes("<coder id>"),
        show = tools.show("<coder id>"),
        review = review_section(run.rounds),
        report = REPORT_FORMAT,
        artifact = ARTIFACT_RULES,
    )
}

fn spawn_line(run: &SpecRun, tools: &Tools) -> String {
    let name = format!("{} coder", run.name);
    if tools.mcp() {
        let mut args = vec![
            format!("provider {}", run.coder.provider),
            "role executor".to_owned(),
        ];
        if let Some(profile) = &run.coder.profile {
            args.push(format!("profile {profile}"));
        }
        if let Some(effort) = &run.coder.effort {
            args.push(format!("effort {effort}"));
        }
        if let Some(model) = &run.coder.model {
            args.push(format!("model {model}"));
        }
        if run.planner_is_session {
            args.push("parent self".to_owned());
        }
        args.push(format!("name {}", quote(&name)));
        if let Some(branch) = &run.worktree {
            args.push(format!("worktree {branch}"));
            if let Some(base) = &run.base {
                args.push(format!("base {base}"));
            }
        }
        args.push("prompt: the plan".to_owned());
        return format!("Launch it once with `spawn_session` ({}).", args.join(", "));
    }
    let mut spawn = format!(
        "{} --json session spawn --provider {}",
        tools.ctl,
        quote(&run.coder.provider)
    );
    if let Some(profile) = &run.coder.profile {
        spawn.push_str(&format!(" --profile {}", quote(profile)));
    }
    spawn.push_str(" --role executor");
    if let Some(effort) = &run.coder.effort {
        spawn.push_str(&format!(" --effort {}", quote(effort)));
    }
    if let Some(model) = &run.coder.model {
        spawn.push_str(&format!(" --model {}", quote(model)));
    }
    if run.planner_is_session {
        spawn.push_str(" --parent self");
    }
    spawn.push_str(&format!(" --name {}", quote(&name)));
    if let Some(branch) = &run.worktree {
        spawn.push_str(&format!(" --worktree {}", quote(branch)));
        if let Some(base) = &run.base {
            spawn.push_str(&format!(" --base {}", quote(base)));
        }
    }
    spawn.push_str(" --prompt-file <plan file>");
    format!("Launch it once:\n\n{spawn}")
}

#[derive(Clone, Debug)]
pub struct DrafterInput<'a> {
    pub spec_run_id: &'a str,
    /// What the drafter starts from, as [`source_section`] or an issue's text.
    pub source: &'a str,
    pub tools: &'a Tools,
    pub publish_artifact: bool,
    /// Forge's scoring of the source, strongest first; a hint to weigh, not a verdict.
    pub suggested_risks: &'a [crate::HypothesisChoice],
}

fn suggested_risks_line(risks: &[crate::HypothesisChoice]) -> String {
    if risks.is_empty() {
        return String::new();
    }
    let named: Vec<String> = risks
        .iter()
        .map(|risk| format!("{} ({})", risk.hypothesis.name(), risk.reason))
        .collect();
    format!(
        "\nForge's reading of the source's words points at these risks: {}. Weigh them in step 4 \
with the others.\n",
        named.join("; ")
    )
}

#[must_use]
pub fn compose_drafter_prompt(input: &DrafterInput) -> String {
    let id = input.spec_run_id;
    let save = input.tools.tool(
        "spec_save",
        &format!("spec save {id} --kind spec --file <spec file>"),
    );
    let exit = input.tools.tool(
        "spec_save",
        &format!("spec save {id} --kind exit --file <exit file>"),
    );
    let mcp_args = if matches!(input.tools.surface, ToolSurface::Mcp) {
        format!(" (run {id}, kind spec or exit, file: the path you wrote)")
    } else {
        String::new()
    };
    let publish = if input.publish_artifact {
        format!(
            "\nThe person asked for the spec as a Claude artifact as well; publish it before you \
save and pass its URL as artifact_url.\n\n{ARTIFACT_RULES}"
        )
    } else {
        String::new()
    };
    format!(
        "You are the drafter of spec run {id} in Forge. You turn the source below into a spec a \
planner and a coder can follow without this conversation, or you end without one. You do not \
write or edit code and you do not commit: you read, search, run read-only commands and ask.

{POWER_OF_THE_SIMPLE}
{PRINCIPLES}
## The source

{source}
{suggested}
{resolve}
## Hand it over

- Write the spec to a file outside the checkout and save it with {save}{mcp_args}. Forge refuses \
a spec without at least one AC-n carrying a validation, with a repeated id, without the \
`forge.spec/v1` block, with a readiness category Missing, or a bug whose reproduction names no \
reachability; fix what it refuses and save again. It accepts with `warnings` a hedged criterion or \
a risk left uncovered: fix those too before you stop, or say in Assumptions why one stays.
- An early exit is saved with {exit}: the first line names the exit (needs_details, \
intended_behavior, not_actionable or too_big) and the rest is its evidence.
- Then stop. The person approves, asks for changes or rejects in Forge, never in this terminal. \
Their text reaches you as a prompt: write each answer into the section it affects and save again.
{publish}",
        source = input.source,
        suggested = suggested_risks_line(input.suggested_risks),
        resolve = resolve_spec_section(QUESTIONS, input.tools, SpecFor::Forge),
    )
}

static GUIDE: LazyLock<String> = LazyLock::new(|| {
    format!(
        "\
# forgectl guide spec

A spec run is a daemon object with six stages: draft the spec, plan, execute, verify, review, report. A person passes three gates in Forge; nothing typed into an agent's terminal counts.
Start from a spec: forgectl --json spec run <spec> [--preset claude|mix] [--coder <provider>] [--coder-profile <name|uuid>] [--planner-profile <name|uuid>] [--worktree <branch> | --checkout] [--delivery no-commit|commit|draft-pr] [--rounds 3]
Start from an idea or an issue: forgectl --json spec draft <text | #123 | issue link | file | artifact link> [same options]
The spec is a Claude artifact link, a Markdown file, the full text, or - for stdin. Forge never fetches a link; an issue is read with gh issue view.
- claude: Claude plans at high effort, Claude codes at medium. mix: Cursor codes; --coder opencode picks OpenCode.
1. A drafter (Claude, its own session, no code edits) writes the spec with AC-n criteria and saves it with spec save --kind spec, or ends early with --kind exit.
2. G1: forgectl spec gate <run> --approve | --changes <reason> | --reject <reason>. A change or a rejection always carries its reason, which reaches the agent verbatim.
3. A planner researches and saves research and a plan with a forge.spec_plan/v1 block: steps with checks, the gate. G2 approves it with --checkout|--worktree, --delivery, --rounds 1-5, --forge-review, --session-per-stage, --save-gate-action.
4. Forge starts the coder as a ledger attempt; one report settles each round. Forge then runs the plan's checks as Task sessions (10 min timeout, killed by process group) into checks/<AC>-t<task>-r<round>-<n>.log.
5. The planner reviews and sends findings with spec round --file, or accepts a task with spec round --accept.
6. The planner saves the report with spec report; a met needs a check of this round that passed. G3: --approve [--manual <AC>] [--next finish|draft-pr|another-round|discard] [--text].
See where runs wait: forgectl spec status [--attention]; read one: forgectl spec show <run>. Files live in /spec-runs/<id>/.
The short route (at most 3 criteria) folds G1 and G2 into one gate and keeps its plan block in the spec. The full route splits the plan into ledger tasks, one round loop each.
Be the planner yourself, without gates: forgectl spec brief <spec> [options] prints the brief to follow in this session.
Providers, effort levels and models are checked before anything launches; a path that does not exist exits 4.

{POWER_OF_THE_SIMPLE}"
    )
});

/// `forgectl guide spec`.
#[must_use]
pub fn guide() -> &'static str {
    &GUIDE
}

#[cfg(test)]
mod tests {
    use super::super::*;
    use super::*;

    fn run(coder: Coder, worktree: Option<&str>) -> SpecRun {
        SpecRun {
            source: SpecSource::Inline("Build it".into()),
            coder,
            forgectl: "/Applications/Forge Node.app/Contents/MacOS/forgectl".into(),
            name: "cache's spec".into(),
            worktree: worktree.map(str::to_owned),
            base: worktree.map(|_| "main".to_owned()),
            rounds: 2,
            planner_is_session: true,
            surface: ToolSurface::Cli,
        }
    }

    fn claude() -> Coder {
        Coder {
            provider: "claude".into(),
            profile: Some("0198a1b2-0000-7000-8000-000000000001".into()),
            effort: Some("medium".into()),
            model: None,
        }
    }

    /// Bullet and numbered lines of one `## ` section through the next `## ` heading.
    fn instructions(text: &str) -> Vec<(String, usize)> {
        let mut sections: Vec<(String, usize)> = Vec::new();
        for line in text.lines() {
            if let Some(title) = line.strip_prefix("## ") {
                sections.push((title.to_owned(), 0));
                continue;
            }
            let trimmed = line.trim_start();
            let numbered = trimmed
                .split_once(". ")
                .is_some_and(|(n, _)| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
            if trimmed.starts_with("- ") || numbered {
                if let Some(last) = sections.last_mut() {
                    last.1 += 1;
                }
            }
        }
        sections
    }

    #[test]
    fn the_brief_names_the_exact_coder_launch_and_the_review_loop() {
        let brief = compose_planner_prompt(&run(claude(), None));
        assert!(brief.contains(
            "'/Applications/Forge Node.app/Contents/MacOS/forgectl' --json session spawn \
             --provider claude --profile 0198a1b2-0000-7000-8000-000000000001 --role executor --effort medium --parent self \
             --name 'cache'\\''s spec coder' --prompt-file <plan file>"
        ));
        assert!(brief.contains("<<<SPEC\nBuild it\nSPEC>>>"));
        assert!(brief.contains("--for idle,waiting,exit"));
        assert!(brief.contains("session send parent --queue --file <report file>"));
        assert!(brief.contains("Stop after 2 review round(s)"));
        assert!(brief.contains("AskUserQuestion"));
        assert!(brief.contains("batches of at most 4"));
        assert!(brief.contains("Clear, Partial or Missing"));
        assert!(brief.contains("patch (a clear fix"));
        assert!(brief.contains("Expected / Found / Why it matters / Proposal"));
        assert!(brief.contains("Noticed, not touched"));
        assert!(brief.contains("not_verified"));
        assert!(brief.contains("do not edit files while it runs"));

        let mixed = compose_planner_prompt(&run(
            Coder {
                provider: "cursor".into(),
                profile: None,
                effort: Some("high".into()),
                model: Some("gpt-5".into()),
            },
            Some("spec/cache"),
        ));
        assert!(mixed.contains(
            "--provider cursor --role executor --effort high --model gpt-5 --parent self"
        ));
        assert!(mixed.contains("--worktree spec/cache --base main --prompt-file"));
        assert!(mixed.contains("workspace.path"));
    }

    #[test]
    fn every_stage_stays_under_forty_instructions() {
        let texts = [
            compose_planner_prompt(&run(claude(), None)),
            drafter_prompt_for_test(),
        ];
        for text in texts {
            for (title, count) in instructions(&text) {
                assert!(
                    count < MAX_STAGE_INSTRUCTIONS,
                    "{title} has {count} instructions"
                );
            }
        }
    }

    fn drafter_prompt_for_test() -> String {
        let tools = Tools::new(ToolSurface::Mcp, std::path::Path::new("/bin/forgectl"));
        compose_drafter_prompt(&DrafterInput {
            spec_run_id: "0198a1b2-0000-7000-8000-00000000abcd",
            source: "Make the loader faster.",
            tools: &tools,
            publish_artifact: true,
            suggested_risks: &[],
        })
    }

    #[test]
    fn the_power_of_the_simple_is_the_same_block_everywhere() {
        let texts = [
            ("drafter", drafter_prompt_for_test()),
            ("planner", compose_planner_prompt(&run(claude(), None))),
            ("coder", coder_rules("Report with `report`.", false)),
            ("guide", guide().to_owned()),
        ];
        for (who, text) in texts {
            assert!(text.contains(POWER_OF_THE_SIMPLE), "{who} lacks the block");
        }
        for step in [
            "1. Understand the problem.",
            "2. Choose the attack.",
            "3. Make only the changes that approach needs.",
            "Then it is a decision",
        ] {
            assert!(POWER_OF_THE_SIMPLE.contains(step), "{step}");
        }
    }

    #[test]
    fn the_drafter_saves_through_forge_and_never_writes_code() {
        let text = drafter_prompt_for_test();
        assert!(text.contains("`spec_save` (run 0198a1b2-0000-7000-8000-00000000abcd"));
        assert!(text.contains("do not write or edit code"));
        assert!(text.contains("needs_details, intended_behavior, not_actionable or too_big"));
        assert!(text.contains("artifact_url"));
        let cli = compose_drafter_prompt(&DrafterInput {
            spec_run_id: "r1",
            source: "x",
            tools: &Tools::new(ToolSurface::Cli, std::path::Path::new("/bin/forgectl")),
            publish_artifact: false,
            suggested_risks: &[],
        });
        assert!(cli.contains("`/bin/forgectl --json spec save r1 --kind spec --file <spec file>`"));
        assert!(!cli.contains("artifact_url"));

        assert!(text.contains("`review_graph_query` (query callers, callees"));
        assert!(text.contains("`review_kb_get`"));
        assert!(!text.contains("forgectl"), "{text}");
        assert!(cli.contains(
            "`/bin/forgectl --json review graph query --callers|--callees|--similar|--imports <function>`"
        ));
        assert!(cli.contains("`/bin/forgectl --json review kb show`"));
        assert!(!cli.contains("`review_graph_query`"));
    }

    #[test]
    fn the_drafter_is_told_what_forge_now_checks() {
        let text = drafter_prompt_for_test();
        for asked in [
            "actual → expected → the mechanism, with file:line",
            "automatic (a command or test alone shows it), sequence",
            "THE SYSTEM SHALL <behaviour>`, never \"might\", \"could\" or \"should consider\"",
            "a heading that reads exactly `## Out of scope`",
            "never a defect",
            "that product's name appears only here",
            "Every path and document you cite must exist",
            "\"schema\": \"forge.spec/v1\"",
            "UntrustedRepoInput",
            "with `warnings`",
        ] {
            assert!(text.contains(asked), "{asked}");
        }
    }

    #[test]
    fn suggested_risks_reach_the_drafter_with_their_reason() {
        let tools = Tools::new(ToolSurface::Mcp, std::path::Path::new("/bin/forgectl"));
        let risks = [crate::HypothesisChoice {
            hypothesis: crate::Hypothesis::MemoryDiskDrift,
            reason: "spec: cache ×2".into(),
        }];
        let text = compose_drafter_prompt(&DrafterInput {
            spec_run_id: "r1",
            source: "Keep the cache.",
            tools: &tools,
            publish_artifact: false,
            suggested_risks: &risks,
        });
        assert!(text.contains("points at these risks: MemoryDiskDrift (spec: cache ×2)."));
        assert!(!drafter_prompt_for_test().contains("points at these risks"));
    }

    #[test]
    fn only_a_spec_forge_saves_carries_the_block() {
        let brief = compose_planner_prompt(&run(claude(), None));
        assert!(!brief.contains("forge.spec/v1"), "{brief}");
        assert!(brief.contains("Clear, Partial or Missing in a table"));
        assert!(drafter_prompt_for_test().contains("Clear, Partial or Missing in the block's"));
    }

    #[test]
    fn the_risk_families_are_hypotheses_review_knows() {
        let named: Vec<&str> = RISKS
            .split(|c: char| !c.is_ascii_alphanumeric())
            .filter(|w| w.len() > 1 && w.chars().skip(1).any(|c| c.is_ascii_uppercase()))
            .collect();
        assert_eq!(named.len(), 9, "{named:?}");
        for name in named {
            assert!(crate::Hypothesis::from_name(name).is_some(), "{name}");
        }
    }

    #[test]
    fn the_block_template_is_a_valid_block_once_filled_in() {
        let filled = SPEC_BLOCK
            .replace("bug | feature | change", "bug")
            .replace("automatic | sequence | adversarial", "sequence");
        let block = super::super::draft::spec_block(&filled).expect("the template parses");
        assert_eq!(block.kind, super::super::draft::SpecKind::Bug);
        assert_eq!(block.risks.len(), 2);
    }

    #[test]
    fn without_a_parent_the_coder_reports_to_a_file() {
        let mut spec = run(claude(), None);
        spec.planner_is_session = false;
        let brief = compose_planner_prompt(&spec);
        assert!(!brief.contains("send parent"), "{brief}");
        assert!(!brief.contains("--parent self"));
        assert!(brief.contains("write your report to <report file>"));
    }

    #[test]
    fn a_queue_conflict_waits_for_the_coder_and_resends() {
        let brief = compose_planner_prompt(&run(claude(), None));
        assert!(brief.contains(
            "a send refused with Conflict means the coder has not consumed the previous one yet: \
wait for it to go idle again, then send once more"
        ));
    }

    #[test]
    fn the_brief_names_mcp_tools_or_cli_commands_by_surface() {
        let mut spec = run(claude(), Some("spec/cache"));
        spec.surface = ToolSurface::Mcp;
        let mcp = compose_planner_prompt(&spec);
        for tool in [
            "`spawn_session`",
            "`wait_session`",
            "`read_session`",
            "`send_prompt`",
            "`session_changes`",
            "`show_session`",
        ] {
            assert!(mcp.contains(tool), "{tool} in {mcp}");
        }
        assert!(!mcp.contains("--json session"), "{mcp}");
        assert!(mcp.contains("parent self"));
        assert!(mcp.contains("worktree spec/cache"));

        spec.surface = ToolSurface::Cli;
        let cli = compose_planner_prompt(&spec);
        assert!(cli.contains("--json session wait <coder id>"));
        assert!(cli.contains("--json session spawn"));
        assert!(!cli.contains("`wait_session`"));
    }

    #[test]
    fn an_artifact_is_read_with_the_artifact_tool_not_fetched() {
        let mut spec = run(
            Coder {
                provider: "opencode".into(),
                profile: None,
                effort: None,
                model: None,
            },
            None,
        );
        spec.source = SpecSource::ClaudeArtifact("https://claude.ai/public/artifacts/abc".into());
        spec.planner_is_session = false;
        let brief = compose_planner_prompt(&spec);
        assert!(brief.contains("read_artifact"));
        assert!(!brief.contains("--effort"));
        assert!(!brief.contains("--profile"));
    }
}
