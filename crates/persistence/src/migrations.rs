//! Schema migrations (§15.2).
//!
//! Versioning is owned by [`rusqlite_migration`], which tracks the applied
//! migration count in SQLite's `PRAGMA user_version`. The plan's conceptual
//! `schema_version` table (§15.2) is therefore *not* created by hand — the
//! `user_version` pragma is its concrete implementation, and it advances by one
//! per applied migration.
//!
//! Migrations are only ever *appended* to [`migrations`]; existing ones are
//! never edited, so already-migrated databases upgrade cleanly.
//!
//! 1. [`INITIAL_SCHEMA`] — exactly the §15.2 schema.
//! 2. [`REFERENTIAL_ACTIONS`] — referential actions plus the foreign-key
//!    indexes the original schema omitted.
//! 3. [`PROJECT_GROUPS`] — organizational groups for related projects.
//! 4. [`AGENT_PROFILES`] — named launch profiles per agent provider.
//! 5. [`PROJECT_ICONS`] — optional per-project icon glyph.
//! 6. [`WORKSPACE_DISPLAY_NAMES`] — optional human label on a workspace.
//! 7. [`SESSION_LAUNCH_COMMAND`] — foreground command a shell ran, for resurrect.
//! 8. [`WORKTREE_SHARES`] — per-project rules for provisioning a worktree.
//! 9. [`SESSION_BASE_COMMIT`] — the commit a session started from.
//! 10. [`PROFILE_CONFIG_DIR`] — a profile's config directory, replacing its
//!     free-form environment.

use rusqlite_migration::{Migrations, M};

/// The one initial migration (§15.2). WAL and `foreign_keys` are enabled per
/// connection in [`crate::db::Db::open`], not here — they are connection
/// pragmas, not schema.
pub const INITIAL_SCHEMA: &str = "\
CREATE TABLE projects (
    id             TEXT PRIMARY KEY,
    name           TEXT NOT NULL,
    root_path      TEXT NOT NULL UNIQUE,
    git_root       TEXT,
    created_at     TEXT NOT NULL,
    last_opened_at TEXT NOT NULL
);

CREATE TABLE workspaces (
    id             TEXT PRIMARY KEY,
    project_id     TEXT NOT NULL REFERENCES projects(id),
    kind           TEXT NOT NULL,
    path           TEXT NOT NULL UNIQUE,
    branch         TEXT,
    managed_by_app INTEGER NOT NULL,
    created_at     TEXT NOT NULL
);

CREATE TABLE sessions (
    id                TEXT PRIMARY KEY,
    workspace_id      TEXT NOT NULL REFERENCES workspaces(id),
    kind              TEXT NOT NULL,
    role              TEXT NOT NULL,
    parent_session_id TEXT REFERENCES sessions(id),
    root_session_id   TEXT NOT NULL,
    agent_provider_id TEXT,
    user_title        TEXT,
    terminal_title    TEXT,
    last_state        TEXT NOT NULL,
    last_exit_code    INTEGER,
    created_at        TEXT NOT NULL,
    ended_at          TEXT
);

CREATE TABLE context_envelopes (
    id                TEXT PRIMARY KEY,
    source_session_id TEXT NOT NULL REFERENCES sessions(id),
    target_session_id TEXT REFERENCES sessions(id),
    summary           TEXT,
    instructions      TEXT,
    artifacts_json    TEXT NOT NULL,
    git_context_json  TEXT,
    created_at        TEXT NOT NULL
);

CREATE TABLE provider_overrides (
    provider_id     TEXT PRIMARY KEY,
    executable_path TEXT NOT NULL
);

CREATE TABLE app_state (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
";

/// Migration 2: give every foreign key an explicit referential action, and index
/// the referencing columns.
///
/// [`INITIAL_SCHEMA`] declared bare `REFERENCES` clauses, which default to `NO
/// ACTION`. That made a session undeletable once it had produced a context
/// envelope: `CloseSession` failed with a constraint error, and `RemoveProject`
/// silently left orphaned rows behind, so a "removed" project came back on the
/// next start. The actions below encode what the domain already means:
///
/// - `context_envelopes.source_session_id` → `CASCADE`: an envelope is owned by
///   the session that produced it (§8.3) and dies with it.
/// - `context_envelopes.target_session_id` → `SET NULL`: the envelope survives a
///   deleted recipient, it simply no longer points anywhere.
/// - `sessions.parent_session_id` → `SET NULL`: a belt-and-braces backstop; the
///   daemon re-parents children to the grandparent before deleting (ADR-010).
/// - `sessions.workspace_id` and `workspaces.project_id` → `RESTRICT`: deletion
///   order is the daemon's job, and a violation here is a bug worth surfacing
///   rather than a silent cascade through the user's whole session history.
///
/// SQLite cannot alter a foreign key in place, so each affected table is rebuilt
/// (create → copy → drop → rename). [`crate::db::Db::open`] runs migrations with
/// `foreign_keys = OFF` for exactly this reason.
pub const REFERENTIAL_ACTIONS: &str = "\
CREATE TABLE sessions_new (
    id                TEXT PRIMARY KEY,
    workspace_id      TEXT NOT NULL REFERENCES workspaces(id) ON DELETE RESTRICT,
    kind              TEXT NOT NULL,
    role              TEXT NOT NULL,
    parent_session_id TEXT REFERENCES sessions(id) ON DELETE SET NULL,
    root_session_id   TEXT NOT NULL,
    agent_provider_id TEXT,
    user_title        TEXT,
    terminal_title    TEXT,
    last_state        TEXT NOT NULL,
    last_exit_code    INTEGER,
    created_at        TEXT NOT NULL,
    ended_at          TEXT
);
INSERT INTO sessions_new SELECT * FROM sessions;
DROP TABLE sessions;
ALTER TABLE sessions_new RENAME TO sessions;

