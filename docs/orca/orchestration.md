# Orca orchestration vs. the Forge harness

Analysis of the MIT-licensed Orca desktop app (extracted Electron bundle) against
Forge's subagent harness. Area: task dispatch, worker settlement, escalation,
hooks, gates.

Orca paths below are relative to the extracted bundle root (`out/`). No Orca code
is reproduced; behaviour is described and located. Forge paths are absolute-from-repo.

Read alongside [`../harness.md`](../harness.md) and `AGENTS.md` § Harness.

---

## 1. How Orca does it

### 1.1 The three nouns

Orca does **not** have one "job" concept. It has three, and the separation is the
single most important thing in the design.

| Noun | What it is | Table |
|------|-----------|-------|
| **Run** | A durable namespace + one coordinator inbox. It never schedules or places anything. | `runs` |
| **Task** | The work item. Carries `deps`, `parent_id`, `spec`, `result`. | `tasks`, `out/main/index.js:93239-93262` |
| **Dispatch** | *One attempt* at a Task by one terminal. Carries the capability, the assignee pane identity, `failure_count`, `last_heartbeat_at`. | `dispatch_contexts`, `out/main/index.js:93264-93295` |

A fourth row, `worker_dispatches` (`out/main/index.js:93091-93110`), exists only when
Orca itself *started* the worker process, and tracks the supervised-process state
machine (`starting → ready → succeeded | failed | stopped | abandoned`, plus the
two honest-unknown states `start_unknown` and `stop_unknown`).

Task status is `pending | ready | dispatched | completed | failed | blocked`
(CHECK constraint, `out/main/index.js:93248-93252`; mirrored client-side at
`out/cli/handlers/orchestration.js:53-60`).

The consequence of Task ≠ Dispatch: **a retry is a new row, not a mutated one.**
Three failed attempts on one Task are three `dispatch_contexts` rows with the same
`task_id`, and the failure count lives on the attempt lineage, not on the work item.

### 1.2 Dispatch: the contract is injected as text

`orchestration dispatch --inject` / `worker-start` writes a preamble into the
worker's agent CLI (`buildDispatchPreamble`, `out/main/index.js:95200-95276`). The
preamble is the entire worker protocol, stated as rules:

- send `worker_done` **exactly once**, with an explicit `--outcome succeeded|failed`,
  carrying **both** `taskId` and `dispatchId`, plus `filesModified` and an optional
  `reportPath`. "Never encode failure only in prose and never silently exit."
- send `heartbeat` every 5 minutes while working (`HEARTBEAT_INTERVAL_MIN = 5`,
  `out/main/index.js:95193`), also carrying both ids, "so a straggler heartbeat from a
  previously-failed dispatch cannot mask a hung retry."
- for a blocking question use `orchestration ask`, and — stated as
  "BEHAVIOR RULE #1 (MUST NOT VIOLATE)" — **never** `AskUserQuestion`, because a
  local TUI prompt is invisible to the coordinator and hangs the session forever.
- for a pre-completion blocker use `escalation`.
- after `worker_done`: end the turn, idle at the prompt, do not poll, do not close
  the terminal. A *direct user instruction* overrides this and starts ordinary
  user-owned work (`buildPostWorkerDoneInstructions`, `out/main/index.js:95277-95305`).

The preamble also carries a **base-drift section** when the worktree is behind its
base branch, listing the 5 most recent missing commit subjects and telling the
worker to rebase or escalate before starting (`out/main/index.js:95307-95319`). A
worktree more than 20 commits behind refuses dispatch entirely unless the spec
contains `allow-stale-base: true` (`out/main/index.js:95723-95733, 95762-95766`).

There is a `dispatch --dry-run --return-preamble` path so a coordinator can read
exactly what a worker will be told before sending it
(`out/cli/handlers/orchestration.js:747-770`).

### 1.3 How a worker reports done — and why Orca believes it

`worker_done` is a **message on a durable bus**, not an exit code. It goes through
three independent layers of proof.

**Layer 1 — payload validation.** `reconcileWorkerDoneMessage`
(`out/main/index.js:95475-95540`) rejects, with a typed code written back onto the
message row, any report missing a JSON object payload (`invalid_payload`), a
`taskId` (`missing_task_id`), a `dispatchId` (`missing_dispatch_id`), a valid
`outcome` (`invalid_outcome`), or referencing rows that do not exist or do not
match (`unknown_task`, `unknown_dispatch`, `task_dispatch_mismatch`).

**Layer 2 — sender authority.** `hasLifecycleAuthority`
(`out/main/index.js:95395-95398`) checks the *pane key* of the sender against the
dispatch's `assignee_pane_key`, falling back to the terminal handle only for
legacy rows. Terminal handles are explicitly "routing metadata rather than durable
identity". A `worker_done` from the right handle but the wrong pane is rejected
`sender_not_assignee`. Heartbeats are checked the same way
(`out/main/index.js:95435-95470`). The CLI refuses to even *send* a lifecycle
message when it cannot prove its own identity — an identity-less subprocess "must
fail closed rather than guess the worker" (`out/cli/handlers/orchestration.js:392-397`).

**Layer 3 — transactional settlement.** `settleWorkerReport`
(`out/main/index.js:90347-90420`) runs `BEGIN IMMEDIATE` and refuses:
- a **duplicate** (both rows already in the expected terminal state → returns
  `settled, duplicate: true`, which is *not* an error);
- an **inactive** dispatch (either row already settled);
- a **conflicting** report while another supervised Dispatch on the same Task is
  still live;
- a **stale** dispatch that is not the current one for the Task.

Only then does it write both the dispatch row and the task row in one transaction,
storing a structured envelope as `tasks.result`:
`{provenance: "worker_report", outcome, messageId, reportedBy, subject, body,
completedBy, filesModified[], reportPath, completedAt}`
(`out/main/index.js:95508-95519`). It then suppresses every earlier unread heartbeat
for that dispatch so the coordinator's next inbox read is not noise
(`out/main/index.js:95554-95562`).

**Layer 4 — client-side re-verification.** This is the part with no Forge analogue.
`requireWorkerDoneSettlement` (`out/cli/handlers/orchestration-worker-settlement.js`)
runs *after* the send returns. If the runtime did not return an explicit lifecycle
verdict, the CLI independently re-reads `dispatchShow` and `taskList` and refuses
to report success unless the dispatch and the task both reached the expected
status **and** `tasks.result` parses to a `worker_report` envelope whose
`messageId`/`reportedBy` matches the receipt it just got. Otherwise it throws
`operation_unknown`: *"The runtime accepted worker_done but did not confirm that
the exact report settled its Task and Dispatch."* The worker is told to retry from
the assigned worker after verifying its active Dispatch.

