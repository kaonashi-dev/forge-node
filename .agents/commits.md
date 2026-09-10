---
description: How to write commit subjects/bodies and PR titles/descriptions for this repo. Apply whenever drafting or creating a git commit or pull request.
---

# Commit and PR messages

Full prose: [`docs/commits.md`](../docs/commits.md). Follow that page; this file
is the checklist.

## Before writing

1. Read the staged (or about-to-stage) diff and name **one** outcome.
2. If there are two unrelated outcomes, stop and split the commit.
3. Prefer the *why* that is not obvious from the diff over a file list.

## Commit subject

- Imperative, sentence case, ≤72 chars, no trailing period.
- No `feat:` / `fix:` / `chore:` prefixes.
- Example: `Ring BEL when a TUI agent needs attention`.

## Commit body

- Blank line after the subject.
- 1–3 short paragraphs: context first, then what changed.
- No subject echo, no `git diff --stat`, no “as discussed”.
- HEREDOC only:

```bash
git commit -m "$(cat <<'EOF'
Subject line without a period

Context the diff does not show. What the change does and why that is the
right shape.
EOF
)"
```

## Pull request

- **Title:** same voice as a commit subject; overall outcome if multi-commit.
- **Body:**

```markdown
## Summary
- …

## Test plan
- [ ] …
```

- Mention migrations, protocol bumps, non-goals, and `Stacked on #N` when
  relevant.
- Test plan: real gate command and/or the manual path CI cannot see.

## Hard rules

- Do not commit or push unless the human asked.
- Never `--no-verify` unless the human explicitly asked.
- Never stage secrets (`.env`, credentials, keys).
- Harness `/feature-commit` still requires `APPROVED` review; the message must
  still match this checklist.
