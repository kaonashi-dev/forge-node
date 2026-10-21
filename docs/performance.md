# Resource discipline: memory, CPU and processes

What "expensive" means in this codebase, and the rules that keep it cheap.
Crates involved: all of them — this page is about a cost model, not a module.

Written from a full audit of the tree on 2026-08-27 (latency, CPU, memory and
process handling, 35 findings). Every rule below is stated with the real defect
that motivated it, so the reasoning survives even when the line numbers drift.
Line references are as of that audit; trust the symbol names.

Related: [terminal.md](./terminal.md) for the delta pipeline and backpressure,
[protocol.md](./protocol.md) for framing limits, [`../AGENTS.md`](../AGENTS.md)
for the invariants this page expands on.

## Focused terminal-path review: 2026-09-06

`client::CellGrid` rebuilt its entire `BTreeMap` of cached history on every
scroll delta to shift negative viewport offsets. A full cache meant moving
5,000 entries for a single new line. The cache now uses stable absolute indices;
`scrollback_row` translates viewport offsets at read time. Scrolling inserts
only newly cached rows and evicts the oldest, without rebuilding the index or
allocating a temporary seed vector. Snapshot tails and fetched blocks are
limited to the cache budget before their rows are cloned.

The local release-mode microbenchmark measured 10,000 one-line scroll deltas
with a 200-column, 50-row viewport and a full 5,000-row history cache:
**530.651 ms before, 7.665 ms after** (about 69× faster for this operation).
Snapshot construction is outside the timer; deltas contain no damaged rows,
so this isolates history bookkeeping and is not an end-to-end rendering claim.
Reproduce with:

```sh
cargo test -p client --release scrollback_delta_timing -- --ignored --nocapture
```

Current-source checks also refine the historical audit below: Tauri's bridge
already owns its `CellGrid` on the runtime thread and emits shell state separately
from terminal frames. The old whole-`Store` emission finding does not describe
that bridge. `client::Client` now uses a bounded event queue that disconnects
on overflow. `Daemon::pump_terminal` still persists title changes under the
core lock. Column patches cover mid-screen edits; a line feed can still force
a whole-grid delta because Alacritty reports `TermDamage::Full`.

## Cross-project review: 2026-09-22

The current implementation removes these additional costs:

| Path | Change | Cost reduced |
|---|---|---|
| `editor-core::syntax`, `editor-cli::App::rescan_edited` | Consume and splice the previous syntax cache; grow scanner storage only as lines are visited. | Untouched span vectors are retained rather than cloned; incremental scans no longer count and preallocate the entire suffix. |
| `editor-core::complete` | Retain the best twelve borrowed candidates during scanning. | At most twelve owned candidate strings, with no full-result sort. |
| `fs-service::name_search` | Partition to the best K hits, then sort those hits. | O(N + K log K) ranking instead of O(N log N), with K capped at 200. Scoring still visits the index. |
| `agents::usage::analytics` | Keep the newest 500 transcript candidates in a bounded heap. | O(500) retained candidates instead of O(N); directory enumeration still visits all candidates. |
| `shared/cell-grid/viewport.ts` | Decode each text run once when constructing row columns. | A 200-column run creates one code-point array instead of 200, visiting 200 code points instead of 40,000. |
| Portable file explorer | Cache filter-independent directory interests against listing/fold changes. | Subsequent filter keystrokes do not reconstruct the unfiltered tree for watches. |
| Git decorations | Clear pending timers and queued refresh debt on disposal. | No stale diff request after teardown. |
| `daemon::environment` | Cap shell capture at 1 MiB and apply the deadline through process exit, with group kill on failure. | Oversized shell output cannot grow capture without bound; closing stdout cannot bypass the timeout. |
| `Daemon::remove_project` | Release session side caches and workspace status clocks. | Removed projects no longer retain idle, resume, read-only or status-check entries. |

A release-mode microbenchmark of 1,000 single-line edits midway through a
20,000-line Rust document measured **667.446 ms before, 66.548 ms after**
(about 10× faster for incremental highlighting). Reproduce the current path with:

```sh
cargo test -p editor-core --release incremental_syntax_timing -- --ignored --nocapture
```

A standalone optimized benchmark of ranking 10,000 synthetic hits down to 200
measured **361.4 ms before, 43.8 ms after** over 2,000 iterations; comparisons
dropped from 134,599 to 24,078 per iteration. This isolates selection and sorting,
not fuzzy matching or IPC. Neither benchmark establishes end-to-end UI latency.
Regression tests cover ranking equivalence, syntax-cache reuse, transcript
retention bounds, Unicode decoding and teardown behavior.

## Performance pass: 2026-10-14

A five-area review (daemon terminal path, client and host, WebView, request
handlers and services, editor) removed these costs. Figures are scratch release-mode
micro-benchmarks at 200×50 unless stated; none is an end-to-end latency claim.

