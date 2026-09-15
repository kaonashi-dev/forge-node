# Editor

Two crates build the terminal editor:

- `crates/editor-core` is the document model — text, selection, transactions,
  history, versions, search. It owns no terminal, no filesystem and no Forge
  type, and every mutation goes through `Document::apply`, so read-only, the
  byte budget, the version counter and undo cannot be bypassed.
- `crates/editor-cli` builds `forge-editor`, the standalone binary: argument
  parsing, a bounded read, raw mode, key bindings and the viewport.

The integrated route — the same binary under the daemon's PTY, with its own
control channel — is **the** editor, and the only one. The DOM editor that used
to live in `apps/tauri/packages/file-workbench` is gone: that package is a file
explorer now. Opening any file in the Code tab opens a `forge-editor` session.

What that cost, stated plainly: a rendered Markdown document, an SVG and a
raster image no longer have a surface of their own — they open as text, and a
binary one is refused by `CreateEditorSession` like any other. `Markdown.tsx`
survives because the pull-request view renders with it, and `CompareView`
survives because the conflict banner does.

## Run it

```sh
cargo run -p editor-cli --bin forge-editor -- path/to/file.rs
cargo run -p editor-cli --bin forge-editor -- +42 path/to/file.rs
```

It is standalone by design: no daemon, no GUI, no network and no Node are
needed to open, edit and save a file. `forge-editor --help` prints the contract
(path, `--read-only`, `--line N` or `+N`, `--` before a path that starts with a
dash).

## The integrated route

Opening a file in the Code tab runs `forge-editor` as a daemon session. There is
no flag: with nothing to fall back to, `ui.editor.terminal` would have been a
switch that turns the editor off.

What happens then:

- `CreateEditorSession { workspace_id, path, line, read_only }` reads the file
  through `fs-service` — off the core lock, and refused for a directory, a
  binary file or one past the 2 MiB budget — then spawns `forge-editor` under
  the daemon's PTY as a `SessionKind::Editor` session. `[editor] executable`
  names the binary; otherwise it is `forge-editor` on the resolved `PATH`,
  then the binary next to this `forge-daemon` (the package layout). A
  missing binary fails the spawn and never falls back to a shell.
- The buffer travels over a private Unix socket named by `FORGE_EDITOR_CONTROL`,
  not through argv and not through the PTY: `crates/editor-control` owns the
  framing, the version and the message set. `--control <socket>` is what selects
  integrated mode, and in it the editor reads no disk at all — the text arrives
  in `Open`.
- The editor publishes `path`, 1-based `line:column`, dirty and read-only back
  over the same socket. The daemon stores the last one on `Session.editor` and
  broadcasts it through the ordinary `SessionUpdated`, coalesced, so typing does
  not put a broadcast on every keystroke. The pane's chrome reads that state and
  says *state unknown* until the first one lands — it never parses the terminal
  it is painting.
- Input, resize and ANSI output ride the existing terminal pipeline, on a side
  attachment with its own frame channel. The main pane keeps its PTY.
- `Session.editor` is runtime state like `terminal_id`: no column, no migration,
  gone on restart. An editor session is never idle-stopped, whatever the
  thresholds say.

**The idle status row is the pane's, not the TUI's.** Standalone, `forge-editor`
paints an inverted status row — path, `[modified]`, `line:column`, `F1 help` —
along the bottom. Integrated, that row is a second copy of what the pane's HTML
chrome already shows, so the TUI drops it and gives the whole height to the
buffer. A prompt (find, go-to-line, confirm) and a transient message still take
the last row in both modes; only the idle bar is silenced.

**Colour is the grammar's, and it is scanned whole.** `editor_core::Syntax`
scans the document into per-line spans on every mutation and never per row: a
block comment opened above decides the colour of everything below it, so the
scan cannot be limited to what is on screen. The grammar comes from the
extension (`Grammar::for_path`) and an unknown one stays plain — a wrong colour
is worse than none. Past 512 KiB the buffer is shown plain, which is what bounds
the cost rather than merely spreading it. The scopes paint in the ANSI 16, so
the reader's terminal theme is the theme.

**The gutter's marks are the daemon's git, not the editor's.** The editor never
opens the checkout, so `git_service::file_marks` runs on the supervisor thread
(`git diff --unified=0` — the hunk headers alone, so the cost is flat in the
size of the file rather than its diff) and the marks travel as `GitMarks`. Sent
on open and after every save that lands. A git failure is no marks, never a
failed buffer: a gutter is decoration. The mark column is always drawn, marks or
not — one that appeared when git answered would shift every line sideways.
<kbd>Alt</kbd>+<kbd>N</kbd>/<kbd>P</kbd> step *blocks*, because a run of marked
lines is one change to a reader.

**Autosave is the opener's preference, carried across.** It rides `Open` and
`SetEditorAutosave` toggles it live; the daemon holds no opinion about it. The
editor arms a one-second pause on a mutation and the main loop *waits on* that
deadline (`Selector::wait_deadline`) rather than polling for it — a
sleep-and-check loop here is the defect `docs/performance.md` names. The
deadline is re-checked when it fires, so a keystroke in between pushes the pause
out, which is the whole point of waiting for stillness. A read-only buffer never
arms one.

