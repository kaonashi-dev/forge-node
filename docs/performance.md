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
| per cell | ~10 000 / frame | `ui::terminal::terminal_row`, `cell_style` | **zero allocations, zero lock acquisitions** |
| per row | ~50 / frame, ~50 / delta | `DeltaBuilder::delta`, `CellGrid::apply_delta` | one `Vec` at most |
| per PTY chunk / per delta | ≤125 /s **per attached terminal** (`FRAME` = 8 ms) | `Daemon::pump_terminal`, `route_terminal_delta`, `runtime_loop` | no syscall that can block, no clone that scales with the grid |
| per frame | every repaint | Tauri frontend render | nothing derivable from the store |
| per store change | user actions and daemon broadcasts | `RuntimeUpdate::State` handling | this is where per-frame work belongs |
| per request | user-initiated | daemon request handlers | subprocesses and disk are fine, **outside the core lock** |
| per sweep | 30 s / 300 s | idle, usage, PR sweepers | anything, within its timeout |

Two rungs deserve special care because they are easy to miss:

- **Unwatched sessions no longer throttle the child.** The 8 ms floor is the
  emit cadence for an attached terminal. Reads wait on `poll` and drain
  available bytes independently of that floor.
- **Per-frame is not per-user-action.** A background agent's delta repaints the sidebar, the diff pane and the PR list too.

## Memory

### Nothing that scales with the grid may be deep-cloned per event

`RuntimeUpdate::State` carries `store: Store` by value, and `Store` owns
`terminals: HashMap<TerminalId, CellGrid>`. Each `CellGrid` holds `visible` plus
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

> **Status: partly fixed.** Grid rows are `Arc`, and both the daemon outbound
> queue and the client event queue are bounded. `Store` itself can still be
> cloned on slower GUI paths; see *Known open* below.

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

A bounded scan that silently truncates is worse than one that refuses: report
`skipped`/`truncated` rather than looking exhaustive.

### One owner, one removal path

Every `HashMap<SomeId, _>` on `Inner` needs exactly one place that removes from
it, and that place must be on the path the close actually takes.

`remove_project` deletes from `sessions`/`workspaces`/`projects` by hand instead
of going through `delete_session_locked` and `delete_workspace_locked`, so
`idle_warned`, `resumed_from` and `status_checks` keep an entry per removed
entity forever. The code already knows the rule — `delete_workspace_locked`
carries the comment *"leaving it behind grew the map by one entry per workspace
the daemon ever saw"*.

**The counter-example matters as much as the rule.** `inner.terminals` is
deliberately **not** cleared by `delete_session_locked`: the `TerminalRuntime` is
owned by `on_terminal_exited`, which removes it *and* reaps the child on PTY
EOF. Dropping it earlier would race that reap and leak a zombie. So the rule is
not "clean every map at every exit" — it is "name the owner, and let only the
owner remove."

### Drain conditions must be reachable

`pending_echoes` is drained only when the active terminal's sequence advanced.
If that terminal has no replica — it exited, or is detached — both sides are
`None`, the condition is false forever, and the `Vec<Instant>` grows one entry
per keystroke.

**Rule.** An accumulator whose drain is conditional needs a second,
unconditional bound: a cap, a clear on session switch, or both.

## CPU

### Compute on change, not on frame

Everything derivable from the store belongs in the `RuntimeUpdate::State`
handler. Per frame, the audit found: the diff pane re-parsing, re-flattening and
re-hashing the whole patch; the sidebar running four sorts whose comparator is
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

`pty_loop` is the reference for the common case: it blocks in `read(2)` and the
frame floor is a sleep *after* a successful read, never a spin. An idle terminal
costs zero.

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

The daemon has one global `Mutex<Inner>`, and `Daemon::pump_terminal` takes it
up to 125 times a second **per terminal**. Everything under it is on the delta
rung.

`AGENTS.md` states the rule as "never hold the core lock across `.await`". Read
it as the stronger thing it means: **never hold the core lock across anything
that can block** — a subprocess, the network, a filesystem walk, or disk I/O.