| Path | Change | Cost reduced |
|---|---|---|
| `protocol::framing::FrameWriter` | The write that fits stays inline; growth, the budget check and the error are a cold call. | MessagePack's 1–3 byte writes made a whole-grid delta encode in ~616 µs; ~200 µs after, per subscriber. |
| `domain::Color` on the binary wire | One integer instead of a variant name (JSON unchanged). `PROTOCOL_VERSION` 32. | A blank cell went from 20 to 5 bytes; a full delta ~204 KB → ~64 KB, and the client decodes it in about half the time. |
| Daemon socket | `SO_SNDBUF` raised to 1 MiB per accepted connection. | macOS's 8 KiB default split one delta into ~25 sends and reads (221 µs → 12 µs measured). |
| `convert_cell` (under the core lock) | Unstyled cells with no combining marks skip the flag map and the per-char build. | ~−55% on conversion for deltas, snapshots, scrollback and resyncs. |
| `Daemon::pump_terminal_batch` | The session title is written after the lock, as a column `UPDATE` on the second connection and only under `persist_history`; the comparison borrows instead of allocating. | No WAL write (or its auto-checkpoint fsync) under the core lock on the delta rung; one fewer allocation per emitted delta and per feed. |
| `Daemon::ensure_env` | Every path into `resolved_env` captures the login shell first, off the lock, behind `env_capture`. | A request racing the startup capture no longer runs `$SHELL -l -i` under the core lock (up to 5.5 s). |
| `registry::route_terminal_delta` | A subscriber is marked behind at 64 queued messages and resynced at 8. | A stopped reader pinned up to ~100 MB of stale deltas; now ~25 MB, with room left for responses. |
| `FetchScrollback` | Clamped to 512 Ki cells as well as 4 096 rows. | A wide fetch no longer overflows the frame and drops the connection after the conversion ran under the lock. |
| `file_index::Cache` | `forget` on every workspace removal; `invalidate` no longer creates slots. | Removed workspaces kept their listing (up to 10 000 entries) and a lingering watch re-created it. |
| CLI `GetSnapshot` | Leaves out discovered history; an agent exit marks history stale instead of locking its cache. | `forgectl` calls no longer trigger transcript scans, and an exit broadcast never waits behind one. |
| `FrameDecoder` | A drained buffer above 1 MiB shrinks to 256 KiB. | One 16 MiB response no longer stays resident per connection. |
| Host `Batch::settle` | A shell publish is withdrawn when the store reports `EventOutcome::Unchanged`. | Editor frames (≤125/s), orchestration progress and notices each rebuilt, serialized and reconciled the whole shell. |
| Host `request_scrollback` | Fetches every missing viewport row plus a margin. | A page jump left rows blank and refetched the same block on every frame. |
| Transcript scans | `agents::jsonl` reads records through one buffer with a 1 MiB (history) / 8 MiB (analytics) cap; typed borrowed parses skip message content; directory walks stat only `.jsonl` files within a 20 000-entry budget. | A whole `String` and `serde_json::Value` per record, tool output included (95 MB of history 20–28 → 13–14 ms; 71 MB of analytics 219–337 → 133–137 ms, synthetic). |
| `git-service::diff` | Batched patch capped at 8 MiB with a bounded per-file fallback; untracked files capped at the per-file budget; valid UTF-8 output decoded without a copy; split into ranges and only kept patches copied. | Peak memory was ~2× the whole-tree patch plus an unbounded capture per untracked file. |
| Content search, change context, checkers, setup scripts | Prefix capture or `.take(cap)` with a drain; `grep_word` reports truncation instead of failing. | Whole subprocess output was resident before the response limit applied. |
| `FOREIGN_KEY_INDEXES` (migration 14) | Indexes the child columns parent deletes search. | Each session/workspace/task delete scanned `runs`, `attempts` or `task_deps`. |
| Editor wrap scroll | `anchor_above` walks up from the caret once. | A far jump in wrap mode stepped the anchor a row at a time: ~600 ms on 20 000 lines. |
| Editor folds | Hidden ranges cached and searched; headers binary-searched; folds shift with inserted/removed lines. | `is_hidden` was O(folds × regions) per row (48 ms per wheel notch with 40 folds); Enter above a fold used to unfold or misplace it. |
| `metrics::display_column` | ASCII prefix counted without grapheme segmentation. | Fold rescan per keystroke ~5 ms → ~1 ms; cheaper `line_rows` everywhere. |
| Editor painting | One row map per frame instead of one viewport walk per row; ruler matches cached per (version, query, height). | O(height²) row mapping per frame; a whole-document search per frame while find is open. |
| `Syntax::edited`, `History` | Restart offset from the line index; typing runs extend in place; the undo stack is a `VecDeque`. | A byte walk to the edit (~200 µs at line 10 000); O(run²) copies while typing; a ~0.9 MB shift per edit once 4 096 entries existed. |
| WebView | Hidden main pane keeps its viewport but does not paint; DOM editor rows and diff files reconcile by key; intra-line marks are lazy; batched snapshot application; keyed external runs and pull requests; memoized tabs/history; renderer skips redundant style assignments and allocations; bounded latency samples on a sorted ring; DPR changes remeasure. | Per-frame paints nobody saw, row/patch remounts per answer, positional rewrites of every later row, and an unbounded `outstanding` list in the editor pane. |

## Why this page exists

