---
name: feature-commit
description: Commits one APPROVED feature via the committer subagent. Does not push.
disable-model-invocation: true
---

Launch `Task` with `subagent_type: "committer"` for feature **$ARGUMENTS** after
`scripts/harness show` confirms APPROVED review. See `.claude/skills/feature-commit/SKILL.md`.
