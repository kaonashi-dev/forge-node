# Orca vs Forge: terminal sessions and buffers

Analysis of the MIT-licensed Orca desktop app (extracted Electron bundle) for
mechanisms worth replicating in Forge. Scope: terminal session lifecycle, PTY
lifecycle, buffers/scrollback, output facts, input, exit handling.

No Orca code is reproduced here and none was copied into the tree. Orca paths are
relative to the extracted bundle root (`out/`). Forge paths are repo-relative
with line numbers as of this reading.

Read alongside [`../terminal.md`](../terminal.md) (the pipeline),
[`../performance.md`](../performance.md) (the cost model) and
[`../../AGENTS.md`](../../AGENTS.md) *Boundaries And Invariants* — every
recommendation is written to fit those, and §4 lists what does not.

---

## 1. How Orca does it

### 1.0 Topology: one daemon, four PTY implementations, three emulators

Orca converged on the same top-level answer Forge did — **a separate long-lived
daemon process owns the PTYs and outlives the app** (`out/main/daemon-entry.js`,
node-pty spawn at `daemon-entry.js:1784`) — and then did not stop there:

| Where | What it owns |
|---|---|
| `out/main/daemon-entry.js` | node-pty children **plus a headless `@xterm/headless` emulator per session** (the authority) |
| `out/main/index.js` (`LocalPtyProvider`, ~`index.js:35316`) | a second node-pty spawn path, used before the daemon is up or when it fails |
| `out/main/index.js` (`RuntimeService.headlessTerminals`, `index.js:217808`) | a **second headless emulator per pty**, in the Electron main process |
| `out/relay/<platform>/relay.js` | a fourth node-pty host for SSH/remote, with its own batcher |
| `out/renderer/assets/*` | the **third** emulator: the visible xterm.js |

Snapshot resolution walks that list in precedence order
(`serializeTerminalBufferFromAvailableState`, `index.js:217880`: provider →
headless-in-main → renderer), and the main-process mirror can even be **hydrated
from the renderer** when it missed the start of a stream
(`maybeHydrateHeadlessFromRenderer`, `index.js:217731`).

Everything expensive in Orca's design descends from that multiplicity:

- **Facts have to be re-derived from bytes**, because the authoritative emulator
  is in a process that may not be the one that needs the fact. There is a
  dedicated per-chunk scanner per fact, each with its own cross-chunk state
  machine and byte cap:

  | Fact | Orca module | Cross-chunk carry |
  |---|---|---|
  | BEL (vs. OSC string terminator) | `shared/terminal-bell-detector.js` | `inOsc` / `pendingEscape` / `pendingOscEscape` |
  | kitty keyboard flags | `shared/terminal-kitty-keyboard-mode-tracker.js` | 4 096-byte scan tail |
  | OSC 133 command start/finish | `shared/terminal-osc133-command-finished.js` | 4 096-byte carry |
  | DEC mode 2031 (color-scheme) | `shared/terminal-color-scheme-protocol.js` | 128-byte tail + provisional decision |
  | GitHub PR URLs | `shared/terminal-github-pr-link-detector.js` | 512-byte carry, 2 048-byte URL cap |
  | trailing incomplete escape | `shared/terminal-partial-escape-tail.js` | 4 096-byte tail |
  | OSC 0/2 titles, agent status | `shared/terminal-output-side-effects.js` | per-chunk, in byte order |

  The carry pattern is applied consistently: every scanner keeps a bounded tail
  and degrades explicitly on overflow rather than growing.
  `terminal-partial-escape-tail.js` even states the fold property it depends on —
  `extract(a + b) === extract(extract(a) + b)` — which is what lets an
  ingest-time tracker advance without retaining the stream.

- **State has to be kept in sync between implementations.**
  `terminal-output-side-effects.js` says its shared title tracker exists because
  "title semantics must not drift" between the renderer transport and main's
  per-PTY tracker. `terminal-kitty-keyboard-mode-tracker.js` mirrors the kitty
  stack because Orca "defensively wipes the renderer terminal's kitty flags at
  moments when the TUI may have died".

Forge's daemon owns the only VT engine (ADR-011) and parses those bytes exactly
once. That difference is the frame for the whole comparison.

### 1.1 Spawn

`node_pty.spawn(file, args, { name: env.TERM ?? "xterm-256color", cols, rows,
cwd, env, ...(win32 ? { useConptyDll: true } : {}) })` — `daemon-entry.js:1784`.
No `encoding` and **no `handleFlowControl`**: flow control to the child is done
by `proc.pause()` / `proc.resume()`, not XON/XOFF (§1.3).

Around it:

- A **spawn health probe** before trusting the backend: `/bin/sh -c "exit 0"` at
  `cols: 2, rows: 1` with `PTY_SPAWN_HEALTH_TIMEOUT_MS = 4000` and a retry loop
  (`daemon-entry.js:1588,1714,1754`). Windows additionally warms ConPTY once.
- Shell fallback chains on both platforms (`UNIX_SHELL_FALLBACKS`,
  `chunks/daemon-ready-identity-*.js:3544`).
- A macOS TCC attribution wrapper around the shell — which is what makes
  `hostReportsChildExitStatus(file)` false for those sessions, and is why the
  exit-cause model has a `host_status_unavailable` case (§1.7).

### 1.2 Output batching: three hops, three batchers

**Hop 1, pty → daemon socket** (`DaemonStreamDataBatcher`, `daemon-entry.js:1269`):
2 ms batch interval, 64 KiB per-write slices (surrogate-safe splits), a 128 KiB
`socket.writableLength` gate above which sessions are *held* rather than written,
a 32 MiB write-through ceiling, and a 4 KiB bypass so tiny sessions are never
held. Held queues are refilled by a zero-length write with a drain callback.
Enqueue coalesces into the tail entry while it is under 64 KiB. An interactive
fast path flushes immediately within 100 ms / 1 024 chars of input.

**Hop 2, daemon → main pending queue** (`SessionOutputPlane`, `daemon-entry.js:221`):
`PENDING_OUTPUT_MAX_BYTES = 2 MiB`; **on overflow the entire pending list is
discarded and `pendingOutputOverflowed` is set**, forcing the client to take a
full snapshot instead of receiving a backlog. Pre-listener buffering is capped at
512 KiB.

**Hop 3, main → renderer over IPC** (`index.js:43276-43289`): 2 ms batch, 16 KiB
flush chunks, at most 2 writes per tick, per-pty in-flight high water 512 KiB,
global 8 MiB, with a 256 KiB interactive reserve and a 512 KiB reserve for the
active pty. The backlog cap is **derived from the user's scrollback setting**
(§1.4): `max(2 MiB, rows × 120)`.

### 1.3 Backpressure: credit-based, with a lossy escape hatch

Three distinct mechanisms, all present at once:

1. **Cumulative-ack credit to the renderer.** The renderer replies on
   `pty:ackData` with cumulative `processedChars`; `applyCumulativeAck`
   (`index.js:43546`) decrements in-flight. If the renderer goes silent, main
   sends `pty:requestDeliveryResync`, and after 5 s **writes off the lost bytes**
   and emits a `pty:modelRestoreNeeded` marker so the renderer re-pulls a
   snapshot (`index.js:43583-43586`, `:43704`).
2. **True producer pause to the child.** `SessionProducerPause`
   (`daemon-entry.js:372`) calls `proc.pause()` / `proc.resume()`, with a
   `PRODUCER_PAUSE_FAILSAFE_MS = 5000` auto-resume so a stuck consumer cannot
   wedge the shell forever. Driven from main by `PtyProducerFlowController`
   (`index.js:37665`: high water 256 KiB, low water 32 KiB, 5 s re-assert).
3. **Lossy drop for backgrounded sessions.** When a pane is hidden
   (`setSessionBackground`, `daemon-entry.js:3134`), the daemon discards the
   *oldest* queued bytes down to a keep-tail (512 KiB per session, 2 MiB global
   budget) and emits a `dataGap` control event carrying `droppedChars` — while
   **salvaging up to 4 KiB of terminal *query* bytes** out of the dropped span so
   DA/DSR replies are not lost. The daemon's own emulator still consumes
   everything, so a later snapshot restores fidelity.