Forge is a terminal multiplexer with a Tauri front end. That combination has a
cost model most apps do not: a single attached terminal streaming output makes
the daemon build, serialize, fan out, decode and repaint a grid **up to 125
times a second**. Work that looks free in a request handler is ruinous three layers down.

The audit found the app doing two HTTPS round trips to the public internet on
every <kbd>Cmd</kbd>+<kbd>T</kbd>, deep-copying up to 20 MB per terminal event
into an unbounded queue, and accepting a `65535×65535` resize straight off the
socket into an allocator. None of those were exotic bugs. All three came from
applying request-shaped thinking to a hot path.

## The frequency ladder

Before adding work anywhere, find its rung. The cost of a line of code here is
its cost times its rung.

| Rung | Rate | Where | Budget |
|------|------|-------|--------|
| per cell | ~10 000 / frame | `runtime/cells.rs` (`cell_style`), `shared/cell-grid/` | **zero allocations, zero lock acquisitions** |
| per row | ~50 / frame, ~50 / delta | `DeltaBuilder::delta`, `CellGrid::apply_delta` | one `Vec` at most |
| per PTY batch | input-driven; not capped by `FRAME` | `Daemon::pump_terminal_batch` | no blocking syscall or grid-sized clone under the core lock |
| per emitted delta | ≤125 /s **per attached terminal** on the daemon (`FRAME` = 8 ms); the host coalesces terminal cells at `CELL_SEND_FLOOR` (16 ms, 33 ms scrolled) | `route_terminal_delta`, `runtime_loop` | no syscall that can block, no clone that scales with the grid |
| per frame | every repaint | Tauri frontend render | nothing derivable from the store |
| per store change | user actions and daemon broadcasts | `applyShellSnapshot`, `createMemo` | this is where per-frame work belongs |
| per request | user-initiated | daemon request handlers | subprocesses and disk are fine, **outside the core lock** |
| per sweep | 30 s / 300 s | idle, usage, PR sweepers | anything, within its timeout |

Two rungs deserve special care because they are easy to miss:

- **Unwatched sessions no longer throttle the child.** The 8 ms floor is the
  emit cadence for an attached terminal. Reads wait on `poll` and drain
  available bytes independently of that floor.
- **Per-frame is not per-user-action.** Terminal frames must stay separate from
  sidebar, diff and PR state; routing a background delta through shared shell
  state would make unrelated surfaces pay its frame rate.

## Memory

### Nothing that scales with the grid may be deep-cloned per event

In the original Rust GUI, `RuntimeUpdate::State` carried `store: Store` by value,
and `Store` owns `terminals: HashMap<TerminalId, CellGrid>`.
Each `CellGrid` holds `visible` plus
a `scrollback_cache` capped at `MAX_SCROLLBACK_CACHE_ROWS = 5_000`, and
`push_scrolled_lines` fills that cache within seconds of any scrolling output.
A `Cell` is 40 B padded, so a 200-column replica saturates at ~40 MB — copied on
every daemon event, at up to 125 /s. Row cells now travel as `Arc<Vec<Cell>>`,
and the client event queue is `flume::bounded(64)` (overflow disconnects).

**Rule.** Anything on the delta rung that is bigger than a cache line ships as
`Arc`, not by value. If a queue can be produced into faster than it is drained,
it is bounded and drops, the way the daemon side already does
(`registry.rs`: `flume::bounded(256)` plus a fresh resync for a `behind`
client — never a replayed backlog).

> **Current Tauri path.** Grid rows are `Arc`, and both the daemon outbound queue
> and the client event queue are bounded. The host builds `ShellSnapshot` from a
> borrowed `Store`, cloning shell metadata but not its terminal grids, and sends
> frames separately. The whole-store emission above is a historical finding.

### Cap before you allocate, not after

Four separate places got this wrong in the same way: they let the input decide
the allocation and applied the budget to the *result*.

- `resize_terminal` passed a wire `PtySize` straight into the emulator. `cols`
  and `rows` are `u16`; `65535×65535` asks for ~4.3e9 cells (~170 GB) **while
  holding the core lock**. Fixed by `PtySize::sanitized()`.
- `git-service::diff` runs `git diff --patch` over the whole tree and copies
  every chunk into owned `String`s, then consults `MAX_DIFF_BYTES`. The budget
  bounds the response; the peak is ~3× the raw patch.
- Transcript scanning reads a JSONL line with no length cap, then parses it into
  a `serde_json::Value` if it contains `"usage"` — and `is_interesting`
  deliberately admits every `"type":"user"` record, which is exactly where
  Claude Code writes tool results.
- The usage HTTP client uses `into_reader().read_to_end()`. In `ureq`, only
  `into_string()` self-limits.

**Rule.** Every byte count that comes from the wire, from a subprocess, from a
file or from the network is hostile until clamped. Clamp at the boundary, before
the allocation.

**Patterns already in the tree that do it right — copy these:**

