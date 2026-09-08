# Implementation plan — Subagent harness (spec → implement → review)

**Status:** implemented (2026-08-25), phases 0-3. Phase 4 partial (2026-08-27):
`docs/harness.md`, `scripts/harness` (the human interface) and the Cursor skills in
`.cursor/skills/{feature,feature-go,feature-status}/`. Still optional: the
implementer's `isolation: worktree`, the `committer` subagent and per-invariant-domain
skills. The current behaviour is summarised in `AGENTS.md` §
*Harness* and in `docs/harness.md`.
**Date:** 2026-08-24 (plan) · 2026-08-25 (execution) · 2026-08-27 (interface)
**Scope:** add to this repository a multi-agent orchestration *harness* in the style of
[betta-tech/ejemplo-harness-subagentes](https://github.com/betta-tech/ejemplo-harness-subagentes),
with a **single action** (`/feature "<specification>"`) that fires the whole
cycle: research → spec → human approval → implementation → review → close, with
every intermediate result on disk and the verifications forced by hooks, not by
the model's good will.

---

## 0. Research findings

### 0.1 Anatomy of the reference repository

`betta-tech/ejemplo-harness-subagentes` is a notes CLI in Python whose value is
not in the code but in the structure. Seven pieces, all version-controlled:

| Piece | Real function |
|-------|---------------|
| `AGENTS.md` | A navigation map with *progressive disclosure*: it is not a rulebook, it is a "which file to read and when" table |
| `CLAUDE.md` | Forces the `leader` role in **every** session and forbids the main thread from editing `src/` |
| `.claude/agents/{leader,implementer,reviewer}.md` | Three subagents with trimmed `tools:` — the reviewer has no `Write`/`Edit`, so it cannot "fix" what it reviews |
| `feature_list.json` | The machine's state: one feature at a time, `pending/in_progress/done/blocked`, with `acceptance` per feature |
| `progress/{current,history}.md` + `progress/impl_*.md`, `review_*.md` | The log on disk; per-subagent artefacts |
| `CHECKPOINTS.md` | C1–C5, objective criteria for "the final state is correct". *You evaluate the destination, not the path* |
| `init.sh` | An executable gate: environment + existence of the base files + JSON validation + tests. Exit code 0 or the session does not move on |
| `.claude/settings.json` | `PostToolUse` (tests after every edit) and `Stop` (`init.sh` before closing) hooks |

The three transferable ideas, in order of value:

1. **The anti-broken-telephone rule.** The subagents write their result to a file
   and return **one line** (`done -> progress/impl_7.md`). The lead never sees
   the diff in chat. This is what stops the orchestrator's context from degrading
   after three delegations.
2. **The verification is run by the harness, not by the agent.** The `Stop` hook
   runs `init.sh`; an optimistic report from the model cannot skip it.
3. **Separation of powers through `tools:`.** A reviewer without `Write` cannot
   turn into an implementer; the implementer does not close the feature itself.

The example's main weakness, which we fix below: `CLAUDE.md` imposes the `leader`
role on **every** session. In a teaching repo that works; in a real working repo
it turns every trivial question into an orchestration.

### 0.2 Repositories that follow the same pattern

| Repo | What it adds over the original |
|------|--------------------------------|
| [LuisFernandoAparicio21/agentic-spec-driven-development](https://github.com/LuisFernandoAparicio21/agentic-spec-driven-development) | An explicit copy of the harness (same file names, same C1–C5) plus what interests us here: a fourth subagent **`spec_author`**, the `spec_ready` status, Kiro-style specs (`specs/<id>-<slug>/{requirements,design,tasks}.md`), requirements in **EARS notation** and an `acceptance → R<n> → evidence` traceability table. It also documents the "review rejects → it gets fixed → is this a durable rule?" cycle |
| [carlosOlcina/secure-vault](https://github.com/carlosOlcina/secure-vault) | The same harness ported to OpenCode (`.opencode/agent/`) on a real Expo/React Native project, with **skills** by convention (`verify-before-done`, `feature-workflow`, `session-lifecycle`) instead of a monolithic `docs/conventions.md`, and a `committer` subagent |
| [github/spec-kit](https://github.com/github/spec-kit) | The de facto SDD standard: `constitution → specify → clarify → plan → tasks → analyze → implement`, each phase a slash command, `spec.md`/`plan.md`/`tasks.md` artefacts in the repo. It is where the idea "the spec is the reviewable artefact, not the chat" comes from |
| [humanlayer/advanced-context-engineering-for-coding-agents](https://github.com/humanlayer/advanced-context-engineering-for-coding-agents) | The **research → plan → implement** cycle and the technical justification for using subagents: they are not "more brains", they are *context isolation* — the noisy exploration happens in another window and comes back compressed |
| [10xChengTu/harness-engineering](https://github.com/10xChengTu/harness-engineering) (96★), [Habitat-Thinking/ai-literacy-superpowers](https://github.com/Habitat-Thinking/ai-literacy-superpowers) (45★), [ai-boost/awesome-harness-engineering](https://github.com/ai-boost/awesome-harness-engineering) | They package the same thing as a reusable plugin/skill and as a curated list of patterns |

Conceptual frame: *agent harness engineering* — "Agent = Model + Harness"
([Addy Osmani](https://addyosmani.com/blog/agent-harness-engineering/),
[O'Reilly Radar](https://www.oreilly.com/radar/agent-harness-engineering/)).

### 0.3 About Aston Martin

There is no public Aston Martin repository with this pattern. What does exist,
and is probably the remembered reference, is the multi-year agreement of the
**Aston Martin Aramco F1 Team with Cognition** (the Devin people) to run software
development tasks autonomously — a commercial use case, with no associated open
source
([autoracing1](https://www.autoracing1.com/pl/471252/formula-1-news-aston-martin-team-signs-ai-software-engineering-specialists-cognition/)).
The team with an Anthropic/Claude partnership in F1 is
[Williams](https://www.williamsf1.com/partners/claude). Neither of them publishes
their harness, so the actionable references are the ones in §0.2.

### 0.4 Constraints specific to this repository

What makes the example's harness impossible to copy as-is:

| forge-node's reality | Consequence for the design |
|----------------------|----------------------------|
| `cargo test --workspace` takes minutes, not seconds | The `PostToolUse` hook **cannot** run the full suite after every `Edit`. Only `cargo fmt` + `cargo check -p <crate>` |
| The canonical gate already exists: `scripts/dev check` (fmt → clippy → test) | We do not create an `init.sh` with its own test logic; the harness' `init.sh` **delegates** to `scripts/dev check` |
| `AGENTS.md` already has the *Boundaries And Invariants* section with ~20 very specific invariants (lock order, a single VT engine, append-only migrations, `run_git_network`, `RESERVED_PROFILE_VARS`…) | `CHECKPOINTS.md` is not invented from scratch: it is derived from that section. It is the best automatic-review material the repo has |
| The repository has **no commits at all** (`git log` empty, everything untracked) | A `git diff`-based reviewer does not work until an initial commit exists. It is a Phase 0 prerequisite |
| `plan.md` is target design and `execution.md` is a historical log, both huge and sometimes contradictory with the code | The harness needs its **own** state (`harness/features.json`), not a reuse of `plan.md` |
| The repo is used daily for normal work (questions, exploration, doc changes) | The `leader` role is activated **only** when the action is invoked, never through a global `CLAUDE.md` |
| Only `.claude/settings.local.json` exists (with `outputStyle`) | A version-controlled `.claude/settings.json` has to be created for the hooks and permissions |

### 0.5 Platform mechanics (verified against the current documentation)

- **Custom commands were merged with skills.** `.claude/commands/feature.md` and
  `.claude/skills/feature/SKILL.md` both produce `/feature`. The skill form
  accepts richer frontmatter and a directory with supporting files, so we use
  that one.
- Useful frontmatter: `description`, `argument-hint`,
  `disable-model-invocation: true` (so only the human fires the flow),
  `allowed-tools` (permissions pre-approved **during that turn**),
  `disallowed-tools`.
- Arguments: `$ARGUMENTS` (all the text as-is), `$0`/`$1` (positional, with
  shell-style quoting). `${CLAUDE_PROJECT_DIR}` is substituted both in the body
  and in the `Bash(...)` rules of `allowed-tools`.
- **Subagents** in `.claude/agents/*.md`: `name`, `description`, `tools`,
  `disallowedTools`, `model`, `permissionMode`, `skills`, `isolation: worktree`,
  `maxTurns`. They can nest up to 3 levels and run up to 20 in parallel.
  `tools: Agent(implementer, reviewer)` restricts **which** subagents one can
  launch.
- Relevant **hooks**: `PostToolUse` (matcher by `tool_name`), `SubagentStop`
  (matcher by **subagent name**, e.g. `implementer`), `Stop` (it can prevent the
  close with exit 2 or `{"continue": false}`), `SessionStart`. Exit 2 = block,
  and stderr is shown to Claude.

---

## 1. Design

### 1.1 Target cycle

```
/feature "<specification in natural language>"
        │
        ▼
 [lead = main thread]  registers the feature in harness/features.json (status: pending)
        │               runs ./init.sh  ── fails ⇒ stop
        ▼
 [Explore ×2-3 in parallel]  scoped research → harness/progress/explore_<id>_<topic>.md
        │                     they return ONE line with the path
        ▼
 [spec-author]  → harness/specs/<id>-<slug>/{requirements,design,tasks}.md   (status: spec_ready)
        │
        ▼
 ══ HUMAN GATE ══  you read the spec and answer  /feature-go <id>  (or ask for changes)
        │
        ▼
 [implementer]  code + tests against tasks.md   → harness/progress/impl_<id>.md  (status: in_review)
        │        it self-verifies with scripts/dev check
        ▼
 [reviewer]  CHECKPOINTS.md + AGENTS.md invariants → harness/progress/review_<id>.md
        │     APPROVED ─────────────┐          CHANGES_REQUESTED ──┐
        ▼                           │                              │
 [lead] status: done                │          back to the implementer (max. 2 rounds)
        history → harness/progress/history.md                      │
                                                                    ▼
                                                        3rd round ⇒ status: blocked, stop and tell you
```

Two human gates, not one: **before implementing** (the spec) and **before
committing** (the harness never runs `git commit` on its own).

### 1.2 File structure to create

```
forge-node/
├── init.sh                          # NEW — the harness gate; delegates to scripts/dev check
├── harness/                         # NEW — all the harness state, version-controlled
│   ├── CHECKPOINTS.md               #   C1–C6 derived from AGENTS.md
│   ├── features.json                #   state machine, one active feature
│   ├── specs/<id>-<slug>/           #   requirements.md · design.md · tasks.md
│   └── progress/
│       ├── current.md               #   the live session
│       ├── history.md               #   append-only
│       ├── explore_<id>_<topic>.md  #   the Explore output
│       ├── spec_<id>.md             #   spec-author blockers
│       ├── impl_<id>.md             #   the implementer's report (incl. the R<n> → evidence map)
│       └── review_<id>.md           #   the reviewer's verdict
├── .claude/
│   ├── settings.json                # NEW — hooks + permissions (version-controlled)
│   ├── skills/
│   │   ├── feature/SKILL.md         # NEW — /feature  (fires the flow)
│   │   ├── feature-go/SKILL.md      # NEW — /feature-go <id>  (human gate → implementation)
│   │   └── feature-status/SKILL.md  # NEW — /feature-status  (cheap state read)
│   └── agents/
│       ├── spec-author.md           # NEW
│       ├── implementer.md           # NEW
│       └── reviewer.md              # NEW
└── AGENTS.md                        # MODIFIED — +1 short section pointing at the harness
```

Deliberate decisions:

- **There is no `.claude/agents/leader.md`.** The lead is the main thread running
  the body of `/feature`; a subagent cannot be the main loop, and duplicating the
  role in two places guarantees they drift apart.
- **`CLAUDE.md` is not touched** (in fact it does not exist: the repo's guide is
  `AGENTS.md`). The orchestrator role lives in the skill and dies with the turn.
  Outside `/feature`, this repo behaves exactly as it does today.
- **Everything under `harness/`.** `docs/` is documentation of what is
  implemented and `plan.md`/`execution.md` are historical; mixing machine state
  in there dirties them.

### 1.3 `harness/features.json`

```jsonc
{
  "project": "forge-node",
  "rules": {
    "one_feature_at_a_time": true,
    "require_human_spec_approval": true,
    "require_green_gate_to_close": true,
    "max_review_rounds": 2,
    "valid_status": ["pending", "spec_ready", "in_progress", "in_review", "done", "blocked"]
  },
  "features": [
    {
      "id": 1,
      "slug": "usage-per-provider",
      "title": "Per-provider usage in the rail",
      "spec_raw": "<what you wrote after /feature, verbatim>",
      "crates": ["domain", "protocol", "daemon", "ui"],
      "acceptance": ["…derived by the lead from spec_raw, reviewable by you…"],
      "status": "pending",
      "review_rounds": 0,
      "created_at": "2026-08-24"
    }
  ]
}
```

`spec_raw` is stored **verbatim**: it is the only source of truth for what you
asked for, and the reviewer uses it to detect scope drift.

### 1.4 `init.sh` (the gate)

Blocks, in order, with `[OK]/[FAIL]` and an exit code:

1. Toolchain: `rustc --version` matches `rust-toolchain.toml` (1.89.0); `git` present.
2. The harness base files present (`harness/CHECKPOINTS.md`, `harness/features.json`, `harness/progress/current.md`, `AGENTS.md`).
3. Validation of `features.json`: valid JSON, valid statuses, **at most one** feature in `in_progress`/`in_review`, every `done` feature with its `specs/<id>-<slug>/` and its `progress/review_<id>.md` carrying an `APPROVED` verdict.
4. `scripts/dev check` (fmt → clippy → test), except with `--fast`, which skips block 4 and only validates the state.
5. Summary and exit code.

`init.sh --fast` exists precisely because block 4 is expensive: the cheap hooks
use `--fast`, the closing gate uses the full version.

### 1.5 `harness/CHECKPOINTS.md`

C1–C3 are mechanical; C4–C6 are what makes this harness worth something in
*this* repo and not in any repo:

- **C1 — The harness is complete.** Base files present; `./init.sh --fast` green.
- **C2 — The state is coherent.** ≤1 active feature; every `done` one with a spec + an APPROVED review; `current.md` describes the live session, not previous garbage.
- **C3 — The gate is green.** The full `scripts/dev check`: `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`. No new `#[ignore]`, no new `#[allow(...)]` without a justification in the same file.
- **C4 — Boundaries between crates (from `AGENTS.md`).** The diff does not introduce: a second VT engine or `terminal-core` outside the daemon; `ui` depending on `protocol` as a non-dev dependency; branching on `provider_id` outside `crates/agents`; Git subprocesses outside `git_service::run_git` / `run_git_network`.
- **C5 — Runtime invariants (from `AGENTS.md`).** The diff does not break: the `inner -> registry` lock order nor "never `.await` while holding the core lock"; the `pump_terminal` unit (feed + routes + clock in a single critical section); `emit_seq` once per emitted delta; runtime-only fields (`terminal_id`, `last_activity_at`, `Workspace::status`) with no SQLite column; migrations **only** appended at the end of `migrations.rs`; `SpawnSpec.env` as a complete environment with `TERM`/`COLORTERM`/`FORGE_*` preserved; `RESERVED_PROFILE_VARS` never coming from a profile.
- **C6 — Traceability.** Every `R<n>` of `requirements.md` has concrete evidence (a named test with its path, or a documented manual case) in `progress/impl_<id>.md`; every task in `tasks.md` is `[x]` or justified; no file touched outside the `crates[]` declared in the feature without an explicit note.

The reviewer walks the checkboxes and **rejects if any is left empty**.

### 1.6 The three subagents

| Subagent | `tools` | Writes | Returns (one line) |
|----------|---------|--------|--------------------|
| `spec-author` | `Read, Glob, Grep, Bash, Write, Edit` | only `harness/specs/**` and `harness/progress/spec_*.md` | `spec_ready -> harness/specs/<id>-<slug>/` |
| `implementer` | `Read, Write, Edit, Glob, Grep, Bash` | `crates/**`, `harness/progress/impl_<id>.md`, `tasks.md` | `implemented -> harness/progress/impl_<id>.md` |
| `reviewer` | `Read, Glob, Grep, Bash` (**no `Write`/`Edit`**) | only `harness/progress/review_<id>.md` through a `Bash` heredoc | `APPROVED -> …` / `CHANGES_REQUESTED -> …` |

Hard rules replicated from the example, adapted:

- The `implementer` **never** marks `done`; at most `in_review`.
- The `reviewer` does not edit code: it says what fails quoting `file:line`.
- None of them runs `git commit`, `git push`, `cargo add` nor touches
  `Cargo.toml` unless it is explicitly in `design.md`.
- If a tool fails unexpectedly, **no workaround is improvised**: the blocker is
  written down and it stops.
- The `spec-author` does not invent requirements that are not in `spec_raw` or in
  an explicit answer from you: if the spec is insufficient, it returns `blocked`.

The requirements go in **EARS** (`The system SHALL…`, `WHEN … SHALL…`,
`WHILE…`, `WHERE…`, `IF … THEN…`), a single SHALL per `R<n>`, closing with the
`acceptance → R<n>` traceability table. It is the part of Aparicio's repo that
pays off the most in automatic review.

### 1.7 `.claude/settings.json` (hooks and permissions)

```jsonc
{
  "hooks": {
    "PostToolUse": [
      {
        "matcher": "Edit|Write",
        "hooks": [{
          "type": "command",
          "command": "${CLAUDE_PROJECT_DIR}/.claude/hooks/fmt-and-check.sh",
          "timeout": 120,
          "statusMessage": "fmt + cargo check of the touched crate…"
        }]
      }
    ],
    "SubagentStop": [
      {
        "matcher": "implementer",
        "hooks": [{
          "type": "command",
          "command": "${CLAUDE_PROJECT_DIR}/init.sh --fast",
          "timeout": 120
        }]
      }
    ],
    "Stop": [
      {
        "hooks": [{
          "type": "command",
          "command": "${CLAUDE_PROJECT_DIR}/.claude/hooks/session-guard.sh",
          "timeout": 60
        }]
      }
    ]
  },
  "permissions": {
    "allow": [
      "Bash(./init.sh*)",
      "Bash(scripts/dev*)",
      "Bash(cargo fmt*)",
      "Bash(cargo clippy*)",
      "Bash(cargo test*)",
      "Bash(cargo check*)",
      "Bash(git status*)",
      "Bash(git diff*)"
    ],
    "deny": ["Bash(git push*)", "Bash(cargo publish*)"]
  }
}
```

- `fmt-and-check.sh`: infers the crate from the edited path, runs
  `cargo fmt -p <crate>` and `cargo check -p <crate>`; returns the first error
  lines on stderr. Seconds, not minutes.
- `session-guard.sh`: it does **not** run the suite. Only `init.sh --fast` and,
  if there is a feature in `in_progress`/`in_review` without an updated
  `progress/current.md`, it exits with **exit 2** to prevent the close and
  explain why.
- The `git push` `deny` is deliberate: the harness produces reviewable work, it
  never publishes.

---

## 2. Implementation phases

Every phase is independently useful and you can stop there.

### Phase 0 — Prerequisites (blocking)

1. **The repository's initial commit.** Today `git log` is empty and everything
   is untracked; without a base tree, `git diff` does not exist and the reviewer
   has nothing to review. What goes into that commit is your decision.
2. Decide whether `harness/` is version-controlled (recommended: **yes**; the
   state on disk is the whole point) and what stays in `.gitignore` (`/tmp`
   harness logs).

*Acceptance:* `git diff HEAD --stat` answers something on a clean tree.

### Phase 1 — Skeleton and gate (no agents yet)

Create `init.sh` (+`--fast`), `harness/features.json` with one real feature
already written by hand, `harness/CHECKPOINTS.md`,
`harness/progress/{current,history}.md` and the new `AGENTS.md` section.

*Acceptance:* `./init.sh` green on a clean tree; `./init.sh` **red** if you put
two features into `in_progress` by hand, an invalid status, or break the
formatting with a pending `cargo fmt`. Test both sides.

### Phase 2 — Subagents and the `/feature` action

Create the three `.claude/agents/*.md` and the three skills. `/feature` includes
in its body the lead's protocol, the escalation table (trivial → implementer
only; medium → implementer + reviewer; refactor → 2-3 Explores first) and the
**anti-broken-telephone rule** written explicitly in the instructions the lead
passes to each subagent.

`/feature` frontmatter:

```yaml
---
name: feature
description: Registers a specification as a harness feature and fires research + spec. Stops at the human gate.
argument-hint: "\"<task specification>\""
disable-model-invocation: true
allowed-tools: Bash(${CLAUDE_PROJECT_DIR}/init.sh*) Bash(scripts/dev*) Read Grep Glob Write Edit Agent(spec-author)
---
```

`disable-model-invocation: true` matters: nobody fires the flow by semantic
resemblance, only you typing `/feature`.

*Acceptance:* `/feature "…"` on a small, real task produces
`harness/specs/1-<slug>/` with the three files, leaves `status: spec_ready` and
**stops**, without having touched a single file in `crates/`.

### Phase 3 — Execution and review

`/feature-go <id>` launches `implementer` → `reviewer` → close, with the limit of
2 review rounds and `blocked` on the third. Add the §1.7 hooks.

*Acceptance (two tests, not one):*
- **Happy path:** a small real feature reaches `done` with `scripts/dev check` green and `progress/review_<id>.md` at `APPROVED`.
- **Failure path:** you deliberately introduce a C4/C5 violation (e.g. a `match` on `provider_id` in `crates/daemon`) and the reviewer returns `CHANGES_REQUESTED` quoting the file and the line. If it approves, the checkpoints are badly written and have to be hardened before trusting the harness.

### Phase 4 — Optional, only if 1–3 hold up

- `isolation: worktree` in the `implementer`: the subagent works in a git
  worktree of its own. It fits almost comically well with this repo, which *is* a
  worktree manager, but it duplicates the GUI build tree — measure before
  adopting.
- A `committer` subagent (an idea from `secure-vault`) that writes the commit
  message from `impl_<id>.md` + `review_<id>.md`, **without** running
  `git commit`.
- Turning the `AGENTS.md` invariants into per-domain skills
  (`.claude/skills/invariants-daemon/`, `…-terminal/`, `…-git/`) preloaded into
  the reviewer through its `skills:` field, instead of a `CHECKPOINTS.md` that
  grows without bound.
- `docs/harness.md` describing the already implemented cycle, and this plan
  moving to "implemented" like `plan-agent-profiles.md`.

---

## 3. Risks and how they are mitigated

| Risk | Mitigation |
|------|------------|
| **Cost/latency.** `cargo test --workspace` is slow; running it on every hook kills the flow | Tiered: `cargo check -p <crate>` per edit, `init.sh --fast` when the implementer finishes, the full gate only on close |
| **Review theatre.** An LLM reviewer that approves everything | C3–C5 are objective checks (commands, greps over the diff); the Phase 3 test with a deliberate violation is mandatory |
| **Implementer scope drift** | Verbatim `spec_raw` + declared `crates[]` + C6 ("files outside the declared crates") |
| **The harness rots.** `features.json` and the code diverge | `init.sh` validates coherence and runs on every `Stop`; a `done` feature without an APPROVED review breaks the gate |
| **Friction in normal work** | The orchestrator role lives in the skill, not in `CLAUDE.md`. Without `/feature`, the repo works as it does today |
| **Infinite review loops** | `max_review_rounds: 2`, then `blocked` and you decide |
| **The hooks block unrelated sessions** | `session-guard.sh` only exits with 2 if there is an active feature; in any other session it is a no-op |

## 4. What this plan does NOT do

- It does not do automatic `git commit` or `git push`.
- It does not touch `plan.md` nor `execution.md`.
- It introduces no new Cargo dependencies: `init.sh` is bash + `cargo` +
  `bun`/`jq` only to validate the JSON.
- It forces no role at the repository level.
- It does not modify `scripts/dev check` nor the CI workflow: the harness
  consumes them, it does not replace them.

---

## 5. References

- [betta-tech/ejemplo-harness-subagentes](https://github.com/betta-tech/ejemplo-harness-subagentes) — this plan's original harness
- [LuisFernandoAparicio21/agentic-spec-driven-development](https://github.com/LuisFernandoAparicio21/agentic-spec-driven-development) — the same base + `spec_author`, EARS and traceability
- [carlosOlcina/secure-vault](https://github.com/carlosOlcina/secure-vault) — the same base ported to OpenCode + skills by convention
- [github/spec-kit](https://github.com/github/spec-kit) — SDD as slash commands and version-controlled artefacts
- [humanlayer/advanced-context-engineering-for-coding-agents](https://github.com/humanlayer/advanced-context-engineering-for-coding-agents) — research → plan → implement; subagents as context isolation
- [ai-boost/awesome-harness-engineering](https://github.com/ai-boost/awesome-harness-engineering) · [10xChengTu/harness-engineering](https://github.com/10xChengTu/harness-engineering) — catalogue and reusable template
- [Agent Harness Engineering — Addy Osmani](https://addyosmani.com/blog/agent-harness-engineering/) · [O'Reilly Radar](https://www.oreilly.com/radar/agent-harness-engineering/) — the conceptual frame
- Platform documentation: [skills and slash commands](https://code.claude.com/docs/en/skills) · [subagents](https://code.claude.com/docs/en/sub-agents) · [hooks](https://code.claude.com/docs/en/hooks)
