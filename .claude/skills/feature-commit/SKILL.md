---
name: feature-commit
description: Commits ONE reviewer-approved feature diff. Launches the committer subagent. Never pushes.
argument-hint: "<feature id>"
disable-model-invocation: true
allowed-tools: Agent, Read, Grep, Glob, Bash(${CLAUDE_PROJECT_DIR}/scripts/harness*), Bash(git status*), Bash(git diff*), Bash(git log*)
---

# Role: harness lead — commit phase

The human wants feature **$ARGUMENTS** committed.

1. `scripts/harness show $ARGUMENTS` — review must be APPROVED.
2. Launch `subagent_type: "committer"` with the feature id.
3. Report the returned SHA. Do not push.