| Pattern | Where |
|---|---|
| length checked against `MAX_FRAME_SIZE` *before* reserving | `protocol::framing`, both sides |
| `metadata()` checked before `File::open` | `fs-service` (`MAX_FILE_BYTES`) |
| reader wrapped in `.take(cap)` | `agents::detection::read_all` (`MAX_PROBE_OUTPUT`) |
| checked arithmetic on a client-supplied offset | `fetch_scrollback` (`MAX_SCROLLBACK_FETCH`) |
| bounded scan that reports what it skipped | `analytics::recent_transcripts` |
| event count and path bytes checked in the watch callback | `daemon::file_watch` (`MAX_PATHS`, `MAX_PATH_BYTES`) |
| bounded stdout/stderr readers with group kill on overflow | `git_service::run_git_bounded`, used by the global file index and the batched diff |
| a prefix kept, the group killed, the cut reported | `git_service::run_git_prefix` (content search, change context, `grep_word`) |
| one reused record buffer that skips an oversized record unheld | `agents::jsonl` (history, analytics) |
| per-stream `.take(cap)` then drain to a sink so the child still exits | `daemon::diagnostics`, `shares::apply::run_in` |
| bounded grep list, per-file `metadata()` before `.take(cap + 1)`, and parse budgets | `fs-service::heuristic`, `syntax-index`, `external-index::ctags` |

A bounded scan that silently truncates is worse than one that refuses: report
`skipped`/`truncated` rather than looking exhaustive.

### One owner, one removal path

Every `HashMap<SomeId, _>` on `Inner` needs exactly one place that removes from
it, and that place must be on the path the close actually takes.

`remove_project` releases every session through `remove_session_cache` and clears
`status_checks` per workspace, and `drop_workspace_rows` does the same for one
workspace, so `idle_warned`, `resumed_from`, `read_only` and `status_checks` do
not keep an entry per removed entity. A new map adds its removal to those paths
instead of inventing another.

**The counter-example matters as much as the rule.** `inner.terminals` is
deliberately **not** cleared by `delete_session_locked`: the `TerminalRuntime` is
owned by `on_terminal_exited`, which removes it *and* reaps the child on PTY
EOF. Dropping it earlier would race that reap and leak a zombie. So the rule is
not "clean every map at every exit" — it is "name the owner, and let only the
owner remove."

### Drain conditions must be reachable

The original Rust GUI's `pending_echoes` was drained only when the active
terminal's sequence advanced. If that terminal had no replica — it exited, or
was detached — both sides were `None`, the condition was false forever, and the
`Vec<Instant>` grew one entry per keystroke. The Tauri host keeps a single `u64`
high-water mark instead (`pending_echo` in
`apps/tauri/src-tauri/src/runtime/bridge.rs`), which cannot grow.

**Rule.** An accumulator whose drain is conditional needs a second,
unconditional bound: a cap, a clear on session switch, or both.

### Watches are bounded and coalesced

A directory watch is a producer the OS runs at its own pace. `file_watch.rs`
gives it the same shape as the delta path: the notify callback does no I/O, no
canonicalization and no unbounded growth — it `try_send`s a `Vec<PathBuf>` into
`flume::bounded(16)` and, on a full queue, an oversized batch or a needed
rescan, sets one resync flag. A worker thread coalesces for 150 ms, caps a
batch at `MAX_PATHS = 32`, and clears the set for a single empty-path
`FileChanged` when the flag is up. The interests are capped before anything is
watched (128 directories, 4096 bytes per path, canonicalized inside the
checkout) and the whole subscription dies with the connection.

The alternative — one recursive `notify` watch on the workspace root — is one
OS watch for the tree and an unbounded event stream for a build spike, handed
to a client whose outbound queue is already carrying terminal deltas.

**Rule.** A native event source is hostile input exactly like the wire: bound
its queue and its batch before the allocation, coalesce, and let the consumer
resync instead of replaying a backlog.

## CPU

### Compute on change, not on frame

Everything derivable from the store belongs where the store changes
(`applyShellSnapshot`, a `createMemo`), not in a render path. Per frame, the
audit found: the diff pane re-parsing, re-flattening and re-hashing the whole patch; the sidebar running four sorts whose comparator is
`a.name.to_lowercase().cmp(&b.name.to_lowercase())` — two `String`s per
comparison — over nested O(P·W·S) scans; the history panel allocating two
`String`s per row in `matches_search`; the file tree rebuilt from scratch with a
`String` per path segment; `latency_p95` sorting 120 samples for a value shown
only in a tooltip.

**Rule.** If the output is a pure function of `(store, a bit of UI state)`,
memoize it against the thing that changes. Hoist `Timestamp::now()` out of row
loops — `status_bar` threads `now` in as a parameter so the whole bar renders
from one instant; that is the pattern.

Build menus inside the lazy `dropdown_menu` closure, not beside the row: a menu
that only appears on click must not cost anything on the frames where nobody
clicked.

### An answer that says nothing is not an update

A watcher fires on every write, and the write that dominates — a file that
already existed being saved again — produces a listing identical to the one on
screen. Three layers each stop it rather than paying for it: `sameListing`
drops a re-listing that names the same paths in the same order, so the store
object never moves; the explorer re-derives its rows only when the tree, the
folds or the filter change, so a loading flag going up and down costs nothing;
and a paint rewrites only the rows whose content moved, reusing the nodes, so
a decoration arriving does not drop the row under the pointer.