The principle: **an acknowledged write is not a settled state**, and the reporter
is responsible for proving settlement, not for assuming it.

### 1.4 How Orca knows finished vs. hung

Four channels, deliberately not collapsed into one:

1. **`worker_done`** — the only thing that means *finished*.
2. **`heartbeat`** → `dispatch_contexts.last_heartbeat_at`
   (`recordHeartbeat`, `out/main/index.js:90138-90140`).
3. **A stale sweep** — `getStaleDispatches` (`out/main/index.js:90141-90147`) selects
   dispatched rows whose `dispatched_at` and `last_heartbeat_at` are both older
   than `HUNG_THRESHOLD_MS = 600_000` (10 min, `out/main/index.js:95735`). Its only
   consumer, `warnStaleDispatches` (`out/main/index.js:95736-95743`), **logs a
   warning**. It does not kill, retry, or fail anything.
4. **Out-of-band provider hooks.** Orca installs hooks into the agent CLI's *own*
   config — for Claude: `SessionStart`, `UserPromptSubmit`, `Stop`, `StopFailure`,
   `SubagentStart`, `SubagentStop`, `TeammateIdle`, `PreToolUse`, `PostToolUse`,
   `PostToolUseFailure`, `PermissionRequest`
   (`out/main/chunks/managed-agent-hook-controls-RcsNtBpP.js:2176-2260`; Codex, Gemini
   and Antigravity get equivalents). The hooks POST a normalized status of
   `working | blocked | waiting | done` (`out/shared/agent-status-types.js:16`) with an
   `interactivePrompt` field carrying the full `AskUserQuestion` JSON. The header
   comment is explicit: status "comes from hooks — never inferred from terminal
   titles". A status row decays after 30 minutes
   (`AGENT_STATUS_STALE_AFTER_MS`, `out/shared/agent-status-types.js:56`).

Channel 4 is what actually answers "hung or thinking". `worker-show`
(`out/cli/handlers/orchestration.js:659-676`) surfaces it as a whole separate line,
with a comment explaining why it is not a status token: *"this is the one state
where the lane is healthy and still needs a person, so it must not read as another
status token. An absent field is unknown, which must not print the same as an
evaluated 'none'."* Three renderings: `Waiting on a human: <reason>`,
`Interactive wait: none`, `Interactive wait: unknown (not evaluated)`. The runtime
side (`inspectWorkerTerminal`, `out/main/index.js:99720-99752`) also distinguishes
`live` / `exited` / `identity_changed` / `unverifiable`.

The skill guide then forbids acting on liveness signals as if they were completion:
*"Treat a `check --wait` timeout or `{count:0}` as a checkpoint, not a worker
failure. Long coding tasks routinely run 15-60 minutes."* and *"Heartbeats and
visible terminal activity mean the worker is alive, not done."*

### 1.5 Escalation and blocking ask/reply

Two different things, deliberately:

**`ask`** is a blocking RPC that creates a durable `question` message in the
dispatch's Run and blocks until the coordinator replies. Default timeout 600 s,
clamped to a 1 800 s max, with a 5 s client grace so the RPC transport does not
abort before the server's own timeout resolves
(`out/shared/orchestration-ask-timeout.js:6-16`;
`out/cli/handlers/orchestration.js:771-839`). **A timeout leaves the question
pending.** The worker resumes it with `ask --resume <message_id>` — which is
idempotent and read-oriented — rather than asking again. The guide is blunt:
*"Never guess among multiple identical question threads."* On packaged Windows
there is even a two-step commit/resume protocol exiting with status `75` across a
launcher boundary.

**`escalation`** is fire-and-forget, for a pre-completion blocker the coordinator
must act on. `applyEscalationToDispatch` (`out/main/index.js:95644-95672`) verifies
the sender owns the Task, refuses while a supervised worker is still live
("Task N remains dispatched until supervised worker D stops or reports"), then
calls `failDispatch`.

**The circuit breaker.** `failDispatch` (`out/main/index.js:90148+`) does
`status = CASE WHEN failure_count + 1 >= 3 THEN 'circuit_broken' ELSE 'failed' END`.
On `circuit_broken` the Task itself is set to `failed` with reason
`Circuit broken: <subject>`. Three attempts, then stop.

### 1.6 Task DAG — real, and cheap

`tasks.deps` is a JSON array of task ids. Initial status is computed **in SQL** at
insert: `ready` if every declared dep exists in the same Run and is `completed`,
else `pending` (`out/main/index.js:93900-93934`). On completion,
`promoteReadyTasks(completedTaskId)` (`out/main/index.js:93993-94001`) walks pending
tasks that list the completed id and flips those whose deps are now all completed
to `ready`. `task-list --ready` is the coordinator's work queue.

Convergence is evaluated, not scheduled: `evaluateDagConvergence`
(`out/main/index.js:95673-95682`) returns `empty | all-done | active` and logs
*"Stuck: N tasks blocked with no active tasks. Resolve decision gates to continue."*

Guidance to coordinators: *"dispatch parallel waves, and avoid dependency chains
deeper than 3-4 steps."*

### 1.7 Decision gates — the human gate as a row

Gates are **coordinator-owned DAG decisions**, distinct from a worker's `ask`.
`decision_gates` (`out/main/index.js:93296-93310`) has
`status IN ('pending','resolved','timeout')`, a `question`, an `options` JSON array
and a `resolution`.

`createGate` (`out/main/index.js:89960-89986`):
1. refuses if a supervised worker is still live on the task
   (`task_not_startable` — "stop or settle its worker first");
2. inserts the gate;
3. settles every active dispatch for the task;
4. **sets the task status to `blocked`**.

`resolveGate` (`out/main/index.js:89987-90002`) records the resolution and sets the
task back to `ready`. The resolution text is then **injected into the next dispatch
preamble** as a `--- DECISION GATE RESOLVED ---` block
(`out/main/index.js:95780-95790`), so the worker that picks the task up reads the
human's decision as part of its task.

`reblockTasksWithPendingGates` (`out/main/index.js:95716-95722`) re-derives `blocked`
from pending gate rows on restart — the block is a *derived* fact, not a status
somebody remembered to set.

### 1.8 No scheduler

