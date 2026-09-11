# CHECKPOINTS — evaluating the final state

> In a multi-agent system you do not evaluate the path, you evaluate the
> destination. The `reviewer` walks every box, ticks `[x]` or `[ ]` in
> `harness/progress/review_<id>.md`, and **rejects if any is left empty**.
>
> C1–C3 are mechanical: a command decides them, not an opinion.
> C4–C7 are the real boundaries and invariants of this repository, derived from
> `AGENTS.md` (§ *Boundaries And Invariants*). When an invariant changes there,
> change it here. C7 expands the cost model of `docs/performance.md`: apply it
> when the diff touches the delta path, the Tauri render/store path, the core
> lock or any `Command`.

## C1 — The harness is complete

- [ ] `init.sh`, `harness/CHECKPOINTS.md` and `scripts/harness` exist, and
      `harness/progress/current_<id>.md` exists for the reviewed feature.
- [ ] The diff commits no harness state: nothing under `harness/progress/`,
      `harness/specs/<id>-<slug>/` or `harness/features.json` is staged.
- [ ] `./init.sh --fast` (or `scripts/harness gate --fast`) finishes with exit
      code 0.

## C2 — The state is coherent

- [ ] At most **one** feature in `in_progress` or `in_review`.
- [ ] The reviewed feature has its `harness/specs/<id>-<slug>/` with
      `requirements.md`, `design.md` and `tasks.md`.
- [ ] `harness/progress/gate_<id>.md` exists from `spec_ready` on.
- [ ] `harness/progress/context_<id>.md` exists from `in_progress` on.
- [ ] `harness/progress/events_<id>.jsonl` is coherent with the feature status
      when present.
- [ ] `gate_attempts` does not exceed `rules.max_gate_attempts` (default 3).
- [ ] `harness/progress/current_<id>.md` describes the live session, not leftovers
      from the previous one.
- [ ] `review_rounds` does not exceed `max_review_rounds` (2).

## C3 — The gate is green

- [ ] `cargo fmt --all -- --check` with no differences.
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` with no warnings.
- [ ] `cargo test --workspace` green.
- [ ] If `apps/tauri` changed: `bun run --cwd apps/tauri lint`,
      `bun run --cwd apps/tauri fmt:check`,
      `bun run --cwd apps/tauri typecheck` and
      `bun run --cwd apps/tauri test` are green.
- [ ] The diff adds no new `#[ignore]`, no `#[allow(...)]` without a
      justification in the same file, and no `unwrap()`/`expect()` in daemon
      runtime paths where the error is recoverable.
- [ ] New comments explain a constraint, a rejected alternative, or a known
      exception (`AGENTS.md` § *Comments And Code*). They do not restate the
      next line, narrate history (`used to`), copy a plan doc / `AGENTS.md`
      into a file header, or cite `§N` as current truth. A touch of an old
      file that already has that noise deletes the redundant comment next to
      the change; it does not rewrite the crate.

## C4 — Boundaries between crates

Every box is a `grep` over the diff, not a general impression.

- [ ] `terminal-core` and any second VT engine still live only in the daemon:
      `client` and `apps/tauri` keep their passive snapshot/delta replica.
- [ ] The Tauri host still reaches wire types through `client`'s re-exports
      (`DaemonEvent`, `ProviderInfo`), and the frontend talks to the daemon only
      through the host runtime bridge. `apps/tauri/src-tauri/Cargo.toml` still
      has no direct `protocol` dependency.
- [ ] No crate outside `crates/agents` branches on `provider_id` nor knows
      about per-provider binaries, probes or launch behaviour.
- [ ] Every application Git subprocess goes through `git_service::run_git`;
      every command that opens a socket goes through `run_git_network` (its own
      timeout, `GIT_SSH_COMMAND` with `BatchMode=yes`, askpass disabled).
      `push` still leaves **only** from `CreatePullRequest`, never from a
      sweeper. Talking to the *host* (not to git objects) still lives only in
      `git-service/src/github.rs`, through the configured `gh`.
- [ ] No Tauri view writes a hex outside `apps/tauri/src/theme/tokens.ts` or
      generated token files. Colours and metrics come from theme tokens, and
      repeated fields/buttons/dialogs use the shared `apps/tauri/src/ui` pieces.
- [ ] `domain` is still shared serializable state and `protocol` is still
      transport-independent framing/messages; the `#[non_exhaustive]` enums are
      still matched with conservative wildcard arms from other crates.

## C5 — Runtime invariants

- [ ] The `inner -> registry` lock order is respected, and the core lock is
      **never** held across an `.await`.
