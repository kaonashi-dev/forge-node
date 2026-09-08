# Execution — ForgeNode (Desktop Agent Terminal)

A living execution document for [plan.md](./plan.md). We tick tasks off as we go.

**Chosen product name:** `Forge Node` (binaries and path namespaces still use the short `Forge` / `forge` forms).
- Display: menus, window title, `.app` / desktop entry → `Forge Node`.
- Binaries: `forge-tauri` (GUI), `forge-daemon` (runtime). Bundle executable stays `Forge` (no spaces).
- Env vars: `FORGE_SESSION_ID`, `FORGE_WORKSPACE`. Sentinels: `__FORGE_ENV_BEGIN__`/`__FORGE_ENV_END__`.
- App dirs (`directories`): empty qualifier, empty org, app `Forge` (path namespace; do not rename without migration).

**Implementation strategy:** the *backend* (daemon-side) was implemented first, testable without the GUI. Phase 0 is complete and verified within the requested local macOS scope: real terminal/TUIs, reconnect and latency measurement. Wayland/X11 are explicitly deferred; therefore `plan.md`'s original cross-platform gate is still open. The `client` crate (protocol client + `CellGrid` replica) does not depend on the GUI (§17).

**Verified environment (2026-08-22):**
- Rust 1.89.0, cargo 1.89.0, target `aarch64-apple-darwin`.
- git 2.50.1, sqlite 3.51.0.
- Network to crates.io: OK.
- Installed providers: `claude`, `codex`, `opencode`, `cursor-agent` present. `agent` on the PATH is **Grok** (`~/.grok/bin/agent`), NOT Cursor → it confirms the need for the `version_probe` with `expect_substring="cursor"` (§7.5).

**Legend:** `[ ]` pending · `[~]` in progress · `[x]` done and verified · `[-]` out of scope for this session / deferred.

---

## Stage 0 — Workspace bootstrap  (shared base, I do it) — ✅ COMPLETE

- [x] `git init` + `.gitignore`
- [x] Root `Cargo.toml` (workspace with all the members) + centralised `[workspace.dependencies]`
- [x] `rust-toolchain.toml` (pinned to 1.89.0)
- [x] `deny.toml` (cargo-deny)
- [x] Initial `README.md` (Stage 4)
- [x] The directory tree of every crate (§17)
- [x] `domain` crate (the shared contract): ids, project, workspace, session, agent, context, **terminal** (§11.4 wire types)
- [x] `cargo build --workspace` green (stubs) + `cargo test -p domain` (9/9)

> Stage 0 decisions:
> - `domain::terminal` hosts the cell/grid types (Row/Cell/Cursor/TermModes/TerminalSnapshot/TerminalDelta…) so that `protocol`, `terminal-core`, `client` and the GUI share them **without** the GUI depending on `terminal-core` (§17). The `TerminalEngine` *trait* and the alacritty impl stay in `terminal-core`.
> - `AgentDescriptor` uses `String`/`Vec` (owned) fields instead of `&'static str` so it can travel over IPC and support custom agents.
> - Versions pinned by MSRV 1.89: `rusqlite 0.37` + `rusqlite_migration 2.3` (a compatible pair, a single `libsqlite3-sys`).

## Stage 1 — Leaf crates  (subagents in parallel, on top of `domain`) — ✅ COMPLETE

- [x] `protocol` — framing (u32 BE, 16 MiB), hello, request, response, event, error (§10) — **20 tests**
- [x] `terminal-core` — PtyBackend/PtyHandle, TerminalEngine, alacritty 0.26 engine, DeltaBuilder, input mapping (§11) — **34 tests** (incl. an insta golden)
- [x] `agents` — registry, descriptor/AgentAdapter, verified detection, builtins (§7.5, §13) — **19 tests**
- [x] `git-service` — command wrapper (30 s timeout), repository, worktree + slug (§8 git, §14) — **19 tests**
- [x] `persistence` — db (WAL+FK), §15.2 migrations, 6 repos, reconcile_orphaned (§15) — **11 tests**
- [x] Fixed 4 clippy warnings in `domain` (derivable Default)
- [x] `test-support` — fake_agent, fake_pty, temp_repo (§21); the empty fake PTY waits for explicit output to avoid EOF races
- [x] **Integration: `cargo build --workspace` + `cargo test --workspace` green → 120 tests, 0 failures**

## Stage 2 — daemon + client integration  (I do it)

- [x] `client` — `ipc` (sync UDS transport + §9.2 handshake) + `store` (`CellGrid` replica + §10.5 seq/resync rules), WITHOUT a GUI toolkit (§17) — **11 tests**
- [x] `daemon` — runtime, client_registry, event routing, services (lib+bin)
  - [x] `paths.rs` — socket path + length validation (<100B, /tmp fallback) + §15.1 dirs (ADR-004) — 3 tests
  - [x] `config.rs` — §15.4 `config.toml` with defaults + scrollback clamp — 4 tests
  - [x] `logging.rs` — tracing → rotated daemon.log (§15.1, §22) — 1 test
  - [x] `lockfile.rs` — `flock` singleton (§9.2) — 2 tests
  - [x] `server.rs` — UDS bind 0600, accept loop, Hello/HelloAck handshake, per-client writer/reader (tokio)
  - [x] `registry.rs` — event routing + backpressure 256 + resync (§10.5) — 3 tests
  - [x] `core.rs` — dispatcher for the ~30 requests + in-memory state + graph
  - [x] ProjectService (add/remove with 3 policies/refresh/rename + git detect) + WorkspaceService (list/create/remove worktree)
  - [x] SessionService: states/transitions, Close/Kill/Restart, graph (depth/cycle/cross-project), re-parenting (§7.3, ADR-010)
  - [x] TerminalService: PTY loop, alacritty engine, seq/resync/backpressure, 8 ms coalescing, attach/write/resize/scrollback (§10.5, §11)
  - [x] AgentService: registry + background detection + overrides (§13)
  - [x] `environment.rs` ShellEnvironmentService: sentinels, 5 s timeout, fallback (§12) — 4 tests
  - [x] PersistenceService wiring + `Orphaned` reconciliation on startup (§15.3)
  - [x] clap CLI (`run`/`info`/`dump`/`stats`); `forge-daemon info` verifies the real paths on macOS
