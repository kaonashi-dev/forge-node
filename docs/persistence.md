# Persistence

SQLite stores **metadata only** (ADR-009): projects, workspaces, sessions and
their graph, context envelopes, provider overrides and opaque app state. It
never stores the terminal stream — scrollback lives in bounded daemon memory —
and never stores runtime-only identifiers such as `terminal_id`.

Crate: `crates/persistence`. Plan references: §15.

## Database

- Path: `<data-dir>/app.db` (`~/Library/Application Support/Forge/app.db` on
  macOS, `$XDG_DATA_HOME/forge/app.db` on Linux — `directories` lowercases the
  application name there). The daemon integration tests use
  `Db::open_in_memory()`.
- Per-connection pragmas, set in `Db::init` (shared by `open` and
  `open_in_memory`): `journal_mode = WAL`, `busy_timeout = 5000` and
  `foreign_keys = OFF`; migrations then run; only afterwards is
  `foreign_keys = ON` set. The deliberate FK-off window exists because
  migration 2 rebuilds tables, which foreign keys would block.
- One `Db` handle, owned by the daemon core and used under its lock; all
  access is synchronous `rusqlite`.

## Migrations (`migrations.rs`)

Versioned by `rusqlite_migration` through SQLite's `PRAGMA user_version`
(the plan's conceptual `schema_version` table is not created by hand). Rules:

- **Append only.** Add a new `M::up(...)` to `migrations()`; never edit or
  reorder an existing one, or already-migrated databases will diverge.
- Ten migrations so far: `INITIAL_SCHEMA`, `REFERENTIAL_ACTIONS`,
  `PROJECT_GROUPS`, `AGENT_PROFILES`, `PROJECT_ICONS`,
  `WORKSPACE_DISPLAY_NAMES`, `SESSION_LAUNCH_COMMAND`, `WORKTREE_SHARES`,
  `SESSION_BASE_COMMIT` and `PROFILE_CONFIG_DIR`.
- `PROJECT_ICONS` adds a nullable `projects.icon`, and null is what every
  existing project keeps: no icon means the UI draws initials, which is what it
  drew before the column existed. The column carries no `CHECK` — what makes a
  string usable as a mark is a Unicode question (`domain::is_valid_icon`) that
  SQLite cannot ask.
- `WORKSPACE_DISPLAY_NAMES` adds a nullable `workspaces.display_name` for the
  human label independent of the git branch; null keeps every existing row
  leading with its branch.

## Schema (v1 — `INITIAL_SCHEMA`)

```sql
projects          (id PK, name, root_path UNIQUE, git_root, created_at, last_opened_at)
workspaces        (id PK, project_id → projects, kind, path UNIQUE, branch,
                   managed_by_app, created_at)
sessions          (id PK, workspace_id → workspaces, kind, role,
                   parent_session_id → sessions, root_session_id, agent_provider_id,
                   user_title, terminal_title, last_state, last_exit_code,
                   created_at, ended_at)
context_envelopes (id PK, source_session_id → sessions, target_session_id → sessions,
                   summary, instructions, artifacts_json, git_context_json, created_at)
provider_overrides(provider_id PK, executable_path)
app_state         (key PK, value)
```

## Schema (v2 — `REFERENTIAL_ACTIONS`)

Migration 2 rebuilds `workspaces`, `sessions` and `context_envelopes` with
explicit referential actions, so deleting a row can no longer leave a dangling
reference or silently orphan a graph edge:

| Foreign key | Action |
|-------------|--------|
| `workspaces.project_id → projects` | `ON DELETE RESTRICT` |
| `sessions.workspace_id → workspaces` | `ON DELETE RESTRICT` |
| `sessions.parent_session_id → sessions` | `ON DELETE SET NULL` |
| `context_envelopes.source_session_id → sessions` | `ON DELETE CASCADE` |
| `context_envelopes.target_session_id → sessions` | `ON DELETE SET NULL` |

It also adds the indexes `idx_workspaces_project`, `idx_sessions_workspace`,
`idx_sessions_parent`, `idx_sessions_root`, `idx_envelopes_source` and
`idx_envelopes_target`.

Column conventions:

- IDs are the UUID string form; timestamps are RFC-3339 `TEXT`.
- Domain enums are stored as explicit, stable text tags (`kind`, `role`,
  `last_state`) written by the repository, not by `serde` — renaming a Rust
  variant is a schema change. `SessionRole::Custom(name)` is stored as
  `custom:<name>`.
- `SessionState::Exited { code, signal }` keeps `code` in `last_exit_code`;
  `signal` has no column and reloads as `None`. `Failed { reason }` reloads
  with a placeholder reason. Both are acceptable because live sessions are
  reconciled to `Orphaned` on startup anyway (below).
- `sessions` has **no `terminal_id` column** by design, and no
  `last_activity_at` one either: both are runtime state describing a PTY that
  cannot outlive the daemon. A loaded session's `last_activity_at` is its
  `ended_at`, else its `created_at`.
- `artifacts_json` / `git_context_json` are JSON blobs of the domain types.

## Schema (v4 — `AGENT_PROFILES`)

```sql
agent_profiles    (id PK, provider_id, name, executable_path, config_dir,
                   args_json, created_at)
                  UNIQUE (provider_id, name COLLATE NOCASE)
sessions          + agent_profile_id
```

Launch profiles (§13.4), the saved form of a shell wrapper like
`CLAUDE_CONFIG_DIR=~/.claude-personal claude --model opus`. Four deliberate
choices:

- `provider_id` is **not** a foreign key: providers are descriptors in
  `crates/agents`, not rows.
- `args_json` is a JSON list, not a child table. Nothing queries inside it and
  its order is meaningful, because arguments are positional.
- `config_dir` is stored **as typed**, relative paths included: what a relative
  path is relative to is the launching user's home, which is a runtime fact and
  not a stored one (`domain::AgentProfile::resolve_config_dir`).
- `sessions.agent_profile_id` is **not** a foreign key either, and nothing
  cascades: a session in the history must survive the deletion of the profile
  that launched it. The GUI falls back to the provider's name.

Unlike sessions, profiles are user configuration and always survive a restart.
Migration 10 (`PROFILE_CONFIG_DIR`) is where `env_json` became this column: a
profile could set any variable it liked, what every real one set was the
provider's config directory, and the backfill keeps that one value.

## Schema (v8 — `WORKTREE_SHARES`)

```sql
worktree_shares   (id PK, project_id FK→projects ON DELETE CASCADE, path,
                   strategy, enabled, position, created_at)
                  UNIQUE (project_id, path)
```

Which files a project shares between its workspaces (§14.2). Three choices,
for reasons that mirror `AGENT_PROFILES`:

- `strategy` is JSON (`{"kind":"clone"}`, `{"kind":"run","command":"pnpm
  install","timeout_secs":600}`), not a tag column: nothing queries inside it,
  and a `Run` rule carries a command and a budget rather than a name.
- `ON DELETE CASCADE`, unlike the bare `REFERENCES` of v1 that migration 2
  exists to correct: a removed project must not leave rules behind.
- `position` is rewritten from the order of the set the client sends. The GUI
  edits a list; the table records that list, not a client's opinion of what
  index each row should hold.

A project with **no** rows behaves exactly as before this migration: the
global `[worktrees] copy` / `setup_script` from `config.toml` is synthesised
into rules and applied. One engine, two sources — and a project with rules
ignores the global list entirely, because merging them would make it
impossible to remove an inherited entry.

## Repositories (`repositories/`)

One repository per table, obtained from the handle: `db.projects()`,
`db.workspaces()`, `db.sessions()`, `db.context()`,
`db.provider_overrides()`, `db.agent_profiles()`, `db.app_state()`. Each maps every domain field to
and from its columns; the daemon loads all rows into memory at startup and
writes through on every mutation (`upsert`/`delete`).

## Startup reconciliation (§15.3)

A PTY never survives the daemon, so every session row a new daemon finds is
already dead. `sessions.persist_history` picks what that means, before any
state is loaded:

```sql
-- persist_history = false (default), Db::purge_sessions
DELETE FROM sessions

-- persist_history = true, Db::reconcile_orphaned
UPDATE sessions SET last_state = 'Orphaned', ended_at = now
WHERE last_state IN ('Starting', 'Running')
```

The default drops the history: without it the tree grows a pile of dead
sessions nobody restarts, one per app run. Envelopes cascade with their source
session; projects, workspaces, provider overrides and app state are untouched.

With the flag on, a session that was live comes back `Orphaned` and the user can
`RestartSession` it (fresh terminal, no scrollback) or close it. Reconciliation
is the only legitimate writer of `Orphaned`.

## What is *not* persisted

| Data | Where it lives instead |
|------|------------------------|
| Terminal grid, scrollback | daemon memory, bounded by `terminal.scrollback_lines` |
| `TerminalId`, process group, PTY handles | `TerminalRuntime` in the daemon core |
| Detection results | in-memory daemon cache (`Inner.detections`); recomputed at startup |
| Resolved login-shell environment | cached in the daemon for its lifetime |
| Client subscriptions, "behind" flags | `daemon/src/registry.rs` |
| GUI layout | opaque `app_state` key/values (the daemon never interprets them) |

## Tests

`crates/persistence/src/tests.rs`: migrations apply cleanly, each repository
round-trips every field, foreign keys are enforced, deleting a session cascades
to its context envelopes, `reconcile_orphaned` only touches live states,
`purge_sessions` clears the graph and its envelopes but keeps the workspace, and
one on-disk case proves the database survives a reopen. `db.rs` carries a
further test for the pragmas themselves. Do not hand-maintain a count here —
`cargo test -p persistence` is the source of truth.
