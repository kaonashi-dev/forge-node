# Agent instructions

Short, agent-facing checklists for this repository. Humans should read the
matching page under [`docs/`](../docs/).

| File | Use when… |
|------|-----------|
| [commits.md](./commits.md) | drafting a git commit or pull request title/body |
| [skills/forge-clean-code/SKILL.md](./skills/forge-clean-code/SKILL.md) | architecture / docs / quality review that implements fixes (`/forge-clean-code`) |

Skills live in `skills/<name>/SKILL.md` and this directory is the only copy.
Cursor discovers `.agents/skills` natively, so the repository keeps no
per-client mirror or symlink. Claude Code reads only `.claude/skills`; a
developer who wants these skills there creates that symlink locally.

Invariants and crate boundaries live in [`AGENTS.md`](../AGENTS.md), not here.
