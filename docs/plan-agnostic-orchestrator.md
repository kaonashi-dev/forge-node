# Plan — An agnostic orchestrator for engineering tasks

**Status:** Phases 0 and 1 are **implemented** (2026-09-01); Phases 2–6 are
still plan only. Phase progress is marked per row in §7.
**Scope:** `harness/`, `crates/harness-service`, `crates/daemon/src/{harness_runner,harness_io,jobs}.rs`,
`crates/agents` (headless contract), and the harness surfaces of `apps/tauri`.
**Read alongside:** [`harness.md`](./harness.md) (how the cycle runs today),
[`plan-subagent-harness.md`](./plan-subagent-harness.md) (why it was built this
way), [`plan-harness-ui.md`](./plan-harness-ui.md) (the shell-era UI plan, now
ported), [`orca/orchestration.md`](./orca/orchestration.md) (the Orca deep dive
this plan builds on), [`plan-ui-ux.md`](./plan-ui-ux.md) (the general UI bar),
`AGENTS.md` § *Harness*.

---

## 0. Summary

**The harness is the right shape and is half-finished.** State on disk, a
deterministic Rust coordinator that advances on exit codes, a human gate the
machine cannot skip, a provider table that spawns `claude -p` and `codex exec`
identically. That is the skeleton of an agnostic orchestrator and it should be
kept. What is missing is the part that makes it *trustworthy* and the part that
makes it *agnostic*:

| Axis | Today | Gap |
|---|---|---|
| Correctness | Status is a free string; `run_harness_step` runs any step from any status; `features.json` is written with `fs::write` by three writers; a failed job blocks forever; a hung job holds a slot forever | §2 H1–H6 |
| Agnosticism | Headless spelling is data (`HeadlessSpec`), but the *worker protocol* lives in `.claude/skills` and `.cursor/skills`; prompts say "run the /feature-go skill", which only Claude and Cursor understand; the UI has no way to pick which provider runs which role | §2 H7–H10 |
| UI | Every surface exists (panel, tab, attention bar, lieutenant, settings) | The gate cannot show the spec it is asking you to approve; "new feature" lives in Settings; running steps show no live line; no diff / commit / PR from the tab; no role picker | §3 |

**Orca** (v1.4.188 installed here, guide read from the binary) is the best
reference for *coordination semantics* — Task ≠ Dispatch, `worker_done` with an
explicit outcome, gates as rows that make a task un-startable, a retry budget
with a circuit breaker, idempotent mutations. It is the wrong reference for
*who coordinates*: it put an LLM in the loop position because its workers are
terminals it does not own. Forge owns its workers. Keep the Rust coordinator.

**The plan** (§7) is six phases. Phase 1 (correctness) and Phase 2 (a
provider-neutral worker protocol plus per-role provider settings) are the ones
that change what the product *is*. Phase 3 is the UI. Phase 4 adds a second
worker transport (Agent Client Protocol) so the orchestrator stops depending on
each CLI's argv. Phases 5–6 are the programmatic surface and an evaluation loop.

---

## 1. What `harness/` is today

### 1.1 Pieces

| Piece | Owner | Role |
|---|---|---|
| `harness/features.json` | repository | state machine: rows + `rules` (`max_review_rounds` 2, `max_gate_attempts` 3, `require_*`) |
| `harness/specs/<id>-<slug>/{requirements,design,tasks}.md` | `spec-author` | the approvable artefact (EARS, traceability table) |
| `harness/progress/events_<id>.jsonl` | everyone, via `scripts/harness event` or the daemon | append-only audit trail |
| `harness/progress/{gate,context,impl,review,current}_<id>.md` | the steps | the human-readable artefacts each step must leave |
| `harness/CHECKPOINTS.md` C1–C7 | repository | what the reviewer ticks; C4–C7 derived from `AGENTS.md` invariants |
| `harness/src/{cli,harness,events,validate}.ts` | Bun | human CLI (`scripts/harness`), validator, verdict parser |
| `init.sh` / `scripts/harness gate` | repository | the gate: env + files + `validate.ts` + `scripts/dev check` |
| `crates/harness-service` | daemon | read/write of the above from the project root (ADR-012) |
| `crates/daemon/src/harness_runner.rs` | daemon | the coordinator: start a step as a job, settle it on exit |
| `crates/daemon/src/jobs.rs` | daemon | headless runs: spawn, stream, reap, bounded to 2 concurrent |
| `.claude/agents/*.md`, `.claude/skills/*`, `.cursor/skills/*` | repository | the worker protocol, twice |
| `apps/tauri/src/{harness,panels/FeaturesPanel,workbench/FeatureView,shell/AttentionBar,panels/LieutenantPanel,settings/HarnessSection}` | GUI | the surfaces |

### 1.2 Two coordinators, one state file

The same cycle runs on two paths, and they share `features.json` without a lock:

| Path | Coordinator | Workers | Writes the state |
|---|---|---|---|
| Daemon | `harness_runner.rs`, deterministic, advances on `JobState` | headless jobs (`claude -p`, `codex exec`, `opencode run`) | via `harness-service` |
| Skill | the `/feature` → `/feature-go` lead, an LLM in a terminal | `Agent` subagents (`spec-author`, `implementer`, `reviewer`, `committer`) | by hand, per the skill text, plus `scripts/harness event` |

The daemon path is the product. The skill path is the dogfooding path and the
only one Cursor and interactive Claude can run. `docs/orca/orchestration.md` §3
already names the divergence as the thing to watch; this plan collapses it by
making the daemon path the only one that *decides*, and the skill path a client
of it (§6.6).

### 1.3 The provider contract that already exists

`domain::HeadlessSpec` (`crates/domain/src/agent.rs:339-365`) is the whole of
provider agnosticism today, and it is well designed: `mode_args`, `stream_args`,
`resume: ResumeStyle`, `prompt: PromptStyle`, `schema: SchemaStyle`,
`session_id_fields`. The daemon spawns every provider through
`HeadlessSpec::command_args_with` and never branches on an id.

| Provider | Headless | Stream | Schema | Resume | Follows the harness protocol? |
|---|---|---|---|---|---|
| `claude` | `-p --output-format stream-json --verbose` | yes | `--json-schema` inline | `--resume` | yes, via `.claude/skills` and `.claude/agents` |
| `codex` | `exec --json -c model_reasoning_summary=detailed` | yes | `--output-schema <file>` | `exec resume` | **only what the prompt says** — it has no skills |
| `opencode` | `run` | plain text | none | `--session` | only what the prompt says |
| `cursor` | none | — | — | — | not runnable as a step |

