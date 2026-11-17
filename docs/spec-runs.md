# Spec runs

A spec run takes an idea, an issue or a spec to a reviewed change with a
report per acceptance criterion. It is a daemon object with stages and gates;
the agents are ordinary sessions, and the coder is an attempt in the
orchestration ledger ([orchestration.md](./orchestration.md)).

Source: `crates/domain/src/spec_run/` (prompts, model, pure validation),
`crates/daemon/src/spec_runs.rs` (effects), `crates/forge-ctl/src/spec.rs`
(CLI), `apps/ui/src/spec_run.rs` (the dialog).

## Stages and gates

| Stage | Who | Ends with |
|---|---|---|
| `drafting` | the drafter, its own Claude session | `spec save --kind spec` (or `--kind exit`) |
| `spec_gate` (G1) | a person | approve, changes (reason), reject (reason) |
| `planning` | the planner | `spec save --kind research`, then `--kind plan` |
| `plan_gate` (G2) | a person | approve with the coder's autonomy, changes, reject |
| `executing` | the coder, a ledger attempt Forge starts | its one `report` |
| `verifying` | Forge | the plan's checks as `Task` sessions |
| `reviewing` | the planner | `spec round` (findings or accept) or `spec report` |
| `result_gate` (G3) | a person | finish, draft PR, another round, discard worktree |

A gate passes only through `SpecGate` (the GUI or `forgectl spec gate`); a
caller that is one of the run's own sessions is refused, and a change request
or a rejection without a reason is refused. The reason reaches the agent
verbatim. Forge refuses an agent launch by the drafter or the planner before G2:
the coder is started by Forge, as a ledger attempt, when the plan is approved.

The drafter is a writable launch (so it has the forge MCP tools) whose prompt
forbids editing code; a read-only launch would have neither MCP nor shell
access to `spec_save`.

Routes: **short** (at most three criteria) keeps its `forge.spec_plan/v1`
block in the spec and folds G1 and G2 into one gate; **standard** is the table
above; **full** splits the plan into ledger tasks run in order, each with its
own rounds, and the gate runs after the last task.

## What Forge checks

- A spec needs at least one `AC-n`, each with an automatic or manual
  validation, and no repeated id.
- A plan's `forge.spec_plan/v1` block names, per step, the criteria it serves
  and a check; every criterion needs a step. The gate is the project action
  marked `gate`, otherwise the plan's proposal; with neither the plan is
  refused. G2 can save an approved proposal as the gate action.
- After each coder report Forge runs the current task's checks (and the gate
  after the last task) in the coder's checkout as `Task` sessions under
  `/bin/sh`, each with a timeout (10 min by default, `timeout_secs` per check),
  its own process group killed on timeout, and only the last 16 KiB of output
  kept in `checks/<AC>-r<round>.log`. Exit 127 is `not_verified`, not a failure.
- A round is a new attempt of the same task in the same checkout, with the
  planner's findings; G2 caps rounds at the ledger's attempts per task.
- A report's `forge.spec_run/v1` block is refused for a `met` without
  evidence or without a check of this round that passed, a path outside the
  reviewed diff (base commit to now, minus what was already modified when the
  coder started in the person's checkout), an id the approved spec lacks, or a
  criterion left out. The overall state is derived from the criteria. A commit
  that contradicts the delivery chosen at G2 is appended as a deviation.
- `artifact_url` is optional and only `https` on `claude.ai` or `claude.site`.
- With delivery `draft_pr`, nothing opens before G3; G3's draft PR goes through
  the background PR agent with the report as the base of the body, and Forge
  appends the report's criteria table when the agent left it out.

## Storage and budgets

`spec_runs` (migration 24) keeps one row of metadata per run; `source.md`,
`spec.md`, `research.md`, `plan.md`, `report.md`, the prompts Forge handed over
and the check logs live in `$FORGE_HOME/spec-runs/<id>/`, never in the
repository. Caps: 56 KiB per spec, 60 KiB per plan, 64 KiB per report, 16 KiB
per check log, 4 KiB per gate reason. Runs live behind their own `Mutex` on
`Daemon`; no git, file or session work happens under the core lock, and the
row is written on its own SQLite connection. The GUI reads a run's files with
`GetSpecRun` (ADR-012).