The watch ack is held to the same rule. The daemon's `WatchFiles` replaces the
directory set in place, which happens on every fold and every file opened
outside the tree; treating each ack as "everything changed" made an ordinary
click re-list the checkout and re-read the open document. An *arm* — the first
watch, a new checkout, a reconnect, a retry after a failure — reconciles the
whole surface. Adding a directory reconciles that path after the ack because
it may have changed while unwatched; removing one or keeping the same set
does not invalidate anything. Compact tree rows also watch their intermediate
directories so a new sibling can split the compact row.

The explorer reads immediate disk children independently per directory, including
root and empty directories. Previous children stay visible during reads and
errors; completed empty reads are remembered. Invalidations retain one refresh
debt per directory while a read is active, and a fixed coalescing deadline does
not slide under continuous writes. Closed directories are stale rather than
recursively re-read. The global navigation index has its own demand-driven load;
the explorer does not pay for a repository-wide listing after each write.

Directory responses carry request IDs and generations; watcher ACKs carry
interest generations. Both reject late answers. The host rejects a missing,
closed or full workbench queue instead of losing commands. Mutation results
settle independently of refresh results; uncertain writes reconcile with bounded
reads and are never replayed automatically.

File dragging (`apps/tauri/src/features/files/explorer/fileDrag.ts`) retains one
pointer payload, one hover deadline and at most one animation frame. Each drag
frame performs one DOM hit test and a bounded 12 px edge-scroll step; it stops scheduling at the scroll
limit. It never traverses the file index or terminal grid. A 600 ms hover may
request one ordinary lazy folder expansion, and release submits one correlated
move or targeted paste. Pointer motion itself performs no daemon requests.

Git decorations retain one pending refresh when a filesystem event arrives
during a diff read or its 10-second cooldown. The pending deadline does not
move with each write, and completion wakes it without polling. Explicit
refreshes bypass the cooldown but still wait for an active read; changing
checkouts discards the old pending refresh.

**Rule.** Before a read's answer is stored, ask whether it differs from what is
already shown. A repaint the user can see is a promise that something changed.

### No allocations and no global locks per cell

`cell_text` returned a `String` per cell for `push_str` to consume and drop —
~10 000 malloc/free per full-screen frame — while `palette()` returned a 164-byte
`Palette` by value from behind an `RwLock`, two to four times *per cell*.

Both are fixed, and the fixes are the patterns to reuse: push the
`CompactString`'s `&str` directly and keep the owning helper only for the
branches that genuinely need a value (cursor, wide characters); and cache the
palette in a `thread_local` tagged with a generation counter that
`configure_with_base` bumps, so the steady-state read is an atomic load and a
`Copy`.

**Rule.** In per-cell code: no heap allocation, no lock acquisition, no
`format!`. `SharedString::from(format!("row-{i}"))` as an element id is the same
mistake one rung up.

### Block, don't poll

The UI runtime thread used a 4 ms `recv_timeout` as a poll interval because it
could not block on two channels — 250 wake/sleep cycles per second at complete
idle. `flume::Selector` blocks on both and the poll disappears.

**Rule.** A sleep-and-check loop is a defect unless it is waiting on something
that genuinely cannot wake it (`waitpid` after EOF, a kill grace). Those two
exceptions exist in the tree and are correct: `reap_child` polls `WNOHANG` for
500 ms then delegates, and `kill_groups_blocking` polls `group_alive` with an
early exit.

`pty_loop` waits in `poll(2)` until input or a frame/synchronized-output deadline,
then drains a bounded batch into `pump_terminal_batch`. It never sleeps after a
successful read: the 8 ms floor limits attached-terminal emits, not PTY reads.

### Damage-driven, and know what damage you actually have

The delta pipeline is damage-driven by design: no shadow grid, no memcmp, no
grid hash. Keep it that way.

But there is a real limit, and it is worth writing down because the obvious
optimization is unsound. A line feed at the bottom of the screen makes alacritty
call `mark_fully_damaged()`, so `Term::damage()` returns `TermDamage::Full` for
the steady state of any scrolling output, and the delta is a whole-grid repaint
(~46 MB/s per attached terminal at 200×50). The tempting fix — treat
`Δhistory == n && damage == Full` as a scroll and send only the newly exposed
rows — **loses data**:

```rust
// alacritty_terminal-0.26.0, Term::damage()
if self.damage.full {
    return TermDamage::Full;   // early return: never reaches TermDamageIterator
}
```

Once the full flag is set, the per-line damage that a mid-screen edit in the
same chunk carried is no longer reachable through the public API. Narrowing to
the scrolled rows would silently drop that edit — a status line above a DECSTBM
scroll region, for instance. Recovering the true damage would need a shadow
grid, which ADR-011's replica model forbids.

**The lesson generalizes:** before optimizing against a dependency's behaviour,
read the dependency's source. The vendored crates are in
`~/.cargo/registry/src/`.

The reachable win on that path is the opposite direction: alacritty's
`LineDamageBounds` also carries `left`/`right`, and `take_damage` throws them
away, so a prompt repaint dirtying 12 columns still ships 200 cells. That is the
interactive path, where damage genuinely *is* `Partial`.

