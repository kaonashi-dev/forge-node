# Editor (standalone spike)

`crates/editor-cli` builds `forge-editor`, a terminal editor written from
scratch in Rust. It is the first milestone of the editor replacement program
(the local plan's F1 spike), not the product: it exists to prove the route — a
real PTY, raw input, a rendered viewport and an atomic save — before the
editing engine is built on top. Syntax highlighting, linting, undo, selections,
the daemon session and the CodeMirror surfaces are not here yet.

## Run it

```sh
cargo run -p editor-cli --bin forge-editor -- path/to/file.rs
```

It is standalone by design: no daemon, no GUI, no network and no Node are
needed to open, edit and save a file. `forge-editor --help` prints the contract
(path, `--read-only`, `--` before a path that starts with a dash).

## Keys

| Key | Action |
| --- | --- |
| arrows, Home/End, PageUp/PageDown | move the cursor |
| printable characters, Enter, Tab | insert; Tab advances to the next 4-column stop |
| Backspace, Delete | grapheme-aware deletion |
| Ctrl-S | save (temp file in the same directory, then rename) |
| Ctrl-Q | quit; with unsaved changes a second Ctrl-Q discards them |
| Ctrl-C | quit |
| bracketed paste | inserts the pasted text |

`--read-only` refuses every edit and says so instead of silently dropping keys.

## Limits, and what replaces them

- **2 MiB and valid UTF-8 only.** A larger or non-UTF-8 file is refused with a
  message before anything is allocated (`fs-service` enforces the same limit on
  the Forge side). Reading with a hard cap and rejecting lossy decodes is a
  prerequisite the product must not relax silently.
- **LF or CRLF round-trip.** A mixed-EOL file is normalized to the terminator
  it mostly uses; per-line terminators belong to the document core.
- One buffer per process, no undo, no selection, no wrapping, no syntax.
- Movement and deletion are grapheme-based, tabs and wide or combining
  characters occupy the right cells, and control bytes in the file are drawn as
  Unicode control pictures — an `ESC` inside a file is shown, never executed.

## Candidates measured

`crossterm` 0.29 was chosen over a widget library for this milestone: the
viewport is arithmetic either way, and the cell rules want no intermediate
buffer to diff. `ratatui` stays a candidate if layout or widgets earn their
cost in a measurement. crossterm adds a second `signal-hook` (0.3 against
alacritty's 0.4) to the workspace; both are small and permissively licensed.

## What the next milestone still has to prove

The route under the daemon's PTY with its separate control channel, the
key-to-paint budget through PTY → VT → IPC → canvas, IME behaviour, and the
control-socket handshake are not covered here. The pieces that are already
exercised end to end live in `crates/editor-cli/tests/pty.rs`: a real PTY, a
typed character on screen, a resize that redraws, a save and a clean exit.
