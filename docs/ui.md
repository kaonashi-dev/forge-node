# UI

Forge's window is the Tauri + Solid shell in `apps/tauri`. The host
(`apps/tauri/src-tauri`) owns the runtime thread, workbench worker, and cell
encoder; the WebView renders chrome and a Canvas terminal from a passive
`client::CellGrid`. There is no second VT engine (ADR-011).

Day-to-day layout and behaviour details live next to the code:

| Area | Where |
|------|--------|
| Shell (title, sidebar container, tabs) | `apps/tauri/src/app/shell/` |
| Projects view and worktree workflows | `apps/tauri/src/features/projects/` |
| Other sidebar views (files, git, history, PR) | `apps/tauri/src/features/` |
| Workbench (diff, PR, previews) | `apps/tauri/src/features/{git,pull-requests,files}/` |
| Editor surfaces (cells and DOM) | `apps/tauri/src/features/editor/{cells,dom}/` |
| Reusable explorer package (no Solid or Tauri) | `apps/tauri/packages/file-workbench/` |
| Terminal Canvas | `apps/tauri/src/shared/cell-grid/` |
| Control kit | `apps/tauri/src/ui/` |
| Theme tokens | `apps/tauri/src/theme/` + [theming.md](./theming.md) |
| Actions / keymap | `apps/tauri/src/actions/` |
| Ownership map and dependency rules | [frontend-architecture.md](./frontend-architecture.md) |
| App README | `apps/tauri/README.md` |

## Layout (summary)

```text
┌──────────────────────────────────────────────────────────────┐
│ title bar + session tabs                                      │
├─────────────────────┬────────────────────────────────────────┤
│ sidebar view strip  │  centre: terminal / workbench tabs      │
│ then one view       │                                         │
└─────────────────────┴────────────────────────────────────────┘
```

One left sidebar holds a strip of view icons above a single visible view, in the
order Projects, Files, History, PR, Git. Projects stays
mounted while another view is up. Whether it is open, its width and the active
view live in `ui.sidebar.open` / `ui.sidebar.width` / `ui.sidebar.view`; theme metrics
(`TITLE_H`, the sidebar default, control ladder) come from `theme/tokens.ts`,
never hardcoded hex in views.

Each workspace has one Code entry in Projects and one Code tab in the title bar,
available even before a file is opened. Individual editor sessions appear as file
tabs inside Code. Closing the last file leaves Code open with its empty state.

## Files

The Files sidebar is a lazy disk tree, independent of the global file index.
Empty and hidden folders are visible; ignored folders are dimmed and loaded
on expansion. Errors retain the last successful children and partial listings
are labelled. Reveal loads the required ancestors before selecting its target.

Right-click, Ctrl-click or Shift+F10 opens Forge's menu on rows and on the
root/background, including an empty checkout. Root offers new file/folder,
refresh, collapse and copy workspace path; rename, move and delete are entry
actions. Rename edits only the basename even on a compact folder chain. Create
accepts a validated nested relative path. The field stays pending until its own
mutation result, preserves its text on refusal, and cannot submit twice.

The embedded files/text selector, filter input and content-search block are
removed. File search remains global: Cmd+P and the macOS Cmd+O alias open the
file palette, with custom keybindings taking precedence. Cmd+Shift+F opens
content search. Refresh remains a tree action and a context-menu item.
Confirmed moves retarget open/parked views, previews, recent files and reveal
requests. Editor tabs keep their session identity and follow daemon path
metadata, including changes initiated in another client.

Drag a row at least 5 px to move it to a folder or to the blank tree area
(checkout root). The hover label names the operation. A 600 ms folder hover
expands it; the tree scrolls at its edges. A drop on the visible terminal instead
pastes the absolute, shell-quoted path, with no Enter or file contents. The file
menu's **Insert reference in terminal** performs the same targeted paste.
If no session terminal is visible and connected, the menu reports that instead.
Escape, pointer cancellation, window blur, unmount, workspace/session changes
and reconnect cancel a gesture. Capture belongs to the stable explorer mount,
not a recycled row, and the release click and terminal mouse reports are consumed.

## Terminal

Damaged rows arrive on `runtime:cells` separately from `runtime:state`, so a
frame of output never serializes the session tree with it. The Canvas renderer
paints style runs; selection, IME, and mouse reporting live under
`features/terminal/` and `shared/cell-grid/`, with clipboard helpers in
`shared/input/`. These paths are relative to `apps/tauri/src/`.
Latency gate: `make latency-tauri` (p95 ≤ 50 ms).

## Controls

Call sites import from `apps/tauri/src/ui/` (Button, Dialog, Menu, …). Look is
Forge CSS + tokens; interaction primitives are `@kobalte/core` behind that
boundary. Structural chrome belongs to `app/shell/`, `app/palette/` and the
owning feature; explorer rows are rendered by `packages/file-workbench`.

## Keyboard

Chords resolve through `actions/` → dispatch. Settings → Keyboard and the
command palette surface the same bindings. Terminal input mapping for the PTY
stays in `terminal-input` / the host bridge, not in Solid.

`Cmd+=` / `Cmd+-` / `Cmd+0` (`Ctrl` on Linux) zoom whichever content is on
screen, as two independent preferences: the open file (`ui.editor.font_size`)
while Code is up, every terminal and agent (`ui.terminal.zoom`) while a
session is up. Chrome type size is Settings → Personalization → Interface
size (`ui.font_size`) and does not follow those chords. Unmatched, the same
chords are swallowed so WKWebView cannot scale the whole window.

`Cmd+D` (`Ctrl+Shift+D` on Linux) splits the centre into two columns: a new
shell beside the current terminal, or the current session beside the open
file. Diff, search and pull-request views stay full width. `Cmd+W` on the extra
column joins the panes without closing the session.

`Cmd+Shift+F` (`Ctrl+Shift+F` on Linux) opens or focuses the Search tab in Code.
Results are grouped by file with line numbers, highlighted matches and three
lines of context on each side; overlapping excerpts merge. Click a line to
open its file at that location. Repeating the shortcut selects the search text
without clearing it or changing the sidebar view.
Excerpts use the file's language and the editor theme for syntax colours;
search marks remain visible over them. Unknown languages and oversized excerpts
stay plain, with their text and search marks intact.
