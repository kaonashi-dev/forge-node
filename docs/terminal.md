# Terminals, PTYs and the authoritative grid

How a session's process becomes cells on screen. Crates involved:
`terminal-core` (engine and PTY), `terminal-input` (pure input mapping),
`daemon` (`terminal.rs`, `registry.rs`, parts of `core.rs`), `client`
(`CellGrid` and input re-exports), `domain::terminal` (wire types).

Plan references: §10.4, §10.5, §11. ADRs: 005 (PTY owned by the daemon),
011 (daemon is the single source of truth for the grid).

## Layers (§11.1)

```
 process  ──►  PTY master  ──►  pty_loop thread  ──►  AlacrittyEngine  ──►  DeltaBuilder
 (shell/agent)  (portable-pty)   (64 KiB reads)       (the ONLY VT engine)    (damaged rows)
                                                                                   │
                                              client registry (bounded queues) ◄───┘
                                                        │
                               ┌────────────────────────┴───────────────┐
                        subscribers                                 non-subscribers
                  TerminalDelta / TerminalResync                 TerminalActivity (≤1/s)
                               │
                        client::CellGrid  ──►  GUI terminal view (renders cells, never emulates)
```

`terminal-core` provides:

- `PtyBackend` / `PtyHandle` — spawn, `reader`, `writer`, `resize`,
  `child_pid`, `process_group`, `try_wait`; implemented over `portable-pty`.
  There is deliberately **no** signal method: signals go to the process group
  through `nix` in the daemon (see *Kill semantics* below).
  `test-support::FakePtyBackend` is the deterministic test double.
- `TerminalEngine` trait + `AlacrittyEngine` (`alacritty_terminal` 0.26):
  `feed(bytes)`, damage tracking, `snapshot()`, title/bell, device replies
  that must be written back to the PTY (e.g. cursor-position reports).
- `DeltaBuilder` — turns damage into `TerminalDelta { rows, scrolled_lines,
  cursor, modes }`.
- `terminal-input` compiles the **pure** key/mouse/paste → byte mapping that
  honors `TermModes` (application cursor keys, bracketed paste, SGR mouse…).
  `client` re-exports it so the GUI uses the same mapping without pulling the
  engine or PTY dependencies.

### Wire types (`crates/domain/src/terminal.rs`)

Both sides share the cell types so the client never links the engine (ADR-011):

- `TerminalSnapshot { seq, size, visible, scrollback_tail, scrollback_len,
  cursor, modes, title }` — sent by `AttachAck` and `TerminalResync`.
  `scrollback_tail` carries the last `DEFAULT_SCROLLBACK_TAIL = 200` lines.
- `TerminalDelta { seq, rows: Vec<(u16, Row)>, scrolled_lines, cursor, modes }`
  — only the damaged rows, addressed by visible-row index.
- `Row { cells, wrapped }`, `Cell { text, fg, bg, flags }` — `text` holds a whole
  grapheme; the trailing half of a wide character is a `WIDE_SPACER` cell.

Field-by-field notes are in
[domain.md](./domain.md#terminal-wire-types-terminalrs-114).

## Spawn (§11.2, §13.3)

The daemon builds one `SpawnSpec` per session:

- `program`/`args` — `$SHELL` (or `sessions.shell` from config) for shells,
  run with `-l` unless `sessions.login_shell = false`, so the session sources
  the same login profile the user's own terminal does; the detected/overridden
  executable plus `default_args` for agents.
- `cwd` — the workspace path.
- `env` — the **complete** environment: the resolved login-shell environment
  (see below) plus `TERM`, `COLORTERM=truecolor`, `FORGE_SESSION_ID=<uuid>`,
  `FORGE_WORKSPACE=<path>`. The PTY launch clears the inherited environment
  before applying it, so these are the only variables the child sees. Agents
  always get `TERM=xterm-256color` (`agents/src/descriptor.rs`); shells get
  `sessions.term` (default `xterm-ghostty`), resolved once per daemon by
  `daemon/src/terminfo.rs`.

### `TERM` for shell sessions (§13.3, `daemon/src/terminfo.rs`)

`sessions.term` only reaches the child if its terminfo entry is findable — an
unknown `TERM` leaves ncurses programs with no capabilities at all. Terminals
that ship a non-standard entry usually keep it in their app bundle and export
`TERMINFO` themselves, which a daemon started from the GUI never inherits, so
the lookup searches `sessions.terminfo_dir`, the session's own
`TERMINFO`/`TERMINFO_DIRS`, `~/.terminfo`, the system databases and the known
app bundles, in that order. An entry found outside what ncurses searches on its
own is exported as `TERMINFO` next to `TERM`; a name found nowhere falls back to
`xterm-256color` with a `DaemonNotice`.

This changes only what sessions *advertise*. The VT engine is still ours, and
`terminal-core`'s key encoding is written against `xterm-256color`, so a name
promising capabilities beyond that is the user's call, not a claim we verify.

Each spawn gets a fresh `TerminalId` and its own process group (`setsid`).

### Login-shell environment (§12, `daemon/src/environment.rs`)

A GUI launched from Finder or a launcher does not inherit an interactive
shell's `PATH`, so the daemon runs once at startup:

```
<shell> -l -c 'printf __FORGE_ENV_BEGIN__; env -0; printf __FORGE_ENV_END__'
```

with a 5 s timeout, parses the NUL-separated variables between the sentinels
(multi-line values stay unambiguous) and caches the result as
`ResolvedEnvironment { source: LoginShell }`. On failure it falls back to the
daemon's own environment with a widened `PATH` (`/usr/local/bin`,
`/opt/homebrew/bin`, `/usr/bin`, … plus `~/.local/bin`, `~/.cargo/bin`,
`~/.bun/bin`, `~/.npm-global/bin`), marks `source: ProcessFallback` and emits a
`DaemonNotice`. Agent detection and every spawn use this environment.

