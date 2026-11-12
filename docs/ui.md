# UI

Forge's window is the Rune client in `apps/ui` (`forge-ui`). It owns no PTY and
no VT engine: a worker thread holds `client::Client`, applies every terminal
delta to a passive `client::CellGrid` replica, and the Rune view paints it. The
daemon remains the only emulator (ADR-011).

`apps/ui` is its own Cargo workspace on Rust 1.99.1, outside the root workspace,
because Rune needs edition 2024. `scripts/dev check` runs both gates.

## Module map

| Area | Where |
|------|--------|
| App state, event application, polling | `apps/ui/src/app.rs` |
| Client worker, reconnect, command channel | `apps/ui/src/connection.rs` |
| Shell, titlebar, sidebar, centre card | `apps/ui/src/ui.rs`, `chrome.rs` |
| Projects rail, groups, checkouts, sessions | `apps/ui/src/projects.rs`, `rail.rs`, `project_ops.rs` |
| Workbench: diff, files, history, PRs, processes | `apps/ui/src/workbench/` |
| Editor surface | `apps/ui/src/editor/` |
| Terminal replica, renderer, input, splits | `apps/ui/src/terminal/` |
| Settings sections and preferences | `apps/ui/src/settings/` |
| Theme roles, bases and the picker preview | `apps/ui/src/theme/`, `appearance.rs` |
| Icons, file marks, file types | `apps/ui/src/glyphs.rs`, `file_icons.rs` |
| Context and native menus | `apps/ui/src/menus.rs`, `native_menus.rs` |
| Palette, keymap, tab strip, toasts | `apps/ui/src/palette.rs`, `keymap.rs`, `tabs.rs`, `toasts.rs` |
| Agent workflow and review | `apps/ui/src/agent_ops.rs`, `handoff.rs`, `code_review.rs` |
| Git/PR depth | `apps/ui/src/workbench/`, `pr_review.rs`, `pr_detail.rs`, `open_pr.rs` |
| Updater | `apps/ui/src/updates.rs` |
| App README and commands | `apps/ui/README.md` |

## Layout

```text
┌─────────────────────┬────────────────────────────────────────┐
│ traffic lights      │ ┌ card ─────────────────────────────┐  │
│ view switcher       │ │ session tabs + ⌕    actions/pill  │  │
│ the active view     │ │ project / branch / path · state   │  │
│ settings            │ │ centre: terminal / editor / view  │  │
│                     │ │                                   │  │
│                     │ └───────────────────────────────────┘  │
└─────────────────────┴────────────────────────────────────────┘
```

The sidebar holds the traffic lights, the view switcher (Projects, Files,
History, Pull Requests, Git, Processes), the active view and Settings. Its
open/width/view state lives in `ui.sidebar.open`, `ui.sidebar.width` and
`ui.sidebar.view`. Projects stays mounted while another view is up. Sessions
waiting on the user are pinned above the tree; with another view open a
"N waiting" pill names them.

The card strip carries the session tabs, the new-session `+` and the session's
actions. A slim bar under it says where the card is and what the working tree
looks like. Settings replace the sidebar's content with their sections and show
the page in the card; Esc twice returns.

## Files and workbench

The Files tree is a lazy, watched disk tree with git decorations, a filter,
reveal/follow and recent files. Markdown, CSV/TSV, SVG and image previews open
without an editor; code opens in an editor session. Writes (new file/folder,
rename, duplicate, move, delete) go through the daemon (`fs-service`), never
`std::fs` in the UI (ADR-012).

Changes is a fused workspace diff; the Git panel adds commit (with Juva), fetch,
stopped rebase/merge/cherry-pick actions and branch compare. A session's own
changes are read separately (`GetSessionChanges`). Pull requests are a cached
read of the daemon's last refresh; the list filters by scope and text, and a
row opens a detail tab. "Review Pull Request…" in the palette takes a pasted
link (`pr_url`) and opens the same detail; one the last refresh did not list is
read with `FetchPullRequest` and kept across refreshes (open ones only).
Opening or composing a pull request is an agent launch or the normal
`CommitAndOpenPullRequest` request.

## Terminal

Every attached terminal is one passive replica (`SharedTerminal`). Deltas apply
in order; a skipped publication merges its damage and a sequence gap forces a
resync. The compositor paints style runs, selection, underline/strike,
links and the cursor. Input goes through `terminal-input`; a key never reaches
a PTY as a different escape sequence here. The split column (⌘D) owns a second
replica with its own input gate, measurement and PTY size. ⌘-hover underlines a
URL or path and ⌘-click opens it; a plain click on the shell's line moves its
cursor. The terminal zoom is `ui.terminal.zoom` (50–200 %). Files dropped from
Finder paste as single-quoted paths.

## Editor

The daemon owns `forge-editor`; `apps/ui/src/editor/` renders an
`EditorFrame` window as Rune rows memoised by `(line, fingerprint)`. Caret,
selection, folds, gutter marks, diagnostics, find, conflict and IME are
overlays. Find drives `EditorFind`; ⌘B and ⌃- are go-to-definition/back in the
editor context. A refused save offers Keep mine / Take disk / Compare.
`ui.editor.font_size`, `ui.editor.line_height` and autosave are applied live.
See [editor.md](./editor.md).

## Controls and theme

Controls come from `rune_kit` (Button, Dialog, Menu, TextInput, Switch, …).
Colours and metrics come from the Forge theme roles in `theme/roles.rs` and
`theme/themes.json`, never a hex in a view. The theme picker previews a base
before it is committed and supports a custom accent, import/export, contrast,
density, motion, glass and trim. See [theming.md](./theming.md).

Icons are Forge's own 59-glyph set on one 16-grid (`glyphs.rs`); language marks
are the 87-file set in `file_icons.rs` with the SVGs under
`apps/ui/assets/lang/`. Brands are the provider exception.

## Keyboard and palette

Chords resolve through `keymap.rs`; Settings › Keyboard lists the same bindings
and captures a replacement, persisted in app state. The palette opens on ⌘K
with scopes: `>` commands, `/` files, `@` sessions and checkouts, `#` symbols.
⌘P opens files directly and ⇧⌘A starts a terminal or an agent (profiles
included). Holding Ctrl and pressing Tab walks sessions in most-recently-used
order in an overlay; releasing commits. Tabs can be dragged to reorder.

`Cmd+=` / `Cmd+-` / `Cmd+0` zoom whichever content is on screen as two
independent preferences: the open file (`ui.editor.font_size`) while an editor
is up, every terminal and agent (`ui.terminal.zoom`) while a session is up.
Chrome type size is Settings → Appearance → Type size → Chrome (`ui.font_size`).

⌘D splits the centre into two terminal columns; the extra column joins without
closing its session. ⇧⌘F opens project content search; ⇧⌘T reopens the last
closed file; ⌥⌘←/→ step between editors.

## Host

Native menus (Forge Node, File, Edit, View, Window, Help) dispatch the same
key path as the keyboard. File dialogs, clipboard, open URL/reveal and the
updater are host modules, not view code: `native_menus.rs`, `updates.rs`,
`menus.rs`. The updater checks `latest-rune.json`, verifies the minisign
signature and installs only a protocol-compatible GUI; a hard update is refused
in place.

## Performance

The cost model in [performance.md](./performance.md) binds this client too:
nothing that scales with the grid is deep-cloned on the delta rung, per-cell
code allocates nothing, and anything derivable from the store is computed when
the store changes, not per frame. `apps/ui/scripts/perf.sh` records the Rune
measurements; `apps/ui/README.md` lists the commands.