`coordinator-start`, `coordinator-stop`, `run`, `run-stop` are **retired**. They
perform no effects and return a migration error pointing at the current skill
(`out/cli/handlers/orchestration.js:862-867`). The old polling scheduler is still in
the bundle (`MAX_CONCURRENT_DEFAULT = 4`, `DEFAULT_POLL_MS = 2000`,
`out/main/index.js:95793-95795`) but is dead code behind those errors.

What replaced it: **the coordinator is an LLM in a terminal**, and the runtime is a
durable bus plus a state store. The coordinator loop is
`check --wait --types worker_done,escalation,question --timeout-ms 900000`,
process the whole batch, `--ack <delivery_id>`, wait again. Delivery is a FIFO
batch of ≤50 messages replayed verbatim until acknowledged
(`out/cli/handlers/orchestration.js:438-513`). Placement and concurrency are the
agent's decisions: *"Agents still choose placement and concurrency; Orca does not
schedule workers or infer conflicts."*

`check --wait` emits a JSON keepalive to **stderr** every 15 s with `elapsedMs` and
`deadlineMs` (`out/cli/handlers/orchestration.js:17-51`). The comment says why: 15 s
"is well under Claude Code's ~2 min Bash-tool silence budget", and stdout stays a
single payload so it remains pipeable.

### 1.9 Idempotent mutations

Every orchestration mutation may carry an `orchestrationRequestId`. When one fails
with an *ambiguous* transport code (`runtime_unavailable`, `remote_runtime_unavailable`,
`runtime_timeout`, `invalid_runtime_response`), `orchestrationMutationRecoveryError`
(`out/cli/orchestration-mutation-recovery.js:5-32`) **rewrites the error message**:
it strips the misleading "Retry the command." advice and substitutes *"The
orchestration mutation may already have taken effect; do not assume it failed.
Re-issue the same command with `--retry-request <id>` to recover idempotently. Do
not retry this mutation without `--retry-request`."*, plus `failedStage` and
`residualResources`. The mutation set is enumerated in
`out/shared/orchestration-rpc-contract.js:16-37`.

### 1.10 Worker terminal lifecycle — release is not cancel

`worker_terminal_resources` (`out/main/index.js:93115-93150`) has an ownership state
machine (`owned | transferred | user_owned | external | released`) and a separate
release state machine (`not_requested | retained | requested | releasing | released
| unknown`). After a settled `worker_done` the coordinator must **account for the
terminal**: reuse it for a follow-up Dispatch (which transfers cleanup ownership),
`worker-retain` it at the user's explicit request, or `worker-release` it. Release
first preserves inspectable output into `worker_terminal_archives`, then closes
only the exact terminal owned by that settled Dispatch. `release_unknown` exits
non-zero; `retained` / `release_pending` / already-released are settled answers,
not failures. Explicitly: *"Do not release a worker because of a timeout, TUI idle
state, heartbeat, status, question, escalation, or rejected/stale `worker_done`."*

### 1.11 Skills, artifacts, automations, hooks (secondary)

- **Skills** (`out/cli/handlers/skills.js`, `out/cli/bundled-skill-guides.js`) are
  bundled markdown guides shipped *inside the binary*, installed into agent CLI
  config dirs by `skills install`. The orchestration guide alone is 409 lines /
  ~40 KB, and error paths return `orchestrationSkillRecoveryData()` — a structured
  `nextCommandArgs: ["skills","get","orchestration","--full"]` telling the agent to
  re-read the guide rather than retry blind
  (`out/shared/orchestration-rpc-contract.js:57-67`).
- **`agent-context`** (`out/cli/agent-context.js`) serializes the live CLI spec table
  to JSON for agent discovery, "so agent discovery cannot drift from the command
  surface it describes".
- **Agent hooks** (`out/cli/handlers/agent-hooks.js`) is only an on/off/status switch
  over the installers; the interesting code is the installer chunk cited in §1.4.
  Note the runtime-first, disk-fallback pattern: it tries `settings.update` on the
  live runtime and only writes the profile JSON when the runtime is unreachable.
- **Automations** (`out/cli/handlers/automations.js`) are scheduled agent runs — an
  RRULE/cron trigger, a provider, a worktree target, a prompt, a precheck, and a
  run history. Explicitly *not* orchestration: no Task, no Dispatch, no lifecycle.
- **Artifacts** / **skill-sharing** are publish-and-share surfaces with a consent
  gate (`out/shared/artifact-sharing-gate.js`, `agent-skill-sharing-gate.js`).
  Not orchestration.

### 1.12 The plugin system

`out/shared/plugins/` (27 files, ~2 100 lines) is a small, tight design:

- A **closed capability set** of 7 unscoped kinds — `workspace:read`, `terminal:send`,
  `notifications:show`, `storage`, `secrets`, `events:subscribe`, `settings:own`
  (`plugin-capabilities.js:20-28`) — chosen so "a typo (or a capability from a newer
  Orca) fails manifest validation instead of silently granting nothing".
- **Consent by fingerprint** over a canonical, order-and-duplicate-insensitive
  serialization of the capability set (`plugin-capabilities.js:44-49`), so
  reformatting a manifest does not re-prompt but widening it does.
- **Out-of-process workers** (`child_process.fork`) with a zod-validated protocol in
  *both* directions, "because the child runs third-party code — nothing it sends is
  trusted structurally" (`plugin-host-protocol.js`).
- **Bounded**: ready 10 s, invoke 30 s, idle reap 5 min, max 5 concurrent workers
  (`plugin-host-protocol.js:93-98`).
- **Re-gated at every boundary** (`plugin-capability-gate.js`), with a disabled or
  stale-consent plugin failing *identically* to an ungranted capability so plugin
  code cannot probe the difference.

---

## 2. Side by side