`agents::summarize_stream_line` (`crates/agents/src/lib.rs:37-95`) understands
the two JSON dialects. Everything a client sees of a running step comes through
it.

---

## 2. Harness review — findings

Severity: **S1** wrong result or lost state, **S2** stuck or unobservable,
**S3** hygiene. Every finding names the file that fixes it.

### Correctness

**H1 · S1 · No transition table; the gate is bypassable.**
`run_harness_step` (`crates/daemon/src/harness_runner.rs`, first `impl Daemon`
block) checks only that the feature has a `workspace_id`. `Request::RunHarnessStep
{ step: Implement }` on a `pending` or `spec_ready` feature runs the implementer
without an approval. `harness_service::set_status` writes any string;
`harness_service::advance` accepts `ApproveSpec` from any status and is not
idempotent — replaying it appends a second `human_gate_resolved` and launches a
second implement job (`crates/daemon/src/harness_io.rs`, `harness_advance`).
`rules.require_human_spec_approval` and `rules.require_green_gate_to_close`
(`harness/features.json:5-6`) are read by nothing in Rust or TypeScript.
`validate.ts` only complains after the fact.

**H2 · S1 · Two jobs can run on one feature.**
Nothing checks `Inner::job_processes` / `job_queue` for a job with the same
`feature_id` before starting another. A click on *Run implement* racing the
automatic Implement → Review chain starts two agents in the same worktree.
`DEFAULT_MAX_CONCURRENT_JOBS = 2` (`crates/daemon/src/jobs.rs:45`) is a global
ceiling, not a per-feature mutex.

**H3 · S1 · `features.json` is not written atomically and has three writers.**
`save` is `fs::write` (`crates/harness-service/src/lib.rs`, `fn save`), no temp
file, no rename, no lock. The daemon, the Bun CLI (`harness/src/cli.ts`) and the
agents themselves (`implementer.md` steps 2 and 5, `feature-go/SKILL.md` §4)
all read-modify-write it. `bump_review_round` is a lost-update away from a free
review round. `AGENTS.md` names parallel worktrees sharing this file as a
supported case.

**H4 · S2 · A step cannot time out, and nobody knows whether it is alive.**
`follow_job` blocks on `lines()` to EOF; there is no wall-clock budget and no
idle detection. `Job::last_line` is a local in `follow_job` and only lands on
the row in `finish_job` — so while a step runs, `last_line` is `None` and the
Features panel card that prints it says nothing. `read_step_trace`
(`crates/harness-service/src/lib.rs`, `pub fn read_step_trace`) — the primitive
that answers "started and never settled" after a daemon restart — has **zero
callers**, and it reads `job_id` / `provider` keys while the runner writes `job`
/ `log`, so it would find neither even if called.

**H5 · S2 · A failed job blocks the feature as hard as a failed review.**
`advance_harness_after_job`: `JobState::Failed` → `block_harness_feature`. A
rate limit, a network blip and a genuine "I could not do this" are the same
outcome, and unblocking is a human action with no button (§3 U7).

**H6 · S1 · The reviewer's structured answer is requested and discarded.**
`VERDICT_SCHEMA` is passed as `JobRequest::schema`; `settle_review` reads the
verdict from `progress/review_<id>.md` with the (now line-anchored, well tested)
`parse_verdict`. The prose parser is a good fallback and a bad primary: a
provider that supports a schema should be believed on the schema, and the file
should be the human artefact. Nothing today records *which* provider produced
the verdict.

### Agnosticism

**H7 · S1 · The worker protocol is Claude-shaped.**
`step_prompt` says *"Run the /feature skill"* and *"Run the /feature-go skill"*.
Codex and OpenCode do not have those skills; they get the one-paragraph prompt
plus `AGENTS.md`. The protocol that matters — resolve `$ROOT`, write the four
artefacts, append events only through `scripts/harness event`, never mark
`done`, the gate retry budget — exists only in `.claude/agents/*.md` and
`.claude/skills/*/SKILL.md`, and again in `.cursor/skills/*`. Three copies, two
providers, none of them the source of truth.

**H8 · S1 · No way to choose which provider runs which role.**
`harness_runner::provider_key` reads `ui.harness.orchestrator` /
`ui.harness.executor` / `ui.harness.reviewer` from `app_state`. Nothing in
`apps/tauri` writes those keys (`grep -rn "ui.harness" apps/tauri/src` is
empty; `HarnessSection.tsx` has no picker). Every step therefore runs on
`first_headless_provider()` — whichever is installed first. "Spec by Claude,
implement by Codex, review by Claude" is the headline use case and it is not
reachable from the product.

**H9 · S2 · Capability is not modelled; failure is late.**
A feature whose reviewer is `opencode` will run and then block with "no verdict
line" because OpenCode has no schema and streams plain text. `HeadlessSpec` has
the facts (`schema: None`, `stream_args: []`) but nothing turns them into
"this provider can be a reviewer / cannot be a reviewer" before the run.

**H10 · S3 · `run_validate` shells out to Bun; the rules live in two languages.**
`harness_service::run_validate` runs `bun harness/src/validate.ts`. The Rust
side has no validator of its own, so the GUI's *Validate* depends on a runtime
the daemon otherwise never needs, and `validateEventCoherence`
(`harness/src/events.ts`) encodes the transition rules that H1 says the daemon
lacks.

### Hygiene

**H11 · S3 · `CHECKPOINTS.md` and the agents still police crates that no longer exist.**
C4 (`harness/CHECKPOINTS.md:56-57`) and C7 (`:160`) name `ui`, `theme tokens`
and "the shell render"; `implementer.md:100,109` does the same. The workspace
ships `apps/tauri` as the GUI. The GUI is `apps/tauri` with its own
gate (`oxlint`, `oxfmt`, `tsc --noEmit`, vitest) that no checkpoint mentions and
`.claude/hooks/fmt-and-check.sh` never runs (it only handles `crates/*.rs`).

**H12 · S3 · Test debris occupies real checkouts.**
`harness/features.json` rows 2–9 are `Test` / `Add support for Grok ai` /
`Add support for toher themes`, all `pending`, five of them bound to Forge
worktrees. One open feature per checkout means those worktrees refuse a real
feature until each is blocked by hand.

**H13 · S3 · Event vocabulary drifts.**
The daemon writes `spec_started`; `harness/src/events.ts` `EventType` does not
list it. `harness_service::read_step_trace` expects `job_id`; `run_harness_step`
writes `job`. `docs/harness.md` says "Each step runs as a headless job" and, in
the same file, "UI integration is not implemented yet".