## The PTY loop (`daemon/src/terminal.rs`)

One dedicated OS thread per live PTY:

1. Read up to 64 KiB from the master.
2. Under the core lock, feed the engine and collect any device replies; then
   **release the lock** and write those replies back to the PTY. A blocked PTY
   write must never happen while the core lock is held.
3. Emit a coalesced delta to subscribers at most every `FRAME = 8 ms`
   (≤125 deltas/s). During the inter-frame sleep the kernel PTY buffer fills,
   so an output flood collapses into one delta per frame.
4. On EOF the child has exited: the session becomes
   `Exited { code, signal }` and the runtime is dropped. This is the **only**
   exit path — `KillSession` just makes EOF happen.

Title changes (OSC 0/2) become `SessionUpdated` (the `terminal` title); BEL
becomes `TerminalBell`. The rail's `needs-you` marker is driven by that bell
alone (`client::Store::pending_bell`, cleared on attach). Agents that do not
emit BEL natively reach the same path through Forge-injected adapters; see
[agents.md](./agents.md#attention-needs-you).

## Sequence numbers and resync (§10.5)

Every emitted delta/snapshot carries `seq`; `emit_seq` advances **once per
emitted delta**, not once per engine feed — otherwise clients would see false
gaps. The client rule (`CellGrid::apply_delta` → `DeltaOutcome`):

| Incoming `seq` | Action |
|----------------|--------|
| `≤ last` | discard (duplicate / stale) |
| `== last + 1` | apply rows, cursor, modes, scroll |
| `> last + 1` | gap → re-attach (`AttachTerminal`) and replace the grid |

On attach the daemon answers with a full `TerminalSnapshot` (visible grid +
last 200 scrollback lines + `scrollback_len`); older history is fetched on
demand with `FetchScrollback { from_line, count }` and merged into the
`CellGrid`'s cached extent.

## Backpressure (`daemon/src/registry.rs`)

Each client has a bounded outbound queue (`CLIENT_QUEUE_CAPACITY = 256`). When
a terminal delta cannot be enqueued:

- the client is marked *behind* for that terminal and further deltas for it
  are dropped (not queued);
- once the queue drains, the client receives a single `TerminalResync` with a
  fresh snapshot instead of a replayed backlog.

The PTY loop never blocks on a slow client and daemon memory stays bounded
(scenario H in the plan). Domain events are never dropped — they are
low-volume.

## Resize

`AttachTerminal { size }` only sets the PTY size when the attaching client is
the **first subscriber**; attaching to a terminal another client already
watches leaves the size alone, so opening a second window never reflows
someone else's `vim`. `ResizeTerminal` always applies — with several clients
attached, **last writer wins** (MVP). Engine and PTY are resized together under
the lock and the next delta reflects the new grid.

## Kill semantics (§11.3)

```
KillSession / SendSignal  ──►  signal the PROCESS GROUP, never a single pid
  shells:  SIGHUP   ┐
  agents:  SIGTERM  ┘──► after sessions.kill_grace_ms (default 3000) ──► SIGKILL
```

Signaling the group means a shell's grandchildren (`npm run dev` started from
`claude`, for instance) never outlive the session. The session transitions to
`Exited` when the PTY reports EOF, so the state is always derived from reality.

## Scrollback

Scrollback lives only in daemon memory, bounded by
`terminal.scrollback_lines` (default 10 000, hard max 100 000). It is never
persisted (ADR-009); after a daemon restart a session is `Orphaned` with no
history, and `RestartSession` starts a fresh terminal.

## Invariants to preserve

- Exactly one VT engine exists, in the daemon. Never add `terminal-core` or a
  second emulator to `client`/`ui`.
- `emit_seq` increments per emitted delta only.
- The core lock is taken per feed and released before any channel send that
  could block; lock order is `inner → registry`.
- `SpawnSpec.env` is complete; keep the daemon-injected `TERM`, `COLORTERM`,
  `FORGE_SESSION_ID`, `FORGE_WORKSPACE` when changing spawn construction.

## Tests

- `crates/terminal-core/tests/golden.rs` — insta snapshots of grid state after
  feeding VT sequences (`cargo insta review` to accept).
- `crates/terminal-core/tests/engine.rs`, `pty_smoke.rs` — engine behavior
  and a real PTY round trip.
- `crates/daemon/tests/integration.rs::end_to_end_shell_session` — real
  daemon, real client, real shell: attach → write → observe → kill → close.
