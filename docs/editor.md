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
| printable characters, Enter, Tab | insert; Tab advances to the next stop of the buffer's own indent unit |
| Backspace, Delete | grapheme-aware deletion; Alt-Backspace deletes a word |
| Ctrl-S | save (exclusive temp file in the same directory, then rename) |
| Ctrl-Z / Ctrl-Y | undo / redo |
| Ctrl-C / Ctrl-X / Ctrl-V | copy / cut / paste through an internal register |
| Ctrl-A / Ctrl-L / Ctrl-K | select all / select line / delete line |
| Ctrl-D | select the next occurrence, keeping the carets already placed |
| Alt-Up / Alt-Down | add a caret on the line above / below |
| Alt-click / Alt-drag | add a caret / a column of them |
| Ctrl-F, Ctrl-N, Ctrl-B | find, next match, previous match |
| Ctrl-T / Ctrl-W / Ctrl-E | toggle match case / whole word / regular expression |
| Ctrl-P | toggle closing brackets as you type |
| Ctrl-Space | complete from the buffer's own words; Up/Down choose, Enter accepts |
| Alt-I / Alt-W | show whitespace / wrap long lines |
| Alt-D | go to the definition of the word at the caret |
| Alt-Enter | what the change on this line replaced |
| Alt-C | run the configured checker and mark what it found |
| Ctrl-R | replace (Tab switches field, **Enter** replaces this match, **Ctrl-R** replaces the rest) |
| Ctrl-G | go to line |
| Alt-N / Alt-P | next / previous changed block (Ctrl-N and Ctrl-B are the find bar's) |
| Alt-F / Alt-Shift-F | fold the block at the caret / unfold everything |
| Ctrl-Q | close; unsaved changes ask save / discard / cancel |
| F1 | key help |
| bracketed paste | inserts the pasted text as one undo step |

`Ctrl-C` copies rather than quitting, and `Esc` closes a prompt without
touching the document — it also takes the find marks with it while keeping the
pattern, so `Ctrl-N` still steps through what was being looked for.

**Find works as you type.** Each character re-searches from where the bar was
opened, so narrowing a query lands on the same match instead of walking forward
one hit per keystroke; Enter steps to the next. Every hit in view is underlined,
not just the one the caret is on, and the status says `pattern: 3/41` — counted
at the gesture and never on a caret move, which would be a document scan on the
movement rung. The counter lives in the editor's own prompt row rather than the
GUI header: it is on screen exactly when it is relevant, and a second copy in
the chrome would be stale the moment Escape closed the bar. With the bar closed,
a short single-line selection underlines its *other* occurrences
(`highlightSelectionMatches`), and that lookup stays literal even when find is
in regex mode: a selected `a.c` means `a.c`.

**On a Mac the platform chords reach the editor.** `editorChords.ts` maps ⌘S/C/X,
⌘A, ⌘Z, ⇧⌘Z, ⌘F, ⌘G, ⇧⌘G, ⌥⌘F and ⌥⌘L onto the editor's own keys. ⌘G is
find-next and not "go to line" — a Mac user pressing it after a search wants the
next match — so go-to-line is ⌥⌘L. ⌘V is deliberately unclaimed: the WebView's
`paste` event is what carries the clipboard into the pane. `--read-only` refuses every edit at the transaction
entry and says so instead of silently dropping keys.

**The mouse moves the caret.** A left click places the caret; a Shift-click
extends the selection to the click; a left drag selects; an Alt-click adds a
caret and an Alt-drag makes a column of them; a click on the gutter's fold
marker folds that block; a click on the overview ruler jumps to the line it
stands for; the wheel scrolls the viewport without moving the caret. The editor turns on mouse capture on entry
(`EnableMouseCapture`), so a real terminal reports the events and the Code pane
forwards them as the same SGR reports the main terminal sends — the editor reads
the mouse itself either way. Out of scope for now: go-to-definition on click,
middle-click paste, and a native right-click menu (the Code pane keeps its own
HTML context menu).

## What the core guarantees

- **Byte-exact round trip.** The buffer holds the file's bytes, terminators
  included, so a mixed LF/CRLF file, a BOM and a missing final newline all
  survive. A new line takes the terminator the current line already uses.
- **A fold is indentation, not a grammar.** A region is a line and everything
  indented under it — the one rule that is right for Rust, TypeScript, Python,
  YAML and Markdown at once, and a fold that disagreed with the brace a person
  can see would be worse than none. A blank line belongs to the block around it
  rather than ending one. Folded lines take *no rows*, so the viewport walk
  steps over them and the row under a folded block is the line after it; a caret
  inside what just folded moves to the header, and a fold whose block an edit
  destroyed is dropped rather than left hiding rows.
- **The overview ruler is a column, not a strip of HTML.** The plan preferred
  the DOM; the editor is where the marks, the matches and the caret already are,
  and the GUI has none of them — a DOM ruler would mean broadcasting every
  changed line of the file on a channel that exists for a caret position. So it
  is the rightmost cell of the grid, one bucket per row, strongest signal
  winning: caret over match over change. It costs exactly one column, appears
  only at 40 cells or wider, and a click on it jumps to the line that bucket
  stands for. The search half is a capped document scan that runs only while the
  find bar is open.
- **Carets are a set, and a set with rules.** A selection is one or more
  cursors, sorted, never overlapping, one of them primary — the one the status
  line reports and the viewport follows. Two that touch are merged the moment
  one is added, because two carets in the same place have no defined result for
  an edit and the transaction carrying them would be rejected as overlapping.
  There is a hard cap of 64: `Ctrl-D` through ten thousand matches is a gesture
  with a reasonable intent and an unreasonable result, and the ones nearest the
  primary are the ones being worked on. A plain arrow key collapses back to one,
  the way CodeMirror does, because keeping them would need a rule per direction
  for what the others do; Escape collapses before it does anything else.
- **A multi-caret edit is one transaction.** Every caret gets the same text in
  the same transaction, and each one lands past *its own* insertion rather than
  past the ones above it. So one gesture is one history entry, and Ctrl-Z puts
  back all of it — with the carets it was made with.
- **One mutation entry.** Typing, paste, replace-all and a reload are all
  transactions with a selection on both sides. Undo restores text *and*
  selection; a run of typing coalesces into one entry, a paste or a
  replace-all never does.
- **Indentation is the file's, not a setting's.** The unit is read off the
  buffer when it opens — tabs win a tie, and a spaced file reports the step it
  actually uses, which is not always four. Tab inserts that unit, Enter carries
  the line's leading whitespace, a line that opens a block adds one level, and a
  closer waiting on the other side of the caret is pushed onto a line of its own
  at the opening depth. A colon opens a block in Python and nowhere else.
- **A closing bracket comes with its opener, and undoes with it.** One
  transaction, so the pair was one keystroke and reads as one. The closer is
  only added where it would not be in the way — before whitespace, a closer or
  the end of a line, never before a word — and typing a closer over one that is
  already there steps past it rather than doubling it. A quote after a word is
  an apostrophe. `Ctrl-P` turns the whole thing off.
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
  one row plus the status line — wrapped or not. With wrap on, a frame compares
  each visible line's row count against what is already on screen: a line that
  still takes the same rows repaints only its own, and the first line whose
  height changed reflows the row↔line map, so everything under it repaints and
  nothing over it does.
- **A clear is a width change, and nothing else.** A scroll, and a viewport that
  only grew taller, repaint their rows without blanking first: a client reading
  the delta between the clear and the rows would paint the gap as a flash.
  `neither_a_scroll_nor_a_keystroke_blanks_the_screen` reads the raw PTY stream
  for `CSI 2J` and fails on it.
- **One paint per frame under a burst.** The loop paints when the input has
  caught up or 8 ms have passed, whichever comes first — the same
  `daemon::terminal::FRAME` floor an attached terminal has. A key repeat that
  lands ten events in a millisecond is one frame on the wire.
- **Colour is re-scanned from the edit, not from the top.** A mutation restarts
  the scan on the nearest line above it that the previous scan passed at top
  level, and stops on the first line boundary below it that both scans agree is
  top level; a span is a column pair inside its own line, so the tail is reused
  as it stands. Opening a block comment still recolours what is under it,
  because that is the case where the two scans do not agree again until the
  comment closes.
- **Decorations are viewport-bounded.** Search marks and the bracket pair are
  looked for inside the visible slice (capped at 64 KiB, because one logical
  line can be taller than the screen), recomputed when one of their inputs moves
  and never per frame. The bracket scan is capped at 64 KiB either side, so an
  unmatched brace at the top of a file costs a bounded walk on a caret move
  rather than the whole buffer. A brace inside a string or a comment is text:
  the grammar already said which bytes those are.
- **The three decorations do not collide.** The selection owns reverse video, a
  search hit is underlined, a bracket is underlined and bold, and the caret's
  line number is bold in the default foreground rather than the gutter grey.
  None of them picks a colour, so none of them fights the terminal's theme.

## Limits, and what replaces them

- Regular-expression search is opt-in (`Ctrl-E`) and runs on `regex`, whose
  guarantee is linear time in the haystack: there is no catastrophic
  backtracking to time out against, so the budgets sit on the *pattern*, which
  is the input a person can grow without bound — 1 KiB of source and a 1 MiB
  compiled program, both refused before the allocation. A pattern that does not
  compile reports why and matches nothing, because one is invalid on the way to
  being valid while it is still being typed. `.` stops at a line break, an empty
  match is dropped (it would be returned once per byte and never advance a
  find-next), and the compiled program is cached on the query, so
  find-as-you-type pays one compile per edit of the pattern rather than one per
  scan.
- One buffer per process. No splits, no Vim profile.
- Whitespace that is easy to mistake — a tab, a no-break space, a zero-width
  one — draws as a placeholder under `Alt-I`, off by default because it is a
  debugging view rather than a reading one. The glyph takes the character's
  first cell and the rest of a tab's width stays spaces, so nothing after it
  shifts.
- Wrapping is a setting (`Alt-W`), not a property of the mode: integrated
  defaults to on because the pane is a fixed width, but a code file often reads
  better unwrapped even in a narrow column.
- Long lines wrap under the daemon and scroll sideways standalone. Integrated,
  the pane is a fixed width the person cannot widen, so a line too wide for the
  text column continues onto the next row (with a blank gutter) rather than
  running off the edge; the caret, a click and the wheel all follow the wrapped
  rows. Standalone, where the terminal can be widened and code reads better
  unwrapped, the view scrolls horizontally to follow the caret instead. The
  scroll anchor is a *row*, not a line: a logical line taller than the viewport
  is scrolled through within itself, and a wheel notch never overshoots a
  screenful of wrapped text. A caret exactly on a wrap boundary — a line whose
  width is an exact multiple of the text column, caret at its end — has no row
  of its own and is not painted; it is the one position the cell grid cannot
  name.
- **Right-to-left text is stored correctly and laid out wrongly.** The buffer
  holds the file's bytes and every edit is byte-exact, so an Arabic or Hebrew
  file opens, saves and round-trips without damage. What it does *not* do is
  reorder: the view is a cell grid walked left to right, so an RTL run is drawn
  in logical order, and the caret's display column is a count of cells rather
  than a visual position. A mixed-direction line is therefore readable as
  content and misleading as layout. This is a real gap and not a rendering
  detail — the Unicode bidi algorithm is a document-level pass with its own
  state, and half of one (reversing a run without resolving embedding levels)
  produces text that is wrong in a way a person cannot see. Closing it means
  adopting a bidi implementation and giving the view a visual-order row, which
  is a piece of work on the scale of the wrap anchor, not a patch. Until then:
  do not claim CodeMirror parity here, because CodeMirror has the browser doing
  it and this does not.
- The save is optimistic, not exclusive: it compares the revision it loaded
  against the disk before writing and refuses once when they differ, which
  catches an agent that already wrote. It is not a compare-and-swap against a
  program that does not cooperate.
- Movement and deletion are grapheme-based, tabs and wide or combining
  characters occupy the right cells, and control bytes in the file are drawn as
  Unicode control pictures — an `ESC` inside a file is shown, never executed.

## Language

Seven grammars, chosen by extension. Extension only: sniffing content would have
to be undone the moment a person types, and a shebang is a guess.

| Grammar | Extensions | What it knows |
| --- | --- | --- |
| Rust | `rs` | line and block comments, attributes, strings and char literals (a lifetime is not a string), numbers, keywords, `Type` by leading capital, `name(` as a call |
| C-like | `ts` `tsx` `js` `jsx` `mjs` `cjs` `go` `java` `kt` `c` `h` `cc` `cpp` `hpp` `cs` `swift` `scala` `php` `dart` | the same, without Rust's attributes and lifetimes |
| Python | `py` `pyi` | `#` comments, triple-quoted strings, numbers, keywords, calls |
| JSON | `json` | keys apart from values, numbers, `true`/`false`/`null` |
| Keyed | `toml` `yaml` `yml` `ini` `cfg` `conf` `env` | `[section]`, `key =` / `key:`, quoted values, `#`/`;` comments |
| Shell | `sh` `bash` `zsh` `fish` | `#` comments, quotes, `$VAR` and `${...}`, keywords |
| Markdown | `md` `markdown` `mdx` | headings, quotes, fenced and inline code, links, list bullets |

Everything else is plain, which is the honest answer rather than a wrong colour.
Against CodeMirror's language packs the gaps are HTML/XML, CSS/SCSS, SQL, Ruby,
Lua, Haskell and the rest of the long tail; each is a scanner arm here, not a
dependency.

Ten scopes reach the terminal as the ANSI 16, and that is not a limitation in
the pane: the canvas resolves those slots through `--forge-ansi-*`, which *is*
the active Forge theme, so a keyword already lands on the theme's own magenta
without a truecolour side-channel. Standalone it lands on the person's terminal
theme, which is the right answer there too.

| Scope | Slot | Scope | Slot |
| --- | --- | --- | --- |
| comment | dark grey | type | cyan |
| keyword | magenta | function | blue |
| control keyword | red | property | dark cyan |
| string | green | constant | dark yellow |
| number | yellow | plain | default |

**Diagnostics are a command somebody configured, run when somebody asks.**
`[editor] diagnostics_command` names one — `["cargo", "check",
"--message-format=short"]`, `["oxlint"]`, anything that prints
`path:line:col: level: message` — and `Alt-C` runs it once in the checkout. Off
by default, and that is the honest default: the daemon would be spawning a build
in somebody's working tree, and what that costs is a question only they can
answer. Not a language server, and deliberately not: a checker that ran by
itself is a subprocess per keystroke.

Findings become gutter marks (`✗`, `!`, `i`) that outrank the git mark on the
one cell, and the caret's own line says what its mark is about. Replacing the
whole set is the clear, so a second run that finds nothing empties the gutter —
the marks mean the *last* answer, not every answer ever given. "No checker
configured" is reported as itself and never as a clean file. The output parse is
a heuristic over ordinary compiler output, so a line that does not parse is not
a diagnostic: a wrong mark is worse than a missing one. Capped at 500 findings
and 4 MiB of output, clamped before the parse. The subprocess runs on the
editor's control thread with `process_group(0)` and the whole group killed on a
timeout — cargo forks rustc, and killing the direct child alone leaves a
grandchild holding the pipes.

**The gutter's marks have details.** `Alt-Enter` on a changed line asks the
daemon what that block replaced and opens a panel with the removed lines and
then the added ones, the way a hunk reads — a modification is what it was before
what it became, not two columns to compare. On demand and not carried with every
mark: the gutter needs only *which* lines changed, and this is the question a
person asks about one of them. Capped at 200 lines a side, truncated and said
so, because a reformatted file is one hunk the length of the file and a panel
that has to be scrolled is not a detail. A block that is gone by the time the
question is asked says so, since the working tree may have moved since the
gutter was drawn.

**Go to definition is a request, not a lookup.** The editor never opens the
checkout, so `Alt-D` sends the word at the caret to the daemon, which greps the
tree through `fs-service`. The symbol is validated as an identifier on both
sides before it reaches `git grep` — a name that arrives from a caret must never
be able to become a regex (`SearchKind::Definition`). What comes back is
*candidates*, capped at 64: the ranking is a heuristic, so the editor offers the
list rather than jumping somewhere it cannot justify, and a single candidate is
still a list because a wrong answer taken silently is the worst of the three
outcomes. Following one is another request — one buffer per process means the
editor cannot open a second — and the daemon opens it as an ordinary editor
session, the same path a click in the file tree takes. The search runs on the
editor's control thread, off the core lock and off the PTY's paint thread.

**Completion is the buffer's own vocabulary.** `Ctrl-Space` offers the words
already in the file that continue the identifier at the caret — no language
server, no index, nothing that leaves the process. Two characters minimum, a
512 KiB window around the caret, twelve candidates, shortest first. The word
being typed is never a candidate for itself, and a prefix with no continuation
says so rather than opening an empty list. Accepting one is a single
transaction, so undo takes the completion and leaves what was typed. A key the
list does not own closes it and still reaches the document, because typing on is
how a person narrows what they meant.

**tree-sitter: not now, and here is the test it has to pass.** It would buy the
long tail of grammars and a real parse tree, and the scanner here is a
hand-rolled approximation that will keep needing arms. Against that: the editor
is a standalone binary that must build with no network and no C toolchain beyond
the one SQLite already needs, each grammar is its own crate or a wasm blob to
ship and version, and the incremental colouring here already re-lexes from the
edit rather than the top — which is most of what an incremental parser was going
to buy. The decision to revisit it is not "is tree-sitter good" but a
measurement: a grammar set that covers what is missing, built for Rust 1.89,
under a licence `deny.toml` accepts, costing less per keystroke on a 2 MiB
buffer than the current scanner's resume-and-resync. Until someone runs that,
adding it would be trading a known cost for an unknown one.

## Accessibility

The canvas is the only visual surface, and an accessibility tree cannot read
one. The pane therefore carries a **hidden mirror** of the editor's own state
next to it — never a second input path, and never anything read back off the
cells.

- The mirror is `role="application"` with `aria-roledescription="code editor"`,
  `aria-label` = the path and `aria-readonly` from the buffer. Not
  `role="textbox"`: the keys go to the `<textarea class="terminal-keys">` the
  way they always have, and a reader that took the mirror for an input would
  offer its own editing keys against a node that has none.
- It says `Line 12 of 300, column 4.` plus the flags a person cannot see — read
  only, unsaved changes, how many carets, how many bytes are selected — and then
  the caret's line as text.
- That line is the one piece of document text on the wire. `EditorState` carries
  `caret_line`, clamped by the editor to 2 KiB and cut on a character boundary,
  because the GUI has no copy of the buffer and the alternative is reading the
  cells back out of the grid the editor just drew. An empty line is announced as
  "blank line": silence is indistinguishable from a failure to announce.
- A polite live region repeats the editor's transient message — `alpha: 3/41`,
  `no match`, a save refusal — which is exactly the status row a person looking
  away from the canvas cannot see. A conflict is announced over it, because the
  daemon is what saw the revision mismatch and the editor was only told a reason.
- `editorAria.ts` holds the rules and is tested on its own; `editorPane.test.ts`
  asserts the pane is wired to them and that it never reads `viewport.rows`.

What this does **not** claim: VoiceOver does not read every ANSI cell, and the
mirror is not a document a reader can navigate by character or by word. What it
does claim is the checklist worth running by hand — open a file and hear its
path and position, move and hear the line, search and hear `n of m`, save and
hear the result, and be told when the file changed under the buffer.

## Candidates measured

`crossterm` 0.29 was chosen over a widget library: the viewport is arithmetic
either way, and the cell rules want no intermediate buffer to diff. `ratatui`
stays a candidate if layout or widgets earn their cost in a measurement.
crossterm adds a second `signal-hook` (0.3 against alacritty's 0.4) to the
workspace; both are small and permissively licensed.

`regex` was taken over `regex-automata` for find. It was already in the graph
through other crates, so the direct edge adds no node, no license and no
advisory surface, and it brings the replacement side (captures, `$1`) that the
lower-level crate would have meant hand-rolling. Default features are on because
`\w` and `\b` are what a find bar's patterns are made of and both need the
Unicode tables.

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
`a_copy_in_the_editor_reaches_the_host_as_a_clipboard_store` drives the whole
chain: the editor writes the escape into its PTY, the daemon's engine is the
only VT that parses it, and the store arrives keyed on the *editor's* terminal
— which is what lets the host forward this one and refuse one from a background
agent.

What is left. A rendered Markdown document, an SVG and a raster image open on
the DOM side — a terminal cannot draw them, so that split is the design rather
than a gap. `previewKindFor` decides by extension, with the same rule the
daemon's `ReadImage` uses, because routing a file to a preview the daemon will
refuse bytes for is worse than opening it as text; text is the default, which is
every file the grid can show. An SVG is drawn through an `<img>` and never as
`innerHTML`: it is a file from a checkout an agent may have written, and an
`<img>` refuses to run the script it may carry. What *is* a gap: find-references, which is the
same channel as go-to-definition asking a different question.

The pane draws a scrollbar thumb: `EditorState`
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

Bracketed paste has an unbounded peak **standalone**, and a bounded one
integrated. crossterm 0.29 accumulates a whole paste into a `String` before it
delivers `Event::Paste`, so the document budget refuses an oversize paste only
once the bytes are already resident. The integrated route is clamped at its
source — `encode_editor_paste` cuts at the document budget, on a character
boundary, before the bytes reach the PTY — so a paste that arrives through the
pane cannot make the peak larger than the budget. A *terminal* paste is
deliberately not clamped: that text is going to a shell, and truncating what
somebody pasted into one is a worse failure than the allocation.

Standalone, the debt stands, and closing it means owning the input loop rather
than writing a line: crossterm exposes no hook between the read and the
`String`, so a cap needs a VT input parser of our own (CSI and SS3 keys, SGR
mouse, UTF-8, the paste markers). Turning bracketed paste *off* is not the
cheaper version of that — the markers are exactly what stops pasted code being
re-indented as it lands, and stops it arriving as one undo step per character.