**H14 · S3 · The daemon-path implementer has no gate-attempt counter.**
`gate_attempts` is bumped by the agent editing `features.json` by hand
(`implementer.md` step 5). On the daemon path nothing observes `scripts/dev
check` failing; the budget is honoured only if the model chooses to.

---

## 3. UI review — findings

The surfaces from `plan-harness-ui.md` all exist in `apps/tauri`. The findings
are about whether a person can *drive* a feature from them without a terminal.

| # | Where | Finding | Fix (§7 phase 3) |
|---|---|---|---|
| U1 | `workbench/FeatureView.tsx` `GateCard`, `harness/types.ts` `hasArtifacts` | **The gate is blind.** `DocsCard` renders only when `hasArtifacts()` is true, and that excludes `spec_ready`. At the one moment the tab asks for a decision, Requirements / Design / Tasks / `gate_<id>.md` are unreachable from it. The the shell version had tabs over those four documents inside the gate card (`docs/ui.md` § *The gate shows what is being approved*); the port lost them | Gate card with the four documents inline, R<n> list, crates, "open in editor" |
| U2 | `settings/HarnessSection.tsx` | **"New feature" lives in Settings.** No entry in the session `+` menu, the palette, the empty centre or the Features panel (`grep -in feature shell/SessionMenu.tsx shell/sessionMenuItems.ts palette/entries.ts` is empty) | A *Feature draft* centre view (sibling of PR compose), reachable from `+`, palette, panel, `⌘⇧N` |
| U3 | draft flow | **No "where".** Registration binds the feature to the current checkout. Forge is a worktree manager and the harness rule is one open feature per checkout, so the natural choice — *new worktree from `main` named after the slug* — is not offered | Draft view: `Run in` = this checkout / new worktree (branch from slug, provisioning as today) |
| U4 | `settings/HarnessSection.tsx` | **No role → provider picker** (H8) | Settings › Harness › *Roles*: Spec / Implement / Review, each a `Select` over installed providers and profiles, capability-filtered (H9); persisted to `ui.harness.*` |
| U5 | `panels/FeaturesPanel.tsx` `FeatureCard`, `shell/Sidebar.tsx` | **Running steps show nothing live.** The card prints `job.last_line`, which is `None` until the job ends (H4). The sidebar tree lists sessions only; headless jobs never appear under the checkout, although the docs describe them as rows of the tree | Daemon broadcasts a throttled `last_line`; panel and sidebar show role glyph, state, elapsed, provider, live line |
| U6 | `FeatureView.tsx` layout | **A stack, not a stage.** Gate → Agents → Preview → Actions → Docs → Stream → Runs → Timeline → Validate, all always expanded. There is no stepper saying *where* the feature is, what ran it, how long each stage took | Header stepper (Spec → Gate → Implement → Review → Done) with provider, duration, attempt count per stage; body as tabs: Overview / Stream / Documents / Timeline |
| U7 | blocked state | **Blocked is a badge.** The reason lives in the last `feature_blocked` event's JSON `detail`; nothing surfaces it, and the only way forward is *Run <step>* | Blocked card: reason, the failing job's tail, actions *Retry step*, *Re-run from spec*, *Close as blocked* |
| U8 | actions row | **No Diff, Commit or Open PR** from the feature (plan-harness-ui §3.2 listed *Show diff*; `viewsStore.openDiff` exists) | *Diff* scoped to the feature's workspace; after `done`: *Commit…* (runs `committer`) and *Open PR…* (PR compose seeded from `impl_<id>.md`) |
| U9 | `shell/AttentionBar.tsx` | Only `spec_ready` reaches the strip. `blocked` and *done, uncommitted* are the other two states that wait on a person | Attention items: Approval, Blocked, Ready to commit; oldest first, one row + count |
| U10 | `FeatureView.tsx` `Timeline` | Raw `ts` and JSON `detail` strings | Event rows with kind icon, relative time, duration since the previous stage event, provider, job link; grouped by stage |
| U11 | `HarnessSection.tsx` | *Not initialized* is a dead end | *Initialize harness* scaffolds `harness/` from an embedded template (features.json with rules, CHECKPOINTS skeleton, PROTOCOL.md) |
| U12 | `HarnessSection.tsx` | Rules are invisible | Rules group: `max_review_rounds`, `max_gate_attempts`, `max_step_attempts`, `max_step_minutes`, `worktree_per_feature`; validated, written through the daemon |
| U13 | `panels/LieutenantPanel.tsx` | Good as is; the answer is prose | Quick prompts (*why blocked?*, *summarize the review*, *what changed?*), and a *Propose* affordance that fills the gate reason field — never resolves a gate itself |
| U14 | feedback | Step finished / blocked / gate opened produce no notification outside the panel | Toast per outcome (`plan-ui-ux.md` U16) plus the OS notification the attention path already has for bells |
| U15 | project view | With worktrees running features in parallel there is no place that shows *all* features by stage | Board view (columns = stages) as a centre view; optional, Phase 5 |

---

## 4. Orca orchestration — the verdict

`docs/orca/orchestration.md` is the deep dive (bundle read, 18-row comparison,
nine ranked recommendations). Read against the *live* guide of the installed
binary (`orca skills get orchestration`, v1.4.188), the model has not moved:
`worker-start` composes worktree + terminal + dispatch; `worker_done` is a
message with `--outcome succeeded|failed`, `--task-id`, `--dispatch-id`,
`--files-modified`; the coordinator is an agent running `check --wait --types
worker_done,escalation,question`; gates are `gate-create` / `gate-resolve`; a
task circuit-breaks after three failed dispatches; `worker-start --retry-of`
never inherits placement; `worker-release` is post-completion cleanup, never
cancellation; every mutation is idempotent through `--retry-request`.

### Borrow

| Orca mechanism | Forge form |
|---|---|
| Task ≠ Dispatch — a retry is a new row | `attempts[]` per feature: `{step, n, job, provider, started, settled, outcome}` (§6.2) |
| `worker_done --outcome` with a validated payload | the step envelope, by schema when the provider has one, by sentinel file otherwise (§6.4) |
| Gate as a row that makes the task un-startable, re-derived on restart | transition table + `gate` field; `Implement` refuses without a resolved gate (§6.3) |
| Circuit breaker at 3 | `max_step_attempts` rule, distinct from `max_review_rounds` (§6.7) |
| Heartbeat + stale sweep (warn only) | last-output clock on the job row; warn at N min, cancel at `max_step_minutes` (§6.7) |
| Idempotent mutations | `HarnessAdvance` carries the feature's current `revision`; a stale one is a no-op with the current row returned (§6.3) |
| Base-drift preamble | `step_prompt` prepends "your worktree is N commits behind `main`" from `git-service` (§6.6) |
| Decision-gate resolution injected into the next dispatch | the human's `revise` / `block` reason is written into `gate_<id>.md` **and** the next step's prompt (§6.8) |
| Bundled guide + "go re-read the guide" recovery | `harness/PROTOCOL.md` named in every prompt; a step whose envelope is missing is told exactly which section it skipped (§6.6) |