- [x] **daemon e2e integration test** (temporary socket, real `client`, real shell PTY): AddProject→CreateShellSession→Attach→WriteInput→see output→Kill→Exited→Close, and create/remove worktree on disk — **2 tests**

> **Stage 2 verified:** `cargo test --workspace` → **160 tests, 0 failures**; `cargo clippy --workspace --all-targets` clean; `cargo fmt --check` clean.
> Note (fixed on 2026-08-22 during the audit): `KillSession` no longer depends only on the PTY loop's EOF nor leaves `signal` as best-effort. `reap_child` does `waitpid(WNOHANG)` in a 500 ms window and reports real `Exited { code }` / `Exited { signal }`; if the child survives the window, a backup thread blocks in `waitpid` so a zombie is never left behind. The kill by *process group* is verified empirically: the `sleep` grandchild dies.

## Stage 3 — GUI (Phase 0 spike) — ✅ COMPLETE ON macOS

`forge-tauri` opens a native window, connects to or starts `forge-daemon`, creates or reuses a shell and keeps a live replica of `client::CellGrid`. `theme tokens` concentrates the compatible host; `apps/tauri` holds the bridge off the render thread, the sidebar, the resizable split, tabs, input, clipboard and the passive renderer. `terminal-input` shares the pure mapper without dragging PTYs or the VT engine into the GUI.

- [x] Tauri 2 + Solid shell in `apps/tauri`.
- [x] macOS build/startup (`aarch64-apple-darwin`): `cargo build -p app` green; `forge` stays in the event loop without panicking. `runtime_shaders` is used so the optional Metal Toolchain component is not required.
- [x] 0.1 macOS shell: sidebar, resizable area, tabs and the custom panel work in the native app.
- [-] 0.1 Linux: Wayland and X11 are not validated in this closure by explicit decision; they remain gates before declaring Linux support.
- [x] 0.2 terminal: live PTY, keyboard input, resize, ANSI/RGB palette, flags, cursor, alternate screen, viewport copy and paste with bracketed-paste. Local smoke green with a shell, `vim` and `htop`.
- [x] 0.3 TUIs: the real `claude`, `codex`, `opencode` and `cursor-agent` reach `Running`, render non-empty cells and accept non-destructive input.
- [x] 0.4 reconnect: a second client rebuilds from a snapshot the output produced with no subscribers and starts receiving live deltas again.
- [x] 0.5 latency: the local smoke of 40 samples measured key byte → authoritative delta p95 = 10.05 ms (target ≤33 ms); the GUI keeps timestamps until the next render and shows its key → render p95. Automatic native key injection requires Accessibility on macOS, so that visual reading is enabled by typing in the app.

> **Gate result:** closed for the requested local macOS scope. `plan.md`'s cross-platform gate remains open solely because of the deferred Wayland/X11 validations; there is no known blocker for terminal, TUIs, reconnect or latency on macOS.

## Stage 4 — Closure

- [x] `scripts/dev` (build/run daemon, app, check, dump)
- [x] `cargo fmt --check` + `cargo clippy` clean across the whole workspace, including the GUI
- [x] `docs/` architecture + development (concise and precise)
- [x] CI (GitHub Actions: fmt + clippy + test)
- [x] `docs/` README (index) + domain/protocol/terminal/agents/worktrees/persistence; development expanded (2026-08-22)
- [x] `scripts/package-macos` / `package-linux` (§17). macOS: a `.app` bundle with `forge` and `forge-daemon` as **siblings** in `Contents/MacOS/` (the `runtime.rs:461` invariant), signing from the inside out, optional signing/notarization through env vars. Linux: `.deb` and AppImage with the runtime deps derived from the code.
- [~] Scenarios A–H: **C, E, F, G covered without the GUI**; A, B, D, H covered in their daemon half. What is missing is strictly GUI (auto-start from the app, `Cmd+Shift+]` <100 ms, "Disconnected" banner, visual nesting, picker).
- [ ] §20 budgets: **none measured**. Measurable without the GUI and still untested: `yes` throughput ≥50 MB/s, deltas/s ≤125, `AttachTerminal`→`AttachAck` with 10k scrollback <200 ms, idle daemon <0.5 % CPU. The memory ones (<15 MB/terminal, <50 MB in H) require sampling RSS, and the daemon runs in-process in the tests.

---

## Progress log