The escape hatch at every level is the same: when the queue cannot be honoured,
**stop replaying and force a fresh snapshot**. That is the one design decision
Orca and Forge independently agree on.

### 1.4 Scrollback: many tiers, and the authority holds the least

| Tier | Limit | Where |
|---|---|---|
| Daemon session emulator (authoritative) | **1 000 rows** (`ORCA_DAEMON_SESSION_SCROLLBACK_ROWS`) | `chunks/daemon-ready-identity-*.js:15132` |
| Main-process mirror emulator | 5 000 rows (default, not configured) | `chunks/…:14493`, `index.js:217808` |
| User setting | 5 000 default, 1 000 min, 50 000 max | `shared/terminal-scrollback-policy.js` |
| Persisted session buffer / replay / store | 512 KiB / 512 KiB / **5 MiB** | `shared/terminal-scrollback-limits.js` |
| On-disk checkpoint / log / meta | **200 MB** / 5 MiB / 64 KiB | `chunks/…:7666` |

Two decisions stand out.

**The backlog cap scales with the user's setting.**
`terminalOutputBacklogCapChars(rows) = max(2 MiB, rows × 120)`, with the reason in
the source: a flat 2 MiB floor would discard lines that a user who raised
scrollback to 50k rows had explicitly asked to keep. 120 chars/row is "80-col text
plus escape-sequence overhead", and the comment is careful to call the result a
memory bound, not a retention guarantee. Every byte cap is UTF-8 aware
(`clampUtf8TextTail`, `measureUtf8ByteLength` with `stopAfterBytes`), so a budget
never splits a multi-byte character and the measurement short-circuits at the
limit.

**The authoritative emulator holds 1 000 rows; history above that lives on disk.**
`TerminalHistorySessionWriter` (`index.js:47831`) writes `meta.json` +
`checkpoint.json` + a framed incremental `output.log` (9-byte generation header,
`FRAME_OUTPUT` / `FRAME_RESIZE` / `FRAME_CLEAR` records). Checkpoints run on a
dirty-session timer (5 s interval, 15 s deadline, 45 s full-checkpoint cooldown;
`index.js:49251`). When a checkpoint exceeds its cap,
`serializeTerminalCheckpointWithinLimit` (`index.js:47826`) **replays into a
scratch emulator and binary-searches the number of scrollback rows that fit**.
Cold restore rebuilds an emulator and replays checkpoint → log, yielding every
64 KiB / 1 024 operations so the event loop stays alive (`index.js:47599`).

### 1.5 The buffer is a string, so restore is a replay

Orca's unit of persistence and transfer is **serialized ANSI text** (xterm's
`SerializeAddon`), not cells. Restore is therefore a replay into a *different*
emulator instance, and everything the text cannot express is reconstructed by
hand:

