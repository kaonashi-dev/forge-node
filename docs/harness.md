# Subagent harness

How to use the **research → spec → human approval → implement → review →
close** cycle in this repository. Inspired by
[betta-tech/ejemplo-harness-subagentes](https://github.com/betta-tech/ejemplo-harness-subagentes)
and strengthened with patterns from
[humanlayer/12-factor-agents](https://github.com/humanlayer/12-factor-agents)
(unified event log, structured human gates, gate retry budget, context bundles).

## Pieces

| Piece | Function |
|-------|----------|
| `scripts/harness` | Human interface: status, list, show, doctor, watch, resume, timeline, event, from-issue, root, validate, gate |
| `harness/src/*.ts` | CLI, validator, event log (TypeScript on Bun) |
| `harness/features.json` | State machine + rules (`max_gate_attempts`, artefact requirements) |
| `harness/specs/<id>-<slug>/` | Approvable spec: requirements (EARS), design, tasks |
| `harness/progress/` | Session log, per-feature artefacts, **event log** |
| `harness/CHECKPOINTS.md` | Objective criteria the reviewer ticks (C1–C7) |
| `crates/harness-service` | Daemon-side reads and writes of the harness files |
| `crates/harness-service/src/transition.rs` | **The transition table**: the one place a feature's status is decided |
| `crates/daemon/src/harness_runner.rs` | Deterministic coordinator that starts jobs and advances on exit codes |
| `crates/daemon/src/jobs.rs` | Headless provider runs, streamed to job logs and UI events |
| `apps/tauri/src/harness` and feature views | Product UI for registration, gates, timelines and artefacts |
| `.claude/agents/` | `spec-author`, `implementer`, `reviewer`, `committer` |
| Skills | `/feature`, `/feature-go`, `/feature-status`, `/feature-commit`, `invariant-c4/c5/c7` |

## Per-feature artefacts

| File | When | Purpose |
|------|------|---------|
| `events_<id>.jsonl` | whole lifecycle | Append-only audit trail (12-factor unified state) |
| `gate_<id>.md` | `spec_ready`+ | Structured human gate (factor 7) |
| `context_<id>.md` | `in_progress`+ | Prefetched context for implementer (factor 3) |
| `explore_<id>_*.md` | research | Scoped findings |
| `impl_<id>.md` | implementation | R<n> → evidence map |
| `review_<id>.md` | review | APPROVED / CHANGES_REQUESTED |

## The cycle

```
/feature "<specification>"  (or: scripts/harness from-issue <n>)
        → register + explore + spec-author + gate_<id>.md
        → status: spec_ready
        → STOPS (you read the spec and gate_<id>.md)

/feature-go <id>
        → context_<id>.md → implementer (worktree) → reviewer
        → status: done | blocked
        → diff in working tree (harness does not commit)

/feature-commit <id>   (optional, human-invoked)
        → committer → one git commit (no push)
```

## Daemon path

The product path is the daemon coordinator, not an LLM deciding the state
machine. The GUI sends a request, the daemon starts a headless job, and the next
state is chosen only from the job's final state or a human gate decision.

```text
RegisterHarnessFeature
        → feature_registered, status pending
RunHarnessStep(Spec)
        → spec_started → job exits 0 → status spec_ready + human_gate_opened
HarnessAdvance(ApproveSpec)
        → human_gate_resolved → RunHarnessStep(Implement)
RunHarnessStep(Implement)
        → impl_started → job exits 0 → impl_done → RunHarnessStep(Review)
RunHarnessStep(Review)
        → review_started → job exits 0 → review_verdict
        → APPROVED: status done + feature_done
        → CHANGES_REQUESTED: bump review_rounds and run Implement again
        → failed: retry up to max_step_attempts, then status blocked
        → missing verdict: status blocked + feature_blocked
```

`JobUpdated` and `JobOutput` events are broadcast while the process runs. The
raw provider stream stays in the job log named by the harness event; the feature
row stores only the status and small summary fields.

### The transition table

Every status the daemon writes comes from one pure function,
`harness_service::transition`, and nothing else may write `status`. It takes the
row, the repository's `rules` and one trigger, and answers with the row it
should become, the events that justify it, and the step to start next — or a
refusal. That refusal is what makes the human gate real rather than
conventional: `RunHarnessStep{Implement}` on a `spec_ready` feature is
`InvalidRequest`, not an implementation without an approval.

Three properties follow, and each closes a defect the earlier code had:

- **Single flight.** A step is refused with `Conflict` while that feature
  already has a queued or running job. The concurrency ceiling is global and
  never stopped two agents landing in one worktree.
- **Idempotent decisions.** Every write bumps the row's `revision`, and
  `HarnessAdvance` carries the revision the client saw. A replayed *Approve*
  answers with the current row instead of resolving a second gate.
- **A retry budget, not a dead end.** A job that fails is a failed *attempt*;
  `max_step_attempts` decides whether the step runs again or the feature blocks,
  and the reason is in the timeline either way.

### Attempts

Each run of each step is written to the row as an attempt:

```json
{ "step": "Implement", "n": 2, "job": "01a0…", "provider": "codex",
  "transport": "cli", "started_at": "…", "settled_at": "…", "outcome": "failed" }
```

A retry is a *new* attempt rather than a mutation of the previous one, which is
what makes the trail readable and restart recovery possible: on boot the daemon
settles every attempt that has no process behind it as
`Failed(daemon restarted)` and puts it through the same budget.

### Liveness

`follow_job` writes `last_line` and `last_output_at` onto the job row while the
process runs, throttled, so the panel, the sidebar and the watchdog all read one
fact. A watchdog thread then warns at `stale_after_minutes` (event `step_stale`;
it never kills — a long implementation legitimately thinks for minutes) and
cancels at `max_step_minutes`, as a *failure*, so the cancelled step reaches the
retry budget like any other.

## One harness, several worktrees

A step runs in a worktree, and that worktree carries its own frozen copy of
`harness/` from the commit it was made at. Only the copy in the checkout that
owns the state — `scripts/harness root`, exported to every job as
`FORGE_HARNESS_ROOT` — is the real one; anything written beside the agent is
lost. That is why `daemon::harness_runner::step_prompt` spells every spec,
gate, progress and event path **absolutely** from that root. A step that used
relative paths finished exit 0, advanced the feature to `spec_ready`, and left
`scripts/harness validate` staring at a status with no artefacts under it.

Code, tests and the gate command are the other half of the rule: those belong
in the agent's own working tree, which is the checkout being changed.

## Human interface

```bash
scripts/harness status
scripts/harness list
scripts/harness show <id>       # JSON + artefact map
scripts/harness doctor [id]     # orchestrator link + events (no Forge UI)
scripts/harness watch [id]      # poll doctor every 2s
scripts/harness resume <id>     # where it left off
scripts/harness timeline <id>   # events_<id>.jsonl
scripts/harness event <id> <type> [--data '{...}']
scripts/harness from-issue <n>  # register from GitHub issue (needs gh)
scripts/harness next
scripts/harness active
scripts/harness root             # checkout that owns harness/ (worktree-safe)
scripts/harness validate
scripts/harness gate [--fast]
```

Tests: `cd harness && bun test` (not part of `scripts/dev check`).

## Rules (`features.json`)

| Rule | Default | Meaning |
|------|---------|---------|
| `require_human_spec_approval` | true | The gate the machine may not skip. `false` sends a written spec straight to the implementer |
| `require_green_gate_to_close` | true | A feature over `max_gate_attempts` may not reach `done` |
| `max_review_rounds` | 2 | Review loop cap |
| `max_gate_attempts` | 3 | Implementer `scripts/dev check` failures before auto-block |
| `max_step_attempts` | 2 | Restarts of a step whose job failed, before the feature blocks |
| `max_step_minutes` | 90 | Wall clock for one attempt; past it the watchdog cancels the job |
| `stale_after_minutes` | 10 | Silence after which an attempt is reported stale — a warning, never a kill |
| `require_gate_file` | true | `gate_<id>.md` required from `spec_ready` |
| `require_context_bundle` | true | `context_<id>.md` required from `in_progress` |

Every rule in this table is read by the code that enforces it. A rule nothing
reads is deleted.

## Statuses

`pending` → `spec_ready` → `in_progress` → `in_review` → `done`  
                                                      ↘ `blocked`

One active feature (`in_progress` / `in_review`) per checkout. `done` requires an
APPROVED review — read from the reviewer's structured answer when the provider
has a schema, and from the last line of `review_<id>.md` when it does not.

## What the harness does not do

- No `git push`.
- No `cargo add` / undeclared dependencies.
- Does not replace CI (`scripts/dev check`).
- Does not commit unless `/feature-commit` launches the `committer`.

See also: `docs/plan-subagent-harness.md` for design history and
[`plan-agnostic-orchestrator.md`](./plan-agnostic-orchestrator.md) for the next
orchestrator phases.
