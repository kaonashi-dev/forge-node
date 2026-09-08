---
name: feature-go
description: >-
  Approves a harness feature's spec and runs implementer → reviewer until
  APPROVED or blocked. Does not commit. Use when the human says
  /feature-go <id> or explicitly approves a ready spec.
disable-model-invocation: true
---

# Role: harness lead — execution phase (Cursor)

The human approved feature **$ARGUMENTS**. Coordinate implement → review. Do not
commit.

## Protocol

1. `scripts/harness gate --fast` + `scripts/harness show $ARGUMENTS` (`spec_ready`).
2. `scripts/harness event $ARGUMENTS human_gate_resolved --data '{"gate":"spec_approval","decision":"approve"}'`
3. Resolve `$ROOT` with `scripts/harness root` (its own Bash call), then write
   `$ROOT/harness/progress/context_<id>.md` (see `.claude/skills/feature-go/SKILL.md` §2).
   `scripts/harness event <id> context_bundled --data '{"path":"…"}'`
4. `Task` → `implementer` (`isolation: worktree` when available). Max 3 gate failures.
5. `Task` → `reviewer` (load `invariant-c4`/`c5`/`c7` skills when relevant). Max 2 review rounds.
6. Close: `done`, `feature_done` event, history.md, full `scripts/harness gate`.
7. Hand over diff; offer `/feature-commit <id>` if they want a commit.