## Processes

### Kill the group, never the pid

Children `setsid` at spawn, and a child that forks (`git` → `ssh`, `sh` →
`npm`) leaves grandchildren holding the pipe write ends. `wait_with_output()`
reads both pipes to EOF *before* it calls `wait()`, so killing the direct child
alone leaves the worker thread blocked in `read` forever, two fds leaked, a
permanent zombie, and — the real damage — the process that was actually hanging
still running.

**Rule.** Every `Command` that can time out is built with `.process_group(0)`
and killed with a negative pgid, with a direct-pid fallback for a child that
moved itself out of the group. `git_service::run_gh` is the reference
implementation. `signal_group` / `group_alive` in the daemon always take a
negative pid; no path may signal a bare one.

A doc comment claiming a group kill is not a group kill: `run_setup_script`
documented exactly this behaviour and killed a positive pid.

### Every spawn is reaped

`Child::drop` does not wait. Five fire-and-forget `spawn()` calls in the GUI —
"open in editor", "reveal folder", a URL click — accumulate one zombie each for
the life of the process. A child the GUI spawns to outlive it (the daemon) also
needs `setsid`, or a group-wide SIGINT from a terminal takes it down with the
app.

**Rule.** `spawn()` is followed by a `wait()`, on a detached thread if you do not
care about the result. An unbounded `wait()` on the calling thread is its own
bug: bound it with `try_wait` to a deadline, then hand it to a reaper —
`reap_child` is the model.

### Coalescing flags need drop guards

`fetching`, `pr_refreshing` and `pr_opening` are set before the work and cleared
after it, in a straight line. `Daemon::lock` deliberately recovers from
poisoning, so a panic in one of those workers does not take the daemon down — it
just latches the flag, and every later request coalesces onto a worker that no
longer exists and returns `Ack` forever.

**Rule.** State that must be unwound on the failure path is unwound by a `Drop`
impl, not by the next statement.

## Locks

The daemon has one global `Mutex<Inner>`, and `Daemon::pump_terminal_batch` takes it
once per processed batch. Batch frequency follows input, not the 125/s emission
floor. Everything under the lock shares that input-driven cost rung.

`AGENTS.md` states the rule as "never hold the core lock across `.await`". Read
it as the stronger thing it means: **never hold the core lock across anything
that can block** — a subprocess, the network, a filesystem walk, or disk I/O.

The tree is disciplined about this almost everywhere, and every exception
carries a comment explaining itself: PTY spawn, version probes, `refresh_project`'s
git, `get_workspace_diff`, the PR refresh worker, `collect_usage`'s HTTP,
transcript discovery, the PTY write and the child reap all run outside it.

One known exception remains: `resolved_env` takes `&mut Inner` and can lazily
run `$SHELL -l -i -c 'env -0'` behind it — measured at **796 ms** on a real
machine before `-i` also sourced `.zshrc`, with a 5 s timeout. Every current
caller first runs `Daemon::ensure_env`, which captures off the lock behind its
own `env_capture` gate (a request that races the startup capture waits on the
gate, not on the core lock), so the lazy branch is only a fallback for a new
caller that forgets it. The session title, the other former exception, is now
written after `drop(inner)` as a column `UPDATE` on the second connection.

Note also that `PRAGMA synchronous` is `NORMAL`, not SQLite's `FULL` default.
That is correct under WAL and it is what removed the fsync from the pump path;
do not "fix" it back.

## Latency: don't drag a workflow onto a keystroke

<kbd>Cmd</kbd>+<kbd>T</kbd> took ~1 s to open a shell. None of it was the PTY —
the daemon's spawn is 3–10 ms. The cost was that `CreateShellSession` answered
`Ack` and threw away the id it had just minted, so the client reloaded the whole
snapshot and guessed which session was new by `max_by_key(created_at)`. That
reload pulled in a transcript rescan (201 MB on the audit machine, behind a 10 s
TTL shorter than a human pause) and a provider-usage probe that made two
sequential HTTPS calls with a fresh, unpooled client each time.

**Rules.**

1. **A mutation that mints an id returns the id.** `Response::SessionCreated`
   carries `session_id` and `terminal_id`; the caller attaches straight away.
   The broadcast still goes out for every other client.
2. **A convenience wrapper must not smuggle in a network call.** `load_store()`
   used to fold in `list_provider_usage()`; the comment even acknowledged the
   cost. Compose the cheap read explicitly, and let slow readings arrive through
   their own event (`ProviderUsageChanged`).
3. **A TTL shorter than the interval between user actions is not a cache.**
   Ten seconds does not survive a person pressing a shortcut twice.
4. **Cache keys are derived from inputs, not from resolved output.**
   `RefreshPullRequests` spawns `git remote` per project *before* consulting a
   cache whose fingerprint comes from the resolved query.
5. **Sequential network calls in a loop are a fan-out you forgot to write**, and
   a client rebuilt per call is a TLS handshake you paid for twice.

## WebView budgets

The rungs above are the daemon's. The following table defines the shell's
budgets for changes to `apps/tauri`.