- 2026-08-22 — Full review of plan.md. Environment verified. execution.md created. Stage 0 started.
- 2026-08-22 — Stage 0 ✅ (workspace + `domain`, 9 tests). Stage 1 ✅ through 6 subagents in parallel: protocol(20), terminal-core(34), agents(19), git-service(19), persistence(11), test-support(14). Integration green.
- 2026-08-22 — Stage 2 ✅ (me): `client`(11) + the complete daemon (core/server/registry/terminal/environment/lockfile) + e2e test(2). Workspace total: **160 tests, 0 failures**, clippy+fmt clean.
- 2026-08-22 — Stage 4 partial: docs (architecture/development), CI, scripts/dev. Stage 3 (GUI) documented as a pending spike.
- 2026-08-22 — Complete backend docs: `docs/README.md` index, `domain.md`, `protocol.md`, `terminal.md`, `agents.md`, `worktrees.md`, `persistence.md`; `architecture.md` with principles P1–P7; `development.md` with the exact commands, a "where to touch" map and test notes.
- 2026-08-22 — the shell spike started: exact compatible revisions, a single copy of the shell in the graph, a runnable native macOS shell with the UI kit and a `CellGrid` panel. Gate 0.1–0.5 still open per the breakdown above.
- 2026-08-22 — Post-spike verification: `cargo build --workspace` and `scripts/dev check` green (197 tests, 0 failures); `scripts/dev app` opens the native event loop and was closed in a controlled way after the 10 s smoke.
- 2026-08-22 — macOS Phase 0 closed: the shell ↔ client ↔ daemon bridge, live shell, input/clipboard/resize, agent launching, automatic reconnect and key → render p95 telemetry.
- 2026-08-22 — Ignored local smokes green with the four installed agents, `vim`, `htop`, colour/alternate-screen and a byte → delta p95 of 10.05 ms. Wayland/X11 are deferred by explicit decision; Docker will not be used for this closure.
- 2026-08-22 — GUI redesign (§16): `theme tokens` becomes the
  single source of colour/geometry and **rewrites the UI kit's global
  `ThemeColor`**, so the borrowed components (tabs, resizable, scrollbars) and
  our own elements share a palette. The shell is recomposed into
  `title_bar` (native traffic lights + project/branch/session breadcrumb +
  toggles), a project rail with a Project → Workspace → Sessions tree
  (collapsible, status dots, activity, a `+` per workspace, inline kill),
  real session tabs, a filterable history panel and a bottom status bar.
  The "Phase status" tab is removed (it was spike scaffolding). The terminal's
  renderer merges contiguous cells with the same style into a single run and
  **measures** the cell advance with `ch_advance` instead of assuming 8×18 px;
  the same `CellMetrics` sizes the PTY and paints the grid. New modules
  `status_bar.rs`, `title_bar.rs`, `widgets.rs`; `sidebar.rs`/`history_panel.rs`
  were written but not declared in `lib.rs` (dead code) and are now alive and
  fixed (the session rows did not paint their title).
  `RuntimeCommand` gains `KillSession(SessionId)` and an explicit workspace in
  `NewShell`/`NewAgent`. Documented in `docs/ui.md`.

---

## plan.md ↔ code conformance audit (2026-08-22)

A complete review of the plan against the real tree, through 6 subagents in parallel. The state
declared before the audit **was not accurate**: the workspace did not compile, `cargo deny`
failed and several points of the plan were unimplemented despite being listed as closed.

**Gate after the audit:** `cargo test --workspace` → **268 pass, 0 failures**, 3 ignored
(smokes that require real agents/`vim`/`htop`). `cargo fmt --all --check` clean.
`cargo clippy --workspace --all-targets -- -D warnings` clean.
`cargo deny check` → `advisories ok, bans ok, licenses ok, sources ok`.

### Real bugs fixed

1. **Zombie processes and `signal` always `None`** (§11.3) — `daemon/src/core.rs`. See the Stage 2 note.
2. **Leaving the daemon did not kill the sessions** (§9.1) — the `KillSession` escalation lived in a
   detached thread that died with the process. `shutdown_sessions()` is now synchronous and covers
   `StopDaemon` and SIGTERM/SIGINT: a signal per group according to the kind → `kill_grace_ms` → SIGKILL.
3. **`RemoveWorktree` corrupted the state** — it deleted the directory and *then* tried to
   `DELETE` the workspace without deleting the `sessions` rows first (FK `ON DELETE RESTRICT`).
   The request failed with `Internal` with the directory already deleted, the workspace stayed in the
   model pointing at a dead path and no `WorkspaceRemoved` was emitted. The order is now DB and model
   first, disk last: if something fails, the worktree is intact and retryable.
4. **A binary that detection had rejected was being launched** (§7.5, DoD §29) —
   `resolve_program` picked the first candidate on the PATH without running the version probe and the
   spawn did not consult `detections`. With `agent` = Grok on the PATH, "New Cursor session" opened Grok.
   The launch now always uses the verified `Installed` executable, and if there is no cached result
   it is probed right then (outside the lock) instead of guessing.
5. **Home/End with the wrong encoding** (§11.6) — they emitted `CSI 1~`/`CSI 4~` (VT220) but
   the daemon announces `TERM=xterm-256color`, whose terminfo declares `khome=\EOH`/`kend=\EOF`.
   No ncurses program reacted to those keys.
6. **Mouse mode 1000 reported movement**, which §11.6 reserves for 1002.
7. **`process_group()` followed the foreground job** — `tcgetpgrp` returns the *foreground* pgid,
   so with `vim` running the pgid became vim's and `kill(-pgid)` would have killed vim
   leaving the shell alive. The pgid is now captured once, in `spawn`.
8. **`unique_slug` broke §14.2's cap of 64** when appending the collision suffix.

### Closed non-conformances

- **§22: there was not a single `tracing` span.** The plan's 11 were added. `request_name()`
  returns a `&'static str`, which makes it impossible for a payload to reach the log; there is a test that
  captures the whole subscriber and demands that neither the keystrokes nor the PTY output appear.
- **§8.2 `NewManagedWorktree`** returned `InvalidRequest "not wired in MVP UI path"` even though
  the plan says it is implemented in the daemon. Implemented.
- **§15.1/§22 the GUI had no `app.log` and did not read `config.toml`.** `apps/tauri/src-tauri` gains `paths`,
  `config` and `logging`: real rotation by size (5 × 10 MiB — `tracing-appender` only rotates by
  time), console only with `RUST_LOG` (a GUI opened from Finder has nowhere to print), and
  a full load of `[terminal]` that never kills the app.
