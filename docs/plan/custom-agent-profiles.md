# Custom agent launch profiles

## Problem

Forge already persists `AgentProfile` values and includes them in the normal
agent launch list, but the creation form sends an empty UUID and therefore a
new profile is rejected before it reaches the daemon. The form also ignores the
provider-specific fields already declared by `crates/agents`, forcing users to
translate common choices into raw argument and environment lines.

The project rail has a related visibility failure: a newly created named group
is removed from the derived tree while it has no projects, so a successful
creation looks like it was ignored. Worktree refusals similarly remain behind a
generic notice while the creation dialog waits for a workspace event that will
never arrive.

## Product decision

A Forge launch profile is a named way to start a known provider. It is not a
new provider and it is not a provider-native persona definition. The profile
keeps an explicit provider id and contributes an optional executable, ordered
arguments, and an environment overlay. This lets Forge preserve the provider's
detection, resume behavior, icon, usage source, and PTY contract even when the
executable is a wrapper.

Provider-native selections are exposed as profile fields:

- Claude Code: config directory, model, and `--agent`.
- Codex CLI: config directory, model, and `--profile`.
- OpenCode: config directory, data directory, model, and `--agent`.

The resulting Forge profile is already a first-class launchable in the tab `+`
menu, workspace menus, command palette, worktree launcher, default-agent
setting, and the direct agent shortcut.

## Implementation

1. Keep named project groups in the rail even when they contain no projects;
   keep omitting the empty implicit ungrouped bucket.
2. Return queue failures from the Tauri command boundary instead of silently
   dropping structural commands.
3. Report a refused worktree creation back to the open branch picker and clear
   its pending state immediately.
4. Mint a stable UUID when the WebView creates a profile draft.
5. Serialize `AgentDescriptor.profile_fields` in the frontend type and render
   those fields as direct controls while retaining advanced argument and
   environment editors.
6. Keep arguments as individual values and launch through the existing process
   builder. Do not parse a shell command or invoke a shell.
7. State that environment values are persisted in clear text and remove secret
   values from the example.
8. Cover group visibility, profile creation/composition, provider field
   declarations, command backpressure, and worktree refusal handling with
   focused tests.

## Acceptance criteria

- Creating an empty named group immediately produces a usable rail row.
- A refused worktree creation stops showing `Creating...` and displays the
  daemon reason in the dialog.
- A full or disconnected runtime command queue rejects the Tauri invocation.
- Creating a profile sends a valid UUID and persists successfully.
- Common provider options can be entered without manually spelling their flags
  or environment names.
- Unknown arguments and environment values survive editing.
- Saving a profile makes it available through every existing launch surface.
- No profile value is interpolated into a shell command.

## References

- Claude Code subagents and CLI: <https://code.claude.com/docs/en/sub-agents>,
  <https://code.claude.com/docs/en/cli-reference>
- Codex configuration profiles: <https://developers.openai.com/codex/config-file/config-advanced>
- OpenCode agents and CLI: <https://opencode.ai/docs/agents/>,
  <https://opencode.ai/docs/cli/>
- Vibe Kanban profile variants: <https://github.com/BloopAI/vibe-kanban/blob/main/crates/executors/src/profile.rs>
- Agent Deck custom tools: <https://github.com/asheshgoplani/agent-deck/blob/main/skills/agent-deck/references/config-reference.md>

## Deferred

- User-defined provider descriptors remain separate work. They require registry
  configuration and validation, not another field on `AgentProfile`.
- Provider-native persona files remain owned by each CLI. Forge selects them by
  flag but does not rewrite Claude Markdown, Codex TOML, or OpenCode JSON files.
- Headless harness jobs do not consume launch profiles in this change.