The tree is disciplined about this almost everywhere, and every exception
carries a comment explaining itself: PTY spawn, version probes, `refresh_project`'s
git, `get_workspace_diff`, the PR refresh worker, `collect_usage`'s HTTP,
transcript discovery, the PTY write and the child reap all run outside it.

Two known exceptions remain, both waiting on the same fix shape — collect inside
the critical section, act after `drop(inner)`:

- the session-title `upsert` in `pump_terminal` (a WAL write, on the delta rung);
- `resolved_env`, which takes `&mut Inner` and lazily runs `$SHELL -l -c 'env -0'`
  behind it — measured at **796 ms** on a real machine, with a 5 s timeout.

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

The rungs above are the daemon's. These are the shell's, from
`plan-ui-ux.md` §3.3, and they are the numbers a change to `apps/tauri` is
measured against.

| Surface | Budget | How it is held |
|---|---|---|
| Editor | keystroke → paint p95 ≤ 16 ms on a 20 000-line file; open ≤ 100 ms after `ReadFile` returns | CodeMirror renders the viewport only (`workbench/editor/`). The `<textarea>` under a `<pre>` this replaced painted every token of the file on every settle and capped colour at 6 000 lines. |
| Diff | expand a 2 000-line patch ≤ 50 ms | The patch is a CodeMirror document (`workbench/diff/PatchView.tsx`), so an expanded file costs its viewport rather than a `<div>` per patch line. |
| Job stream | 1 000 lines/s with main-thread idle ≥ 70 % | `store/jobOutput.ts` appends at absolute store paths and copies the tail only when it overshoots budget by `OUTPUT_SLACK`; `JobStreamView` uses `Index`, and reads layout on scroll rather than per batch. |
| File tree | 50 000 paths at 60 fps; filter keystroke ≤ 8 ms | Windowed rows with an overscan; the filter narrows the daemon's listing rather than re-scoring it. |
| Bundle | initial JS ≤ 350 kB gz; editor chunk ≤ 250 kB gz | `apps/tauri/scripts/check-bundle.mjs`, run by `pnpm build`. Fails the build when either is exceeded. |

Two costs on this side are deliberate and documented rather than fixed:

- **A scrolled viewport gets a full frame.** `cells::frame` sends every row
  whenever `scroll_offset > 0`, because a damage list is expressed in
  live-viewport rows and means nothing against a window of the scrollback. The
  floor is raised from 16 ms to 33 ms while scrolled (`CELL_SEND_FLOOR_SCROLLED`
  in `runtime/bridge.rs`), which halves it. That is not a latency regression:
  latency is measured against the keystroke that produced the output, and a
  scrolled viewport is by definition not showing what a keystroke would produce.
- **The colour of a cell is cached, the font already was.** `ColorCache`
  (`terminal/palette.ts`) holds one `#rrggbb` per indexed slot and a bounded map
  for truecolour. Before it, a full-screen 256-colour TUI rebuilt a string per
  run per frame — per-cell work on the frame rung.

## Known open

Carry these forward; they are real, verified, and not yet fixed.

| Item | Where | Why it still matters |
|---|---|---|
| `Store` deep-cloned on slower GUI paths | `ui::runtime`, `client::Store` | Terminal rows are `Arc` and both event queues are bounded. Remaining clones are whole-store snapshots, not per-cell. |
| Whole-grid delta on a line feed | `DeltaBuilder::delta` | Mid-screen edits now travel as column patches. A line feed can still report `TermDamage::Full` (see Alacritty `Term::damage()`), so that path still repaints the viewport. |
| WAL write under the core lock | `Daemon::pump_terminal` | fsync is gone (`synchronous = NORMAL`) but the write still holds the global mutex on the delta rung. |
| `resolved_env` under the core lock | `Daemon::resolved_env` | 796 ms measured, 5 s worst case, taken while the socket is being bound. |
| One `impl Render`, no list virtualization | `apps/tauri` | Every delta repaints the whole window; every scroll list builds its off-screen rows. |

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