**A save is a request, never a write from the editor.** `Ctrl-S` sends
`SaveRequest` with the buffer text; the daemon writes it through
`fs-service::write_file` at the path *it* opened, conditioned on the revision
*it* last wrote, and answers `Saved { revision }` or `SaveRefused { reason }`.
The editor confirms the snapshot it sent, so a keystroke that lands mid-write
leaves the buffer dirty — which is the truth. A revision mismatch is a refusal,
not an error: an agent wrote the same path and the buffer is intact.

`read_only` is enforced **in the daemon**, not only in the editor: it travels in
`Open` as a courtesy, but a read-only session refuses a `SaveRequest` before it
reaches `fs-service`, because a buggy or replaced editor must not be able to
write through a buffer that was opened read-only.

When a save is refused, the daemon keeps the two sides — the draft it would not
write and the bytes that were there instead — behind its own `Mutex`, like
`pull_requests` and for the same reason. `Session.editor.conflict` is the flag
that rides `SessionUpdated`; the documents themselves only travel in the answer
to `GetEditorConflict`, because two buffers on the broadcast channel are exactly
the payload its bound exists to keep off. The pane then offers the same three
answers the DOM editor does — *keep mine* (`OverwriteEditorBuffer`), *take disk*
(`ReloadEditorBuffer`) and *compare*, which reuses `CompareView`.

A second open of a file that already has a live editor session becomes a
`RevealInEditorSession` on that session — the caret moves in the tab that is
there. A rival editor would be a second process, a second PTY and a second
draft of one file. Closing the Code view detaches it: the process survives and
the session stays in the rail until it is killed.

**An editor is a file, not a window tab.** One `SessionKind::Editor` session per
open file lives in the daemon, but the GUI never lists it in the window's
session strip beside the shells and agents — it belongs to the Code surface,
shown as a sub-tab under the `Code` pill with the file's basename. Focusing one
from the rail or history opens Code and reveals the file rather than swapping the
main terminal. Closing the Code view detaches the pane; the process can go on
living with no window pill of its own.

## Keys