| # | Orca mechanism | Forge harness equivalent | Verdict |
|---|---|---|---|
| 1 | Task ≠ Dispatch: a retry is a new attempt row (`out/main/index.js:93239-93295`) | One row per feature; `harness/features.json` + `review_rounds`/`gate_attempts` counters (`crates/domain/src/harness.rs:9-46`) | **Different tradeoff.** Forge's unit is a *feature*, not a task; one attempt row per step would cost more than it buys. But Forge has no attempt identity at all, which is why #4 and #7 below are impossible. |
| 2 | `worker_done` message with `--outcome succeeded\|failed`, validated payload, structured `result` envelope (`out/main/index.js:95475-95540`) | Process exit code only (`crates/daemon/src/jobs.rs:460-484`), plus a markdown file the agent was asked to write | **Gap.** Exit 0 means "the CLI ran", not "the step succeeded". A `claude -p` that gives up politely still exits 0. |
| 3 | Sender authority: pane-key identity checked before any lifecycle mutation (`out/main/index.js:95395-95398`) | Not applicable — the daemon spawns the process itself and owns the handle (`crates/daemon/src/jobs.rs:267-341`) | **Forge already better.** Orca's whole authority layer exists because a worker is an arbitrary terminal the user could also type into. Forge does not have that problem. Do not import it. |
| 4 | Client-side settlement re-verification, `operation_unknown` (`out/cli/handlers/orchestration-worker-settlement.js`) | None | **Gap, in principle.** Mostly moot given #3: Forge's writer and reader are the same process. Worth one narrow borrowing (see R2). |
| 5 | Heartbeat + 10-min stale sweep (warn only) (`out/main/index.js:90138-90147, 95735-95743`) | None | **Gap.** |
| 6 | Provider hooks reporting `working\|blocked\|waiting\|done` + `interactivePrompt` (`out/main/chunks/managed-agent-hook-controls-RcsNtBpP.js:2176-2260`, `out/shared/agent-status-types.js:16`) | None on the harness path. Forge reads the stream itself (`crates/daemon/src/jobs.rs:382-401`) and already parses provider session ids from it | **Gap, but a cheap one to close** — Forge is already reading a structured event stream that carries the same facts. |
| 7 | `worker-start --retry-of`, circuit breaker at 3 attempts (`out/main/index.js:90148+`) | `max_review_rounds: 2` for the review loop only (`harness/features.json:8`, `crates/harness-service/src/lib.rs:487-500`). A *failed job* has no retry at all: `JobState::Failed` → block (`crates/daemon/src/harness_runner.rs:251-258`) | **Gap.** A transient provider hiccup blocks the feature exactly as hard as a real failure. |
| 8 | Blocking `ask` with a durable, resumable question thread; timeout leaves it pending (`out/shared/orchestration-ask-timeout.js`, `out/cli/handlers/orchestration.js:771-839`) | None. A headless job cannot ask anything; `ask_harness` (`crates/daemon/src/harness_runner.rs:145-189`) is the operator asking *about* the harness, not an agent asking the operator | **Gap** — but see §4: this is the one Orca feature Forge should *not* copy wholesale. |
| 9 | `escalation` distinct from `worker_done --outcome failed` (`out/main/index.js:95644-95672`) | One outcome: block, with a prose reason (`crates/daemon/src/harness_runner.rs:371-374`) | **Different tradeoff.** Forge's `blocked` already means "a human must look". Orca needs two because its coordinator is itself an agent that might resolve the blocker. |
| 10 | Decision gate as a DB row that *sets the task blocked*, re-derived on restart (`out/main/index.js:89960-90002, 95716-95722`) | `gate_<id>.md` file + `spec_ready` status + `human_gate_opened`/`human_gate_resolved` events (`harness/src/events.ts:13-29`, `crates/daemon/src/harness_runner.rs:277-282`) | **Roughly equal, with one real hole.** Forge's gate has better artefacts (a markdown brief a human reads). But nothing *enforces* it — see R1. |
| 11 | Task DAG with `deps`, SQL-computed readiness, `promoteReadyTasks` (`out/main/index.js:93900-93934, 93993-94001`) | None. Features are independent; `is_open()` only gates the checkout (`crates/domain/src/harness.rs:53-55`) | **Different tradeoff, correctly.** One feature per checkout at a time is a deliberate serialization. A DAG would be machinery for a fan-out Forge does not have. |
| 12 | Parallel workers: many Dispatches per Run, one *active supervised* Dispatch per Task enforced **in the settle transaction** (`out/main/index.js:90391-90400`) and in `createGate` (`out/main/index.js:89978-89986`) | One open feature per **checkout** — but enforced only at *registration* (`crates/harness-service/src/lib.rs:396-400, 413-421`) and after the fact by `validate.ts:131-139`. Nothing stops two jobs running on one feature: `run_harness_step` checks no in-flight job (`crates/daemon/src/harness_runner.rs:80-129`) | **Forge already better on the unit, gap on the enforcement.** The working tree is the right thing to serialize on. But Orca re-checks liveness at every mutation; Forge checks once, at registration, and never again. See R5. |
| 12b | Every rule in the schema is read by the code that enforces it | `require_human_spec_approval` and `require_green_gate_to_close` (`harness/features.json:6-7`) are declared and **never read** by any Rust or TypeScript code | **Gap.** Two of the seven rules are decoration. |
| 13 | Idempotent mutations with `--retry-request` and rewritten ambiguous-failure advice (`out/cli/orchestration-mutation-recovery.js`) | None. `set_status` / `advance` are unconditional writes (`crates/harness-service/src/lib.rs:502-550`) | **Gap** — and worse than it looks, see #14. |
| 14 | All state in SQLite, `BEGIN IMMEDIATE` + savepoints (`out/main/index.js:90347-90356`) | `features.json` read-modify-written non-atomically with `fs::write`, no lock (`crates/harness-service/src/lib.rs:160-173`) | **Gap.** Concurrent writers are not hypothetical: the daemon writes it while agents run `scripts/harness event` and the `/feature-go` skill edits it by hand. |
| 15 | Long-poll `check --wait` with 15 s stderr keepalives sized to Claude Code's silence budget (`out/cli/handlers/orchestration.js:17-51`) | Not needed — the daemon owns the process and streams `JobOutput` events (`crates/daemon/src/jobs.rs:422-438`) | **Forge already better.** This is Orca paying for not owning the process. |
| 16 | Base-drift guard: refuse dispatch >20 commits behind base unless the spec opts out (`out/main/index.js:95723-95733, 95762-95766`) | None | **Gap, small but cheap.** Forge already has `git-service`. |
| 17 | Bundled skill guides shipped in-binary, with structured "go re-read the guide" recovery (`out/cli/bundled-skill-guides.js`, `out/shared/orchestration-rpc-contract.js:57-67`) | `.claude/agents/*.md` + `.claude/skills/*` on disk, and `step_prompt` building prompts in Rust (`crates/daemon/src/harness_runner.rs:97`) | **Different tradeoff.** Orca must ship guidance because it drives *someone else's* agent. Forge's prompts are its own. |
| 18 | Plugin system: 7 closed capabilities, consent fingerprint, forked zod-validated workers, bounded (`out/shared/plugins/`) | None | **Gap by choice** — see §4. |

---

## 3. Recommendations, ranked