- **`cargo deny check` failed** (`licenses` + `advisories`), that is, the CI job would have failed on
  its first run, breaching ADR-002/§32. `enum-iterator`'s `0BSD` added to the allow-list; the 7
  advisories are all `unmaintained` from the shell's pre-1.0 stack, documented one by one;
  `vulnerability` and `unsound` are still errors. `unknown-git` goes from `allow` to `deny`.
- **The CI's Linux job could not even compile**: zero system deps even though `the shell` drags in
  `wayland-sys`, `fontconfig-sys`, `freetype-sys`, `xcb`, `xkbcommon`, `clang-sys`. It did not run
  headless either despite there being two `#[test]`. Added apt deps + `xvfb-run` + timeouts + `--locked`.
- **§17 `scripts/package-macos` / `package-linux`** did not exist. Written.
- **§19 scenarios C, E, F, G** had no test. Written, with a hermetic harness
  (`crates/daemon/tests/common/mod.rs`): a file DB so the daemon can be restarted and an isolated PATH,
  with a guard that fails the *setup* if the login shell were to stop producing variables — without it
  the daemon falls back to §12's fallback, which widens the PATH, and detection starts seeing the real machine.
- ~15 inaccuracies in `docs/` corrected against the code (e.g. `protocol.md` denied the
  existence of `SessionRemoved`, and `worktrees.md` said that worktree reconciliation on
  startup was not implemented).

### Pending, with the user's decision

