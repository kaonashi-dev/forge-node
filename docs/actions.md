# Project actions

A project action is a one-click command a project runs in its workspaces: the
dev server, the tests, the linter. Clicking it opens a visible Forge terminal
running the command; while that process is alive a single-instance action
cannot start again in the same workspace, and the click focuses the live one.

## Model

- **`ProjectAction`** (`crates/domain/src/action.rs`) is per project and shared
  by every workspace of it: main and each worktree may run their own `dev`.
  Persisted in `project_actions` (migration 15) and cached in `Inner` so a run
  decides under the lock without a query.
- **A run is a `SessionKind::Task` session.** `$SHELL -l -c "<command>"` from
  the action's `cwd`, so the session ends when the command does: `Exited
  { code }` is the action's own result, the output stays on screen, and there
  is no prompt afterwards. Tasks follow the shell idle rule and receive
  SIGTERM on stop, which a server such as `docker compose up` treats as a clean
  shutdown.
- **`Session.action: Option<ActionRun>`** is runtime-only, like `terminal_id`:
  the action id, a copied label, the resolved port and the readiness. A daemon
  restart purges sessions (or orphans them under `persist_history`), so nothing
  counts as running afterwards.

## Running

`RunAction { workspace_id, action_id }` answers one of:

| Answer | When |
|--------|------|
| `SessionCreated { session_id, terminal_id }` | spawned |
| `ActionAlreadyRunning { session_id }` | single-instance and live in that workspace; focus it |
| `ActionQueued { session_id }` | waiting for its dependencies; no PTY yet |

The single-instance check and the session's creation are **one critical
section**: two clicks or two windows cannot start it twice. Resolving the
`cwd` (canonicalized, inside the checkout) and choosing a port (a bind probe)
happen before that section, off the lock.

- **Editing** an action changes the next run, not the live one. `RestartSession`
  on a `Task` re-resolves the action as it is now.
- **Deleting** an action leaves its live sessions running, untagged; a restart
  re-runs the command they last ran.
- **`single_instance = false`** starts a new session every time.

### Ports

`port = { base, per_worktree }`. With `per_worktree` the daemon takes the first
free port in `base..base+50` that no live action session holds; otherwise
exactly `base`. A fixed port that is taken still launches, with readiness
`PortInUse` so the GUI warns. The port is injected as `PORT` and
`FORGE_ACTION_PORT`, and substituted for every `{port}` in the command, since
many servers ignore `PORT` (`vite --port {port}`).

### Readiness

| `ready` | Probe |
|---------|-------|
| `none` | `Unprobed`; running is all there is to know |
| `port { timeout_secs }` | TCP connect to `127.0.0.1:<port>` every 500 ms on its own thread |
| `output { pattern, timeout_secs }` | case-insensitive **literal** substring of the output, ANSI stripped |

The output watch is owned by the task's PTY thread, handed to it before the
first read, and scans each batch *after* `pump_terminal_batch` returns — never
under the core lock. It keeps a 4 KiB tail, and is dropped the moment it
settles, so a ready server costs nothing per batch afterwards. A pattern is not
a regex: a repository-supplied pattern must not be able to cost more than a
scan. `Ready { url }` takes the first `http(s)://` URL on the matched line,
else `http://localhost:<port>`. Past the deadline the readiness is `TimedOut`;
the process may still be serving.

### Dependencies

`depends_on` lists actions of the same project (acyclic, at most 8, at most 4
deep — refused at `SetProjectActions`). Running a dependent first runs or
reuses each dependency in the same workspace, then parks the dependent as
`Starting` with `Waiting { on }` and no process. A probed or `Dev` dependency
lets it go once it serves; any other (a migration) once it exits 0. A
dependency that exits non-zero, times out or is closed fails the dependent
with a reason. The waiter blocks on a bounded channel that session state and
readiness changes ping, so it does not poll; `KillSession` on a waiting task
fails it ("stopped before it started").

## Suggestions

`SuggestActions { workspace_id }` is a bounded, synchronous read of the
workspace root (`actions::detect`): `package.json` scripts (runner from
`packageManager`, else the lockfile), `Cargo.toml`, `Makefile`/`justfile`
targets that take no arguments, a compose file, `pyproject.toml`. Each file is
`metadata()`-checked before it is opened, capped at 256 KiB and never followed
through a symlink; a broken manifest suggests nothing rather than failing.
Nothing is saved until the dialog's Save sends `SetProjectActions` with the
whole set.

## `.forge/actions.toml`

A repository can **offer** actions; it can never run one. The file is parsed
into suggestions; importing one copies it into `project_actions` with
`source = repo { file_hash }`, the SHA-256 of the bytes the user approved. The
saved copy is what runs, so a checkout, pull or branch switch changes what is
offered, never what a click executes. When the file's hash differs from a
saved copy's, that action is listed as `drift` for review; it never updates by
itself and still runs its approved copy.

```toml
[[action]]
label = "dev"
command = "bun run dev -- --port {port}"
kind = "dev"            # dev | test | lint | build | custom
pinned = true
port = { base = 5173, per_worktree = true }
ready = { kind = "port", timeout_secs = 60 }
depends_on = ["db"]     # labels within the file

[[action]]
label = "db"
command = "docker compose up db"
ready = { kind = "output", pattern = "ready to accept connections", timeout_secs = 60 }
```

Unknown keys are an error shown in the dialog, so a typo is not silently a
default. The file is capped at 64 KiB.
