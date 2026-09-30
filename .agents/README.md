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

`C4`, `C5` and `C7` are checkpoint numbers kept only as skill names; the rules
are the ones in [`AGENTS.md`](../AGENTS.md), which is where invariants and crate
boundaries live, not here.