### The framing: two orchestrators, one state file

Most of what follows is a consequence of one structural fact. Forge has **two
independent implementations of the same cycle** sharing `harness/features.json`:

- the **daemon path** — `crates/daemon/src/harness_runner.rs` + `jobs.rs`, deterministic
  Rust advancing on job exit codes;
- the **skill path** — `/feature` → `/feature-go`, an LLM lead launching `Agent`
  subagents and editing `features.json` and the event log itself.

Both have been exercised: `orchestrator_session_id` is set on several rows in
`harness/features.json`. They already disagree on the review verdict vocabulary
(R1), on how review rounds are counted (R1), and on who writes the state file (R4).
Neither locks it.

That divergence is the thing to watch. Every recommendation below either narrows it
or makes the state robust enough that it does not matter. Nothing below proposes
adding a third path.

### R1 — Make the review verdict structured, and fix the two bugs it is hiding
**Value: highest. This is a correctness bug today, not a feature request.**

`settle_review` (`crates/daemon/src/harness_runner.rs:314-368`) decides a feature's
fate by uppercasing the whole of `progress/review_<id>.md` and asking
`verdict.contains("APPROVED")`, then `verdict.contains("REJECTED")`.

Three things are wrong with that, and they compound:

1. **Substring match on prose.** `reviewer.md:86` tells the reviewer to write a
   header line reading literally `**Verdict:** APPROVED | CHANGES_REQUESTED`. Any
   review that keeps that template line — including a rejecting one — contains the
   substring `APPROVED` and is settled as `done`. `harness/src/validate.ts:125` uses
   the same `.includes("APPROVED")`, so the validator agrees with the false
   approval instead of catching it.
2. **Vocabulary mismatch.** The runner branches on `REJECTED`. Every agent and skill
   says `CHANGES_REQUESTED` (`reviewer.md:78, 86, 117, 124, 128`;
   `.claude/skills/feature-go/SKILL.md:97-98`). A genuine rejection that avoids bug
   #1 therefore falls through to the `else` branch and gets **blocked** rather than
   sent back for another round — silently defeating `max_review_rounds` on the
   headless path.
3. **The structured answer is requested and thrown away.** `VERDICT_SCHEMA`
   (`crates/daemon/src/harness_runner.rs:72`) already asks the provider for
   `{"verdict": "APPROVED"|"REJECTED", "summary"}` and `JobRequest::schema` carries
   it (`crates/domain/src/job.rs:125`). Nothing reads the result. This is Orca's
   `--outcome succeeded|failed` — Forge already built it and left it unwired.

**What to change.** Read the verdict from the job's structured answer, not from
prose. Extend `Job` with the parsed final result (or read the last schema-conformant
line of `log_path`, which `follow_job` already touches at
`crates/daemon/src/jobs.rs:390`), and make `settle_review` branch on that. Keep the
markdown as the human artefact. If the structured answer is absent or unparseable,
that is `blocked` with reason "the reviewer returned no verdict" — which is what the
existing `else` branch already says, correctly.

Align the vocabulary in the same change: pick one pair (`APPROVED` /
`CHANGES_REQUESTED` reads better and matches the agents) and use it in
`VERDICT_SCHEMA`, `settle_review`, `reviewer.md`, `feature-go/SKILL.md` and
`validate.ts:125`.

While there, reconcile the round arithmetic: `bump_review_round` allows another
round while `rounds < max` (`crates/harness-service/src/lib.rs:487-500`, so with
`max_review_rounds: 2` the implementer gets one retry), whereas
`.claude/skills/feature-go/SKILL.md:102-103` tells the lead *"Limit: 2 rounds. On
the third, mark the feature `blocked`"*. The two paths through the same state
machine currently count differently.

**Files:** `crates/daemon/src/harness_runner.rs` (72, 314-368),
`crates/daemon/src/jobs.rs` (348-419, 460-484), `crates/domain/src/job.rs`,
`harness/src/validate.ts` (121-128), `.claude/agents/reviewer.md`,
`.claude/skills/feature-go/SKILL.md`.

**What could break.** Providers that do not support `schema` — `JobRequest::schema`
is documented as *dropped, not refused* (`crates/domain/src/job.rs:122-125`), so a
schema-less provider would suddenly always land in the no-verdict branch. Keep a
prose fallback for those, but anchor it: match a line that *is* the verdict
(`^\s*(\*\*Verdict:\*\*\s*)?(APPROVED|CHANGES_REQUESTED)\s*$`), not a substring of
the file. Existing `review_*.md` files in `harness/progress/` were written against
the old rule; a re-validate may newly fail features whose review file only *mentions*
approval. That is the bug surfacing, not a regression.

### R2 — Verify the state actually moved before reporting a step succeeded
**Value: high. Small change, directly borrowed from `orchestration-worker-settlement.js`.**

`advance_harness_after_job` (`crates/daemon/src/harness_runner.rs:242-264`) discards
the result of every write: `let _ = harness_service::set_status(...)`,
`let _ = harness_service::append_event(...)` (also at 113-114, 278-281, 284-289,
322-329). A features.json that is momentarily unreadable — mid-write by a concurrent
`scripts/harness` invocation (see R4) — silently loses a transition, and the feature
sits in the previous status with a job that already exited.

**What to change.** After a step writes its outcome, re-read the feature and confirm
the status is what was just written; if not, block with a reason that says *what was
attempted and what was read back*, the way Orca's `operation_unknown` does. Orca's
lesson is precisely that an accepted write is not a settled state.

**Files:** `crates/daemon/src/harness_runner.rs` (113-114, 242-305, 314-369),
`crates/harness-service/src/lib.rs` (502-508).

**What could break.** More features reaching `blocked` in situations that previously
looked fine. That is the point, but it will be noisy until R4 lands — do R4 first or
in the same change.

### R3 — A wall-clock budget per step, and a heartbeat derived from the stream
**Value: high. Forge currently has no answer at all to "is it hung?".**

`follow_job` (`crates/daemon/src/jobs.rs:348-419`) blocks on
`BufReader::new(stdout).lines()` until EOF, then calls a bare, unbounded
`process.child.wait()` (`crates/daemon/src/jobs.rs:407-411`). There is no timeout
anywhere in the job path, no idle detection, and `DEFAULT_MAX_CONCURRENT_JOBS = 2`
(`crates/daemon/src/jobs.rs:45`). A provider that stalls holds one of two slots
forever; two of them wedge the harness until the daemon restarts, at which point
every job row is lost (jobs live only in memory —
`crates/domain/src/harness.rs:78-85` documents this) while the feature keeps its
`in_progress` status and its checkout slot indefinitely.