- **§9.1 spawning the daemon from the GUI** (`ui/src/runtime.rs:429`): no `setsid`, no minimal env
  (today it **inherits the GUI's complete environment, sensitive variables included**) and no redirection to
  `daemon.log`. It is the most concrete security gap left.
- **§23 paths outside the home** only emit a warning; requiring confirmation needs a new field in
  `Request::AddProject`.
- **§14.4 `RemoveWorktree` pre-checks** come back as text in the error message, not
  structured; the GUI cannot reason about each check. It requires touching `protocol`.
- **§22 `forge-daemon stats`** and **ADR-004 `dump --json`** are still unimplemented.
- **§9.1 an explicit SQLite flush** on exit (WAL leaves the commits durable, but the step is not there).
- **`LICENSE-MIT` / `LICENSE-APACHE` do not exist** even though `Cargo.toml` declares
  `MIT OR Apache-2.0`. It is needed before distributing (§32).
- **Cursor detection**: it skips `expect_substring` when the binary's name already contains the
  mark, because `cursor-agent --version` prints only a version number. A deliberate deviation
  from §13.1 — without it Cursor would be undetectable. `agent` (generic) never benefits from the bypass.
- **§16.6 shortcuts with the shell Actions/KeyContext, the command palette, scrolling with `FetchScrollback` in
  the view, a `ShapedLine` cache and a bundled OFL font**: those are Phase 8/9, not non-conformances.
- **The repository does not have a single commit.** All the work exists only as a working tree and
  the CI has never run, so the workflow is still without real validation.

- 2026-08-22 — Complete conformance audit (6 subagents). 8 real bugs fixed,
  §8.2/§9.1/§15.1/§22 non-conformances closed, scenarios C/E/F/G with tests, packaging written,
  `cargo deny` green for the first time. Workspace: **268 tests, 0 failures**, fmt/clippy/deny clean.

- 2026-08-23 — **Settings screen** (`ui/src/settings.rs`, a route with `cmd-,`). Sections
  *Agents* (a list of providers with their detection, re-detect and a default agent selector) and
  *General* (the base palette live, `theme.json` reload, config paths, daemon state).
  The `Dialog::AgentSettings` modal is absorbed into the route and disappears. Preferences persisted in
  `app_state` (`ui.default_agent`, `ui.theme_base`) through new `client::Client::{set_app_state,
  get_app_state, set_provider_executable}` wrappers and `RuntimeCommand::SetAppState`;
  `theme::configure_with_base` allows changing the palette without restarting. Workspace: **334 tests**, fmt/clippy clean.

- 2026-08-24 — **Idle sessions and the cost of the PTY loop.** Three fronts:

  *Activity clock.* `Session` gains `last_activity_at`, runtime state like `terminal_id`:
  it is never a column and when loading from SQLite it is rebuilt from `ended_at`/`created_at`, so there
  is no migration and no write per second of terminal traffic. It is advanced by the PTY's output
  (coalesced to ≤1/s by the PTY's own thread) and also by the client's input — keys, attach,
  resize — because a session you are typing into is not idle even if the child is silent.
  A `SessionUpdated` that only carries the bump is limited to one every 30 s per terminal.

  *Idleness policy* (`crates/daemon/src/idle.rs`, a pure function of (session, now, attached,
  already-warned); the sweep in `core.rs` applies the effects). Two different clocks: inactivity
  ("nobody is using it", reset by any activity) and age ("it has been open too long", which
  grows even while it works). Only inactivity can stop a session; age only warns.
  New config in `[sessions]`: `idle_warn_after_secs` (30 min, the only rule active by
  default), `idle_stop_after_secs` (0 = never; killing somebody's process is not inferred from a
  timer), `idle_stop_attached`, `idle_include_shells` and `long_running_warn_after_secs`. The
  stop uses the normal kill path (SIGTERM to the group, SIGKILL after `kill_grace_ms`), so the session
  ends up `Exited` and restartable, and it is always announced with a warning. The sidebar shows idleness
  as a faint `45m` from 10 minutes on.

  *Performance.* A PTY chunk cost three lock acquisitions (feed under the core lock,
  `has_subscribers` under the registry's, and the core again to emit or note), up to 125 times
  per second and per terminal, competing with every client request: it is now
  `Daemon::pump_terminal`, a single critical section. With no subscribers the floor between reads goes
  from 8 ms to 50 ms — nothing is sent to anyone, so a background agent's spinner has no reason
  to wake its thread 125 times per second. `external_agents::Cache` puts a 10 s TTL with
  a fingerprint of the scanned directories in front of the transcript sweep, which until now
  re-read up to 60 JSONL files per provider and directory on **every** `GetSnapshot` (that is, on every
  GUI reconnect). And two maps that grew without bound: `Inner::activity` disappears (its clock
  lives in the PTY thread) and `status_checks` is cleaned when the workspace is deleted.

  Workspace: **441 tests**, fmt/clippy clean.

- 2026-08-24 — **OpenCode profiles (§13.4).** OpenCode was the only installed provider that
  offered nothing in the profile editor: an empty `profile_fields` ever since profiles landed.
  It now declares four fields, verified against the real binary (1.18.21), not against the
  documentation: `OPENCODE_CONFIG_DIR` (a directory) moves `opencode.json` and the agents,
  commands and plugins next to it, but **leaves the credentials where they were**; `XDG_DATA_HOME`
  (a directory) is the real account switch, because `auth.json` and `storage` hang off
  `<data>/opencode`; `--model` takes a `provider/model` pair, not a bare alias; `--agent` picks
  which configured agent the TUI starts with. It is the first provider with **two** directory
  variables, so `ensure_profile_dirs` has a test that it creates both.

  `settings::suggested_value` derived the suggested directory's name from the *variable*
  (`_config_dir`/`_home` trimmed), which with `XDG_DATA_HOME` gave `~/.xdg_data-work`. It now
  derives it from the **provider** (`directory_base`): `~/.opencode-work` and `~/.opencode-data-work`, with
  no collision between the two directories of the same provider and without changing what
  `CLAUDE_CONFIG_DIR` and `CODEX_HOME` already suggested. It still does not branch on `provider_id` outside
  `agents` (P2).

  Cursor is left without fields on purpose: it documents no configuration switch, and
  suggesting an invented one is worse than suggesting nothing. OpenCode does not declare a `usage_source`
  either: there is no allowance to measure when each model is paid for at its own provider (`opencode stats` gives cost and
  tokens, not a percentage of a limit). Workspace: **496 tests**, fmt/clippy clean.

  *A known gap, not specific to OpenCode:* a profile that moves `XDG_DATA_HOME` also moves
  the transcripts the history panel scans, and `external_agents` resolves the directory
  from the daemon's own environment — those sessions do not show up. Claude has exactly the same
  gap with `CLAUDE_CONFIG_DIR` ever since profiles exist.

- 2026-08-24 — **The opencode history was empty: opencode moved to SQLite.** The panel showed
  not one opencode session, and it was not a filtering bug: `discover_opencode` only read
  `~/.local/share/opencode/storage`, the JSON tree of one file per session. opencode 1.17 started
  writing **a single SQLite database** (`opencode.db`) and stopped touching that tree. On this machine the
  tree is frozen in February 2026 and the database has 381 sessions — 22 of this very
  repository. The scanner read real, correct history from six months ago, which is worse than
  reading nothing, because it looks like it works.

  A new `crates/daemon/src/opencode_db.rs`: it opens `opencode*.db` in **read-only** with
  `query_only = ON`, checks the columns with `PRAGMA table_info` before selecting them (so an
  older or newer schema degrades instead of breaking), and pulls per directory the top-level
  non-archived sessions, their turns, their subagents, the last agent message that was prose
  (discarding `synthetic` and non-`text` parts, as the JSON reader already did) and the `modelID`
  that wrote it. A detail you only see by looking at the data: the `session.model` column arrived late
  and is **NULL in 302 of the 381 rows**, so the reliable model is the last message's, not the
  session's.

  `external_agents` now reads **both layouts**: the database first, the tree afterwards, and a
  session already listed from the database is skipped when going through the tree — an updated machine has the
  same session in both places and the tree's copy is the one that stopped being updated. Without a database
  (an old opencode) nothing is lost. The directory is compared against the path Forge knows *and*
  its canonical form, because the column stores the path opencode was started with and
  `/tmp` versus `/private/tmp` is the difference between all the history and none. For a
  session from the database there is no per-session file: `transcript_path` is the database itself.

  Reference: Orca solves this the same way (`src/main/ai-vault/session-scanner-opencode-sqlite*.ts`) —
  database and tree read at once, deduplicated by session id, with the database as the source of truth.

  Verified against the machine's real data: 0 → **13 opencode sessions** in two
  projects, with title, model, turns, subagents and preview. The opencode pass costs **37 ms**
  over a 890 MB database (the rest of the scan is Claude's JSONL files), below the 10 s TTL
  of `external_agents::Cache`. Workspace: **508 tests**, fmt/clippy clean.

- 2026-08-24 — **Branches, remotes and worktrees from the GUI.** The worktree backend had been
  complete and tested for months (`CreateWorktree`, `RemoveWorktree`, `git_service::create/remove`), and the GUI
  **could not call it**: `RuntimeCommand` did not have a single variant. Every worktree the user saw
  had been created by hand and discovered by `rescan_worktrees` when the daemon started. That, and
  the fact that `fetch` did not exist in the whole workspace, is what is closed here. The full plan is
  in [`docs/plan-branches-and-worktrees.md`](./docs/plan-branches-and-worktrees.md).

  *The model.* Choosing a branch and creating a worktree are **the same action**. In a
  worktree-first product (P4) "switch me to that branch" means "open me the workspace that has it", so there
  is no `git switch` anywhere and that is deliberate: changing the branch under a directory changes the
  ground under every agent already running in it. Conductor, Crystal and Vibe Kanban arrive at the same place.
  The dialog (`ui/src/branch_picker.rs`, `cmd-shift-n`) merges three sources into one list: **Create**
  (what was typed, when it is not already a branch), **Local** and **Remote**. A local branch already *checked out*
  appears **disabled, naming the worktree that holds it** — `GitError::Conflict` shown
  before you can hit it, not after.

  *Remote branches without `--track`.* `create` does not change its signature. With `branch = "feature/x"` and
  `base = Some("origin/feature/x")` it falls into `worktree add -b … origin/feature/x`, and since the starting
  point is a tracking branch, git configures the upstream **on its own** (`branch.autoSetupMerge`).
  Verified against real git, not against the documentation: `git-service` has the test and
  `scenario_branches.rs` repeats it end to end.

  *The fetch is asynchronous, and that is the important decision.* `FetchRemote` answers `Ack` when the
  fetch **starts** and the result arrives as `RemoteRefsUpdated`. The reason is not in the daemon:
  `runtime_loop` drains **a single channel** carrying the mutations *and* `RuntimeCommand::Input`
  — every keystroke —, with a blocking `client.request()`. A synchronous fetch would have frozen typing
  for between 300 ms and 30 s, and the bug would have read as "the terminal sometimes hangs". Two fetches of the
  same project are **coalesced**, not queued (`Inner::fetching`).

  *Network: a documented deviation from ADR-008.* `run_git_network` with its own budget
  (`[git] fetch_timeout_secs`, 120 s by default), because a single number does not serve `rev-parse` and
  `fetch` at once. And above all: **`GIT_TERMINAL_PROMPT=0` does not reach `ssh`**, which reads the TTY
  directly, so a key with a passphrase and no agent loaded blocked until the timeout. We
  add `GIT_SSH_COMMAND` with `BatchMode=yes`, `StrictHostKeyChecking=accept-new`,
  `ConnectTimeout=10` and askpass pointing at `/usr/bin/false`. A failure now reaches the GUI as
  `Permission denied (publickey)`, not as `Timeout`. Nothing in `git-service` writes to a remote:
  `push` is absent and that is not a gap — a daemon that can push is a daemon that can lose
  somebody's work from a background thread.

  *`RefreshProject` stopped lying.* It only re-ran `discover_root`; it now reconciles the
  project's worktrees (`rescan_project_worktrees`, extracted from the startup sweep) and re-reads
  the branch and the state of each workspace. A worktree created in the terminal shows up when the user asks
  to look — which is exactly when they ask, because that is the state in which Forge and the terminal disagree.

  *`Workspace.status`* (`dirty/ahead/behind/measured_at`) is **runtime state, never a column**,
  like `Session::terminal_id`: no SQLite migration. `measured_at: None` means "nobody has
  looked", which is not the same as clean — the sidebar draws *nothing*, not a green tick.

  *Provisioning the worktree.* A managed worktree lives under `worktrees.root`, far from the repo, so
  it is born without `.env` or anything untracked: the agent fails on the first command and the failure does not point
  at the missing file. `[worktrees] copy` and `setup_script` fix it after `worktree add`. It is
  **best effort on purpose**: the checkout already exists, so a script that exits with 1 produces a
  warning, never a refusal. `copy` only accepts relative paths without `..` and plain files —
  `copy = ["../../.ssh/id_rsa"]` cannot duplicate a secret and `["node_modules"]` cannot fill the disk.

  *Surface.* `+`/`⑂` on the project row, context menus on project and workspace (New
  worktree, Fetch, Rescan, Copy path, Remove/Forget worktree), a **Branches** group in the command
  palette to jump between checkouts, `cmd-shift-n` (the same key as Conductor), and deletion in
  two acts: the first press asks without `force`, the daemon answers with the pre-checks that
  failed and the second — with those reasons on screen — forces.

  *Not included, said explicitly:* `push`, PRs, diff, `auto_fetch_secs` on by default
  (network traffic nobody asked for, and in a repo with expired credentials a failure that repeats
  forever), and `ChildWorkspacePolicy::NewManagedWorktree`, which is still rejected even though
  `create_managed_worktree` is already factored out to serve it.

  Workspace: **560 tests** (from 508), fmt/clippy clean. The 10 new ones in
  `crates/daemon/tests/scenario_branches.rs` use a second real repository as `origin` by local
  path — a first-class git transport — so the fetch path is the real one and nothing
  touches the network.

- 2026-08-24 — **Choosing a branch ends up inside the branch.** A review of the picker's complete
  path: the fetch backend was fine (async, coalesced, `run_git_network` with `BatchMode` and
  askpass disabled), but the navigation stopped one step short of the end in three places.

  *Creating a worktree did not take you to it.* `CreateWorktree` is acked before the worktree exists,
  so the dialog closed and the user was still in the previous terminal, with a new row in the
  sidebar they had to find. The shell now stores the requested branch (`pending_worktree`) and enters
  the workspace when the broadcast arrives: the most recent session if there is one, a shell otherwise.
  Matching by branch is valid because the picker never offers a branch already *checked out* — that row comes back
  disabled, naming the worktree that holds it. A rejection from the daemon clears the intent.

  *A click on the workspace row only folded it.* With the keyboard, switching branch took you inside; with
  the mouse, it unfolded a tree for you. The click becomes `ShellAction::FocusWorkspace` — the same path as the
  palette — and folding is the chevron, which is now its own target with `stop_propagation`. The three
  routes (palette, sidebar, freshly created worktree) share `AppShell::focus_workspace`, which also
  unfolds the row: arriving somewhere and finding it closed hides the session you just opened.

  *A real bug: `enter` could do nothing.* The picker painted with the clamped index and confirmed with
  the raw one. The list changes shape **while it is open** — a `fetch --prune` that deletes a remote
  branch shortens it and the re-list arrives by event — so the highlighted row and the row `enter`
  acted upon stopped being the same and `enter` returned silently. It is centralised in
  `branch_picker::selected_index`, used by the render, the guard and the confirm, and the state is re-clamped
  on receiving `Branches`.

  *And the click's side effect:* a row that looks like a folder gets double-clicked, so a
  `NewShell` on an empty workspace suppresses another for 750 ms (`last_shell_request`). It is bounded
  in time and not by a flag on purpose: a request the daemon never answers cannot leave the row dead.

  Five new tests in `app_shell` (create→enter, a list that shrinks, focus with and without a previous session,
  double click), verified red before the fix. Workspace: **565 tests**, fmt/clippy clean.

  *Still open, seen in the same review:* the fetch coalescing is keyed by `project_id` and not
  by `(project_id, remote)`, so asking for `upstream` while `origin` is running acks without fetching anything
  (latent: the GUI always sends `remote: None`); the picker's filter is a subsequence with no scoring,
  so an exact name can end up below a more recent branch; the palette's *Branches* group
  sorts by the workspace's `created_at` instead of by activity; and you cannot paste into
  the picker (`on_dialog_key` only consumes `printable_text`).

- 2026-08-25 — **Knowing which agent needs you.** The product exists to run
  many agents at once, and that task fails in a specific way: not knowing
  which one has stopped to ask. All the chrome answered *is this alive?*;
  none answered *does this want me?*.

  *The signal already existed and nobody read it.* Agent CLIs ring the bell
  when they finish a turn, `alacritty_terminal` reports it and the daemon emits
  `TerminalBell` by **broadcast**, for every terminal. The GUI threw them all
  away: `Store::terminals` only keeps the *attached* replica, so putting the
  flag in a `CellGrid` meant the only session able to carry it was the
  one you were already looking at — precisely the one that by definition is not asking to
  be found. `Store` now carries `pending_bell` and `pending_activity` next to
  the replicas. Along the way it fixes the amber wash of *unread activity* in the
  tabs, dead forever for exactly the same reason:
  `TerminalActivity` is only sent to the **non**-subscribed.

  *`ui::attention`* is all the interpretation on top. Agents only — a shell's `\a`
  is usually an autocompletion beep, and a rail that lights up because
  zsh could not complete a name teaches you to ignore the only mark that means
  *come here*. Never the attached session: arriving spends the mark (`attach_terminal`
  clears both sets). And the longer wait first, because the forgotten agent
  is the one the group exists to surface; the duration comes from
  `last_activity_at`, which the bell itself bumped.

  *Surface:* a **NEEDS YOU** group pinned at the top of the rail (**absent**, not
  empty, when nothing is waiting), a `needs_you` tint + a left rule +
  a ringed `!` on the session row, the same plus an underline on the tab, and
  the glyph on any **folded** workspace or group — folding cannot be a
  way to hide a question. `needs_you` is spent here and nowhere
  else, which is what keeps the colour meaning one single thing.

  *Chrome diet, in the same step.* Out go the permanent `p95` and the second
  connection dot of the status bar: a window does not need two lights for one
  fact. Both survive in the tooltip of the daemon indicator, in the title
  bar, which already carried it. The bar goes from `faint` to `muted`/`text`
  (`faint` over `rail` measures ~3:1 and those are numbers to act on). `TAB_MIN_W`
  96 → 120: a status glyph, a provider mark and a close button eat 47px
  before drawing a single character. And the tab strip drops to `bg` so the
  active one — which keeps `term_bg` — reads continuous with the panel: in Gruvbox
  `rail` and `term_bg` are the same value, so active and inactive were
  told apart only by the text colour.

  *The history filter stops being a button that cycles.* `⚟` went through
  all → agents → shells: the button looked identical in two of its three states
  and the only thing that named the active one was the count line. It moves to the same
  segmented control the scope already used one row above — the scope bounds
  *where* the timeline reaches, the filter *what* it is made of — and the count
  stops repeating the name.

  *A dead session stops being "no session".* The panel fell back to the generic
  empty state ("No session attached") when the terminal no longer existed, throwing away
  the name, the reason and the two things you can do. It now has its own state:
  what died and how (`state_label`), the reason when there is one, that
  the checkout is intact, and Restart / Close. The exit code also rises to
  the rail's row — a lone ✕ forces you to open the session to learn the only thing
  you wanted to know about it.

  Workspace: **581 tests** (from 565), fmt/clippy clean. Six new: two in
  `client::store` (the bell of a non-attached terminal is the one asking for the user;
  closing the session forgets its mark) and four in `ui::attention`.

  *Still open, from the same design review:* the command palette's groups
  do not say what `enter` will do nor which worktree each row belongs to; the reason why
  a branch comes back disabled in the picker is trailing text, which is the first thing
  to be truncated, and the picker does not highlight the substring you matched; a provider that is not
  installed says "not on PATH" without saying where it looked nor offering to locate it; and
  the first launch still paints two competing empty states (rail and panel).

- 2026-08-25 — **The rail, legible at a glance.** Four levels (workspace →
  project → checkout → session) painted almost all the same: every project with the
  same folder icon, every checkout in grey `text_xs`, and the number of
  sessions reduced to a badge with one digit. A tree like that is not scanned, it is
  read whole.

  *Its own header.* The rail is titled **Projects**, pinned outside the scroll
  area, with the two creation gestures next to it: a folder (a new organisational
  Workspace) and `+` (add a project). They used to share a dropdown, and "add
  a folder I already have" and "create a group to put folders in" are the two
  first things somebody who opens the app for the first time does — a menu
  you have to open to discover it carries both is a menu that gets opened once.

  *The project's identity.* Each project carries a **square tile** with its
  initials over a tone derived from a stable hash of its **name**: the same
  project comes out the same colour on every start, and also after removing it
  and adding it again (a new `ProjectId` would repaint it). The tones come from
  the ANSI block and not from the semantic tokens: identity is the only colour of the
  chrome that means **nothing**, and spending `green`, `amber` or `needs_you` on a
  square that never changes is exactly how a status colour stops
  being read. No red, which next to a failed session reads as part of the failure.

  *The checkout says what is happening inside.* A header glyph: `attention_marker`
  if an agent is asking, otherwise a dot in the colour of the liveliest session. Here
  life beats failure — the opposite of the attention rule — because a failed
  session keeps its row and its `✕` until you close it, and letting it paint the
  checkout red would be a permanent alarm about agents that work fine. The
  branch keeps the blue the name used to have, and the repository's own checkout is
  labelled `primary`: any other row at that level is a branch somebody opened on
  purpose, and this is the one that cannot be deleted.

  *Folding moves to its own row* — "2 agents", with the chevron — instead of
  being a chevron on top of the checkout's row. The checkout's row is a
  place you **go to**, and the control that folded it shared its surface with
  the click that travels there. And the count now says what of: "3" answers neither
  how much is running nor whether any of it is an agent.

  *A clock on every session row.* Alive, it is still idle time and it is still
  silent until `IDLE_LABEL_AFTER` (no running session carries a permanent
  stopwatch); finished, how long ago it ended, which is the only thing left to
  say about it and what separates the agent that stopped an hour ago from the one that stopped in
  March. `ROW_H` 26 → 28: at 26 the tile, the status glyph, the name and the badge
  were crammed onto one line.

  Workspace: **590 tests** (from 581), fmt/clippy clean. Nine new: five
  in `ui::widgets` (initials that respect how a repository is named, a stable tone
  insensitive to case, the identity palette touching neither status hues nor repeating itself)
  and four in `ui::sidebar` (the folding summary, which state represents a checkout, and the two
  halves of the clock).

- 2026-08-25 — **One icon per project.** The tile's initials are a
  *fallback*, not the design: they identify, but you have to read them.
  `Project::icon` (`Option<String>`) becomes a field of the project, not a GUI
  preference — it travels with the project, it is persisted in its row and it reaches every
  client through `ProjectUpdated`, just like a rename.

  *End to end:* migration 5 (`PROJECT_ICONS`, a nullable column, no
  backfill: null is what was already drawn), `Request::SetProjectIcon`,
  `Client::set_project_icon`, a handler in `core.rs` and a `RuntimeCommand`. The
  string is opaque on purpose, not an enum of known icons: the point of the
  feature is for the user to choose the mark, and an enum would make that a code
  change. The rule about what can be stored lives once, in
  `domain::is_valid_icon`: a drawable mark, without spaces, up to
  `MAX_ICON_CHARS` (8, because an emoji is rarely one `char` — a flag is
  two, 👨‍💻 is three and a family seven). It does not try to answer "is this an
  emoji?": a letter, a rune or a `⌘` are legitimate marks and the Unicode
  question has no answer that survives a font change.

  *The daemon does not trust the GUI.* Empty clears it — that is how the initials come back — but
  a label is **rejected** with `InvalidRequest` instead of clearing silently:
  deleting the icon the user was trying to set is the only
  outcome they would not expect. `InvalidIcon` is a type and not `()` so that the
  sentence explaining the rule lives with the rule; the GUI paints that same sentence
  under the box.

  *The picker* (`Project Icon…` in the `⋯` menu) is a grid **and** a box. The
  grid makes choosing one click with nothing to memorise; the box
  exists because the grid is a starting point and the field accepts anything
  the system knows how to draw. The first cell is *initials* — the way
  back, drawn as what it gives you — because a picker that can only set is
  a trap. A click on a cell resolves the dialog: asking for a second button
  would turn the grid into a list of suggestions. And a key **replaces**
  instead of appending: an emoji behind another would build a string the daemon
  rejects.

  *Where it is seen:* the rail's tile (the chosen glyph goes over `surface`, without the
  derived tone — an emoji brings its own colours and a tinted background would be two
  identities fighting over 18px) and, as an echo, next to the project's name in
  the history panel's header, where a project without an icon keeps
  exactly the row it had.

  Workspace: **600 tests** (from 590), fmt/clippy clean. Ten new: three in
  `domain` (what a mark is, what a label is, and that clearing and rejecting are
  not the same), three in `daemon` (it is stored and cleared, a label does not delete
  what was there, a non-existent project), one in `persistence` (a v4 database upgrades without
  an icon and accepts one afterwards) and three in `ui` (the command goes out, a label does not
  close the dialog, and every cell of the grid is storable).

- 2026-08-25 — **Three corrections to the icon picker, seen in use.**
  *The grid left an empty band:* fixed-width cells (7×40px = 304 of the card's
  408 usable px), so every row ended a hundred points before the
  box below — and an empty strip of card to one side reads as a
  row that did not get drawn, not as margin. The cells now share
  the width (`flex_1`) and `ICON_CHOICES` holds a whole number of rows (24 in
  rows of 8, with a test that holds it).

  *Typing two letters left one:* the box deleted and rewrote on every
  key. It was a defence against building a string the daemon would reject,
  and it was in the wrong place: two letters are a legitimate mark. It is
  written by **appending**, with the `domain::MAX_ICON_CHARS` ceiling applied where
  the key lands, so what cannot be paid for cannot even be typed.

  *Pasting did nothing:* `PasteTerminal` is bound to the terminal's key
  context, so with a modal in front ⌘V went nowhere — bad
  treatment for the one field whose content is normally pasted (Raycast, the
  system picker). All text entering a dialog now goes through
  `insert_into_draft`, keyboard or paste. In the icon box, pasting **replaces**
  (a pasted mark is a complete answer, and appending it to a box that already
  has one is exactly how a paste ends up looking like it did nothing) and the
  spaces and newlines of the paste are dropped instead of invalidating it.

  Workspace: **602 tests** (from 600), fmt/clippy clean.
