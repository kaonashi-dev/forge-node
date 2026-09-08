---
name: feature-status
description: Shows the harness state: features, the phase of each one and what is missing to move forward. Cheap read, launches no subagents.
disable-model-invocation: true
allowed-tools: Bash(${CLAUDE_PROJECT_DIR}/init.sh --fast), Bash(${CLAUDE_PROJECT_DIR}/scripts/harness*), Bash(bun harness/src/validate.ts), Bash(git status*), Read, Glob
---

Show the harness state. **Do not launch subagents and do not modify anything.**

Every path below is under the checkout `scripts/harness root` prints, not under
your cwd. Prefer the CLI: it already resolves that.

1. Preferred: `scripts/harness status` (covers fast gate + features + next).
   For one feature in detail: `scripts/harness show <id>`, `scripts/harness
   resume <id>`, or `scripts/harness timeline <id>`.
2. For each feature, one line: `<id> · <slug> · <status>` and, if it is active,
   what is missing to move forward:
   - `pending` → the spec is missing: `/feature` already registered it, relaunch
     it or wait for the `spec-author`.
   - `spec_ready` → waiting on the human: read `gate_<id>.md`, then
     `/feature-go <id>`.
   - `in_progress` / `in_review` → which subagent has the ball and which review
     round it is on; check `scripts/harness timeline <id>`.
   - `blocked` → the reason, quoting the file from
     `$(scripts/harness root)/harness/progress/`.
3. If the working tree has uncommitted changes, say so in one line.

Answer in under 15 lines. It is a glance, not a report.
