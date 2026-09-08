# Plan — PR tab with an agent (§16.8)

## What it is

A new tab, sibling to the Diff one, that turns "I have changes" into "there is
an open PR" without leaving Forge. It does not replace `Open PR…` in the rail:
that stays the short one-line path. This is the path with an agent inside.

```
[ zsh ][ claude ][ ◧ Diff ][ ⇅ PR ][+]
┌──────────────────────────────────────────────┐
│ main → 12 files · +340 −87              ⚙   │  summary
│ ▸ Prompt                                     │  folded behind the gear
│   ○ Draft the PR      ○ Implement and open   │  templates
│   ┌────────────────────────────────────────┐ │
│   │ (whatever you write is appended)       │ │
│   └────────────────────────────────────────┘ │
│ [ Launch agent ]                             │
├──────────────────────────────────────────────┤
│ ● Claude Code · running · 2m                 │  progress
│ → PR #128 "Add diff view" ↗                  │  result
└──────────────────────────────────────────────┘
```

## Decisions taken

- **A new tab next to Diff.** The `Open PR…` flow is untouched.
- **The template decides what the agent does.** Two modes, and the mode is a
  field of the template, not a separate switch:
  - `Draft` — the agent only returns a title and a body. The push and the
    `gh pr create` are still done by the daemon, which already knows how
    (`CreatePullRequest`). It is Juva with a real CLI instead of a template.
  - `Implement` — the agent edits, commits, pushes and opens the PR itself.
    Forge only launches it and watches.
- **The prompt is folded.** A gear unfolds it. By default only the summary and
  the button are visible: the normal case is not reading the prompt.
- **The user's text is appended, not substituted.** The template is the body and
  what is written is concatenated at the end, which is what "extend" means.

## The real blocker: the initial prompt

`AgentCapabilities::supports_initial_prompt` has existed in `domain` from the
start and **is `false` in all four providers**, because there is no wiring
behind it: nothing turns a prompt into launch arguments. Without that, the
`Implement` mode does not exist.

It is solved with the same shape resume already has (§13.5): a declarative
`PromptStyle` in the descriptor, so that no other crate branches on the provider
id (P2).

```rust
pub enum PromptStyle {
    /// `claude "<prompt>"` — the prompt is a final positional argument.
    Positional,
    /// `<cli> --prompt "<prompt>"`.
    Flag { flag: String },
}
```

`supports_initial_prompt` stops being informative and starts saying exactly the
same thing as "this descriptor carries a `PromptStyle`", just as
`supports_resume` restates the presence of a `ResumeStyle`.

## Layers

1. **`domain`** — `PromptStyle`; `PrTask { id, name, mode, body }` and the set of
   built-in templates. Runtime-only: the built-in ones live in code like the
   agent descriptors, and whatever the user writes travels in the request, it is
   not persisted yet.
2. **`agents`** — each builtin declares its `PromptStyle` and its real
   `supports_initial_prompt`; the argument builder applies it.
3. **`protocol`** — `CreateAgentSession { initial_prompt: Option<String> }`.
   Rejected with `InvalidRequest` when the provider declares no way of receiving
   it, just like `resume`.
4. **`daemon`** — the `SpawnSpec` includes the prompt arguments.
5. **`ui`** — `pr_tab.rs`: the summary from the `WorkspaceDiff` we already ask
   for, the template selector, the folded prompt, the launch, the progress and
   the result.

## Progress and result, without guessing

"Seeing the progress" is the state of the session that was launched — the tab
links to it, it does not duplicate it. "The PR link" is **not** scraped from the
terminal output: when the session ends, `RefreshPullRequests` fires and the pull
request whose `head_ref` is this checkout's branch is looked up. That reuses the
caching that already exists and does not depend on an agent printing a URL.

## Out of scope

Persisted custom templates (for now only the built-in ones plus whatever text is
typed), several agents in parallel over the same checkout, and editing the PR
once it is open.