Note that `harness/CHECKPOINTS.md:185-188` already requires *"every `spawn()` is
reaped; no `wait()` without a bound"*. The unbounded `wait()` at `jobs.rs:407-411`
is a standing C7 violation in code the harness itself is meant to police.

Two pieces of the answer already exist in the repo and are unused:

- **`read_step_trace`** (`crates/harness-service/src/lib.rs:241-312`, with a
  `SETTLING` event list at `:263-269`) computes exactly the right primitive — a
  `*_started` event with no settling event after it. It has **zero callers**
  anywhere in `crates/`, `apps/`, or `harness/`.
- **A bounded-command helper** already exists at `crates/daemon/src/core.rs:4509-4583`
  and kills the negative process group on expiry. It is used for git, `gh`, and
  version probes; jobs do not use it. `spawn_job` already sets
  `.process_group(0)` and `stdin(Stdio::null())`
  (`crates/daemon/src/jobs.rs:300, 307`), so the kill path a watchdog needs is in
  place. (`run_validate` and `register_from_issue` in
  `crates/harness-service/src/lib.rs:552-568, 577-618` also shell out unbounded and
  should adopt it.)

**What to change**, in ascending cost:

1. **A last-output clock.** `follow_job` already sees every line and already tracks
   `last_flush` (`crates/daemon/src/jobs.rs:379, 396-399`). Record `last_line_at` on
   the `Job` row. This is Orca's heartbeat, for free, and *more* reliable than
   Orca's — Orca has to ask the worker to send one because it does not own the pipe.
   Wire `read_step_trace` at the same time so the *feature* side can also answer
   "started, never settled" after a daemon restart, when the in-memory job row is
   gone but the event log is not.
2. **A warn threshold**, Orca's shape exactly: no output for N minutes → log and
   broadcast, **do not kill**. Orca's `warnStaleDispatches`
   (`out/main/index.js:95736-95743`) at 10 minutes deliberately only warns; the guide
   says long coding tasks routinely run 15-60 minutes.
3. **A hard per-step budget** as a `features.json` rule (`max_step_minutes`,
   alongside `max_review_rounds` at `harness/features.json:4-19`), enforced by a
   watchdog that calls the existing `cancel_job` (`crates/daemon/src/jobs.rs:158-183`)
   and blocks the feature with a timeout reason. Note that `JobState::Cancelled`
   currently means "the feature keeps its status and waits"
   (`crates/daemon/src/harness_runner.rs:259-261`), so a watchdog cancel needs its
   own path or the feature will sit open.

**Files:** `crates/daemon/src/jobs.rs` (45, 348-419, 407-411, 460-484),
`crates/domain/src/job.rs` (34-46 — a `TimedOut` state, 62-101),
`crates/daemon/src/harness_runner.rs` (242-264),
`crates/harness-service/src/lib.rs` (241-312 — wire `read_step_trace`; rules parsing),
`crates/daemon/src/core.rs` (4509-4583 — reuse the bounded-command helper),
`harness/features.json` (rules), `harness/src/validate.ts` (52-57).

**What could break.** A too-tight budget kills legitimate long implementations —
this is exactly the failure Orca's guide warns about repeatedly. Default the budget
generously (60+ min) or leave it unset, and ship (1) and (2) before (3). The
watchdog also needs a thread or timer that does not hold the core lock; `follow_job`'s
doc comment is explicit that the core lock is never held across a read
(`crates/daemon/src/jobs.rs:344-347`) and C5/C7 in `harness/CHECKPOINTS.md` police
this.

### R4 — Make `features.json` writes atomic and serialized
**Value: high. Cheap. Prevents a whole class of silent state loss.**

`save` (`crates/harness-service/src/lib.rs:169-173`) is `fs::write` — truncate then
write — with no lock, no atomic rename and no fsync, and every mutation is a
read-modify-write via `load_mut` (160-167). Concurrent writers exist **by design**,
from three directions:

- the daemon, on every step transition;
- the TypeScript CLI, which does its own independent read-modify-write
  (`harness/src/cli.ts`);
- the agents themselves — `/feature-go` has the lead edit `features.json` by hand
  (`.claude/skills/feature-go/SKILL.md:98-107`), and both `spec-author.md:84` and
  `implementer.md:70-72, 89` instruct their agents to do the same.

Two consequences beyond the obvious lost update. A reader that hits the truncated
window gets `HarnessError::NotInitialized` or a JSON parse error — which R2 would
turn into a spurious block. And `bump_review_round`
(`crates/harness-service/src/lib.rs:487-500`) is a non-atomic read-increment-write, so
a lost update **silently grants an extra review round**, quietly defeating the one
budget the harness does enforce.

`AGENTS.md:70` documents parallel worktrees sharing one `features.json` as a
supported case. Today that case is last-writer-wins.

**What to change.** Write to a sibling temp file and `rename` (Orca does exactly this
for its own JSON state — `out/cli/handlers/agent-hooks.js:53-72`), and take an
advisory lock around read-modify-write so two writers serialize instead of racing.
`crates/daemon/src/lockfile.rs` already exists in this repo; reuse the pattern rather
than adding a dependency. `append_event`
(`crates/harness-service/src/lib.rs:314-335`) is already append-only `O_APPEND`, which
is fine.

**Files:** `crates/harness-service/src/lib.rs` (160-173, 402-450, 487-550).

**What could break.** A stale lock if a process dies holding it — bound the wait and
fall back to the current behaviour with a warning rather than deadlocking the
daemon. Note the lock is per-repository-checkout, and `docs/harness.md:51-63` is
emphatic that only the state-owning checkout counts; lock that path, not the caller's.

### R5 — Give `run_harness_step` preconditions: the gate, and one job per feature
**Value: medium-high. Currently both are conventions, not rules.**

`Request::RunHarnessStep { step }` is routed straight to `run_harness_step`
(`crates/daemon/src/core.rs:625-629` → `crates/daemon/src/harness_runner.rs:80-129`),
which checks only that the feature has a `workspace_id`. Two holes follow:

**The gate is bypassable.** Any client can start `HarnessStep::Implement` on a
feature still in `pending` or `spec_ready`, skipping `ApproveSpec` entirely.
`harness_service::advance` (`crates/harness-service/src/lib.rs:509-550`) likewise
accepts `ApproveSpec` from any status, and `set_status` (502-508) writes any string
with no transition check — `rules.valid_status` (`harness/features.json:12-19`) is
enforced only after the fact by `validate.ts:80-83`. There is **no from→to
transition table anywhere in the repository**; the state machine in `AGENTS.md:70`
and `docs/harness.md:97-98` is documentation. And `rules.require_human_spec_approval`
(`harness/features.json:6`) — the flag that would say this matters — is read by
nothing. Because `advance` is not idempotent (row 13 of the table above), replaying
`ApproveSpec` appends a duplicate `human_gate_resolved` event *and* launches a
second `Implement` job.

**Two jobs can run on one feature.** `run_harness_step` does not check for an
in-flight job for that feature. Two `RunHarnessStep` requests, or a UI click racing
the automatic `Implement → Review` chain at `harness_runner.rs:290-298`, start two
processes in the same worktree editing the same files.
`DEFAULT_MAX_CONCURRENT_JOBS = 2` (`crates/daemon/src/jobs.rs:45`) is a global
ceiling, not a per-feature mutex. This is precisely what Orca refuses in
`settleWorkerReport`'s `conflictingWorker` query (`out/main/index.js:90391-90400`)
and in `createGate` (`out/main/index.js:89978-89986`) — *"stop or settle its worker
first"*.

Orca's answer is that the gate is a **row that makes the task un-startable**:
`createGate` sets the task `blocked` and settles its dispatches
(`out/main/index.js:89975-89982`), and `reblockTasksWithPendingGates`
(`out/main/index.js:95716-95722`) re-derives that on restart so nobody has to
remember.

**What to change.** Put a transition table in `harness-service` and route every
status change through it: `Implement` requires the current status to be `spec_ready`
with a resolved `human_gate_resolved` event, `Review` requires `in_progress`, `done`
requires an approved verdict. `validate_event_coherence`
(`harness/src/events.ts:147-189`) already encodes most of these rules — the Rust side
should share them rather than restate them. Reject with `ErrorCode::InvalidRequest`
instead of running the step. Read `require_human_spec_approval` and
`require_green_gate_to_close` while you are there, or delete them from the schema.

Separately, refuse a step whose feature already has a live job: the daemon holds
`Inner::job_processes` (`crates/daemon/src/core.rs:88-97`) and `Job::feature_id`
(`crates/domain/src/job.rs:71`), so the check is one lookup. Return
`ErrorCode::Conflict` — `harness_err` already maps `HarnessError::Conflict` to it
(`crates/daemon/src/harness_io.rs:9-22`).

**Files:** `crates/harness-service/src/lib.rs` (502-550),
`crates/daemon/src/harness_runner.rs` (80-129, 290-298),
`crates/daemon/src/harness_io.rs` (9-22, 132-150), `harness/src/events.ts` (147-189),
`harness/features.json` (6-7).

**What could break.** Recovery flows that deliberately re-run a step out of order
(re-running `Review` after a manual fix, for instance). Keep an explicit
force/override on the request rather than leaving the hole open, and log it as an
event so the audit trail shows the gate was bypassed on purpose.

### R6 — Give a failed step a bounded retry, separate from the review loop
**Value: medium.**

`JobState::Failed` → `block_harness_feature` unconditionally
(`crates/daemon/src/harness_runner.rs:251-258`). A provider that dies on a rate limit,
a network blip, or a transient CLI crash blocks the feature exactly as hard as an
agent that genuinely could not do the work — and unblocking is a human action.

Orca separates *attempt* failures from *work* failures: `failure_count` on the
dispatch, `circuit_broken` at 3 (`out/main/index.js:90148+`), and `--retry-of` for a
retry that explicitly does **not** inherit placement.

**What to change.** Add `step_attempts` beside `review_rounds` and `gate_attempts` in
`features.json`, and a `max_step_attempts` rule (default 2). On `JobState::Failed`,
if the budget remains, re-run the same step; otherwise block with the reason it
already writes. Append a distinct event kind (`step_retried`) so the timeline shows
it. Resist retrying on a *non-zero exit that the agent chose* — but Forge cannot tell
those apart until R1 lands, which is another reason R1 comes first.

**Files:** `crates/daemon/src/harness_runner.rs` (242-264),
`crates/harness-service/src/lib.rs` (487-500 as the template),
`harness/features.json`, `harness/src/events.ts` (13-29),
`harness/src/validate.ts` (90-98).

**What could break.** Retrying a step whose partial work is already on disk. The
implementer writes into the working tree, so a re-run starts from a dirty tree — the
retry prompt must say so, or the second attempt will be confused about what it
already did. Orca sidesteps this by making a retry choose fresh placement.

### R7 — A structured step report, not just a markdown file
**Value: medium.**

Orca's `worker_done` carries `filesModified[]` and `reportPath`, and the settled
`tasks.result` is a provenance-stamped JSON envelope
(`out/main/index.js:95508-95519`). Forge's equivalent is `Job::last_line`
(`crates/domain/src/job.rs:95`) plus a markdown file whose existence is checked but
whose contents are only grepped.

**What to change.** Extend the per-step schema (already plumbed as
`JobRequest::schema`) to ask each step for a small envelope — outcome, one-line
summary, files touched, artefact path — and record it on the feature row and in the
event log. This makes `scripts/harness timeline` genuinely useful, gives the Feature
tab something to render, and gives the *next* step's prompt real input instead of
"go read the file".

**Files:** `crates/daemon/src/harness_runner.rs` (72, 97, 267-305),
`crates/domain/src/job.rs`, `crates/harness-service/src/lib.rs` (314-335),
`harness/src/events.ts` (13-35).

**What could break.** Schema-less providers again (see R1); degrade to prose. Also
watch payload size — Orca caps every status field explicitly
(`out/shared/agent-status-types.js:43-54`) and Forge should too, or a chatty agent
puts a megabyte into `features.json`.

### R8 — Borrow the base-drift guard
**Value: low-medium. Cheap and self-contained.**

Before dispatching, Orca checks how far the worktree's HEAD is behind its base
branch, injects the 5 most recent missing commit subjects into the preamble, and
**refuses** to dispatch past 20 commits unless the spec opts out with
`allow-stale-base: true` (`out/main/index.js:95307-95319, 95723-95733, 95762-95766`).