| Surface | Budget | How it is held |
|---|---|---|
| Editor (`surface = cells`) | keystroke → paint p95 ≤ 16 ms on a 20 000-line file; open ≤ 100 ms after `ReadFile` returns | `forge-editor` under a PTY, painted by the same canvas renderer as a terminal (`docs/editor.md`). The cost is a frame of cells: the editor damages the rows an edit reached and the loop paints at most once per 8 ms. |
| Editor (`surface = dom`) | one window per input burst, ≤125/s; a window is what is on screen plus 2×24 overscan, never the file | The rung is **visible DOM rows**, not VT cells. A headless host publishes `ViewFrame`s and the browser composites the scroll, so scrolling costs the rows that entered the window and nothing repaints. Frames are clamped while they are built (`VIEW_ROW_BUDGET`), because the per-row caps do not compose into a frame budget. A window of 88 syntax-coloured Rust rows measures ~19 KB; the whole window is resent on every frame, so a sustained scroll tops out near 2.4 MB/s over a local Unix socket — a per-row `stale` delta would be the fix if that ever shows up in a profile. Rows ship behind an `Arc`, so broadcasting to a second client is a refcount. |
| Diff | expand a 2 000-line patch ≤ 50 ms | Unified and split patches mount viewport rows plus 24 rows of overscan on either side; closed and distant patches mount no rows. |
| File tree | 50 000 paths at 60 fps; filter keystroke ≤ 8 ms | Windowed rows with an overscan; the filter narrows the daemon's listing rather than re-scoring it. |
| Bundle | initial JS ≤ 350 kB gz; editor chunk ≤ 250 kB gz | `apps/tauri/scripts/check-bundle.ts`, run by `bun run build`. Fails the build when either is exceeded. |

Two costs on this side are deliberate and documented rather than fixed:

- **A scrolled viewport gets a full frame.** `cells::frame` sends every row
  whenever `scroll_offset > 0`, because a damage list is expressed in
  live-viewport rows and means nothing against a window of the scrollback. The
  floor is raised from 16 ms to 33 ms while scrolled (`CELL_SEND_FLOOR_SCROLLED`
  in `apps/tauri/src-tauri/src/runtime/bridge.rs`), which halves it. That is not a
  latency regression: latency is measured against the keystroke that produced the
  output, and a scrolled viewport is by definition not showing what a keystroke would produce.
- **The colour of a cell is cached, the font already was.** `ColorCache`
  (`apps/tauri/src/shared/cell-grid/palette.ts`) holds one `#rrggbb` per indexed
  slot and a bounded map for truecolour. Before it, a full-screen 256-colour TUI
  rebuilt a string per run per frame — per-cell work on the frame rung.

### Diff viewport windowing

`features/git/diff/WindowedPatchRows.tsx` reserves the full patch height from
theme font metrics while mounting only the visible row slice. Each vertical
scroller owns one passive scroll listener, one intersection observer and one
resize observer (`patchViewport.ts`). A scroll burst schedules at most one
animation frame; that frame reads positions only for nearby patches before
publishing any DOM changes. Parsing and intra-line marks remain memoized against
the patch, not the scroll position. The final subscription removes the listeners,
disconnects the observers and cancels its pending frame.

Split rows share the outer vertical scroller instead of synchronizing two nested
scrollbars. Hunk navigation scrolls by document row coordinates, so its target
need not be mounted. Stable reserved heights prevent row-window changes from
moving the scrollbar. Native text selection covers mounted rows, not an entire
offscreen patch.