### Refuse

- **An LLM coordinator.** Orca retired its scheduler because its workers are
  arbitrary terminals. Forge spawns and reaps its workers; the daemon *is* the
  authority. Keep `harness_runner.rs` deterministic. The lead skill becomes a
  client that asks the daemon to advance (§6.6), not a second brain.
- **Pane-key lifecycle authority, federation, legacy-compat banners.** Cost with
  no question behind it here.
- **Blocking `ask` from a headless step.** A job that waits is a job that does
  not end observably. A step that needs input writes a `question` in its
  envelope and exits `blocked`; the human answers at the gate surface and the
  answer rides into the retry prompt. The exception is Phase 4: over ACP a
  worker *can* ask mid-run without a PTY, and the same envelope shape carries it.
- **A DAG *between* features.** The conflicting resource is a working tree; one
  open feature per checkout is the right serialization. Ordering, if ever, is
  between tasks *inside* one spec.
- **`reset --all`.** `git checkout harness/` is the reset and keeps the trail.

---

## 5. Other references, and what each one contributes

| Reference | What it is | The one thing to take |
|---|---|---|
| [OpenAI Symphony](https://openai.com/index/open-source-codex-orchestration-symphony/) (Apache-2.0, Apr 2026) | Spec that turns an issue tracker (Linear) into the control plane: one fresh workspace per issue, a poller that picks up work and restarts crashed or stalled agents, humans review PRs | **The tracker is the queue.** `from-issue` already exists; add the reverse edge (status → issue label/comment) and a poll mode, so a GitHub Project column can *be* the Features panel (Phase 5) |
| [Vibe Kanban](https://vibekanban.com/) (Apache-2.0, community since Bloop closed in Apr 2026) | Board UI; a worktree per task; executors for Claude / Codex / Amp / Cursor / Gemini; line-by-line diff review with comments | **Board + worktree-per-task + review as first-class.** U3, U15, U8 |
| [Superset](https://superset.sh/) (source-available) | Any CLI agent, worktree per task, and — the distinguishing part — a CLI, a TypeScript SDK and an **MCP server** for programmatic control | **Expose the orchestrator as an API**, so an external orchestrator or an agent in a terminal can drive it without the GUI (Phase 5) |
| Conductor (proprietary, macOS) | Claude / Codex / Cursor in parallel worktrees; polish per session | The bar for the session UI, not for orchestration |
| [Claude Code agent teams](https://code.claude.com/docs/en/agent-teams) (experimental) | Lead + teammates, shared task list, `TaskCreated` / `TaskCompleted` / `TeammateIdle` hooks that can exit 2 to keep a teammate working | **Hooks as gates, not prompts.** Forge already does this for `Stop` / `SubagentStop`; extend to the TS gate (H11). Provider-specific, so it stays a *transport detail*, never the protocol |
| [Agent Client Protocol](https://zed.dev/acp) (Zed, JSON-RPC 2.0 over stdio; Gemini CLI native, Claude Code and Codex via adapters, JetBrains and Kiro clients) | The LSP of coding agents: the client launches the agent as a subprocess and gets structured session updates, tool calls and permission requests | **A second worker transport** that is provider-neutral by construction, streams structured events without per-CLI parsing, and allows a mid-run question (Phase 4) |
| [12-factor agents](https://github.com/humanlayer/12-factor-agents), [spec-kit](https://github.com/github/spec-kit), [betta-tech harness](https://github.com/betta-tech/ejemplo-harness-subagentes) | already the foundation (`plan-subagent-harness.md` §0) | Unified event log, human gates as structured artefacts, specs as the reviewable unit, reviewer without `Write` |

### Comparison

| | Forge today | Orca | Symphony | Vibe Kanban | Superset | Claude agent teams |
|---|---|---|---|---|---|---|
| Unit of work | feature (spec → review) | task + dispatch | issue | task/card | task | task list item |
| State | git-tracked JSON + JSONL | SQLite | the tracker | SQLite | local DB | in-session |
| Coordinator | Rust daemon | an agent + durable bus | poller | app | app / SDK | lead session |
| Worker interface | CLI argv table | injected preamble + CLI hooks | Codex | executors per CLI | any CLI | Claude only |
| Human gate | `spec_ready` + Approve | `gate-create` row | PR review | board column | board + review | hooks |
| Retry | none (block) | 3, circuit breaker | restart on crash | manual | manual | — |
| Isolation | worktree (optional) | worktree | fresh clone | worktree | worktree | shared |
| Multi-provider | yes, no role picker | yes | no | yes | yes | no |
| Programmatic | `scripts/harness`, IPC | CLI | spec | CLI + web | CLI + SDK + MCP | — |

Forge is the only one whose coordinator is deterministic **and** whose state is
a reviewable file in the repository. That combination is the product.

---

## 6. Target design — the Forge orchestrator

### 6.1 Principles

1. **State is a file in the repository**, human-readable, git-tracked, one per
   repository, shared by its worktrees. Nothing moves to SQLite.
2. **The coordinator is deterministic Rust** and advances only on observable
   facts: an exit code, a parsed envelope, a human decision. LLMs are workers.
3. **A provider is a descriptor plus a transport.** No crate outside `agents`
   names one. A step names a *role*; the user maps roles to providers.
4. **Every step ends with an envelope** (§6.4). Exit 0 with no envelope is a
   failed step, not a success.
5. **One worker protocol, provider-neutral** (`harness/PROTOCOL.md`). Skills,
   agent files and prompts are views of it, generated or checked against it.
6. **Transitions are a table**, gates are a field, and every mutation is
   idempotent under a revision.
7. **Budgets are rules**, read by the code that enforces them. A rule nothing
   reads is deleted.
8. **Liveness is measured, not asked for.** The daemon owns the pipe; the
   last-output clock is the heartbeat.
9. **Isolation by default.** A feature runs in its own worktree unless the user
   says otherwise.
10. **Every decision a human makes is reachable from every surface** — tab,
    attention strip, CLI, IPC — and lands in the same event.

### 6.2 Nouns

| Noun | Today | Target |
|---|---|---|
| **Feature** | row in `features.json` | unchanged, plus `revision`, `gate`, `attempts[]`, `worktree` |
| **Stage** | `HarnessStep` (Spec, Implement, Review) | unchanged; `Commit` added as an optional fourth (the `committer`) |
| **Attempt** | implicit (`review_rounds`, `gate_attempts`) | explicit: `{step, n, job, provider, transport, started_at, settled_at, outcome, envelope}` — Orca's Dispatch at feature granularity |
| **Gate** | `status == "spec_ready"` + `gate_<id>.md` | `gate: {kind: "spec_approval", opened_at, resolved_at?, decision?, reason?}` on the row; the markdown stays the brief |
| **Worker** | a job | a job **or** an ACP session (§6.5), both producing the same event stream and the same envelope |
| **Envelope** | `last_line` + a markdown file the runner greps | a JSON object with a schema (§6.4) |

### 6.3 State machine, as a table

```
pending      --spec job succeeds, envelope ok-->      spec_ready   (gate opened)
spec_ready   --ApproveSpec (human)-->                 in_progress  (implement job starts)
spec_ready   --ReviseSpec (human)-->                  pending      (spec job starts, reason in prompt)
in_progress  --implement job succeeds-->              in_review    (review job starts)
in_review    --envelope APPROVED-->                   done
in_review    --envelope CHANGES_REQUESTED, rounds<max--> in_progress (implement job starts, review in prompt)
any running  --job Failed, attempts<max-->            same status  (retry, dirty-tree note in prompt)
any          --job Failed, attempts==max-->           blocked
any          --envelope outcome=blocked-->            blocked      (question in gate)
any          --Block (human)-->                       blocked
blocked      --RetryStep (human)-->                   the step's running status
done|blocked --(no transitions out except Reopen)-->
```

Enforced in `harness-service` by one function, `transition(feature, event) ->
Result<Feature, Refusal>`, and nowhere else. `set_status` becomes private.
`RunHarnessStep` refuses with `Conflict` when the feature has a live attempt,
and with `InvalidRequest` when the transition table has no row — with an
explicit `force: true` that is allowed, logged as `gate_bypassed`, and never
set by the GUI. `HarnessAdvance` carries `revision`; a stale revision returns
the current row and does nothing. `require_human_spec_approval: false` is the
one way `spec_ready` auto-approves, and it is read here.

### 6.4 The step envelope

Asked for by schema where the provider has one (`SchemaStyle`), and **also**
written by the agent to `harness/progress/step_<id>_<attempt>.json` — the file
is the fallback for providers without a schema and the audit copy for those
with one. The runner prefers the schema answer, then the file, then blocks.

```json
{
  "step": "review",
  "outcome": "succeeded | failed | blocked",
  "verdict": "APPROVED | CHANGES_REQUESTED",
  "summary": "one paragraph",
  "files_modified": ["crates/daemon/src/core.rs"],
  "artifacts": ["harness/progress/review_7.md"],
  "question": null,
  "gate_command": {"ran": "scripts/dev check", "exit": 0, "attempts": 1}
}
```

`verdict` is required for `review`, forbidden elsewhere. `question` is what a
`blocked` outcome carries and what the gate surface shows. `gate_command` is
how H14 is closed on the daemon path: the runner reads `attempts` from the
envelope instead of trusting the agent to edit `features.json`.

### 6.5 Worker transports

```rust
pub enum WorkerTransport {
    /// Spawn the CLI once, read stdout to EOF (today's jobs.rs).
    Cli(HeadlessSpec),
    /// Spawn an ACP agent (or adapter) and speak JSON-RPC 2.0 over stdio.
    Acp(AcpSpec),
}
```

`AcpSpec` is `{command, args, env}` plus a permission policy. Over ACP the
daemon receives `session/update` events (text, tool calls, file edits) as
typed data instead of a dialect `summarize_stream_line` has to know, gets
`request_permission` calls it can answer from policy or route to the gate
surface, and can send a follow-up `prompt` on the same session — which is the
one thing a headless CLI cannot do. Claude Code and Codex ship adapters; Gemini
CLI, Kiro and others are native. The job row, the log file, the envelope and
the UI are identical for both transports; only `spawn_job` / `follow_job` gain
a second arm. `AgentCapabilities` grows `can_review` (has schema or ACP),
`can_stream` (has stream args or ACP), and Settings filters on them (H9).

### 6.6 One worker protocol

`harness/PROTOCOL.md` — one document every step is told to read, in this order:

1. resolve the state root (`scripts/harness root` or `$FORGE_HARNESS_ROOT`);
2. what your role reads, what it writes, what it must not touch;
3. the events you append, only through `scripts/harness event`;
4. the envelope you must write, and where;
5. the budgets (`max_gate_attempts`, `max_step_minutes`) and what to do when
   you hit one — write the envelope with `outcome: blocked` and stop;
6. the invariants pointer (`AGENTS.md` § *Boundaries And Invariants*,
   `harness/CHECKPOINTS.md`).

`step_prompt` stops naming skills. It carries: the role, the feature id and
slug, the absolute state root, `spec_raw`, the relevant paths for this attempt
(review to address, question that was answered, base-drift summary), and the
line *"Follow harness/PROTOCOL.md § <role>"*. The Claude and Cursor skills
become thin: their body is the protocol section plus the platform-specific
mechanics (`Agent(...)` subagents, `isolation: worktree`). A unit test in
`harness/test` asserts every rule sentence in the skills appears in
`PROTOCOL.md`, so there is one place to edit. The `/feature` and `/feature-go`
leads stop editing `features.json` by hand and call `scripts/harness register`
/ `scripts/harness advance <id> approve` — which go through the daemon when it
is running and through the same `transition()` logic (ported to TS, or the
daemon exposing a `forge harness` subcommand) when it is not.

### 6.7 Budgets and liveness

| Rule | Default | Enforced by |
|---|---|---|
| `max_review_rounds` | 2 | `transition` on `CHANGES_REQUESTED` |
| `max_gate_attempts` | 3 | the envelope's `gate_command.attempts` |
| `max_step_attempts` | 2 | `transition` on `JobState::Failed` |
| `max_step_minutes` | 90 | a watchdog thread in `jobs.rs`: cancel, then `transition(Failed)` with a `TimedOut` reason |
| `stale_after_minutes` | 10 | the same watchdog: warn (event `step_stale`, toast), never kill |
| `max_concurrent_jobs` | 2 | unchanged |

`follow_job` records `last_line` and `last_output_at` on the row, throttled to
`OUTPUT_FLUSH`, so the panel, the sidebar and the watchdog read one fact. On
daemon start, `read_step_trace` (fixed to the keys the runner writes) is called
for every open feature; an attempt with no live job is settled as
`Failed(daemon_restarted)` and goes through the same retry budget.

### 6.8 Human gates, everywhere

One request, `HarnessAdvance { id, revision, action }`, with
`ApproveSpec | ReviseSpec{reason} | Block{reason} | RetryStep | Reopen`. It is
callable from the gate card, the attention strip, `scripts/harness advance`,
the Lieutenant's *Propose* (which only fills the reason, the human clicks), and
the MCP surface (Phase 5). Every path appends the same `human_gate_resolved`
event with `decision`, `reason`, `via`. The reason is written into
`gate_<id>.md` and into the next attempt's prompt verbatim, which is Orca's
`DECISION GATE RESOLVED` block.

### 6.9 Worktree per feature

`RegisterHarnessFeature` gains `placement: CurrentCheckout | NewWorktree{branch}`.
`NewWorktree` calls the existing `CreateWorktree` path (provisioning, copy
list, setup script), binds the feature to it, and records `worktree: {workspace_id,
branch, base}`. `done` offers *Remove worktree* after commit. The base-drift
note (§4) is computed against `base` at every attempt start.

### 6.10 Programmatic surface

- `forge harness` — a daemon-backed subcommand (`forge-daemon harness list|show|register|advance|run|watch --json`) that is what `scripts/harness` calls when a daemon is up, so the Bun CLI stops being a second writer.
- An MCP server (stdio) over the same requests: `list_features`, `register_feature`, `advance`, `run_step`, `read_artifact`, `watch`. This is what makes the orchestrator drivable by *any* agent in a terminal, by CI, or by a Symphony-style poller, without the GUI.
- Issue sync: `from-issue` already reads; add `status → label` and `blocked → comment` writes behind `[github] sync_issues`, through `run_gh`.

### 6.11 The UI model

```
+  New feature…  ──▶  Feature draft (centre view)
                       title · spec · from issue · run in: [this checkout | new worktree] · roles (from settings, overridable)
                       [Start]  → register + spec job

Features panel (right) ──▶ Feature tab (centre)
  cards: #id title · stage pill          ┌ stepper: Spec ✓ 2m claude · Gate ● · Implement · Review · Done
  live line · provider · elapsed         │ Overview: gate card (docs inline) | blocked card | agents | actions
  filters: scope · stage · text          │ Stream | Documents | Timeline | Diff
                                         └ actions: Approve · Revise · Block · Retry · Diff · Commit… · Open PR…

Attention strip: Approval #7 · Blocked #9 · Ready to commit #4   (+n more)
Sidebar: under each checkout, sessions and headless steps as rows with role glyph and live line
Settings › Harness: Roles · Rules · Initialize · Validate
Board (Phase 5): columns Pending · Spec ready · In progress · In review · Done · Blocked
```

---

## 7. Phases and deliverables

Each phase ships alone and is independently useful. Files are named so the
work can be registered as harness features itself.

### Phase 0 — Hygiene (½ day) · ✅ done

| # | Deliverable | Files |
|---|---|---|
| 0.1 | Block features 2–9 with reason `test data`, or delete them; regenerate `history.md` | `harness/features.json`, `harness/progress/` |
| 0.2 | Rewrite C4 and C7 for the Tauri GUI: `apps/tauri` reaches the daemon only through the host's IPC bridge; no hex outside `theme/tokens.ts`; no `std::fs` on a workspace; the TS gate is `oxlint` + `oxfmt` + `tsc --noEmit` + vitest | `harness/CHECKPOINTS.md`, `.claude/agents/implementer.md`, `.claude/skills/invariant-c4/SKILL.md`, `invariant-c7` |
| 0.3 | `fmt-and-check.sh`: for `apps/tauri/**.ts(x)` run `oxfmt` and `oxlint` on the file and `tsc --noEmit -p apps/tauri` | `.claude/hooks/fmt-and-check.sh` |
| 0.4 | `EventType` gains `spec_started`, `step_retried`, `step_stale`, `gate_bypassed`; `read_step_trace` reads `job` | `harness/src/events.ts`, `crates/harness-service/src/lib.rs` |
| 0.5 | `docs/harness.md`: delete "UI integration is not implemented yet"; add the daemon path diagram from `harness_runner.rs` | `docs/harness.md` |

### Phase 1 — Correctness core (3–4 days) · ✅ done

The table lives in `crates/harness-service/src/transition.rs` — one pure
function over `(row, rules, trigger)`, with `set_status` and `bump_review_round`
deleted so nothing else can write a status. `apply()` is the shell that loads
under the lock, transitions, appends the events and saves; events are written
*before* the row so a crash leaves the trail ahead of the status, which
`validate.ts` accepts, rather than behind it, which it does not.

| # | Deliverable | Files | Acceptance |
|---|---|---|---|
| 1.1 | Atomic `save`: write `features.json.tmp`, fsync, rename; advisory lock (`lockfile.rs` pattern) around every read-modify-write, bounded wait, warning on fallback | `crates/harness-service/src/lib.rs` | a test with two threads bumping `review_rounds` 100 times ends at 100 |
| 1.2 | `transition()` table (§6.3); `set_status` private; `advance` and `run_harness_step` route through it; `revision` on the row; `force` flag logged as `gate_bypassed` | `crates/harness-service/src/lib.rs`, `crates/domain/src/harness.rs`, `crates/daemon/src/{harness_runner,harness_io}.rs`, `crates/protocol/src/request.rs` | `RunHarnessStep{Implement}` on `spec_ready` returns `InvalidRequest`; replaying `ApproveSpec` with the old revision starts no second job |
| 1.3 | Single flight: refuse a step while `Inner::job_processes` or `job_queue` holds a job for the feature | `crates/daemon/src/harness_runner.rs` | two concurrent `RunHarnessStep` → one `Job`, one `Conflict` |
| 1.4 | `attempts[]` on the row; `*_started` events carry `attempt`, `provider`, `transport` | `crates/domain/src/harness.rs`, `harness_runner.rs`, `harness/src/harness.ts` | `scripts/harness show` prints the attempts |
| 1.5 | Structured verdict first: read the last schema-conformant line of the job log, then `parse_verdict` on the file, then block | `harness_runner.rs` `settle_review`, `jobs.rs` (`final_result` on the row) | the existing `parse_verdict` tests stay; a new test with a schema answer and an empty file lands `done` |
| 1.6 | `max_step_attempts` + retry on `Failed`; `step_retried` event; retry prompt states the tree may be dirty | `harness_runner.rs`, `harness/features.json` rules, `validate.ts` | a job that exits 1 once and 0 the second time reaches the next stage |
| 1.7 | Live `last_line` / `last_output_at`, throttled; watchdog thread (`stale_after_minutes` warn, `max_step_minutes` cancel → `Failed(TimedOut)`) | `jobs.rs`, `crates/domain/src/job.rs` | a `sleep 600` job is cancelled at the budget and the feature retries or blocks |
| 1.8 | Restart recovery: on boot, `read_step_trace` for every open feature; unsettled attempt with no job → `Failed(daemon_restarted)` through 1.6 | `crates/daemon/src/core.rs` startup, `harness-service` | kill the daemon mid-implement, restart: the feature retries once and the timeline says why |
| 1.9 | Read `require_human_spec_approval` and `require_green_gate_to_close`, or delete them | `harness-service`, `validate.ts`, `features.json` | no rule in `rules` is unread |

**Open after the Phase 1 review (2026-09-01)**, none of them gate-red:

- Single flight has a window: the live-job check and the job insertion sit
  under different locks with the file lock between them. Reserve the feature
  under the core lock (`Inner::harness_starting`, released by `Drop`, the
  `pr_opening` pattern) before `apply`.
- Boot recovery races detection: `recover_harness_after_restart` runs while
  `detect_agents` is still on its thread, so with no `ui.harness.*` key the
  retry finds no provider and the second failure blocks the feature. Run the
  recovery after detection, or settle only and retry lazily.
- `verdict_in_log` reads the whole transcript; read a bounded tail (C7).
- `harness/src/cli.ts` still writes `features.json` with `writeFileSync`, no
  lock, no rename — closed by 2.7.
- `Reopen` does not check that the checkout is free.
- `apply`'s doc comment says row-then-events; the code and the inline comment
  say events-then-row. Fix the sentence.
- `gate_attempts` is never incremented on the daemon path until the envelope
  (2.3) carries `gate_command.attempts`; `require_green_gate_to_close` is a
  counter proxy until then.

### Phase 2 — Provider-neutral protocol and roles (2–3 days)

| # | Deliverable | Files |
|---|---|---|
| 2.1 | `harness/PROTOCOL.md` (§6.6), one section per role, plus the envelope schema as `harness/schema/step.json` | new |
| 2.2 | `step_prompt` rewritten: role, ids, absolute root, attempt context, base-drift line, "follow PROTOCOL.md § role"; no skill names | `crates/daemon/src/harness_runner.rs`, `crates/git-service` (ahead/behind against `base`) |
| 2.3 | Envelope plumbing: per-role schema (`step.json` narrowed), sentinel file fallback, `outcome: blocked` → gate with `question` | `harness_runner.rs`, `harness-service`, `domain::HarnessGate` |
| 2.4 | `AgentCapabilities::{can_review, can_stream}` derived from `HeadlessSpec`; `harness_provider(step)` refuses a provider that cannot fill the role, naming the reason | `crates/domain/src/agent.rs`, `crates/agents/src/builtins.rs`, `harness_runner.rs` |
| 2.5 | Settings › Harness › Roles: three `Select`s (provider or profile), capability-filtered, writing `ui.harness.{orchestrator,executor,reviewer}`; the timeline shows the provider per attempt | `apps/tauri/src/settings/HarnessSection.tsx`, `settings/harnessRoles.ts` (+ test), `harness/types.ts` |
| 2.6 | Skills and agent files reduced to protocol section + platform mechanics; `harness/test/protocol.test.ts` asserts every rule line of the skills is in `PROTOCOL.md` | `.claude/agents/*.md`, `.claude/skills/*`, `.cursor/skills/*`, `harness/test` |
| 2.7 | `scripts/harness register|advance` go through the daemon when it is up (`forge-daemon harness …`), else through the TS port of `transition()` | `harness/src/cli.ts`, `crates/daemon/src/main.rs` |

### Phase 3 — UI (4–5 days)

| # | Deliverable | Files |
|---|---|---|
| 3.1 | Feature draft view (`{kind:"feature_draft"}`): title, spec, from-issue, `Run in` (this checkout / new worktree from base with branch from slug), role overrides; *Start* registers and opens the feature tab. Entries: session `+` menu, palette *New feature…*, empty centre, Features panel `+`, `⌘⇧N` | `workbench/FeatureDraftView.tsx`, `workbench/views.ts`, `store/viewsStore.ts`, `shell/sessionMenuItems.ts`, `palette/entries.ts`, `actions/actions.ts`, `shell/EmptyCenter.tsx`, `panels/FeaturesPanel.tsx` |
| 3.2 | Gate card with Gate / Requirements / Design / Tasks inline (lazy `loadArtifact`), R<n> list parsed from `requirements.md`, crates, Approve / Revise (reason required) / Block | `workbench/FeatureView.tsx` → `workbench/feature/GateCard.tsx`, `harness/requirements.ts` (+ test) |
| 3.3 | Stepper header: stage, provider glyph, duration, attempt count; body tabs Overview / Stream / Documents / Timeline / Diff | `workbench/feature/Stepper.tsx`, `harness/stages.ts` (+ test: stages from `attempts[]` and events) |
| 3.4 | Blocked card: reason, failing job tail, *Retry step*, *Re-run from spec*, *Close as blocked* | `workbench/feature/BlockedCard.tsx` |
| 3.5 | Actions: *Diff* (workspace-scoped `openDiff`), *Commit…* (`committer` as a job, message from `impl_<id>.md`), *Open PR…* (PR compose seeded from `impl_<id>.md`) | `FeatureView.tsx`, `harness/api.ts`, daemon `RunHarnessStep{Commit}` |
| 3.6 | Live steps: `last_line` on cards; job rows under the checkout in the sidebar tree with role glyph, state marker, elapsed | `panels/FeaturesPanel.tsx`, `shell/Sidebar.tsx`, `shell/tree.ts` (+ test) |
| 3.7 | Attention strip: Approval / Blocked / Ready to commit; oldest first | `shell/AttentionBar.tsx`, `harness/types.ts` `attentionItems()` (+ test) |
| 3.8 | Timeline rendering: kind icon, relative time, stage duration, provider, job link; grouped by attempt | `workbench/feature/Timeline.tsx`, `harness/timeline.ts` (+ test) |
| 3.9 | Settings: Rules editor, *Initialize harness* (template embedded in `harness-service`), role picker from 2.5 | `settings/HarnessSection.tsx`, `harness-service::init_template` |
| 3.10 | Toasts for step finished / blocked / gate opened; OS notification for gate opened | `ui/Toast.tsx` (from `plan-ui-ux.md` U16), `runtime/events.ts` |
| 3.11 | Lieutenant quick prompts and *Propose* (fills the reason field) | `panels/LieutenantPanel.tsx` |

Performance notes that bind this phase (`docs/performance.md`): the job tail
store is already a ring buffer with indexed writes (`store/jobOutput.ts`,
`plan-ui-ux.md` P1–P3 and P7, in the working tree as of 2026-09-01), so the
sidebar may read `last_line` per row without a per-batch copy; artefacts load
on click, never in the detail poll; nothing in the stepper is recomputed per
frame — `stages()` is a memo over `attempts[]` and the timeline.

### Phase 4 — ACP transport (3–4 days, after 2)

| # | Deliverable | Files |
|---|---|---|
| 4.1 | `WorkerTransport::{Cli, Acp}`; `AcpSpec{command,args,env,permissions}` on the descriptor; built-ins for Claude Code and Codex adapters and Gemini CLI native | `crates/domain/src/agent.rs`, `crates/agents/src/builtins.rs` |
| 4.2 | `crates/daemon/src/acp.rs`: spawn, `initialize`, `session/new`, `session/prompt`, stream `session/update` into the same `JobOutput` events and log file; answer `request_permission` from policy (read: allow; write inside the worktree: allow; network / outside: block → gate) | new |
| 4.3 | Mid-run question: an ACP worker's question becomes a `question` gate without ending the job; the human's reply is sent as the next `prompt` | `harness_runner.rs`, `domain::HarnessGate` |
| 4.4 | Capability matrix in Settings shows transport per provider; `can_review` true for every ACP provider (structured final message) | `settings/HarnessSection.tsx` |
| 4.5 | Integration test with a fake ACP agent in `test-support` | `crates/test-support`, `crates/daemon/tests/integration.rs` |

### Phase 5 — Programmatic surface, board, issue sync (3 days)

| # | Deliverable | Files |
|---|---|---|
| 5.1 | `forge-daemon harness …` subcommands with `--json`; `scripts/harness` delegates when the socket answers | `crates/daemon/src/main.rs`, `harness/src/cli.ts` |
| 5.2 | MCP server (stdio) over the harness requests; documented in `docs/harness.md` § *Driving the harness from an agent* | `crates/daemon/src/mcp.rs` or `apps/mcp/` |
| 5.3 | Board view (`{kind:"board"}`): columns by stage, cards from the panel, drag disabled (status changes are decisions, not gestures) | `workbench/BoardView.tsx` |
| 5.4 | Issue sync: label per stage, comment on blocked, PR link on done; `[github] sync_issues = false` by default | `crates/git-service/src/github.rs`, `harness_runner.rs` |

### Phase 6 — Evaluation loop (ongoing)

| # | Deliverable |
|---|---|
| 6.1 | Two golden features in `harness/eval/`: a happy path (small real change) and a failure path (a deliberate C4 violation the reviewer must reject). `scripts/harness eval` runs both headless on each configured provider and reports stage durations, rounds, cost |
| 6.2 | Cost per feature: every attempt carries `provider_session_id`; `UsageAnalytics` gains a `feature_id` filter so the tab shows tokens and estimated cost per stage |
| 6.3 | Metrics in the Stats page: median time to `spec_ready`, approval rate, rounds per feature, blocked rate by reason, per provider and per role |

---

## 8. Acceptance

The orchestrator is done when all of the following hold on this repository:

1. A feature registered from the draft view with *new worktree*, spec by Claude,
   implement by Codex, review by Claude, reaches `done` with no terminal opened,
   and the tab shows the spec at the gate, the live line during implement, the
   verdict from the schema, and the diff.
2. The same feature with the reviewer set to a provider that cannot review is
   refused at Settings, not at the end of a 40-minute run.
3. Killing the daemon during implement and restarting it retries the step once
   and says so in the timeline; killing the agent CLI is the same.
4. Two `RunHarnessStep` requests for one feature produce one job.
5. `git log -p harness/features.json` reads as a sequence of decisions; no row
   ever has a status the transition table did not produce (`gate_bypassed` is
   the only exception and is an event).
6. `harness/PROTOCOL.md` is the only place a worker rule is stated; the skills
   test proves it.
7. `docs/orca/orchestration.md` §3 R1–R8 are each either implemented or
   explicitly declined in this document.

## 9. What not to do

- Do not add a Cargo dependency for JSON-RPC in Phase 4 without checking
  `deny.toml`; a hand-rolled length-delimited reader over `serde_json` is under
  200 lines and the daemon already has one for the IPC.
- Do not move harness state into SQLite. The value of the harness is that the
  state is a reviewable diff.
- Do not let the GUI write `harness/` (ADR-012). Every mutation goes through
  the daemon, including *Initialize*.
- Do not put a second coordinator in an LLM. The lead skills become clients.
- Do not make `ask` block a headless job. Block the feature, gate the question.
- Do not build a DAG between features. One open feature per checkout stays.
- Do not run a step without an envelope contract. Exit 0 is not success.
- Do not ship 1.7's hard budget before 1.7's warning has been watched on real
  runs; Orca's guide is right that long implementations run 15–60 minutes.

## 10. Sources

- This repository: `docs/orca/orchestration.md`, `docs/orca/agent-providers.md`,
  `docs/orca/terminal-sessions.md`, `docs/harness.md`, `docs/plan-subagent-harness.md`,
  `docs/plan-harness-ui.md`, `docs/ui.md` § *Harness / feature workflow*,
  `docs/plan-ui-ux.md`, `AGENTS.md`, `harness/CHECKPOINTS.md`.
- Orca: `orca skills get orchestration` (v1.4.188, read 2026-09-01) and the
  bundle analysis above.
- [OpenAI Symphony](https://openai.com/index/open-source-codex-orchestration-symphony/) ·
  [Help Net Security summary](https://www.helpnetsecurity.com/2026/04/28/openai-symphony-codex-orchestration-linear/) ·
  [Better Stack guide](https://betterstack.com/community/guides/ai/openai-symphony/)
- [Vibe Kanban](https://vibekanban.com/) · [awesome-agent-orchestrators](https://github.com/andyrewlee/awesome-agent-orchestrators)
- [Superset comparisons](https://superset.sh/compare) (Conductor, Orca, agent orchestrators)
- [Claude Code agent teams](https://code.claude.com/docs/en/agent-teams)
- [Agent Client Protocol](https://zed.dev/acp) · [ACP explained](https://blog.marcnuri.com/agent-client-protocol-acp-introduction) · [Kiro ACP docs](https://kiro.dev/docs/cli/acp/)
- [12-factor agents](https://github.com/humanlayer/12-factor-agents) ·
  [spec-kit](https://github.com/github/spec-kit) ·
  [betta-tech/ejemplo-harness-subagentes](https://github.com/betta-tech/ejemplo-harness-subagentes)
