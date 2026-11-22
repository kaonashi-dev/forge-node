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
verbatim. A rejection also ends a run outside a gate (a lost coder, a
restart), so no run is stuck at a stage nobody can leave. The caller is the
session `forgectl` declares from `FORGE_SESSION_ID`, the ledger's identity model:
it keeps an agent from passing its own gate by accident, not by force. Forge refuses an agent launch by the drafter or the planner before G2:
the coder is started by Forge, as a ledger attempt, when the plan is approved.

An issue is read with `gh issue view` on a worker thread: `StartSpecRun`
answers at once and the drafter starts when the issue has been read.

The drafter is a writable launch (so it has the forge MCP tools) whose prompt
forbids editing code; a read-only launch would have neither MCP nor shell
access to `spec_save`.

Routes: **short** (at most three criteria) keeps its `forge.spec_plan/v1`
block in the spec and folds G1 and G2 into one gate; **standard** is the table
above; **full** splits the plan into ledger tasks run in order, each with its
own rounds, and the gate runs after the last task.

## What Forge checks

- A spec needs at least one `AC-n`, each with an automatic or manual
  validation, and no repeated id. The spec and the plan are pinned by their SHA-256 when
  saved; one edited on disk afterwards stops the run instead of changing what
  Forge checks.
- A plan's `forge.spec_plan/v1` block names, per step, the criteria it serves
  and a check; every criterion needs a step. The gate is the project action
  marked `gate`, otherwise the plan's proposal; with neither the plan is
  refused. G2 can save an approved proposal as the gate action.
- After each coder report Forge runs the current task's checks (and the gate
  after the last task) in the coder's checkout as `Task` sessions under
  `/bin/sh`, each with a timeout (10 min by default, `timeout_secs` per check),
  its own process group killed on timeout, and only the last 16 KiB of output
  kept in `checks/<AC>-t<task>-r<round>-<n>.log` (`n` numbers the round's
  checks, since two steps may serve one criterion). Exit 127 is
  `not_verified`, not a failure. Each check runs under the sandbox below.
- A round is a new attempt of the same task in the same checkout, with the
  planner's findings; G2 caps rounds at the ledger's attempts per task. A G2
  approval that fails after creating the worktree keeps it on the run, and the
  next approval with a worktree placement starts in it.
- A report's `forge.spec_run/v1` block is refused for a `met` without
  evidence or with any check of this round for it that did not pass, before
  the last task's review on the full route, for a path outside the
  reviewed diff (base commit to now, minus what was already modified when the
  coder started in the person's checkout), an id the approved spec lacks, or a
  criterion left out. The overall state is derived from the criteria. A commit
  that contradicts the delivery chosen at G2 is appended as a deviation.
- A rejection, accepted in any open stage, cancels the coder's ledger tasks
  (killing a live attempt's session) and any running check; a check thread or
  a gate that finishes afterwards leaves the run rejected.
- `artifact_url` is optional and only `https` on `claude.ai` or `claude.site`.
- With delivery `draft_pr`, nothing opens before G3; G3's draft PR goes through
  the background PR agent with the report as the base of the body, and Forge
  appends the report's criteria table when the agent left it out.
- The Forge Review that follows the report runs one reviewer, whatever
  `[review] reviewers` says.

## The checks' sandbox

On macOS the check's `Task` session runs `/usr/bin/sandbox-exec -f
checks/<stem>.sb /bin/sh checks/<stem>.sh`; the profile is the one sandbox
checks use (`runtime_sandbox::profile`), written next to the script and removed
with it. A check:

- cannot read or write Forge's home or the socket's directory, and cannot
  connect to the socket even with the network on, so it cannot run
  `forgectl spec gate` and pass its own run's gate;
- writes only to the coder's checkout, the run's `checks/` directory, the temp
  directory and `[spec_runs] check_write_paths` (default `~/.cargo`,
  `~/.rustup`, `~/.bun`, `~/.npm`, `~/Library/Caches`; `~` is the user's home, a
  missing path is dropped, and a listed path inside Forge's home stays denied);
- keeps the network, because the person approved these commands at G2. G2's
  "Run checks without network" (`checks_offline`, `forgectl spec gate
  --checks-offline`) turns it off.

Off macOS, or without `/usr/bin/sandbox-exec`, a check runs unsandboxed: its log
says `not sandboxed: <reason>`, its record carries the reason in
`not_sandboxed`, and the planner's review prompt shows it beside the result.

## What the agents are told to use

The planner's research step asks the codebase graph who calls each function a
criterion touches and what it calls (MCP `review_graph_query`, CLI `forgectl
--json review graph query --callers <function>`), and records every caller with
file:line. Before it classifies findings, the review step reads the project's
learned rules and false positives (`review_kb_get`, `forgectl --json review kb
show`) and does not raise what they mark as a false positive.

## Forge Review into another round

When the run's Forge Review has completed with findings that are not
dismissed, G3 offers "Send Forge Review's findings to another round": it fills
the round's text with each finding's severity, file:line, title, body and
suggestion (`domain::spec_run::stages::review_round_text`) and picks Another
round. The text stays within the 4 KiB reason cap; findings that do not fit
are left out whole and the text says how many. `forgectl spec gate <run>
--approve --next another-round --from-review` composes the same text from
`GetReview`. The daemon sees an ordinary `AnotherRound`.

## Storage and budgets

`spec_runs` (migration 24) keeps one row of metadata per run; `source.md`,
`spec.md`, `research.md`, `plan.md`, `report.md`, the prompts Forge handed over
and the check logs live in `$FORGE_HOME/spec-runs/<id>/`, never in the
repository. Caps: 56 KiB per spec, 60 KiB per plan, 64 KiB per report, 16 KiB
per check log, 4 KiB per gate reason. Runs live behind their own `Mutex` on
`Daemon`; no git, file or session work happens under the core lock, and the
row is written on its own SQLite connection. The GUI reads a run's files with
`GetSpecRun` (ADR-012).