| Key | Action |
| --- | --- |
| arrows, Home/End, PageUp/PageDown | move; hold Shift to select |
| Ctrl or Alt + arrows | move by word |
| printable characters, Enter, Tab | insert; Tab advances to the next 4-column stop |
| Backspace, Delete | grapheme-aware deletion; Alt-Backspace deletes a word |
| Ctrl-S | save (exclusive temp file in the same directory, then rename) |
| Ctrl-Z / Ctrl-Y | undo / redo |
| Ctrl-C / Ctrl-X / Ctrl-V | copy / cut / paste through an internal register |
| Ctrl-A / Ctrl-L / Ctrl-K | select all / select line / delete line |
| Ctrl-F, Ctrl-N, Ctrl-B | find, next match, previous match |
| Ctrl-T | toggle match case |
| Ctrl-R | replace all (Tab switches field, Enter applies) |
| Ctrl-G | go to line |
| Alt-N / Alt-P | next / previous changed block (Ctrl-N and Ctrl-B are the find bar's) |
| Ctrl-Q | close; unsaved changes ask save / discard / cancel |
| F1 | key help |
| bracketed paste | inserts the pasted text as one undo step |

`Ctrl-C` copies rather than quitting, and `Esc` closes a prompt without
touching the document. `--read-only` refuses every edit at the transaction
entry and says so instead of silently dropping keys.

**The mouse moves the caret.** A left click places the caret; a Shift-click
extends the selection to the click; a left drag selects; the wheel scrolls the
viewport without moving the caret. The editor turns on mouse capture on entry
(`EnableMouseCapture`), so a real terminal reports the events and the Code pane
forwards them as the same SGR reports the main terminal sends — the editor reads
the mouse itself either way. Out of scope for now: go-to-definition on click,
middle-click paste, and a native right-click menu (the Code pane keeps its own
HTML context menu).

## What the core guarantees

- **Byte-exact round trip.** The buffer holds the file's bytes, terminators
  included, so a mixed LF/CRLF file, a BOM and a missing final newline all
  survive. A new line takes the terminator the current line already uses.
- **One mutation entry.** Typing, paste, replace-all and a reload are all
  transactions with a selection on both sides. Undo restores text *and*
  selection; a run of typing coalesces into one entry, a paste or a
  replace-all never does.
- **Budgets before allocation.** 2 MiB of valid UTF-8 per document, checked
  against `metadata` *and* against a capped reader, because the file can grow
  in between. A transaction that would cross the limit is refused whole. Undo
  history has its own byte and entry budget and reports what it dropped.
- **Dirty is content identity, not a counter.** Undoing back to the saved text
  clears it. A save confirms the snapshot it wrote, so a keystroke that lands
  during the write leaves the buffer dirty — which is the truth.
- **Search limits are two different numbers.** 5 000 results are listed; a
  replace-all scans past that and either rewrites every match or refuses and
  rewrites none.
- **Rows, not frames.** A keystroke damages one line and the renderer repaints
  one row plus the status line.

## Limits, and what replaces them

- Regular-expression search is not implemented: it needs a dependency this
  workspace has not approved. Find is literal, with match-case and whole-word.
- One buffer per process. No splits, no Vim profile. The mouse places the
  caret, extends a selection and scrolls, but not more than that.
- Long lines wrap under the daemon and scroll sideways standalone. Integrated,
  the pane is a fixed width the person cannot widen, so a line too wide for the
  text column continues onto the next row (with a blank gutter) rather than
  running off the edge; the caret, a click and the wheel all follow the wrapped
  rows. Standalone, where the terminal can be widened and code reads better
  unwrapped, the view scrolls horizontally to follow the caret instead. The
  scroll anchor is a whole line, so a single logical line taller than the
  viewport cannot be scrolled through within itself — fine for prose, where a
  line wraps to a few rows, and the case a future sub-row anchor would cover.
- The save is optimistic, not exclusive: it compares the revision it loaded
  against the disk before writing and refuses once when they differ, which
  catches an agent that already wrote. It is not a compare-and-swap against a
  program that does not cooperate.
- Movement and deletion are grapheme-based, tabs and wide or combining
  characters occupy the right cells, and control bytes in the file are drawn as
  Unicode control pictures — an `ESC` inside a file is shown, never executed.

## Candidates measured

`crossterm` 0.29 was chosen over a widget library: the viewport is arithmetic
either way, and the cell rules want no intermediate buffer to diff. `ratatui`
stays a candidate if layout or widgets earn their cost in a measurement.
crossterm adds a second `signal-hook` (0.3 against alacritty's 0.4) to the
workspace; both are small and permissively licensed.

A rope is still a candidate for the core's storage. The current `Text` keeps
one `String` plus a line index it shifts rather than rescans, which is enough
for the 2 MiB budget; adopting a rope is a dependency decision that needs a
version compatible with Rust 1.89.

## What the next milestone still has to prove

**A copy leaves through OSC 52.** `Ctrl-C` fills the editor's own register *and*
emits `ESC ] 52 ; c ; <base64> BEL`; `terminal-core` captures the store,
clamping it to `MAX_CLIPBOARD_BYTES` before it is kept, and the daemon
broadcasts `ClipboardStore`. The Tauri host forwards it only for a terminal the
window is showing — the focused one or an open editor pane — because OSC 52 lets
whatever runs in a PTY set the clipboard, and a background agent replacing it
would be a real hazard. OSC 52's *read* is never answered, by the engine or by
anyone: it would hand the person's clipboard to the child process.

What is left. A rendered Markdown document,
an SVG and a raster image still open on the DOM side — a terminal cannot draw
them, so that split is the design rather than a gap. What *is* a gap: go-to-definition and
find-references, which need a channel from the editor to `SearchFiles` and are
the one item here that changes shape rather than filling in; the system
clipboard, where `Ctrl-C` still fills an internal register, so a copy does not
leave the editor (the pane's own menu pastes from the system clipboard, and a
mouse selection copies through the terminal's existing path); the change
*details* panel, where the gutter has the marks but not the before/after rows;
and the overview ruler. The pane does draw a scrollbar thumb: `EditorState`
carries `top_line` / `visible_lines` / `total_lines`, so the GUI reports where
the editor's viewport is without keeping a second copy of the text. It is a
read-out and not a handle — the wheel still goes to the TUI, which owns the
viewport, and the pane's renderer opts out of hiding the caret for a scrollback
offset that the editor never moves. `forge-editor` now ships in the bundle
(`scripts/package-macos`, `scripts/package-linux`), so `[editor] executable`
falling back to a `PATH` lookup resolves in a packaged install and not only in
a dev checkout.

End to end, `crates/editor-cli/tests/pty.rs` drives a real PTY in both modes: a
typed character on screen, bracketed paste as one undo step, a resize mid-paste,
CRLF preserved through a save, a control byte drawn rather than executed, a
read-only refusal, a dirty close that asks first, and — integrated — a buffer
that came from the socket, a reveal that moves the caret, a `Ctrl-S` that
becomes a request and never a write, a refusal that is reported, and a display
path that is never opened. `crates/daemon/tests/scenario_editor.rs` covers the
daemon's half: the control socket in the child's environment, the text
`fs-service` read on the child's screen, state surviving a client reconnect,
detach leaving the process alive, a crash marking the session and releasing the
supervisor, a save that reaches the disk through `fs-service`, and a read-only
session that refuses one. Its stand-in reads `CONTROL_VERSION` rather than
hardcoding it, so a bumped wire fails loudly instead of as a mute handshake.

Bracketed paste still has an unbounded peak: crossterm 0.29 accumulates the
whole paste into a `String` before it delivers `Event::Paste`, so the document
budget refuses an oversize paste only after the bytes are already resident. It
is the same debt in both modes — the integrated route reuses the same input loop
— and capping it means owning the input loop or vendoring crossterm's parser,
which is a piece of work of its own rather than a line in this one.
