# Agent instructions

Short, agent-facing checklists for this repository. Humans should read the
matching page under [`docs/`](../docs/).

| File | Use when… |
|------|-----------|
| [commits.md](./commits.md) | drafting a git commit or pull request title/body |
| [skills/forge-clean-code/SKILL.md](./skills/forge-clean-code/SKILL.md) | architecture / docs / quality review that implements fixes (`/forge-clean-code`) |

Skills live in `skills/<name>/SKILL.md`. `.claude/skills` and `.cursor/skills`
are relative symlinks to this shared directory, not separate copies. The
harness skills (`feature`, `feature-go`, `feature-status`, `feature-commit`,
`invariant-c4/c5/c7`) follow [docs/harness.md](../docs/harness.md).

Invariants and crate boundaries live in [`AGENTS.md`](../AGENTS.md), not here.