- `shared/terminal-mode-reset-profiles.js` is a whole module of mode-reset
  literals, one profile per replay context — post-replay reattach, reattach
  keeping mouse modes (an alt-screen reattach re-arms the live TUI's modes, so
  wiping them one write later hands drags back to xterm's own selection),
  live-agent reattach (a live agent owns focus reporting; resetting `?1004h`
  suppresses the focus-in it needs to re-anchor its cursor for IME), cold-restore
  seed, and byte-gap recovery. The byte-gap reset uses **CAN (0x18), not a bare
  ESC** — xterm dispatches OSC/DCS/APC with `success = code !== CAN && code !== SUB`,
  so ESC grounds the parser but *commits* what the gap truncated: a half-read
  OSC 0 retitles the pane and OSC 52 writes the clipboard.
- `shared/terminal-serialize-absolute-cursor.js` appends an absolute CUP, because
  the serializer restores the cursor with *relative* moves computed from where it
  assumes replay leaves it — and when the final row is filled exactly to the right
  margin, replay leaves the terminal wrap-pending and the maths lands one column
  short. It also re-establishes the VT100 DECSC register by hand, since the
  serialized screen cannot carry it.
- `buildSnapshotReplayPrologue` grounds a documented subset of state and
  deliberately does *not* ground G1–G3 charsets: `enacs=\E(B\E)0` designates G1
  once at init and then uses bare SO/SI, so grounding G1 would render a live app's
  box drawing as letters.
- Seeding history into a *new* PTY is a chunked, sha256-verified upload
  (`startHistorySeedTransfer` …, `daemon-entry.js:2274`; 512 KiB chunks, 8
  concurrent transfers, 30 s TTL) consumed with `emulator.writeSync(chunk)`.

The decision worth naming: **serializing the buffer as bytes buys portability and
costs fidelity**, and the cost is a long tail of state invisible in the text
(wrap-pending, the saved-cursor register, kitty flags, partial escapes, scroll
region, origin mode) — each with its own module and its own issue number.

### 1.6 Persisted scrollback is kept only where nothing else is authoritative

`shared/workspace-session-terminal-buffers.js`.
`pruneLocalTerminalScrollbackBuffers` strips `buffersByLeafId` and
`scrollbackRefsByLeafId` from persisted session state for **local** worktrees,
with the reason inline: local daemon history/checkpoints are authoritative, so
keeping renderer-captured buffers "makes every persisted state write scale with
old terminal output". Remote/SSH/paired-runtime tabs keep them, because teardown
may leave no local history.

- The decision comes from the **execution host encoded in the pty id**, not a path
  (`shared/terminal-execution-host.js`: an SSH or remote-runtime id embeds its
  owner; an id that parses but whose owner does not round-trip is `'foreign'`;
  `null` means "the id says nothing", so fall back to the worktree).
- When the repo catalog is **not hydrated**, the worktree is treated as remote —
  the uncertain case fails toward keeping data rather than deleting the only copy.

### 1.7 Exit cause is evidence-graded, not a number

`shared/terminal-exit-cause.js` (implementation at
`chunks/daemon-ready-identity-*.js:6381`) is the sharpest module in the set:

- `operator_close` — we asked for this
- `signaled { signal }`
- `exited { exitCode }`
- `unknown { stop_unverified | host_status_unavailable | cause_unreported }`

The reasoning, from the source:

- A **negative** exit code from the stop paths means "we asked it to stop and never
  saw it die" — an *absence of evidence*, not an exit status → `stop_unverified`.
- A host that reports a **wrapper's** status rather than the child's
  (`hostReportsChildExitStatus: false`, the macOS TCC wrapper) yields
  `host_status_unavailable` rather than the wrapper's misleading number.
- For an exit delivered with **no cause at all** (an older daemon, or the SSH relay
  which forwards a bare code), **zero is refused as ambiguous** — node-pty pairs 0
  with every signalled death and a wrapper spawn returns 0 for any outcome — so 0
  becomes `cause_unreported`, while a nonzero status is still reported because
  nothing fabricates a nonzero status that way.
- `describeTerminalExitCause` gives one line an operator *or a coordinating agent*
  can read without decoding a number; `isDeliberateTerminalExit` is true only for
  `operator_close`.

At runtime the cause is refined once more: `onPtyExit` (`index.js:219002`)
overrides it to `operator_close` when a stop was requested *and confirmed*, and
keeps an SSH surface alive as `unverifiable` when the code is negative and host
status was never confirmed. Kill escalation is 5 s to a **POSIX process group**,
2 attempts, 250 ms retry, with dead sessions leaving tombstones (max 1 000) so a
late reattach gets an answer instead of "not found".

### 1.8 Three kinds of hibernation

1. **Keep the PTY, drop the client** — the default. The daemon outlives the app;
   `detach` releases the client and the producer pause. On re-attach the daemon
   detaches all previous clients and returns a serialized snapshot
   (`createOrAttachTerminalSession`, `daemon-entry.js:770`). The daemon
   self-shuts-down when idle (`shutdownIfIdle`, `daemon-entry.js:3195`).
2. **Keep the PTY, stop sending** — background sessions (§1.3.3).
3. **Kill the PTY, keep the buffer** — worktree "terminal sleep"
   (`sleepTerminalsForWorktree`, `index.js:227432`). Under a per-worktree mutex it
   stops each live PTY *with `keepHistory`* so the checkpoint survives, and emits
   `started → committed → partial | sleeping → woken` client events. **Wake is
   implicit**: `acquireWorktreeTerminalSpawn` clears the sleep state the moment
   anything spawns a terminal in that worktree.

On top of (3), `shared/workspace-session-sleeping-agents.js` persists, per pane:
the agent kind (restricted to a `RESUMABLE_TUI_AGENTS` allow-list), the **provider
session locator** (`key: 'session_id' | 'conversation_id'`, the id, and an optional
transcript path), the prompt, a coarse state (`working | blocked | waiting |
done`), timestamps, the terminal title, the last assistant message, an
`interrupted` flag, and a `launchConfig` (command, args, env, resume-file path).
`origin` records *why*: `worktree-sleep | quit | live`.

Three decisions carry across cleanly:

1. **A hydrated record must be directly resumable.** The schema `.refine()`s that
   `getAgentResumeArgv(agent, providerSession)` is non-null — a record whose resume
   spelling cannot be reconstructed is rejected at load, not stored as a dead entry
   that fails later at launch. The transcript path is retained explicitly because
   one provider resumes by its session *file*.
2. **Launch env is sanitized at the boundary and voided whole on any violation.**
   Keys rejected on: empty, `__proto__`/`constructor`/`prototype`, containing `=`,
   containing any byte ≤ 0x1f or 0x7f; values rejected on NUL. One bad key discards
   the entire env rather than launching with a partial one.
3. **Corruption is per record, not per file.** `salvagingRecord` drops individual
   malformed entries; `shared/workspace-session-schema.js` states the policy —
   tolerant of unknown fields, strict about the types actually read, and "one bad
   tab record must not cost every worktree its state". Optional fields use
   `.catch(undefined)` so an unknown future enum degrades instead of failing the
   whole-session parse.

### 1.9 Tab close is a pure reducer that returns the PTYs to kill

`shared/workspace-session-terminal-tab-close.js` returns
`{ session, ptyIdsToKill, closed, pinned }` with no side effects; the caller kills.

- **Kill by refcount, not by ownership.** It gathers the tab's row pty, every leaf
  pty in its split layout, and its remote session id, then collects the pty ids
  referenced by *every other tab* and kills only the set difference.
- **Pinned refuses**, returning an unmodified session.
- **The split tree is pruned, not patched**: a split whose child disappeared
  collapses into its surviving sibling.
- **Next-active is MRU with a positional fallback** (`recentTabIds` backwards,
  then the first survivor after the closed one, then the last).
- Sleeping-agent records keyed `${tabId}:` are dropped in the same pass, and the
  active surface is re-derived across terminal/browser/editor/simulator.

### 1.10 Foreground-process tracking: how Orca knows a shell is idle

This is the mechanism with no Forge counterpart at all.

**POSIX**: `ps -axo pid=,ppid=,stat=,command=` (3 s timeout) behind a
TTL-cached snapshot reader (500 ms TTL, plus a serialized "must start after my
request" barrier for a *fresh* read) —
`chunks/daemon-ready-identity-*.js:4536-4605`.

- `resolveAgentForegroundProcessFromPs` (`:4985`) collects descendants of the
  shell pid and uses the **`+` flag in `ps stat`** to identify the actual
  foreground job, then matches it against known agent CLIs and unwraps wrapper
  processes.
- `isShellProcess(name)` (`:4634` — `bash|zsh|sh|fish|cmd|powershell|pwsh|nu`) is
  the **idle test**: `hasChildProcesses = fg !== null && !isShellProcess(fg)`.
- A separate, much cheaper path exists for signalling:
  `ps -p <pid> -o pid=,tpgid=,tty=` with a 125 ms timeout and a 64 KiB output cap,
  used by `signalPosixPtyForegroundGroup` (`:6074-6099`).
- Cadence is carefully tiered: 1 s foreground cache, 5 s retry for a shell,
  15 s on Windows when idle, a 10 s "hot window" after output, 5 s startup
  bootstrap (`daemon-entry.js:1583`).

**Windows** uses `Get-CimInstance Win32_Process` with a `wmic` fallback, plus
ConPTY console membership to filter candidates to processes actually attached to
that pseudoconsole.

A second idleness signal exists for releasing held startup bytes:
`createShellPromptReadinessProbe` (`chunks/…:6337`) combines an `stty`-based
line-editor probe on the pts with the `ps stat` `+` check, settling for 50 ms and
probing at most 4 times.

### 1.11 Resize is a per-pty coalescing queue, not a debounce

Renderer resizes enter at `index.js:45996` and pass three gates: a **global 500 ms
suppression window** after a fit-override change; an **ownership check** (if a
mobile or remote-desktop client owns the size, the renderer's resize is ignored);
then `provider.resize` → daemon `resize` → `Session.resize`
(`daemon-entry.js:622`), which validates the size, resizes **the headless
emulator first**, then the pty, and appends a `{kind:"resize"}` record to the
history log so replay reproduces the geometry change.

Programmatic resizes (mobile fit, restore) go through `enqueueLayout`
(`index.js:219605`): one in-flight slot per pty, and a pending tail entry that is
**overwritten** when the new target coalesces with it — so a burst of fits costs
one apply, not N. `TERMINAL_FIT_RESTORE_DEADLINE_MS = 15_000`
(`shared/terminal-fit-restore-deadline.js`) races the pending restore against a
timer; a sibling module (`terminal-zero-dimensions-diagnostic.js`) says 0×0 fits
are a known failure.

### 1.12 Title is a derived, debounced fact with provenance

`shared/terminal-output-side-effects.js`:

- **Every OSC title in a chunk is applied, in byte order.** Reading only the last
  one drops intra-chunk working→idle transitions inside a coalesced payload
  (their issue #1083).
- **`stripBrailleSpinnerGlyphs`** removes U+2800–U+28FF so titles differing only
  by animation frame compare equal — stated purpose: "avoid fan-out churn on
  spinner ticks".
- **A 3 s stale-working timer** (`STALE_WORKING_TITLE_TIMEOUT_MS = 3000`): if the
  last title read as "working" and title-less output keeps arriving, the working
  indicator is cleared — and the synthesized change is **tagged with provenance**
  (`staleWorkingTitleClear`) so downstream can tell a paused agent from a genuine
  completion.
- **Scanners are constructed only when a consumer exists**, so "headless serve
  never pays the per-chunk 133/URL scans".
- A suppression mode for spans where delivered bytes may be gapped **resets every
  cross-chunk carry** on re-enable, because stale state could swallow a real bell
  or mint a phantom fact.

### 1.13 Bell detection is a state machine because BEL is overloaded

`shared/terminal-bell-detector.js`. BEL is also an OSC string terminator, so a
naive `includes('\x07')` mints a phantom bell on every OSC title. The detector
carries `inOsc` / `pendingEscape` / `pendingOscEscape` across chunks, honours the
ECMA-48 escape-cancel codes CAN/SUB (a malformed OSC must not swallow the next
real BEL), and treats a BEL after an orphan ESC as a real bell. Its fast path
skips per-byte scanning when there is no pending state, no BEL and no OSC
introducer — and the introducer test is **hoisted into the caller and passed in as
a hint**, so a hot chunk is scanned for `\x1b]` once, not twice.

### 1.14 Kitty keyboard: a tri-state mirror, fed by application output only

The daemon's headless emulator already enables the protocol
(`vtExtensions: { kittyKeyboard: true }`, `chunks/…:14495`). The 300-line mirror in
`shared/terminal-kitty-keyboard-mode-tracker.js` exists anyway, because the
*renderer's* xterm has its flags defensively wiped and the serializer does not
carry them. It replicates xterm's push (`CSI > u`) / pop (`CSI < u`) / set
(`CSI = u`) stack semantics per screen, the flag-slot swap on DECSET/DECRST
47/1047/1049, RIS reset, DECSTR soft reset, and the 16-frame stack cap.

Two ideas are portable independently of the mirroring:

- **Tri-state, not a value.** State is `flags` + a `known` bit per screen + a
  `baselineProven` bit. `snapshotFlags` returns `undefined` when unproven, with the
  reason: "silence must not become a proven zero" — laundering a constructor-fresh
  zero into a host-proven inactive protocol would make a preview commit raw text
  against a bit-3 TUI.
- **Replay is not live.** `scanReplay` applies a push as an idempotent *set*: a
  retained-history replay redelivers the application's one-time startup push, and
  stacking it makes the TUI's eventual single pop land on a stale frame, leaving
  Option chords kitty-encoded in a plain shell. The module documents the residual
  limit it accepts (a nested push/push/pop inside the replay window collapses to 0)
  instead of pretending it does not exist.

### 1.15 Input is chunked, capped, and measured without blocking

`shared/terminal-input.js`: 16 KiB per PTY write chunk, 16 MiB total refusal,
UTF-8-safe chunk boundaries, a size check that short-circuits at the limit rather
than measuring the whole string, and a yielding/deferred variant so measuring a
huge paste does not stall the event loop.

### 1.16 Smaller things worth naming

- **Id grammar enforced at the schema boundary**: pane keys are `tab:leaf`, so
  `isValidTerminalTabId` forbids `:` in a tab id; host surface ids are `tab::leaf`
  and are folded into a local id by URL-encoding behind a `web-terminal-` prefix
  (`shared/terminal-surface-id.js`, `shared/terminal-tab-id.js`).
- **A stable save-failure code**: `ORCA_TERMINAL_SESSION_STATE_SAVE_FAILED`, with
  a legacy free-text matcher kept for old messages.
- **Link targets**: `shared/terminal-file-link-conformance.js` is a 19-case
  conformance table for "what did the user tap" — absolute and relative paths,
  `path:12:7`, `~/`, paths containing **spaces**, a path followed by prose ending
  in a filename (both halves separately tappable), surrounding punctuation, a bare
  `README.md`, and a negative case. `shared/terminal-file-url-target.js` parses
  `#L12C7` fragments and trailing `:line:col`, rejecting the trailing-colon form
  when the prefix ends in a path separator.
- **Workspace statuses** (`shared/workspace-statuses.js`,
  `workspace-status-defaults.js`) are a *user-editable kanban column set*
  (id/label/color/icon, 32-char labels, slugged ids, one-shot repairs for two
  known-bad persisted payloads). A different concept from Forge's
  `Workspace::status`; see §4.8.

---

## 2. Side by side

| # | Orca mechanism | Forge equivalent | Verdict |
|---|---|---|---|
| 1 | PTYs in a long-lived daemon that outlives the app | Same (ADR-005); `crates/daemon`, singleton lock, `pty_loop` per terminal (`crates/daemon/src/terminal.rs:118`) | **Convergent.** Independent agreement on the top-level shape. |
| 2 | Three emulators (daemon headless, main headless, renderer xterm) with a precedence chain and renderer→main hydration | Exactly one, in the daemon (`crates/terminal-core/src/alacritty_engine.rs:293`); AGENTS.md forbids a second | **Forge better.** Every re-scanner and sync module in §1.0 is work Forge does not have to do. |
| 3 | Buffer travels as serialized ANSI; restore is a replay with hand-built mode-reset prologues | Buffer travels as cells: `TerminalSnapshot` / `TerminalDelta`, `crates/domain/src/terminal.rs:173,188` | **Forge better**, and the fidelity bug class Orca patches cannot occur. |
| 4 | Bell via a hand-rolled OSC-aware byte scanner | `Event::Bell` from the VT parser → `take_bell()` → `TerminalBell` (`alacritty_engine.rs:60,395`; `core.rs:3649,3703`), interpreted by `apps/tauri | **Forge better.** Do not port the detector. |
| 5 | Output coalesced at 2 ms with size-based flush and an interactive fast path | One coalesced delta per `FRAME = 8 ms` attached / `IDLE_FRAME = 50 ms` unwatched, 64 KiB reads (`crates/daemon/src/terminal.rs:20,31,33`) | **Different tradeoff, Forge simpler.** Orca needs three batchers because it has three hops; Forge has one, and `IDLE_FRAME` is a saving Orca has no equivalent of. |
| 6 | Credit/ack flow control to the renderer; on silence, write off and force a snapshot | Bounded 256-slot per-client queue; overflow marks *behind*, drops, then one fresh `TerminalResync` (`crates/daemon/src/registry.rs:144`) | **Convergent on recovery, Forge simpler.** Both end at "stop replaying, force a snapshot"; Forge gets there without an ack protocol. |
| 7 | Producer pause (`proc.pause()`/`resume()`) with a 5 s failsafe | None: `pty_loop` always drains the master into the engine | **Forge better by construction.** Forge's sink is a bounded grid, so there is nothing to pause; Orca must pause because its sink is a growing queue. The 5 s failsafe is the tell — it exists because a pause can wedge the shell. |
| 8 | Backgrounded sessions drop oldest bytes to a keep-tail with a `dataGap` marker, salvaging query bytes | Bytes are never dropped; an unwatched terminal is still fed, just at `IDLE_FRAME` | **Forge better.** Dropping bytes before the emulator corrupts the grid; Orca has to salvage DA/DSR replies out of the dropped span to keep TUIs from hanging. Forge feeds everything and drops only *deltas*. |
| 9 | Exit graded as evidence (`operator_close / signaled / exited / unknown{…}`) | `SessionState::Exited { code: Option<i32>, signal: Option<i32> }` (`crates/domain/src/session.rs:29`), always written by `on_terminal_exited` (`core.rs:3765`) | **Gap.** Forge cannot distinguish a user stop, an idle-policy stop and a spontaneous exit, and the SIGKILL-after-grace path genuinely produces "asked, never confirmed". |
| 10 | Title normalized (spinner glyphs stripped) before it fans out | `pump_terminal` compares the raw title, then does a SQLite `upsert` **under the core lock** plus a `SessionUpdated` broadcast (`core.rs:3655-3684`) | **Gap, and a cost defect.** An agent animating a braille spinner changes the title every frame, on the ≤125/s rung. `docs/performance.md` already lists that upsert as open. |
| 11 | Foreground process resolved from `ps` (`+` flag) with a 500 ms TTL; `isShellProcess` is the idle test | Idle is inferred purely from PTY traffic: `Session::last_activity_at` + `IdlePolicy::evaluate` (`crates/daemon/src/idle.rs:104`) | **Gap — and Forge can do it far cheaper than Orca.** `portable-pty`'s `process_group_leader()` is `tcgetpgrp(master)`; Forge caches it at spawn (`crates/terminal-core/src/pty.rs:179-181`) but never re-reads it. |
| 12 | Persisted "sleeping agent": provider session locator + resume argv + launch env + origin, validated as resumable at load | `resumed_from: HashMap<SessionId, String>` on `Inner` (`core.rs:78`), read by `restart_session` (`core.rs:3106`); `ResumeStyle` per provider (`crates/agents/src/builtins.rs:15,23,31`) | **Partial gap.** The launch half exists, but the id is in-memory only and only ever user-supplied, so an `Orphaned` agent restarts blank under its old title. |
| 13 | Kill-on-close by cross-tab refcount; pure reducer, caller performs effects | One session ↔ one terminal; `close_session` refuses an active session (`core.rs:2921`); `delete_session_locked` deliberately leaves `inner.terminals` alone (`core.rs:3005`) | **Different tradeoff, Forge fine.** No splits, nothing to refcount. The *shape* (pure decision + caller effects) Forge already uses in `idle.rs:104` + `core.rs:4036`. |
| 14 | Scrollback: authority holds 1 000 rows, the rest is a 200 MB on-disk checkpoint + framed log with binary-searched truncation | Authority holds it all: `DEFAULT_SCROLLBACK_LINES = 10_000`, `MAX_SCROLLBACK_LINES = 100_000` (`alacritty_engine.rs:32,34`), served by `FetchScrollback` capped at `MAX_SCROLLBACK_FETCH = 4_096` rows/call (`core.rs:45,3520`); never persisted (ADR-009) | **Forge better and much simpler.** No checkpoint cadence, no generation headers, no cold-restore replay, no binary search. |
| 15 | Client-side scrollback budget derived from the user's row setting; every byte cap UTF-8 aware | `MAX_SCROLLBACK_CACHE_ROWS = 5_000` **rows** (`crates/client/src/store.rs:449`) | **Gap in shape.** A row cap does not bound memory: 5 000 × 200 cols × 40 B ≈ 40 MB per attached terminal — the top open item in `docs/performance.md`. |
| 16 | Input chunked at 16 KiB, refused above 16 MiB, UTF-8-safe splits | `write_terminal_input` clones the writer, drops the core lock, then one `write_all` of the whole payload (`core.rs:3455`); wire bound `MAX_FRAME_SIZE = 16 MiB` (`crates/protocol/src/framing.rs:19`) | **Small gap.** Same ceiling by accident; Forge can block in one unbounded `write_all` on the shared writer lock. |
| 17 | Kitty keyboard mirrored in 300 lines because the emulator is unreachable | None: `crates/terminal-core/src/input.rs:8` is explicit ("no Kitty protocol") | **Gap, but a cheap one.** `alacritty_terminal` 0.26 implements the whole stack behind `Config::kitty_keyboard` and exposes it as `TermMode` bits (`term/mod.rs:75-85,320,1029`). Forge needs a flag, 5 bools, and encoder branches — not a mirror. |
| 18 | OSC 133 prompt marks scanned per chunk for command start/finish + best-effort exit code | Not surfaced; `vte-0.15.0`'s `osc_dispatch` logs 133 as unhandled | **Gap, medium cost.** Needs a delegating `Handler` wrapper in `terminal-core`, not a second scanner. |
| 19 | Resize: global suppression window, ownership gate, per-pty coalescing queue, emulator resized before the pty | `resize_terminal` clamps with `PtySize::sanitized()`, resizes pty then engine under one core lock, last-writer-wins (`core.rs:3484-3510`) | **Mixed.** Ordering is a non-issue for Forge (both happen inside one lock the PTY thread also needs), and clamp-before-allocate is better than a 15 s deadline. But Forge has **no coalescing**: a window drag reflows the whole grid per event. |
| 20 | Attach can reflow every other viewer; a `ClaimViewport` opcode was added later to fix it | `attach_terminal` adopts the attacher's size **only when it is the first subscriber** (`core.rs:3417-3436`) | **Forge better**, and it got there without a protocol opcode. |
| 21 | Link detection with a 19-case conformance table | No link affordance in `apps/tauri no OSC 8 in `terminal-core` | **Gap.** The table is reusable as a spec; the implementation site must differ (§3.7). |
| 22 | Execution host derived from the pty id, which embeds its owner | `TerminalId` is an opaque runtime id; everything is local | **N/A for the MVP.** Worth remembering if remote execution lands: encode the host in the id, not in a lookup. |
| 23 | Schema-validated session JSON with per-record salvage and `.catch()` degradation | Terminal state is not a persisted JSON blob; SQLite + the startup purge cover it | **Not comparable.** |
| 24 | `workspace-statuses.js`: user-editable kanban columns | `Workspace::status` is a runtime-only *measured git* fact; `measured_at: None` ≠ clean | **Different concept.** See §4.8. |

---

## 3. Recommendations, ranked

### 3.1 Normalize the terminal title before it fans out — and get the DB write off the delta rung

**Value: highest. Cost: low. Risk: low.**

**What.** In `Daemon::pump_terminal` (`crates/daemon/src/core.rs:3655-3684`) the
change test is `title != rt.last_title` on the raw OSC string. Agent CLIs animate
their title (braille spinner frames U+2800–U+28FF, and other decorations), so
"changed" fires on every frame — and each firing costs
`inner.db.sessions().upsert(&snap)` **while holding the core lock**, plus a
`SessionUpdated` broadcast to every client, on the ≤125/s per-terminal rung.
`docs/performance.md` already lists "WAL write under the core lock" as open; this
is the input that makes it fire continuously rather than occasionally.

Two changes, in value order:

1. **Compare on a normalized title.** Strip decorative animation glyphs
   (U+2800–U+28FF is the range Orca uses) and collapse whitespace runs, then
   compare and store the normalized form. Guard the degenerate case: a title that
   normalizes to empty must not clear a previously good title — treat it as
   "no change".
2. **Move the persistence off the hot path.** The upsert does not have to happen
   inside the critical section: collect the changed session inside the lock,
   `drop(inner)`, then write — the "collect inside, act after" shape
   `docs/performance.md` already prescribes for this exact line. Or debounce the
   write; the broadcast can stay immediate, the row only has to be right eventually.

**Files.** `crates/daemon/src/core.rs` (`pump_terminal`);
`crates/domain/src/session.rs` if the normalizer belongs next to `SessionTitle`
(it is a pure string function and unit-testable there).

**Why.** It removes an unbounded-rate DB write and broadcast that no user can
perceive: nobody needs 125 spinner frames a second in the session list.

**What could break.** (a) A program legitimately using a braille glyph in its
title loses it from the session name — acceptable, and only in the session list,
not the terminal. (b) `apps/tauri` renders `SessionTitle::resolve`; normalized text
changes what title assertions see. (c) With a debounced write, a daemon killed
between the last change and the flush persists a stale title — harmless, since
sessions are purged at startup unless `persist_history`. (d) Do **not** normalize
the reset path: `pump_terminal` deliberately propagates `ResetTitle` (an empty
title) so a shell that cleared its title is not pinned to a stale one; keep `None`
and "normalizes to empty" distinct.

### 3.2 Read the PTY's foreground process group to know whether a session is really idle

**Value: high. Cost: low. Risk: low.**

**What.** Forge's idle policy is a pure function of PTY traffic
(`Session::last_activity_at`, `crates/daemon/src/idle.rs:104`). That cannot tell a
shell sitting at a prompt from an agent that is thinking hard and printing
nothing, and it is the input to a policy that can **kill** a session
(`IdleAction::Stop`). Orca answers the same question with a `ps` scan behind a
500 ms TTL cache (§1.10) — expensive, and a subprocess.

Forge can answer it with a single ioctl. `portable-pty`'s
`MasterPty::process_group_leader()` is `tcgetpgrp(master)` — the *current
foreground process group of the terminal*. `crates/terminal-core/src/pty.rs:179-181`
already calls it, but resolves it **once at spawn** and caches it ("Resolved once,
here, and never again"). Because the child `setsid`s, at spawn `fg_pgid == child_pgid`.
Later:

- `fg_pgid == rt.process_group` → the shell itself is foreground → **at a prompt**;
- `fg_pgid != rt.process_group` → a job is running in the foreground;
- `fg_pgid == -1` / error → no foreground group; treat as unknown, never as idle.

Concretely: add `fn foreground_group(&self) -> Option<i32>` to `PtyHandle`
(`crates/terminal-core/src/pty.rs:96-107`), implemented by calling
`process_group_leader()` on the retained master; feed the result into
`IdlePolicy::evaluate` as a fourth input alongside `attached`. Call it **only on
the sweep rung** (30 s, `docs/performance.md`'s frequency ladder) — never in
`pump_terminal`.

The rule to carry over from Orca's exit-cause module applies here too: an unknown
answer must stay unknown. Make the new input `Option<bool>` and let `None` mean
"do not use this signal", so a platform or a race that cannot answer never turns
into "definitely idle".

**Files.** `crates/terminal-core/src/pty.rs` (trait + impl, and the module doc's
§11.2 criteria list, which already documents the mechanism);
`crates/test-support` (`FakePtyBackend` needs the method);
`crates/daemon/src/idle.rs` (policy input); `crates/daemon/src/core.rs:4036`
(the sweeper supplies it, as it already does for `attached`).

**Why.** The policy's most consequential action is stopping a process. Traffic
silence is a proxy; the foreground group is the fact. It also unlocks better
titles ("`npm run dev`" vs "`zsh`") and a truer "is this agent busy" signal than a
bell, for the cost of one ioctl per session per sweep.

**What could break.** (a) `IdlePolicy::evaluate` is a documented pure function of
four inputs (AGENTS.md names it explicitly); adding a fifth is fine but the
purity and the "effects live in the sweeper" split must survive. (b) A shell with
job control disabled, or a session where the child moved out of its group, reports
something surprising — hence `Option`. (c) Do not extend this into a `ps` scan or
a process-tree walk: that is a subprocess on a sweep that already has a timeout
budget, and AGENTS.md's "never hold the core lock across anything that can block"
applies.

### 3.3 Grade the exit cause instead of shipping a bare `(code, signal)`

**Value: high. Cost: medium. Risk: medium (wire + persistence surface).**

**What.** Add an `ExitCause` to `crates/domain/src/session.rs`, modelled on §1.7
but only with the cases Forge can actually observe:

- `OperatorClose` — a `KillSession` / `SendSignal` the user asked for;
- `IdlePolicy` — the sweeper stopped it (`core.rs:4036`);
- `Signaled { signal }` / `Exited { code }` — reaped and reported;
- `Unknown(StopUnverified)` — `kill_groups_blocking` ran out of grace and SIGKILLed
  a group that was still alive, or `reap_child` hit its deadline and delegated.

The producers are all in one file: `kill_session` (`core.rs:2778`), the kill-grace
loop (`core.rs:~2890-2919`), `on_terminal_exited` (`core.rs:3765`), and the
sweeper's stop branch. The pattern to copy is narrower than the enum: **a cause
the daemon did not observe must be named as an absence, not encoded as a value.**
Today a `code: Some(0)` after a SIGKILL escalation is a lie Forge can tell.

Keep a `describe()`-style rendering next to the enum so notices, the session list
and a future coordinating agent read the same sentence — that is what Orca's
`describeTerminalExitCause` is for.

**Files.** `crates/domain/src/session.rs`; `crates/daemon/src/core.rs` (the four
producers); `apps/tauri / `session_tree.rs` /
`history_panel.rs` wherever `Exited` is rendered;
`crates/persistence/src/migrations.rs` **only if** the cause must survive
`persist_history = true` — append a migration, never edit one.

**Why.** With N agents running, "it stopped" is the wrong granularity. "The agent
finished" / "you stopped it" / "the idle policy stopped it" / "we asked and it
never died" is the difference between ignoring a row and hunting a leaked process
group.

**What could break.** `SessionState` is `#[non_exhaustive]`, so cross-crate matches
already carry wildcard arms — but changing the `Exited` struct variant breaks
in-crate exhaustive patterns; expect compile errors in `core.rs` and `ui`. If you
persist it, remember AGENTS.md: append-only migrations, and `terminal_id` /
`last_activity_at` stay runtime-only — do not follow them into a column by accident.

### 3.4 Budget the client scrollback cache in cells, not rows

**Value: high. Cost: low. Risk: low.**

**What.** `MAX_SCROLLBACK_CACHE_ROWS = 5_000` (`crates/client/src/store.rs:449`)
bounds a count that does not determine the cost. A `Cell` is 40 B padded, so the
true ceiling is `rows × cols × 40 B` — ~16 MB at 80 columns, ~40 MB at 200.
`docs/performance.md` prices that as the highest-value open item, because `Store`
is deep-cloned into the UI channel per event.

Replace the row count with a **cell budget** (`MAX_SCROLLBACK_CACHE_CELLS`) and
have `trim_scrollback_cache` (`store.rs:668`) evict until the running cell total is
under it. Keep the eviction *order* exactly as it is — smallest (furthest-back) key
first, with the existing rationale that the daemon stays authoritative and
`FetchScrollback` can re-fetch. Track the total incrementally in
`push_scrolled_lines` / `merge_scrollback` / `seed_scrollback_tail` rather than
recomputing, so the trim stays off the per-delta rung.

Orca's second idea applies only if Forge grows a user-facing scrollback setting:
**derive the client budget from the configured daemon scrollback** rather than
pinning a constant, so raising `terminal.scrollback_lines` does not leave the
client cache stuck at a fraction of it.

**Files.** `crates/client/src/store.rs` (the constant, `CellGrid`, the three insert
sites, `trim_scrollback_cache`, and the bound test at `store.rs:1129`).

**Why.** "Cap before you allocate, and cap the thing that costs" is already the
house rule (AGENTS.md; `docs/performance.md` *Cap before you allocate*). A row
count is the budget-after-the-fact shape applied to memory.

**What could break.** `scrollback_cache_is_bounded_and_evicts_the_furthest_rows`
and `scrollback_cached_extent` assertions assume a row count; the virtual scrollbar
in `apps/tauri reads `scrollback_cached_extent()`, and a narrower
terminal now caches *more* rows — the intended effect, but it changes what a scroll
test observes.

### 3.5 Persist and discover the agent's provider session so a restart resumes the conversation

**Value: high. Cost: medium-high. Risk: medium.**

**What.** Forge already has the launch half: `ResumeStyle::Flag` /
`ResumeStyle::Subcommand` per provider (`crates/agents/src/builtins.rs:15,23,31`),
threaded through `AgentLaunch` into `build_spawn_spec` and read back by
`restart_session` (`core.rs:3106`). Missing is everything Orca's sleeping record
carries:

1. **The id is in-memory only.** `resumed_from` is a `HashMap<SessionId, String>`
   on `Inner` (`core.rs:78`). A daemon restart marks live sessions `Orphaned`
   (`docs/terminal.md`) and drops the map, so `RestartSession` on an orphan launches
   a *blank* conversation under the old title — precisely the failure
   `restart_session`'s own comment says it exists to avoid.
2. **The id is never discovered.** It exists only if the user supplied it at
   `CreateSession`. Forge already scans provider transcripts
   (`daemon::external_agents`, `agents::collect_analytics`, which knows Claude's
   `message.id` dedupe and Codex's cumulative `total_token_usage`), so the id the
   agent actually minted is derivable from the same source.

Recommended shape, in payoff order:

- Persist the resume locator as **session metadata** (a nullable column appended in
  `crates/persistence/src/migrations.rs`) so it survives a daemon restart. This is
  metadata, not a terminal stream, so it is inside ADR-009's line.
- On session end (`on_terminal_exited`) or on a cheap sweep, resolve the provider
  session id from the transcript for that workspace + provider and store it.
- **Validate at load, not at launch** (Orca's `.refine()`): a session whose provider
  declares no `ResumeStyle`, or whose stored locator does not reconstruct into
  argv, loads with `resume: None` and a notice — never as a record that fails at
  spawn time.

**Files.** `crates/domain/src/session.rs`; `crates/persistence/src/migrations.rs`
(appended) and the session mapper; `crates/daemon/src/core.rs` (`create_session`
capture, `on_terminal_exited`, `restart_session`); `crates/agents` for anything
provider-shaped — AGENTS.md forbids branching on provider ids outside that crate,
so transcript → session-id extraction belongs next to `collect_analytics`.

**Why.** "Restart" that loses the conversation is a different feature from
"resume". With the daemon restarting more often than an agent finishes a task,
this is the difference between an orphan row and a recoverable one.

**What could break.** (a) A stale id makes the CLI fail at launch → the session
goes `Failed`; the load-time validation is what keeps that rare, and
`mark_session_failed` already exists. (b) Transcript scanning sits behind a 10 s
TTL cache with a fingerprint key — do **not** put a scan on `on_terminal_exited`
synchronously under the core lock; that path already invalidates the cache
off-lock, so reuse the same shape. (c) Do **not** copy Orca's persisted
`launchConfig` env map: `SpawnSpec.env` is a complete environment built by the
daemon and `AgentProfile.env` is the sanctioned overlay with
`RESERVED_PROFILE_VARS` filtering. A per-session env would be a third source of
environment and a way around that filter.

### 3.6 Turn on the kitty keyboard protocol — it is nearly free here

**Value: medium-high. Cost: low. Risk: low-medium.**

**What.** `crates/terminal-core/src/input.rs:8` says "no Kitty protocol", and
`AlacrittyEngine::with_scrollback` (`alacritty_engine.rs:107-112`) builds `Config`
with only `scrolling_history` set, leaving `kitty_keyboard: false`. But
`alacritty_terminal` 0.26 implements the entire protocol — push/pop/set stacks per
screen, the alt-screen swap, the report query — behind that flag, and exposes the
negotiated state as ordinary `TermMode` bits (`DISAMBIGUATE_ESC_CODES` …
`REPORT_ASSOCIATED_TEXT`, `term/mod.rs:75-85`). Orca's own daemon emulator enables
it the same way (`vtExtensions: { kittyKeyboard: true }`).

So Forge gets it for roughly the cost of Orca's *constants* file, not its tracker:

1. `Config { kitty_keyboard: true, .. }` in `with_scrollback`;
2. the five bits read out in `build_modes` (`alacritty_engine.rs:180`) into new
   `TermModes` fields — that wire type already travels with every snapshot and delta
   (`crates/domain/src/terminal.rs:149`);
3. encoder branches in `crates/terminal-core/src/input.rs` (compiled by both
   `terminal-core` and the `terminal-input` leaf crate the GUI links) that emit the
   CSI-u form when the relevant bits are set and the existing xterm form otherwise.

Take Orca's tri-state lesson only where it is cheap: Forge's engine is
authoritative and on the byte path, so the flags are always proven and there is no
snapshot-replay path that could launder an unknown into a zero. Do not port the
`known` / `baselineProven` machinery — it is a symptom of Orca's architecture, not
of the protocol.

**Files.** `crates/terminal-core/src/alacritty_engine.rs`;
`crates/domain/src/terminal.rs` (`TermModes`); `crates/terminal-core/src/input.rs`;
golden tests in `crates/terminal-core/tests/`.

**Why.** Claude Code, Codex and other TUI agents use the protocol for Shift+Enter,
Ctrl+Enter and unambiguous Escape. Without it those keys either do nothing or send
an ambiguous byte — a visible defect in the product's primary use case.

**What could break.** (a) `TermModes` is a snapshot/delta wire type; adding fields
is a serialization change — `#[serde(default)]` on the new bools keeps a
mixed-version client readable. (b) Turning the flag on makes the engine *answer*
the protocol query, so TUIs that previously fell back will now negotiate; key
handling changes for real programs, so `crates/terminal-core/tests/golden.rs` and
the `vim`/`htop` smokes are the gate. (c) Config-gate it (`[terminal]
kitty_keyboard`) so it can be turned off without a rebuild while it settles.

### 3.7 Coalesce resizes on the client before they reach the daemon

**Value: medium. Cost: low. Risk: low.**

**What.** `resize_terminal` (`core.rs:3484-3510`) takes the core lock, issues
`TIOCSWINSZ`, and calls `engine.resize(size)` — a full grid reallocation and
reflow — once per request, with no coalescing. A window drag or a pane-split
animation emits a resize per frame, so a one-second drag is ~60 whole-grid reflows
under the lock every PTY thread needs, and ~60 SIGWINCHes to the child.

Orca's answer is `enqueueLayout` (§1.11): one in-flight apply per pty and a
**pending tail entry that is overwritten** by the next target, so a burst costs one
apply. The Forge version is simpler and belongs on the client: hold the latest
requested size and send `ResizeTerminal` at most once per frame (or on a short
trailing timer), keeping only the newest — the same "coalesce rather than queue"
rule AGENTS.md already states for `FetchRemote` / `RefreshPullRequests`.

Two Orca details **not** to copy: the 500 ms global suppression window (a timing
hack), and resizing the emulator before the pty (irrelevant for Forge — both happen
inside one core lock that the PTY thread must also hold to feed, so the child's
redraw cannot be processed against the old geometry).

**Files.** `apps/tauri (or wherever the resize command is emitted)
and `apps/tauri (the command channel). No daemon change is required;
`PtySize::sanitized()` already clamps.

**What could break.** A trailing-edge-only coalescer makes the final size arrive
one tick late, so the terminal visibly settles after the drag ends — send on a
leading edge plus a trailing flush. Do not coalesce across terminals: the pending
size is per `TerminalId`.

### 3.8 Chunk large terminal input

**Value: medium-low. Cost: very low. Risk: very low.**

**What.** `write_terminal_input` (`core.rs:3455`) clones the shared writer, drops
the core lock — correct — then issues one `write_all` for the entire payload. The
wire bounds that at `MAX_FRAME_SIZE = 16 MiB`
(`crates/protocol/src/framing.rs:19`), which happens to equal Orca's own refusal
threshold. But a 16 MiB `write_all` to a PTY whose child stopped reading blocks in
the kernel for as long as the child likes, holding the per-terminal `SharedWriter`
mutex the whole time — so every subsequent keystroke and every device reply from
`pump_terminal` queues behind it.

Split into ~16 KiB writes on **UTF-8 boundaries**, re-acquiring the writer lock per
chunk, so a keystroke can interleave with a giant paste instead of waiting it out.

**Files.** `crates/daemon/src/terminal.rs` (`write_pty`) or
`crates/daemon/src/core.rs` (`write_terminal_input`).

**What could break.** Interleaving is the point, but a paste is no longer atomic
with respect to other writers. With bracketed paste on (`TermModes` carries the bit
and `terminal_core::input::paste` already wraps it) the child sees one paste
regardless; without it, an interleaved device reply can land mid-paste. Chunk
boundaries must never split a UTF-8 sequence or a bracketed-paste marker.

### 3.9 Reuse Orca's link *specification*, not its implementation

**Value: medium. Cost: low-medium. Risk: low.**

**What.** Orca detects clickable file paths and GitHub PR URLs by scanning the PTY
byte stream per chunk, with SGR stripping, cursor-control guards, a 512-byte carry
and a rule that a URL ending exactly at a chunk boundary is not yet complete.
**Forge must not do it that way** — that is per-chunk work on the delta rung and a
second parser over bytes the engine already parsed (§4.1).

The right site in Forge is the client, over `Row` text, computed **on store change**
(`RuntimeUpdate::State`), never per frame — `docs/performance.md`'s "anything
derivable from the store is computed when the store changes". Row text is already
normalized: no escapes, one grapheme per `Cell::text`, wide characters flagged. The
hard part of Orca's detector simply does not exist on that side of ADR-011.

What *is* worth copying in spirit is the conformance table in
`shared/terminal-file-link-conformance.js`: its 19 cases encode the genuinely hard
judgement calls — a path containing spaces, `path:12:7` vs. a trailing colon, a path
followed by prose ending in a filename (both separately tappable), surrounding
punctuation, a bare `README.md`, and a negative case. Write those as Forge unit
tests over a pure `fn detect_targets(row_text: &str) -> Vec<Target>`. Add
`shared/terminal-file-url-target.js`'s two forms: `#L12C7` fragments and trailing
`:line:col` with the "prefix must not end in a separator" guard.

**Files.** A new pure module under `crates/client/src/` (or `theme tokens` if it is
presentation-only), memoized in the `RuntimeUpdate::State` handler in
`apps/tauri wired into `apps/tauri for hit testing.
Opening the target goes through the existing plumbing (ADR-012: the GUI never does
`std::fs` on a workspace) — `ReadFile` / `FileContents`, not a direct open.

**What could break.** (a) Per-frame cost if memoization is skipped — the exact
anti-pattern `docs/performance.md` names; key the detection on the row and
recompute only when that row changes. (b) A false positive that opens the wrong
file is worse than no link, which is why the negative and ambiguous cases matter
more than the positive ones. (c) Any spawn to open an external target must be
reaped (`Child::drop` does not wait).

### 3.10 Smaller, cheap, worth doing at some point

- **A stale "working" title clear.** Orca's 3 s timer (§1.12) maps to: an agent that
  exits without resetting its title leaves a stale title forever. Forge already
  propagates `ResetTitle`, so only the *implicit* case is missing. Nearly free once
  §3.1 exists, since the normalizer is where a decoration-only title is detected.
- **DEC mode 2031**: write `CSI ?997;1n` / `;2n` to every attached PTY when the app
  theme changes so TUIs restyle. The subscription bit is not in `TermMode`, so
  broadcast unconditionally — an unsubscribed TUI ignores it; teaching the engine
  the mode is not worth an engine change.
- **OSC 133 prompt marks** would give per-command exit codes, durations and
  "jump to previous prompt". In Forge that means a delegating `Handler` wrapper in
  `terminal-core` around `Term<EventProxy>` — real boilerplate, but one-time and
  with zero per-chunk cost, versus Orca's permanent second scanner. Worth it once
  the shell-integration story is deliberate. It would also make §3.2 sharper: OSC
  133 `C`/`D` is a *semantic* prompt boundary where `tcgetpgrp` is a heuristic.
- **Exited-terminal tombstones.** `AttachTerminal` on a terminal whose runtime is
  gone returns `not_found`; Orca keeps a bounded tombstone map so a late reattach
  gets the exit cause instead. Only worth doing alongside §3.3.
- **A stable notice code** for a persistence failure, so anything that needs to
  branch is not matching free text.

---

## 4. What not to copy, and why

### 4.1 Any per-chunk byte scanner in the daemon

Orca's bell detector, kitty tracker, OSC 133 scanner, mode-2031 scanner, PR-link
detector, partial-escape tracker and title extractor all exist because Orca's
authoritative emulator is in another process from the consumer. Forge's daemon
feeds every byte to `alacritty_terminal` on the way past
(`alacritty_engine.rs:293`), so a second parser would be:

- **Redundant** — the engine already has the fact (bell, title, kitty flags, modes,
  cursor are `TermMode` bits or `EventProxy` events);
- **On the wrong rung** — `docs/performance.md` prices per-PTY-chunk work at ≤125/s
  per attached terminal, and `pump_terminal` runs entirely under the core lock.
  N scanners multiply the critical section by N;
- **A drift source** — Orca's own module header says the shared tracker exists so
  the two paths "must not drift". Forge has one path; do not create a second and
  then a module to keep them in sync.

**Rule:** a new terminal fact goes into `AlacrittyEngine`'s `EventProxy` /
`build_modes`, or into a delegating `Handler` wrapper — never into a byte scanner
beside the engine.

### 4.2 Serialized-ANSI snapshots, checkpoints, and replay-based restore

`terminal-mode-reset-profiles.js`, `terminal-serialize-absolute-cursor.js`,
`terminal-partial-escape-tail.js`, `terminal-restore-parity-fixture.js`,
`terminal-snapshot-unavailability.js`, the 200 MB checkpoint writer with its
binary-searched truncation, the generation-headed incremental log, and
`ColdRestoreReplayWriter` are all consequences of one decision: the buffer is a
string replayed into a *different* emulator. Forge ships cells, so wrap-pending,
the DECSC register, charset designations, scroll regions and partial escapes are
either represented or irrelevant.

Adopting any of it would also cross ADR-009 and AGENTS.md — *"Persistence stores
metadata only, never terminal streams"*. A serialized buffer written to disk is a
terminal stream, and a 200 MB checkpoint is a large one.

### 4.3 Renderer-captured scrollback persisted in session state

`workspace-session-terminal-buffers.js` keeps buffers only for repos where the
daemon is **not** the authority (SSH, paired runtimes) and strips them everywhere
else, precisely because they make every state write scale with old output. Forge
has no non-authoritative case: the daemon owns every PTY (ADR-005) and every grid
(ADR-011). Adopting the mechanism means adopting the problem first.

### 4.4 Credit-based flow control and producer pause

`terminal-multiplex-flow-control.js`, the cumulative-ack protocol, and
`SessionProducerPause` buffer and stall the producer. Forge's registry does the
opposite on purpose: a full 256-slot queue marks the client *behind*, **drops**
further deltas for that terminal, and sends one fresh `TerminalResync` when the
queue drains (`crates/daemon/src/registry.rs:144`; `docs/terminal.md`
*Backpressure*). AGENTS.md states the rule directly — "a queue that can be produced
into faster than it is drained is bounded and drops … plus a fresh resync for a
`behind` client — never a replayed backlog".

Note that Orca's *recovery* is the same as Forge's (write off the gap, force a
snapshot) — it just spends an ack protocol, a producer pause and a 5 s failsafe
timer getting there. And the pause has a cost Forge should not import: pausing the
PTY means the shell blocks on write. Forge's sink is a bounded grid, so it can
always drain the master; that is strictly better, and the failsafe timer in Orca
is the evidence.

Likewise the versioned binary opcode frame with per-stream capability negotiation:
Forge has one framing (`crates/protocol/src/framing.rs`) and one transport, and
protocol enums are already `#[non_exhaustive]` for forward compatibility.

### 4.5 Dropping PTY bytes before the emulator

Orca's backgrounded-session drop discards the oldest queued bytes and emits a
`dataGap` marker — and then has to **salvage terminal query bytes out of the
dropped span** so a TUI waiting on a DA/DSR reply does not hang, and has an entire
module of mode-reset literals for grounding the parser after a gap. That whole
complex exists because bytes can be lost *before* an emulator sees them. In Forge
nothing between the PTY and the engine may drop: `pty_loop` reads and
`pump_terminal` feeds unconditionally, and only the *delta* is dropped, downstream
of a grid that is already correct. Keep it that way.

### 4.6 A kitty-keyboard *mirror*

Take the protocol (§3.6); do not take the 300-line tracker, the `known` bit, the
`baselineProven` bit or the replay-vs-live scan distinction. Each exists because
Orca must guess at state it cannot read and must survive replaying its own retained
history into an emulator. Forge's engine is authoritative and never replays bytes
into itself, so flags are never unproven and a push is never redelivered.

### 4.7 Cross-tab PTY refcounting, splits, and pane-key grammar

`workspace-session-terminal-tab-close.js` and the `tab:leaf` / `tab::leaf` id
grammar solve problems Forge does not have: one session owns one terminal, and
`delete_session_locked` deliberately leaves `inner.terminals` alone because
`on_terminal_exited` is the single owner and removal path (`core.rs:3005`; the
counter-example in `docs/performance.md` *One owner, one removal path*). Adding a
refcount creates a second removal path for the same map — exactly the defect that
section warns about.

If split panes ever land, revisit §1.9: the *reducer* shape (pure decision, caller
performs the kills) is right and matches `idle.rs` + the sweeper. Import it then,
with the feature, not before.

### 4.8 Orca's "workspace status"

`workspace-statuses.js` is a user-editable kanban column set persisted per
worktree. Forge's `Workspace::status` is a **measured git fact**, runtime-only,
never a column, and AGENTS.md is explicit that `measured_at: None` means "not
measured", which is not the same as clean. Merging the concepts would make a
measurement editable and would need a column for something deliberately kept out of
the schema. If Forge ever wants board columns, they are a *separate* field with a
separate name.

### 4.9 The 15 s fit-restore deadline and the 500 ms resize suppression window

`TERMINAL_FIT_RESTORE_DEADLINE_MS` waits out an asynchronous renderer measurement,
and `suppressResizesForMs(500)` papers over the resulting races. Forge has no such
handshake: the client sends a size with `AttachTerminal` and the daemon clamps it
with `PtySize::sanitized()` before it reaches the engine (`core.rs:3425`,
`crates/domain/src/agent.rs:48`) — the cap-before-you-allocate pattern that exists
because an unclamped `65535×65535` asked for ~170 GB under the core lock. Copy the
*coalescing* from §1.11 (see §3.7); leave the timers.

### 4.10 A second PTY spawn path as a fallback

Orca carries `LocalPtyProvider` in the Electron main process as a fallback for when
the daemon is unavailable, and then has to keep its exit-cause handling, its shell
fallback chain and its foreground detection in sync with the daemon's. Forge's GUI
starts or connects to the daemon and has no local spawn path — one owner, one
`SpawnSpec` builder, one place where `TERM` / `COLORTERM` / `FORGE_*` are injected.
A "fallback" spawn would duplicate all of that and immediately violate ADR-005.