- [ ] `Daemon::pump_terminal_batch` is still a single critical section per
      processed PTY batch (feed + optional coalesced emit + the activity note
      + idle clock). `pty_loop` waits on `poll` with a deadline rather than
      sleeping after a successful read, and `FRAME` is the emit floor, not the
      read cadence. It has not been split again into feed/`has_subscribers`/emit.
- [ ] `TerminalRuntime::emit_seq` advances once per **emitted** delta, not per
      feed to the engine.
- [ ] The runtime-only fields still have no SQLite column and are rebuilt on
      load: `Session::terminal_id`, `Session::last_activity_at`,
      `Workspace::status` (with `measured_at: None` ≠ clean). A
      `ShareState`/`ShareAction`/`ShareCandidate` is read on demand, never
      stored. `Session::base_commit` is the one that *is* a column (migration
      9): resolved once at creation, between two lock sections, and never reset
      by a restart.
- [ ] Every coalescing flag is released by a `Drop` guard, not by the happy
      path — `fetching`, `pr_opening`, `pr_refreshing`, `provisioning`,
      `drafting` — because `Daemon::lock` recovers from poisoning and a panicked
      worker would latch it forever. A worker that fails to *spawn* clears its
      own flag, since the guard only runs inside the thread.
- [ ] Provisioning (`daemon::shares`) still runs off the request thread and
      without the core lock, still decides in the pure `plan` half, and still
      refuses to overwrite a path unless the caller named that rule.
- [ ] The migrations in `crates/persistence/src/migrations.rs` have only been
      **appended at the end**; no existing one was edited or reordered.
      Persistence still stores metadata, never terminal streams.
- [ ] `SpawnSpec.env` is still a complete environment (not an overlay) and the
      launch preserves `TERM`, `COLORTERM`, `FORGE_SESSION_ID` and
      `FORGE_WORKSPACE`.
- [ ] `domain::RESERVED_PROFILE_VARS` never arrives from an `AgentProfile.env`:
      the builder drops them and `SaveAgentProfile` refuses to store them.
- [ ] A read-only launch (§16.9) still applies the descriptor's `ReviewStyle`
      flags **after** a profile's own arguments so they win, is still *refused*
      (`InvalidRequest` / `AgentError::ReviewUnsupported`) for a provider that
      declares none rather than downgraded to one that can write, and is still
      remembered in `Inner::read_only` so a restart re-applies it — unlike the
      initial prompt, which a restart must not re-send.
- [ ] The idle policy is still a pure function in `crates/daemon/src/idle.rs`
      and the effects still live in the `core.rs` sweeper.
- [ ] Every request that opens a socket keeps the same shape: ack when it
      *starts*, result by event and coalescing, not a queue —
      `FetchRemote`/`Inner::fetching` (per project),
      `CreatePullRequest`/`Inner::pr_opening` (per workspace),
      `RefreshPullRequests`/`Inner::pr_refreshing` (global). A worker that does
      not start clears its own flag.
- [ ] `GetSnapshot` still does not touch the network: the pull request state is
      cloned out of `pull_requests::Cache`, which has its own `Mutex` and is
      never taken under the core lock. A `PullRequest` still has no SQLite
      column.

- [ ] `GetWorkspaceDiff` is still a local, synchronous read like
      `ListBranches`: no ack-and-event and no broadcast, and the core lock is
      released before invoking git. `WorkspaceDiff` is still runtime-only (no
      column, no migration, no field in `Store`). `GetSessionChanges` and
      `GetWorkspaceReview` are the same kind of read, and so are
      `SessionChanges` / `WorkspaceReview` / `SessionTranscript`.
- [ ] A read against a *base* sources its file list from
      `diff --name-status`, not from `status`: `status` cannot see a path a
      commit changed and left clean, which is most of what a session did.
- [ ] `change_summary` still costs a fixed number of subprocesses whatever the
      file count — untracked additions come from a `.take(cap)` read, never a
      `--no-index` process each — because the split that reads it refreshes
      itself.
- [ ] `DraftWithJuva` acks and reports through `JuvaDraftReady`; it does **not**
      answer with `Response::JuvaDraft`. Its endpoint opens a socket, so a
      synchronous answer would put a network write on the request path.
- [ ] `juva::draft_remote` falls back to the local draft on every failure and
      reads its key from the environment variable `[juva] api_key_env` names —
      never from the config file or the database.
- [ ] `git-service::diff` still keeps the number of subprocesses flat in the
      number of files: one `status`, one `numstat` and one batched
      `diff --patch` split by `diff --git `. Only untracked files cost one
      process each (`--no-index`, whose exit status **1** is the success case).
- [ ] A patch that exceeds the budget is discarded **whole** and marked
      `truncated`; it is never cut in half.

