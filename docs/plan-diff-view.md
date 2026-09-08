# Plan — Workspace diff view (§16.7)

## 1. How OpenCode does it

Sources read: `packages/tui/src/feature-plugins/system/diff-viewer.tsx`,
`packages/opencode/src/project/vcs.ts`, `packages/opencode/src/git/index.ts`
(repo `anomalyco/opencode`).

**Server (`vcs.diff(mode, { context })`)** — there is no diff library: it is the
system `git`, captured, and the TUI receives *one unified patch per file*.

| Data | Command |
| --- | --- |
| file list | `git status --porcelain=v1 --untracked-files=all --no-renames -z -- .` |
| list against a ref | `git diff --no-ext-diff --no-renames --name-status -z <ref> -- .` |
| `+`/`-` per file | `git diff --no-ext-diff --no-renames --numstat -z <ref> -- .` |
| patch of everything (1 process) | `git diff --patch --no-ext-diff --no-renames --unified=N <ref> -- .` |
| patch of one file | `git diff --patch ... --unified=N <ref> -- <file>` |
| untracked file | `git diff --no-index --patch ... -- /dev/null <file>` |

Details that matter:

- The big patch is split by `diff --git ` and reassigned to each file; the
  per-file one is only used as a *fallback* (untracked, or a capped batch).
- There are two caps: a global `MAX_TOTAL_PATCH_BYTES` and a per-file one. When
  exceeded, the file travels with an empty patch instead of breaking the answer.
- `mode: "git"` = working tree against `HEAD` (or against the empty tree if
  there is no HEAD); `mode: "branch"` = against the `merge-base` with the default
  branch.
- The TUI asks for `context: 12`, not 3.

**TUI** — the viewer parses nothing else: it receives the unified text and paints
it.

- A 32-column left panel with a tree built from the paths, collapsing
  single-child directories; it is hidden when there are no files.
- Split (two columns) when the patch panel measures ≥100 columns; otherwise
  unified. `diff_style: "stacked"` forces it to unified.
- Hunk navigation: the lines of the patch text starting with `@@` are scanned
  and it jumps to their `y` within the scroll. There is no hunk model.
- `m mark reviewed` is a local `Set<string>` of file names, in memory.
- `d switch source` rotates working tree / branch / last turn.

## 2. What we build in Forge

The user's decision: **working tree only (against `HEAD`), with no source
switch**; it opens from the `+` menu of the tab strip **and** from the context
menu of a branch/checkout in the rail.

### 2.1 `git-service` — `src/diff.rs` (new)

`working_tree_diff(repo, options) -> Result<WorkingTreeDiff, GitError>`, with the
same set of commands as OpenCode and the caps that already exist in the crate:

- `status --porcelain=v1 --untracked-files=all --no-renames -z -- .` gives the
  list and the status (`A`/`M`/`D`), with the exact paths (`-z` avoids quoting).
- `diff --numstat -z HEAD -- .` gives `+`/`-` for what is tracked; a `-` in
  numstat means binary.
- A single `diff --patch --unified=N HEAD -- .`, split by `diff --git `.
- Every untracked file is asked for with `diff --no-index --patch -- /dev/null
  <file>`, which **exits with status 1 by design**: 1 is accepted, the rest is
  rejected.
- With no `HEAD` (a freshly created repo) the ref does not exist: everything is
  "added" and only the `--no-index` path is used.
- Caps: a global `MAX_DIFF_BYTES` and a per-file `MAX_FILE_PATCH_BYTES`; when
  exceeded, the file travels with an empty `patch` and `truncated: true` — a
  patch is never cut in half (a cut patch is an invalid patch).

### 2.2 `domain` — `src/diff.rs` (new)

`WorkspaceDiff { workspace_id, branch, files, truncated }` and
`DiffFile { path, status, additions, deletions, patch, binary, truncated }`.
Runtime-only, like `PullRequest`: no column, no migration.

### 2.3 `protocol`

`Request::GetWorkspaceDiff { workspace_id, context_lines }` →
`Response::WorkspaceDiff(WorkspaceDiff)`. A local synchronous request, just like
`ListBranches` and `GetChangeContext`: it does not open a socket, so the "ack
now, event later" invariant does **not** apply.

### 2.4 `daemon`

`core.rs`: `get_workspace_diff` resolves the path with `workspace_path` (already
present) and calls `git_service::working_tree_diff` **outside** the core lock,
like the rest of the git paths.

### 2.5 `client`

`Client::workspace_diff(workspace_id, context_lines)`. No state in `Store`: the
diff is a view you ask for, not shared state that gets broadcast.

### 2.6 `ui`

- `runtime.rs`: `RuntimeCommand::LoadDiff { workspace }` →
  `RuntimeUpdate::Diff { workspace, diff }` / `RuntimeUpdate::DiffFailed`.
  It goes through the runtime thread, never through the render one.
- `diff_view.rs` (new): the panel. File tree on the left (the same single-child
  directory collapsing as OpenCode), patch on the right. The patch is converted
  to a `Vec<DiffRow>` — one row per line, pairing the `-`/`+` runs of each hunk —
  and painted in two columns when the width allows, or in one when it does not.
  Each row is a direct child of the scrolling container, so
  `ScrollHandle::scroll_to_item` jumps to a hunk without measuring pixels.
- `session_tabs.rs`: an optional extra tab for the diff, looking the same as the
  session ones and with its own `x`.
- `session_menu.rs`: a `Diff` entry at the end of the `+` menu.
- `sidebar.rs`: `SidebarAction::ShowDiff(WorkspaceId)` in the branch menu.
- `app_shell.rs`: `diff: Option<DiffState>` and `active_view`, keys inside the
  panel (`tab`, `n`/`p`, `[`/`]`, `m`, `v`, `r`, `escape`).

## 3. Out of scope

No `branch`/`last turn` as sources, no syntax highlighting, no intra-line
highlighting, no staging and no discarding from the view. The view is read-only:
it validates changes, it does not apply them.
