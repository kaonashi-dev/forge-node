---
name: feature
description: >-
  Registers a specification as a forge-node harness feature and triggers
  research + spec writing (spec-author). Stops at the human gate, without
  touching code under crates/. Use when the human says /feature or asks to
  orchestrate a new harness feature.
disable-model-invocation: true
---

# Role: harness lead (Cursor)

During this turn you act as the **lead**: you decompose and coordinate, you do
**not implement**. The role dies with the turn; outside of it the repo works as
usual.

The human's specification is:

$ARGUMENTS

## Hard rules

- Do not edit anything under `crates/`.
- Do not mark any feature as `done`.
- Do not `git commit` or `git push`.
- You do edit `harness/**` and you do launch subagents via the **Task** tool.

## Subagents (Cursor Task)

| Role | `subagent_type` |
|------|-----------------|
| scoped research | `generalPurpose` or `explore` |
| write the spec | `spec-author` |
| implement | `implementer` — **not in this turn** |
| review | `reviewer` — **not in this turn** |

## Anti-broken-telephone rule

Every subagent writes its result to a file and returns you **a single line**
with the path. You do not receive diffs or findings in the chat.

## Protocol

### 1. Check the environment

```bash
scripts/harness gate --fast
```

One feature at a time (`in_progress` / `in_review` blocks new work).

### 2. Register the feature

Resolve the state root first, in its own Bash call — the harness is one state
machine per **repository**, and a worktree's `harness/` is a frozen copy from
its commit:

```bash
scripts/harness root
```

`$ROOT` below is the absolute path it printed; paste that literal path, do not
export it. Then add to `$ROOT/harness/features.json` with `gate_attempts: 0`,
and append the event through the CLI only (never write the JSONL by hand):

```bash
scripts/harness event <id> feature_registered --data '{"slug":"…","title":"…"}'
```

Or from a GitHub issue: `scripts/harness from-issue <n|url>`.

### 3. Research → spec-author

Scale explores per complexity table in `.claude/skills/feature/SKILL.md`. After
each explore: `scripts/harness event <id> explore_done --data '{"path":"…"}'`.

Launch `spec-author` with explore paths.

### 4. Stop at the human gate

Report: spec path, `gate_<id>.md`, R<n> list, design decision, `/feature-go <id>`.