- [ ] The GUI does not do `std::fs` over a workspace (ADR-012): `ListFiles` /
      `ReadFile` / `ReadImage` / `WriteFile` / `SearchFiles` are local,
      synchronous reads like `GetWorkspaceDiff`, resolved by `fs-service`
      outside the core lock, with every canonical path inside the checkout.
      `FileTree` / `FileContents` / `ImageContents` / `SearchResults` are still
      runtime-only (no column, no migration, no field in `Store`).
- [ ] `WriteFile` requires the `revision` of the last `ReadFile` and answers
      `PreconditionFailed` if the disk changed; the GUI re-reads, it does not
      embed the content in the error. The Tauri editor still sends reads and
      writes through the runtime bridge rather than doing workspace filesystem
      work in the WebView.

- [ ] The two usage readings stay separate: `ProviderUsage` (what the provider
      says is left) and `UsageAnalytics` (what we count as spent). Token parsing
      and the price table still live in `crates/agents`; `GetUsageAnalytics` is
      still a local, synchronous read like `GetWorkspaceDiff` (no
      ack-and-event, no broadcast, no column) and goes through
      `usage_stats::Cache`, with its own `Mutex` and a TTL per window.
- [ ] The counting still does not double-count: dedupe by `message.id` in
      Claude, deltas over Codex's cumulative `total_token_usage`, and reasoning
      tokens outside `TokenTotals::total`. A model with no published price adds
      to `unpriced_turns` instead of cost, and a bounded scan reports `skipped`
      instead of looking exhaustive.

## C6 — Traceability

- [ ] Every `R<n>` of `requirements.md` has concrete evidence in
      `harness/progress/impl_<id>.md`: a named test with its path and its exact
      command, or a manual case documented with its real output. "It was
      tested" is not evidence.
- [ ] Every task in `tasks.md` is `[x]`, or `[ ]` with the reason written down.
- [ ] No file touched outside the `crates[]` declared in the feature without an
      explicit note in `impl_<id>.md`.
- [ ] The diff has not widened the scope beyond `spec_raw`: no opportunistic
      refactors and no new `Cargo.toml` dependencies that are not in
      `design.md`.

## C7 — Resource cost (memory, CPU, processes)

> Evaluated when the diff touches `crates/terminal-core`, the daemon's delta
> path, `apps/tauri` render/store code, the core lock, or adds/modifies a
> `Command`. See `docs/performance.md` for the reason behind each box.

- [ ] Nothing that scales with the grid or the scrollback is deep-cloned on the
      delta step (≤125/s **per attached terminal**): it travels as an `Arc`, or
      it does not travel.
- [ ] No new queue is `unbounded` unless the consumer proves it outruns the
      producer. If it can pile up, it is `bounded` and it drops, like
      `registry.rs` with `flume::bounded(256)` + a fresh resync to the `behind`
      client — never a replay of the backlog.
- [ ] The per-cell terminal path does not allocate on the heap, does not acquire
      locks and does not use `format!`. Palette/theme data is cached before the
      cell loop, not read through a global lock per cell.
- [ ] Nothing derivable from the store is recomputed per frame: in Tauri it
      lives in store updates or memoized selectors keyed on the data that
      changed. `Timestamp::now()` is hoisted out of row loops and menus are
      built lazily when opened.
- [ ] Every size arriving from the wire, from a subprocess, from a file or from
      the network is bounded **before** allocating, not after. A bounded scan
      reports `skipped`/`truncated` instead of looking exhaustive.
- [ ] Every new map in `Inner` has **one** owner and **one** deletion path, and
      it is the one the real close takes. An accumulator with conditional
      draining also carries an unconditional bound.
- [ ] No new loop sleeps-and-checks unless it waits on something that cannot
      wake it (`reap_child`, `kill_groups_blocking`).
- [ ] The core lock is not held over anything that can block: a subprocess, the
      network, an FS walk or disk I/O. Every new exception carries its comment
      explaining why, or it does not land.
- [ ] Every `Command` that can time out is built with `.process_group(0)` and
      killed with a **negative** pgid, with a fallback to the direct pid
      (`git_service::run_gh` is the reference).
- [ ] Every `spawn()` is reaped; no `wait()` without a bound on the calling
      thread. State that must be undone on the error path (`fetching`,
      `pr_refreshing`, `pr_opening`) is undone with a `Drop`, not with the next
      statement.
- [ ] A mutation that mints an id returns the id; no caller reloads a snapshot
      to guess what it just created. No convenience wrapper sneaks in a network
      call, and no cache key is computed from the already-resolved output.
- [ ] If the diff optimises against a dependency's behaviour, the review note
      quotes the source read in `~/.cargo/registry/src/`, not an assumption.
