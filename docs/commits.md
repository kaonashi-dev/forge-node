# Commits and pull requests

How to name and describe a change in this repository. Agents must follow the
same rules; the short form for them is [`.agents/commits.md`](../.agents/commits.md).

A reader who sees only the title and the first paragraph of the body should
understand **why** the change exists and **what** it does, without opening the
diff.

## Commit title (subject)

- One line, imperative mood, sentence case: `Ring BEL when a TUI agent needs
  attention`, not `Added attention` or `fix: attention bell`.
- ≤72 characters. No trailing period. No conventional-commit prefixes
  (`feat:`, `fix:`, `chore:`) — this repo does not use them.
- Name the outcome or the constraint, not the file list. Prefer *why* over
  *what*: `Keep plans out of docs` beats `Delete docs/plan-*.md`.
- One idea per commit. If the subject needs “and” for two unrelated outcomes,
  split the commit.

## Commit description (body)

- Separate from the subject with a blank line.
- 1–3 short paragraphs (or a few bullets when mapping to acceptance criteria).
- State the **context** the diff does not show: the bug, the invariant, the
  rejected alternative, or the user-visible behaviour.
- Then state what the change does, still in prose — not a file dump.
- Wrap near 72 characters. Do not paste `git diff --stat`, ticket IDs alone, or
  “as discussed”.
- Do not repeat the subject as the first sentence of the body.

Good shape:

```text
Install a release build into Applications from make.

Adds scripts/dist local and make install-local so a development Mac can
rebuild, replace Forge Node.app, and relaunch without stopping the daemon.
```

Weak shape (do not):

```text
Update scripts and Makefile.

- scripts/dist
- Makefile
- README
```

## Pull request title

Same voice as a commit subject: imperative, precise, one outcome. When the PR
is a single commit, the titles may match. When it stacks several commits, the
PR title names the **overall** outcome, not the last commit.

## Pull request description

Use this skeleton (HEREDOC-friendly):

```markdown
## Summary
- <why / what, one bullet each; 1–3 bullets>

## Test plan
- [ ] <command or manual check that would catch a regression>
```

Rules for the summary:

- Lead with behaviour or invariants, not path lists.
- Mention migrations, protocol version bumps, or deliberate non-goals when a
  reviewer would otherwise miss them.
- For stacked PRs, say which PR this sits on (`Stacked on #N`) and what must
  merge first.

Rules for the test plan:

- Prefer `scripts/dev check` or an exact `cargo test -p … -- --exact` when that
  is the real gate for the change.
- Add the one manual path a green CI would still miss (GUI gesture, real agent
  CLI, macOS-only install).

## Mechanics

- Pass the message with a HEREDOC (`git commit -m "$(cat <<'EOF' … EOF)"`) so
  wrapping and blank lines survive.
- Never `--no-verify` unless the human explicitly asks.
- Do not commit until asked. Do not push unless asked.
- Harness feature commits still go through the `committer` agent after an
  `APPROVED` review; their subject/body must still satisfy this page.

## See also

- [`development.md`](./development.md) — gate and workflow commands
- [`AGENTS.md`](../AGENTS.md) — invariants the change must not break
- [`.agents/commits.md`](../.agents/commits.md) — agent checklist