Forge has `crates/git-service` and runs steps in worktrees that can sit for days.
Adding the drift summary to `step_prompt` (`crates/daemon/src/harness_runner.rs:97`)
is a few lines, and it turns a class of confusing review failures ("the reviewer says
this API does not exist") into a stated fact.

**Files:** `crates/daemon/src/harness_runner.rs` (97, `step_prompt`),
`crates/git-service`.

**What could break.** Git calls on the step-start path — `run_git` is already the
only sanctioned route (AGENTS.md invariant), and this must not become a network call.
Use the local base ref only.

### R9 — Do not build a plugin system yet; borrow its shape when you do
**Value: low now.**

Forge has no plugin system and does not currently need one: it has no third-party
extension surface, no marketplace, and one first-party UI. Building one now is
machinery for a user who does not exist.

When it *is* needed, `out/shared/plugins/` is the right size and shape to copy — the
closed capability enum, the consent fingerprint over a canonical serialization, the
forked out-of-process worker with schema validation in both directions, the explicit
timeouts and idle reap, and the rule that a disabled plugin fails identically to an
ungranted capability. Note it is ~2 100 lines for the *shared* half alone; the host
and marketplace are larger. Record it as prior art in an ADR and move on.

---

## 4. What NOT to copy, and why

**The entire lifecycle-authority layer** (pane keys, capability hashes,
`launch_token_hash`, `process_incarnation`, `hasLifecycleAuthority`,
`sender_not_assignee`, `capability_revoked_at`). Orca needs all of it because a
worker is an arbitrary terminal that the user can also type into, that can be
restarted, re-minted, taken over, or moved to another machine — so "who is allowed
to settle this dispatch?" is a genuinely hard question. Forge spawns the process
itself, holds the `Child`, and reaps it (`crates/daemon/src/jobs.rs:267-341, 407-417`).
The daemon *is* the authority. Importing any of this would be pure cost.

**The federation layer** (`federated_dispatches`, `remote_dispatch_attachments`,
`federation_relay_items`, `remote_questions`, `--on <environment>`, peer
fingerprints, sequence-numbered relay with ack watermarks —
`out/main/index.js:93158-93230`). Cross-machine worker dispatch is roughly a third of
Orca's orchestration schema and answers a question Forge does not have.

**The legacy-compatibility machinery** (`legacy_adoptions`,
`legacy_compatibility_principals`, `legacy_operation_receipts`, `legacy_mail_receipts`,
the `[LEGACY COMPATIBILITY]` / `[LEGACY RECOVERY REPLAY]` / `[LEGACY READ-ONLY]`
message banners, the Windows exit-75 commit/resume dance —
`out/main/index.js:93400-93475`, skill guide §Contract Migration). This is the scar
tissue of having shipped a scheduler and retired it while live runs were in flight.
It is a cautionary tale about protocol versioning, not a pattern.

**A task DAG for features.** Forge's constraint is one open feature per *checkout*
(`crates/harness-service/src/lib.rs:396-420`), which is the right unit — the thing that
actually conflicts is a working tree. `deps` on features would let two features be
"ready" in the same checkout, which the validator (`harness/src/validate.ts:135-139`)
correctly forbids. If Forge ever needs ordering, it is between *tasks inside one
spec* (`tasks.md` already lists them), not between features.

**A second agent as coordinator.** Orca retired its scheduler and made an LLM the
coordinator because it is a general multi-agent IDE where a human directs a
supervisor. Forge's harness is a *pipeline with a fixed shape* — spec, gate,
implement, review — and its coordinator is `harness_runner.rs`, deterministic Rust
that advances on observable facts. That is a better fit for the constraint, and the
whole reason `docs/harness.md` and `crates/domain/src/job.rs:1-16` were written the
way they were. Do not put an LLM back in the loop position.

**`ask` as a blocking worker→human channel — at least not now.** Orca needs it
because its workers are interactive sessions that would otherwise hang on a TUI
prompt nobody can see. Forge's steps are headless by construction: `claude -p` /
`codex exec` do not prompt. A blocking ask would reintroduce the exact property jobs
were chosen to avoid — a run that does not end observably
(`crates/domain/src/job.rs:9-12`). Forge's `blocked` status *is* the ask, resolved by
a human at the Feature tab rather than by an agent in a terminal. If a step needs
input mid-flight, the right answer is to block, not to wait.

**`orchestration reset --all`.** A destructive "clear the local state" verb
(`out/cli/handlers/orchestration.js:905-916`) that the guide has to warn people away
from. Forge's state is a git-tracked JSON file plus append-only JSONL — `git checkout`
is already the reset, and it leaves an audit trail.

**The 15-second stderr keepalive.** `out/cli/handlers/orchestration.js:17-51` is a
workaround for Claude Code backgrounding a silent subprocess after ~2 minutes. Forge
does not shell out to a long-poll; the daemon streams `JobOutput` events
(`crates/daemon/src/jobs.rs:422-438`). Copying it would be cargo cult.

**Warning-only staleness as the *whole* answer.** Orca's stale sweep only logs
(`out/main/index.js:95736-95743`) because Orca genuinely cannot tell a thinking agent
from a hung one without the hook channel, and killing on a guess would be worse than
waiting. Forge is in a better position — it owns the pipe and reads every line — so it
should warn *and* eventually act (R3), rather than adopting Orca's resignation.

---

## Appendix: primary sources

**Orca** — `out/cli/handlers/orchestration.js` (917 lines, the full verb surface),
`out/cli/handlers/orchestration-worker-settlement.js`,
`out/cli/orchestration-mutation-recovery.js`,
`out/shared/orchestration-ask-timeout.js`, `out/shared/orchestration-rpc-contract.js`,
`out/shared/agent-status-types.js`, `out/shared/plugins/*`,
`out/main/index.js` (schema 93085-93330; task store 93900-94060; dispatch/settlement
90120-90420; lifecycle reconciliation 95190-95800; worker inspection 99700-99800),
`out/main/chunks/managed-agent-hook-controls-RcsNtBpP.js` (hook installers),
and the bundled `orchestration` skill guide extracted from
`out/cli/bundled-skill-guides.js` (409 lines) — the clearest statement of the model.

**Forge** — `crates/domain/src/harness.rs`, `crates/domain/src/job.rs`,
`crates/daemon/src/harness_runner.rs`, `crates/daemon/src/jobs.rs`,
`crates/daemon/src/harness_io.rs`, `crates/daemon/src/core.rs:600-640`,
`crates/harness-service/src/lib.rs`, `harness/features.json`, `harness/src/validate.ts`,
`harness/src/events.ts`, `harness/CHECKPOINTS.md`, `docs/harness.md`,
`.claude/agents/*.md`, `.claude/skills/feature-go/SKILL.md`.
