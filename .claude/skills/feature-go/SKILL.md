---
name: feature-go
description: Approves a feature's spec and runs the implementer → reviewer cycle until APPROVED or blocked. Does not commit.
argument-hint: "<feature id>"
disable-model-invocation: true
allowed-tools: Agent, Read, Grep, Glob, Write, Edit, Bash(${CLAUDE_PROJECT_DIR}/init.sh*), Bash(${CLAUDE_PROJECT_DIR}/scripts/harness*), Bash(bun harness/src/validate.ts), Bash(git status*), Bash(git diff*), Bash(git rev-parse*)
---

# Role: harness lead — execution phase

The human approved the spec of feature **$ARGUMENTS**. You coordinate its
implementation and its review. You still do not implement and do not commit.

## Where the harness state lives

The harness is one state machine per **repository**. `harness/features.json`,
`harness/progress/` and `harness/specs/` live in the checkout that owns them —
which is **not** necessarily the one you are standing in. A git worktree carries
a frozen copy of those files from its commit, and anything you write there is
invisible to Forge, to `scripts/harness` and to `bun harness/src/validate.ts`.

Resolve it once, as your first action, in its own Bash call:

```bash
scripts/harness root
```

Below, `$ROOT` means the absolute path that command printed. Paste that literal
path into every Read / Write / Edit, into every shell command, and into every
prompt you hand a subagent. Do not export it: each Bash call is a fresh shell
and the variable will not survive.

`scripts/harness`, `./init.sh` and `scripts/dev` stay **relative** on purpose —
they act on the code in *your* checkout, and `scripts/harness` resolves the
state root by itself.

Append to the event log **only** through `scripts/harness event`. Never write
`events_<id>.jsonl` by hand: the CLI is what puts the line in the right file
with a real timestamp.

## Protocol

### 1. Check the preconditions

- `scripts/harness gate --fast` (or `./init.sh --fast`) green.
- `scripts/harness show $ARGUMENTS` confirms the feature; it must be in
  `spec_ready`. If it is in `pending`, the spec is missing: say so and stop. If
  it is in `done`, it was already closed.
- `$ROOT/harness/specs/<id>-<slug>/` has the three files.
- `$ROOT/harness/progress/gate_<id>.md` exists.
- There is at least one commit in the repository (`git rev-parse HEAD`). With no
  base tree the `reviewer` has no diff to review against: stop and say so.

Append the human gate resolution:

```bash
scripts/harness event $ARGUMENTS human_gate_resolved --data '{"gate":"spec_approval","decision":"approve"}'
```

### 2. Context bundle (required before implementer)

Write `$ROOT/harness/progress/context_<id>.md` containing:

- `spec_raw` verbatim from `$ROOT/harness/features.json`;
- the declared `crates[]`;
- bullet highlights from each `$ROOT/harness/progress/explore_<id>_*.md` (not
  full paste);
- 3-6 invariant bullets from `AGENTS.md` relevant to those crates.

Then:

```bash
scripts/harness event <id> context_bundled --data '{"path":"harness/progress/context_<id>.md"}'
```

### 3. Implement

Launch `subagent_type: "implementer"` with the feature id, spec path, and
context path. Remind it:

- use worktree isolation when available;
- pass it the resolved `$ROOT` as a literal absolute path — it runs with
  `isolation: worktree`, so its own `harness/` is a frozen copy;
- write to `$ROOT/harness/progress/impl_<id>.md`;
- final answer: one line, `implemented -> …` or `blocked -> …`;
- never mark `done`; at most `in_review`;
- respect `max_gate_attempts` (default 3).

If it returns `blocked`, summarise to the human in three lines and stop.

### 4. Review

Launch `subagent_type: "reviewer"` with the feature id. Load invariant skills
when the diff touches boundaries (`invariant-c4`), runtime (`invariant-c5`), or
cost paths (`invariant-c7`).

- `APPROVED` → step 5.
- `CHANGES_REQUESTED` → increment `review_rounds` in
  `$ROOT/harness/features.json` and launch the `implementer` again, passing it
  the path of the review.

**Limit: 2 rounds.** On the third, mark the feature `blocked`, append
`feature_blocked`, write why in `$ROOT/harness/progress/current_<id>.md` and stop.

### 5. Close

- Mark the feature `done` in `$ROOT/harness/features.json`.
- Append `scripts/harness event <id> feature_done`.
- Move the summary from `$ROOT/harness/progress/current_<id>.md` to the end of
  `$ROOT/harness/progress/history.md` and delete the per-feature note.
- Run the **full** `scripts/harness gate` (without `--fast`) one last time.

### 6. Hand over to the human

Report in a few lines: what was implemented, the `R<n> → evidence` map, the
files touched per crate, and the gate result.

**Do not `git commit`.** The diff stays in the working tree. If they want a
commit, they invoke `/feature-commit <id>` (launches the `committer` subagent).