CREATE TABLE context_envelopes_new (
    id                TEXT PRIMARY KEY,
    source_session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    target_session_id TEXT REFERENCES sessions(id) ON DELETE SET NULL,
    summary           TEXT,
    instructions      TEXT,
    artifacts_json    TEXT NOT NULL,
    git_context_json  TEXT,
    created_at        TEXT NOT NULL
);
INSERT INTO context_envelopes_new SELECT * FROM context_envelopes;
DROP TABLE context_envelopes;
ALTER TABLE context_envelopes_new RENAME TO context_envelopes;

CREATE TABLE workspaces_new (
    id             TEXT PRIMARY KEY,
    project_id     TEXT NOT NULL REFERENCES projects(id) ON DELETE RESTRICT,
    kind           TEXT NOT NULL,
    path           TEXT NOT NULL UNIQUE,
    branch         TEXT,
    managed_by_app INTEGER NOT NULL,
    created_at     TEXT NOT NULL
);
INSERT INTO workspaces_new SELECT * FROM workspaces;
DROP TABLE workspaces;
ALTER TABLE workspaces_new RENAME TO workspaces;

CREATE INDEX idx_workspaces_project    ON workspaces (project_id);
CREATE INDEX idx_sessions_workspace    ON sessions (workspace_id);
CREATE INDEX idx_sessions_parent       ON sessions (parent_session_id);
CREATE INDEX idx_sessions_root         ON sessions (root_session_id);
CREATE INDEX idx_envelopes_source      ON context_envelopes (source_session_id);
CREATE INDEX idx_envelopes_target      ON context_envelopes (target_session_id);
";

/// Migration 3: named organizational groups. Existing projects remain in the
/// General section because the new foreign key is nullable.
pub const PROJECT_GROUPS: &str = "\
CREATE TABLE project_groups (
    id         TEXT PRIMARY KEY,
    name       TEXT NOT NULL,
    created_at TEXT NOT NULL
);

ALTER TABLE projects ADD COLUMN project_group_id TEXT
    REFERENCES project_groups(id) ON DELETE SET NULL;

CREATE INDEX idx_projects_group ON projects (project_group_id);
";

/// Migration 4: named launch profiles per agent provider (§13.4).
///
/// A profile is a saved way to start a known provider — its own command,
/// arguments and environment — so `claude` can be launched as "Personal" or
/// "Work" without a hand-written shell wrapper.
///
/// - `provider_id` is **not** a foreign key: providers are descriptors in the
///   `agents` crate, not rows.
/// - `args_json` and `env_json` hold a JSON array and a JSON array of
///   `[name, value]` pairs. They are lists, not a normalized child table:
///   nothing ever queries *inside* them, and order matters for both.
/// - The name is unique per provider, case-insensitively, so two profiles
///   cannot render as the same entry in a launch menu.
/// - `sessions.agent_profile_id` is likewise not a foreign key: a session in
///   the history must survive the deletion of the profile that launched it,
///   and the UI falls back to the provider's own name.
pub const AGENT_PROFILES: &str = "\
CREATE TABLE agent_profiles (
    id              TEXT PRIMARY KEY,
    provider_id     TEXT NOT NULL,
    name            TEXT NOT NULL,
    executable_path TEXT,
    args_json       TEXT NOT NULL,
    env_json        TEXT NOT NULL,
    created_at      TEXT NOT NULL
);

CREATE INDEX idx_agent_profiles_provider ON agent_profiles (provider_id);
CREATE UNIQUE INDEX idx_agent_profiles_name
    ON agent_profiles (provider_id, name COLLATE NOCASE);

ALTER TABLE sessions ADD COLUMN agent_profile_id TEXT;
";

