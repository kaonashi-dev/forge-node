# Editor

Two crates build the terminal editor:

- `crates/editor-core` is the document model — text, selection, transactions,
  history, versions, search. It owns no terminal, no filesystem and no Forge
  type, and every mutation goes through `Document::apply`, so read-only, the
  byte budget, the version counter and undo cannot be bypassed.
- `crates/editor-cli` builds `forge-editor`, the standalone binary: argument
  parsing, a bounded read, raw mode, key bindings and the viewport.

The GUI's own editor still lives in `apps/tauri/packages/file-workbench` and is
the default surface. The integrated route — the same binary under the daemon's
PTY, with its own control channel — is built and opt-in; see *The integrated
route* below.

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

The Code tab can run `forge-editor` as a daemon session instead of the DOM
editor. Turn on `ui.editor.terminal` in Settings → Editor (off by default) and
the open file grows an **Open in terminal editor** action.

What happens then:

- `CreateEditorSession { workspace_id, path, line, read_only }` reads the file
  through `fs-service` — off the core lock, and refused for a directory, a
  binary file or one past the 2 MiB budget — then spawns `forge-editor` under
  the daemon's PTY as a `SessionKind::Editor` session. `[editor] executable`
  names the binary; otherwise it is `forge-editor` on the resolved `PATH`. A
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

**H1 is read-only.** Integrated save with a disk revision does not exist yet, so
the buffer refuses edits at the transaction entry rather than letting a draft
exist that nothing could persist; `Ctrl-S` says *integrated save is not
available* and touches no file. The DOM editor's Save and autosave are not
offered for the terminal pane, and closing the Code view detaches it — the
process survives and the session stays in the rail until it is killed.

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
| Ctrl-Q | close; unsaved changes ask save / discard / cancel |
| F1 | key help |
| bracketed paste | inserts the pasted text as one undo step |

`Ctrl-C` copies rather than quitting, and `Esc` closes a prompt without
touching the document. `--read-only` refuses every edit at the transaction
entry and says so instead of silently dropping keys.

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
- One buffer per process. No syntax highlighting, no mouse, no splits, no
  wrapping, no Vim profile.
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

Integrated save with a revision, full parity with the DOM editor, and retiring
it are later milestones; so is packaging `forge-editor` with the app, which for
now is found on the `PATH` a dev build shares with `forge-daemon`.

End to end, `crates/editor-cli/tests/pty.rs` drives a real PTY in both modes: a
typed character on screen, bracketed paste as one undo step, a resize mid-paste,
CRLF preserved through a save, a control byte drawn rather than executed, a
read-only refusal, a dirty close that asks first, and — integrated — a buffer
that came from the socket, a reveal that moves the caret, a save that is refused
without touching the file, and a display path that is never opened.
`crates/daemon/tests/scenario_editor.rs` covers the daemon's half: the control
socket in the child's environment, the text `fs-service` read on the child's
screen, state surviving a client reconnect, detach leaving the process alive,
and a crash marking the session and releasing the supervisor.

Bracketed paste still has an unbounded peak: crossterm 0.29 accumulates the
whole paste into a `String` before it delivers `Event::Paste`, so the document
budget refuses an oversize paste only after the bytes are already resident. It
is the same debt in both modes — the integrated route reuses the same input loop
— and capping it means owning the input loop or vendoring crossterm's parser,
which is a piece of work of its own rather than a line in this one.
