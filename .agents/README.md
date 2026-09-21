# Agent instructions

Short, agent-facing checklists for this repository. Humans should read the
matching page under [`docs/`](../docs/).

| File | Use when… |
|------|-----------|
| [commits.md](./commits.md) | drafting a git commit or pull request title/body |
| [skills/forge-clean-code/SKILL.md](./skills/forge-clean-code/SKILL.md) | architecture / docs / quality review that implements fixes (`/forge-clean-code`) |

Skills live in `skills/<name>/SKILL.md`. `.claude/skills` and `.cursor/skills`
are relative symlinks to this shared directory, not separate copies. The
`invariant-c4`, `invariant-c5` and `invariant-c7` skills provide review checklists
for crate boundaries, runtime invariants and resource costs.

Invariants and crate boundaries live in [`AGENTS.md`](../AGENTS.md), not here.