/// Migration 5: a per-project icon (§7.1).
///
/// A nullable column, and null is the state every existing project keeps: no
/// icon means the UI draws the project's initials, which is what it drew
/// before this column existed. There is no default to backfill.
///
/// The column is plain `TEXT` with no `CHECK`: what makes a string usable as a
/// mark is a Unicode question (`domain::is_valid_icon`) that SQLite cannot ask,
/// and a constraint that only half-checks would give the schema an opinion it
/// cannot enforce.
pub const PROJECT_ICONS: &str = "\
ALTER TABLE projects ADD COLUMN icon TEXT;
";

/// Migration 6: an optional human label on a workspace (§7.2).
///
/// Independent of the git branch: several worktrees of one project often share
/// opaque branch names (`hind`, `langostino`), and the rail needs a word the
/// user chose for the work itself. Null keeps every existing row leading with
/// its branch, which is what the UI did before this column existed.
pub const WORKSPACE_DISPLAY_NAMES: &str = "\
ALTER TABLE workspaces ADD COLUMN display_name TEXT;
";

/// Migration 7: the foreground command a shell session was running.
///
/// Snapshot on graceful shutdown so a restart re-runs it (tmux-resurrect
/// semantics: the program comes back, not its live state). Null is a fresh
/// shell, which is what every restart did before this column existed.
pub const SESSION_LAUNCH_COMMAND: &str = "\
ALTER TABLE sessions ADD COLUMN launch_command TEXT;
";

/// Migration 8: per-project file-sharing rules (§14.2).
///
/// A managed worktree starts without the untracked files a project needs to
/// run. The global `[worktrees] copy` list could only say the same thing for
/// every project at once; these rows are the project's own answer, and the
/// global list stays as the default for a project that has none.
///
/// `strategy` is JSON rather than a tag column for the reason
/// `agent_profiles.args_json` is: nothing queries inside it, and a `Run` rule
/// carries a command and a timeout, not a name. `ON DELETE CASCADE` because
/// migration 2 exists to stop new tables from repeating the bare-`REFERENCES`
/// mistake — a removed project must not leave rules behind.
pub const WORKTREE_SHARES: &str = "\
CREATE TABLE worktree_shares (
    id         TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    path       TEXT NOT NULL,
    strategy   TEXT NOT NULL,
    enabled    INTEGER NOT NULL,
    position   INTEGER NOT NULL,
    created_at TEXT NOT NULL
);

CREATE INDEX idx_worktree_shares_project ON worktree_shares (project_id);
CREATE UNIQUE INDEX idx_worktree_shares_path ON worktree_shares (project_id, path);
";

/// The commit a session started from, so what it changed can be read back.
///
/// Nullable: a session in a folder workspace has no git base, a repository
/// with no commits yet has no `HEAD` to record, and every row written before
/// this migration has neither. `Db::purge_sessions` still drops the rows on
/// startup unless `sessions.persist_history` is set, so this column earns its
/// keep across a session's life rather than across a daemon restart.
pub const SESSION_BASE_COMMIT: &str = "\
ALTER TABLE sessions ADD COLUMN base_commit TEXT;
";

/// Migration 10: a profile is a directory, not an environment (§13.4).
///
/// A profile could set any variable it liked; what every real one actually set
/// was the provider's config directory, and the rest was a second, worse copy
/// of the agent's own configuration file. The column replaces `env_json`, and
/// the backfill keeps the one value that survives the change.
///
/// The variable names are listed here rather than read from the descriptors:
/// a migration is a statement about the rows as they were when it ran, so it
/// must not shift under a later edit to the `agents` crate. Whichever of them a
/// profile set is now simply "the directory" — for OpenCode, which used two,
/// the first one wins and the second is dropped rather than guessed at.
pub const PROFILE_CONFIG_DIR: &str = "\
ALTER TABLE agent_profiles ADD COLUMN config_dir TEXT;

UPDATE agent_profiles SET config_dir = (
    SELECT json_extract(pair.value, '$[1]')
      FROM json_each(agent_profiles.env_json) AS pair
     WHERE json_extract(pair.value, '$[0]') IN
           ('CLAUDE_CONFIG_DIR', 'CODEX_HOME', 'OPENCODE_CONFIG_DIR', 'XDG_DATA_HOME')
     LIMIT 1
);

ALTER TABLE agent_profiles DROP COLUMN env_json;
";

/// The full, ordered migration set. Appended to over time; never reordered.
#[must_use]
pub fn migrations() -> Migrations<'static> {
    Migrations::new(vec![
        M::up(INITIAL_SCHEMA),
        M::up(REFERENTIAL_ACTIONS),
        M::up(PROJECT_GROUPS),
        M::up(AGENT_PROFILES),
        M::up(PROJECT_ICONS),
        M::up(WORKSPACE_DISPLAY_NAMES),
        M::up(SESSION_LAUNCH_COMMAND),
        M::up(WORKTREE_SHARES),
        M::up(SESSION_BASE_COMMIT),
        M::up(PROFILE_CONFIG_DIR),
    ])
}