This follows the visible-block/placeholder approach in Rebased's IntelliJ-based
[`CombinedDiffViewer`](https://github.com/DetachHead/rebased/blob/master/platform/diff-impl/src/com/intellij/diff/tools/combined/CombinedDiffViewer.kt),
not its JVM editor implementation. Unlike that loader, Forge still parses every
expanded patch on a diff change; parsing itself is not lazy or off-thread.

A local Chromium headless smoke used twenty files, each with 2,005 unified rows,
in a 1,200 × 600 px scroller. Before/after: **40,100 → 84 mounted rows** and
**240,734 → 778 DOM elements**, reserving the full document height (**704,560 →
704,685 px**; fractional row rounding differs). Across 120 animation-frame scroll
steps of 140 px, the measured frame-interval p95 was **22.0 → 17.1 ms** (median
16.7 ms in both runs). These are
synthetic browser measurements, not a Tauri WebView latency or FPS guarantee.
Unit tests cover window bounds, inactive-block costs, shared/coalesced scheduling,
font/patch changes, unmounted hunk targets and teardown. Browser smokes also
checked a jump deep into the file list, intra-line marks, line activation and
split alignment.

Syntax colouring shares `shared/syntax/highlight.ts` with project-search excerpts.
`patchSyntax.ts` tokenizes a file lazily when a row first mounts and retains the
result against the path and parsed-row identity. Scrolling reuses those tokens;
theme changes update scope colours without re-tokenizing. Each hunk starts fresh,
and old/new source has independent lexer state. Intra-line marks wrap coloured
tokens rather than replace them, and source stays text rather than HTML.

Syntax work is capped before excerpt/token arrays: 4,096 patch rows and 262,144
UTF-16 units across both sides, plus the shared 32,768-unit excerpt limit per
hunk side. Over-budget and unknown-language fragments stay readable as complete
plain text. The earlier measurements above predate syntax colouring. A subsequent
syntax-enabled Chromium smoke of the same twenty-file workload retained **83
rows / 978 DOM elements**, with a **18.4 ms** frame-interval p95; expanding a
2,005-row patch and waiting for its next frame measured **29.3 ms**. These remain
synthetic browser checks, not native WebView guarantees.

## Known open

Carry these forward; they are real, verified, and not yet fixed.

| Item | Where | Why it still matters |
|---|---|---|
| Whole-grid delta on a line feed | `DeltaBuilder::delta` | Mid-screen edits now travel as column patches. A line feed can still report `TermDamage::Full` (see Alacritty `Term::damage()`), so that path still repaints the viewport. |
| `resolved_env` lazy fallback | `Daemon::resolved_env` | Only reachable by a caller that skips `ensure_env`; it would still capture under the lock. |
| Full fold rescanning | `editor-cli::App::rescan_edited` | Edits still rebuild fold regions (now ~1 ms at 20 000 lines); an indent-signature splice would make it incremental. |
| Eager diff parsing | `PatchView.tsx`, `SplitPatchView.tsx` | Expanded patches are parsed when their text changes; unchanged files keep identity and intra-line marks are lazy. |
| Git work before PR cache lookup | `Daemon::refresh_pull_requests` | Remote resolution still spawns Git per project before consulting the cache. |
| Trailing blank cells on the wire | `domain::Row` | ~70% of a typical full delta is default blanks. Eliding them is a decode-side amplification risk (a 6-byte row can claim 1 000 cells) unless the decoder is budgeted per frame. |
| Frames as evaluated script | `bridge.rs` `app.emit("runtime:cells")` | Tauri events inline JSON into `eval`; a `tauri::ipc::Channel` with raw bytes would skip the parse and the per-frame object graph. |
| Whole-shell publishes | `Emitter::shell`, `ShellSnapshot` | One changed section still resends every list (`external_agents` at 600 cards is ~486 KB). |
| Transcript text under the lock | `Daemon` session transcript read | Builds up to 4 096 rows of cells under the core lock to keep a 36–96 KB text tail. |
| Filesystem work in `build_spawn_spec` | `core.rs` | `inject_attention`, `ensure_config_dir` and executable resolution run under the core lock. |
| `GetSnapshot` on the runtime thread | `RefreshSnapshot` in `bridge.rs` | A slow snapshot can overflow the client's 64-slot event queue and force a reconnect. |

The original Rust-GUI observations about `ui::runtime` cloning the whole `Store`
and one `impl Render` repainting the window are historical, not descriptions of
the current Tauri implementation. The host bridge borrows `Store` when constructing
shell snapshots and emits terminal frames separately; the portable explorer is
windowed. This does not establish virtualization or render cost for every other
list. Current frontend ownership and lifecycle follow-ups are tracked in
[frontend-architecture.md](./frontend-architecture.md).

## How this was measured

Reproducible, and worth repeating before and after any change on the delta rung:

- **Daemon timings** come free from the log — `~/Library/Application Support/Forge/logs/daemon.<date>.log`
  on macOS. The `env.resolve` span is where the 796 ms above came from.
- **Endpoint latency**: `curl -w "%{time_total} %{time_connect} %{time_appconnect}"`.
- **Shell startup**, which is the *user's* cost and not the app's, but shows up
  as "the terminal is slow": compare `$SHELL -l -c exit` against
  `$SHELL --no-config -c exit`. On the audit machine that was 0.58–0.75 s versus
  0.00 s, almost all of it one line in the user's config.
- **Transcript volume**: `du -sh ~/.claude/projects`.
- **Dependency behaviour**: read the vendored source under
  `~/.cargo/registry/src/`. Do not infer it.

`scripts/dev check` is the gate for correctness, not for cost. Nothing in it
would have caught any finding on this page.

## Orchestration traffic

`forgectl hook` fires at turn boundaries or attention events, not every tool
call, and activity reports do not touch SQLite. Claude installs turn/question/
permission hooks, Codex installs a turn-complete notify command, and OpenCode
V2 installs a terminal-local event plugin. Its reporter keeps at most one child
and one pending state, bounds execution, kills the process group on timeout,
and removes subscriptions on unload. A `ClientKind::Cli` connection skips `TerminalActivity`,
`TerminalBell`, `ClipboardStore`, `EditorFrame`, and `ProviderUsageChanged`,
so a controller blocked in `run wait` is not queued behind a terminal burst.
Ledger message bodies are not written into a PTY. Explicit `SendSessionPrompt`
is separate: one 8 KiB slot per target, a per-session admission gate, and a PTY
writer lock revalidated against hook activity before any submission. Audit I/O
uses a second SQLite connection outside the core lock. Session names use one
cached snapshot; `session wait` and worktree creation wait on events, not disk
or snapshot polling. Result files and board values are
clamped with `metadata` plus `Read::take` before the rest of the bytes are
resident. See [orchestration.md](./orchestration.md).
