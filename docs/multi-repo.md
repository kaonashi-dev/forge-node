# One planner, several repositories

Add the parent directory, for example `~/dev/workspace/`, as an ordinary Forge
project. It need not be Git: no diff or pull request in that parent is expected.
Add `api` and `web` separately so each worker has the checkout, Git state and
worktree isolation it needs. Folder nesting is organizational, not a
cross-project session graph edge.

## Planner instructions

Create the parent folder's `AGENTS.md` yourself; Forge does not write it.
For a Claude planner, also reference these instructions from its `CLAUDE.md`.
Adapt this template to the actual repository names and checks:

```markdown
# Workspace planner

Repositories:
- api: ./api; verify with cargo test --workspace.
- web: ./web; verify with bun run verify.

Read and plan here. Do not edit repositories, commit, push, merge, or approve
worker permissions. Delegate only the requested work to independent Forge
sessions. Ask a person when scope, intent, verification or permissions are unclear.
Keep the repository boundaries and interface contracts in every task prompt.

Read `forgectl guide sessions`. Spawn workers with explicit project names and
unique human titles; retain the returned UUIDs. Prefer isolated worktrees for
writing tasks. Wait by events, then read the transcript and relevant diff/tests.
Idle means a turn ended, not that work succeeded. Never use --now to answer
permissions. Do not resend an uncertain prompt without inspecting the terminal.
```

Launch the planner from a shell outside an agent, choosing a provider that
declares read-only mode:

```sh
forgectl --json session spawn --project workspace --provider claude \
  --name planner --read-only --prompt 'Read the repo map and propose the next task.'
```

The plan-mode provider may ask for approval before running dispatch commands;
do not weaken its edit permissions to avoid that. Workers are ordinary writing
agents unless explicitly launched read-only:

```sh
forgectl --json session spawn --project api --provider claude \
  --name auth-impl --worktree auth-impl --base main --prompt-file plan/auth.md
forgectl session wait api/auth-impl --for idle
forgectl session read api/auth-impl
forgectl session send api/auth-impl --queue --text 'Check the API contract with web.'
```

Use separate worktrees if two writing workers need the same repository. These
commands are not the run ledger: they do not enforce attempt rails or settle
reports. Use `run`/`task` when those guarantees are needed.

## Symlinked repository folders

Claude's permissions follow the real paths. If the parent holds symlinks,
declare the actual repositories in the parent `.claude/settings.json`:

```json
{"permissions":{"additionalDirectories":["/Users/me/dev/api","/Users/me/dev/web"]}}
```

Additional directories grant visibility, not permission to write. Avoid putting
`--add-dir` at the end of a launch profile: it accepts multiple values and can
consume the positional initial prompt. The settings file avoids that ambiguity.
A profile's explicit `--settings` suppresses Forge's Claude hook injection;
include the generated activity hooks yourself if that profile needs them.

## Raycast script command

Save this as an executable `.sh` script in a Raycast script-command directory.
Replace the binary path and project name. It requires an already-running daemon:

```sh
#!/bin/sh
# @raycast.schemaVersion 1
# @raycast.title Forge: API worker
# @raycast.mode fullOutput
# @raycast.packageName Forge Node
# @raycast.argument1 {"type":"text","placeholder":"Task"}

exec /absolute/path/forgectl --json session spawn \
  --project api --provider claude --name api-worker --prompt "$1"
```

Repeated launches can have the same title; retain the returned UUID or rename
them. Ambiguous titles are refused with exit 5 rather than choosing a session.
