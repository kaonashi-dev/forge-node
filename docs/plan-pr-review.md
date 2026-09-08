# Plan — automatic pull-request review (§16.9)

**Status:** implemented.
**Scope:** see the open pull requests, narrow them to the ones that concern you,
open one, and start an agent's first pass over it from inside Forge. The pass is
a *read*: it produces findings in a session you can follow, and never anything
on GitHub.

## What it is

```
┌ PR ──────────────────────────────────────────┐
│ (All 12)(Assigned 3)(To review 5)(Mine 4)    │  scope chips + search
│ ┌──────────────────────────────────────────┐ │
│ │ #128  Add the diff view          [draft] │ │
│ │ acme/widget · rin · diff-view → main     │ │
│ │ [ Review ]                               │ │
│ └──────────────────────────────────────────┘ │
└──────────────────────────────────────────────┘

[ zsh ][ claude ][ ⇅ Pull Request ][ ☑ PR Review ][+]
┌──────────────────────────────────────────────┐
│ #128 Add the diff view      [↗] [Close]      │
│ acme/widget · diff-view → main · 12 files    │
│  ○ Review the change   ○ Deep review         │  recipes
│  ○ Security review     ○ Custom              │
│ Agent [ Claude Code · plan mode  ▾ ]   ⚙     │
│ [ Start review ]  reads with `gh` in plan    │
│                   mode — cannot write here   │
│ ● Claude Code · running       open the sess. │
└──────────────────────────────────────────────┘
```

## Decisions taken

- **The review is read-only, twice over.** The launch applies the provider's own
  read-only flags (`AgentDescriptor.review`, §16.9), and the recipe's prose
  forbids editing, committing and posting. Neither guard is trusted alone.
- **It runs in a checkout that already exists**, never in one made for it. The
  agent reaches the pull request through `gh pr view` / `gh pr diff`, which
  works for a fork's head as well as for a branch of the same repository, and
  nothing switches the branch under a working tree the user is using. The cost
  is that the agent cannot run the pull request's tests — a deliberate trade for
  a first pass that starts in one press and cannot break anything.
- **A tab, not a dialog.** The agent takes minutes; the point is to start it and
  go on reading. The tab is keyed on the pull request, so pressing Review again
  comes back to the run in flight rather than starting a rival one.
- **The findings stay in the session.** No `gh pr review`, no `gh pr comment`.
  The person watching decides what reaches the pull request.
- **Scope chips cost nothing.** The daemon already asks GitHub the three viewer
  questions in the one GraphQL query it runs (`assigned`, `reviewRequested`,
  `authored`, §16.6) and ships the answers on every row, so filtering is a read
  over data in hand — a scope the user picks must not cost a `gh` subprocess.

## The blocker this had to solve

Three of the four providers already took a prompt at launch; **none of them
could be asked to launch without write access**. Starting a review in the user's
main checkout with an agent that can edit it is not a smaller version of the
feature, it is a different one.

It is solved with the same shape resume and prompt already have, so that no
other crate branches on a provider id (P2):

```rust
pub struct ReviewStyle {
    /// The provider's own flags for a read-only session, in order.
    pub args: Vec<String>,
    /// What the provider calls that mode, for a menu that has to say.
    pub label: String,
}
```

Every built-in has one, read off its own `--help`: Claude's
`--permission-mode plan`, Codex's `-s read-only`, the `plan` agent OpenCode
ships, Cursor's `--mode ask`. A provider that declared none would be refused
rather than launched able to write.

OpenCode also gained the prompt spelling it always had: `--prompt <text>`. Its
positional is a *project directory*, which is why the descriptor previously
declared none — the flag is the form that does not launch it in a folder named
after the prompt.

## Layers

1. **`domain`** — `ReviewStyle`; `AgentDescriptor.review`;
   `AgentCapabilities.supports_review`; `LaunchAgentRequest.read_only`; and
   `pr_review.rs` with the four built-in recipes and `compose_review_prompt`.
2. **`agents`** — each built-in declares its read-only spelling; `build_launch`
   appends those flags after a profile's own so they win, and refuses with
   `AgentError::ReviewUnsupported` when there is none.
3. **`protocol`** — `CreateAgentSession { …, read_only }`, refused with
   `InvalidRequest` like `resume` and `initial_prompt` are.
4. **`daemon`** — threads it into the `SpawnSpec`, tags the session
   `SessionRole::Reviewer`, and remembers it in `Inner::read_only` so a
   **restart keeps the flags** — the opposite of the initial prompt, which is
   deliberately not re-sent.
5. **`apps/tauri`** — `panels/prFilters.ts` (scope + search, pure),
   `workbench/prReview.ts` (recipes, prompt, agent choice, adoption, pure),
   `workbench/PrReviewView.tsx` (the tab), and the Review action on the PR card
   and on the PR detail.

## Progress, without guessing

The runtime command channel is one-way — it also carries keystrokes, so it
cannot block on an answer — and the id of the session a launch creates does not
come back. The tab records the moment it asked and adopts the newest agent
session that appeared in the target checkout after it (`adoptLaunched`). That is
the same shape the PR compose tab uses; the timestamp is what keeps it from
adopting an agent that was already running there.

## What is deliberately not here

- **No worktree per pull request.** It was the alternative considered: fetch
  `refs/pull/<n>/head`, check it out, let the agent run the tests. It costs a
  fetch, a worktree, provisioning and a cleanup story, and it is a different
  feature — "reproduce this branch" rather than "read this change". The `gh`
  path gives the first pass with none of that.
- **No posting back to GitHub.** See the decisions above.
- **No review of a pull request from a repository Forge does not have.** `gh`
  needs a directory whose remote is the repository to be authenticated against
  it; the tab says so rather than starting something that would fail.
