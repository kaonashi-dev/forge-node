//! The database handle, connection setup, error type and daemon-startup
//! reconciliation (§15, ADR-009).

use std::path::Path;

use domain::Timestamp;
use rusqlite::{params, Connection};

use crate::migrations::migrations;
use crate::repositories::{
    AgentProfileRepo, AppStateRepo, ContextRepo, IgnoreRepo, ProjectGroupRepo, ProjectRepo,
    ProviderOverrideRepo, SessionRepo, ShareRepo, WorkspaceRepo,
};

/// Errors produced by the persistence layer.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum DbError {
    /// A `rusqlite` error (I/O, constraint violation, type mismatch, ...).
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    /// A migration failed to apply.
    #[error("migration error: {0}")]
    Migration(#[from] rusqlite_migration::Error),

    /// A JSON column (artifacts / git context) failed to (de)serialize.
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    /// A stored value could not be decoded back into a domain type (an invalid
    /// id, timestamp, or enum tag). Indicates external corruption of the file.
    #[error("could not decode {0}")]
    Decode(String),

    /// A domain value has no column encoding in this build — a `#[non_exhaustive]`
    /// enum grew a variant the persistence layer does not know how to store.
    /// Reported instead of panicking: a panic inside a daemon request handler
    /// poisons the core lock for every other client.
    #[error("could not encode {0}")]
    Encode(String),
}

impl DbError {
    /// Build a [`DbError::Decode`] from a value and an underlying error.
    pub(crate) fn decode(what: &str, value: &str, err: impl std::fmt::Display) -> Self {
        DbError::Decode(format!("{what} from {value:?}: {err}"))
    }

    /// Build a [`DbError::Decode`] from a value and a static reason.
    pub(crate) fn decode_msg(what: &str, value: &str, reason: &str) -> Self {
        DbError::Decode(format!("{what} from {value:?}: {reason}"))
    }
}

/// A handle to the SQLite metadata database (ADR-009: metadata only — never the
/// terminal stream).
///
/// On open the connection is put in WAL mode with foreign-key enforcement on,
/// and the schema is migrated to the latest version (§15.2). Repositories are
/// reached through the accessor methods ([`Db::projects`], [`Db::sessions`], ...);
/// each borrows the underlying connection for the duration of the call.
pub struct Db {
    conn: Connection,
}

impl Db {
    /// Open (creating if needed) the database at `path` and migrate it.
    ///
    /// # Errors
    /// Returns [`DbError`] if the file cannot be opened, the pragmas cannot be
    /// set, or a migration fails.
    pub fn open(path: &Path) -> Result<Db, DbError> {
        let conn = Connection::open(path)?;
        Self::init(conn)
    }

    /// Open a fresh in-memory database and migrate it. Intended for tests; the
    /// database vanishes when the [`Db`] is dropped.
    ///
    /// # Errors
    /// Returns [`DbError`] if the pragmas cannot be set or a migration fails.
    pub fn open_in_memory() -> Result<Db, DbError> {
        let conn = Connection::open_in_memory()?;
        Self::init(conn)
    }

    /// Apply connection pragmas (§15.2: WAL + `foreign_keys = ON` +
    /// `busy_timeout`) and run migrations. Foreign keys are enforced from the
    /// first application write; see the note on the migration window below.
    fn init(mut conn: Connection) -> Result<Db, DbError> {
        // `journal_mode = WAL` returns a row ("wal"), so it must not go through
        // `execute`; `execute_batch` discards result rows and runs each pragma
        // outside a transaction, which `foreign_keys` requires.
        // WAL lets readers and one writer coexist, but a second writer still
        // contends: without `busy_timeout` that returns `SQLITE_BUSY`
        // immediately instead of retrying, and the daemon writes from both the
        // request handlers and the PTY threads (§9.3).
        //
        // `synchronous = NORMAL` is the correct durability level under WAL: the
        // default `FULL` fsyncs the WAL on *every* commit, and one of those
        // commits sits on the hot PTY path (a terminal title change on every
        // shell `precmd`, under the core lock — C9). NORMAL fsyncs only at
        // checkpoint, which cannot lose a committed transaction to an
        // application crash — only a power loss can, and metadata that a rescan
        // rebuilds does not need that guarantee (ADR-009).
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;\n\
             PRAGMA synchronous = NORMAL;\n\
             PRAGMA foreign_keys = OFF;\n\
             PRAGMA busy_timeout = 5000;",
        )?;
        // Migrations run with `foreign_keys = OFF`. SQLite cannot add a clause to
        // an existing foreign key, so a migration that needs `ON DELETE CASCADE`
        // has to rebuild the table (create → copy → drop → rename); with the
        // pragma on, the intermediate `DROP TABLE` would trip constraints from
        // rows that are about to be re-pointed. The pragma goes back on below,
        // before any application statement runs, so runtime writes are still
        // fully enforced (§15.2).
        migrations().to_latest(&mut conn)?;
        conn.execute_batch("PRAGMA foreign_keys = ON;")?;
        Ok(Db { conn })
    }

    /// The raw connection, for callers that need bespoke queries.
    #[must_use]
    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    /// Reconciliation step 2 of §15.3: any session that was `Starting` or
    /// `Running` when the daemon last stopped could not have kept its PTY alive
    /// (§3.3), so it is marked `Orphaned` with an `ended_at` of now. Returns the
    /// number of rows updated.
    ///
    /// This is the sole safe transition for those states after a restart — a
    /// PTY never survives the daemon (§3.3), so we never pretend otherwise.
    ///
    /// # Errors
    /// Returns [`DbError`] if the update statement fails.
    pub fn reconcile_orphaned(&self) -> Result<usize, DbError> {
        let now = Timestamp::now().to_rfc3339();
        let updated = self.conn.execute(
            "UPDATE sessions SET last_state = 'Orphaned', ended_at = ?1 \
             WHERE last_state IN ('Starting', 'Running')",
            params![now],
        )?;
        Ok(updated)
    }

    /// Drop every session row: the "fresh start" alternative to
    /// [`Db::reconcile_orphaned`] (§15.3 step 2), selected by
    /// `sessions.persist_history = false`.
    ///
    /// A PTY never survives the daemon (§3.3), so on startup *every* persisted
    /// session is already dead. Keeping them only grows a tree of `Orphaned`
    /// rows nobody restarts, so by default the daemon starts on an empty
    /// session list instead. Context envelopes are owned by their source
    /// session and cascade with it (§8.3); projects, workspaces, provider
    /// overrides and app state are untouched. Returns the number of rows
    /// deleted.
    ///
    /// # Errors
    /// Returns [`DbError`] if the delete statement fails.
    pub fn purge_sessions(&self) -> Result<usize, DbError> {
        let deleted = self.conn.execute("DELETE FROM sessions", [])?;
        Ok(deleted)
    }

    /// Clear every application row while retaining the migrated database.
    ///
    /// The transaction is the persistence boundary for a factory reset: a
    /// failed statement leaves the complete previous state available for a
    /// retry rather than a half-empty project tree.
    ///
    /// # Errors
    /// Returns [`DbError`] if the transaction cannot be started, cleared, or
    /// committed.
    pub fn reset(&mut self) -> Result<(), DbError> {
        let tx = self.conn.transaction()?;
        tx.execute_batch(
            "DELETE FROM context_envelopes;
             DELETE FROM sessions;
             DELETE FROM workspaces;
             DELETE FROM projects;
             DELETE FROM project_groups;
             DELETE FROM agent_profiles;
             DELETE FROM provider_overrides;
             DELETE FROM app_state;",
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Projects repository (§7.1).
    #[must_use]
    pub fn projects(&self) -> ProjectRepo<'_> {
        ProjectRepo::new(&self.conn)
    }

    /// Organizational project groups repository.
    #[must_use]
    pub fn project_groups(&self) -> ProjectGroupRepo<'_> {
        ProjectGroupRepo::new(&self.conn)
    }

    /// Workspaces repository (§7.2).
    #[must_use]
    pub fn workspaces(&self) -> WorkspaceRepo<'_> {
        WorkspaceRepo::new(&self.conn)
    }

    /// Sessions repository (§7.3).
    #[must_use]
    pub fn sessions(&self) -> SessionRepo<'_> {
        SessionRepo::new(&self.conn)
    }

    /// Context envelopes repository (§8.3).
    #[must_use]
    pub fn context(&self) -> ContextRepo<'_> {
        ContextRepo::new(&self.conn)
    }

    /// Provider executable overrides repository (§13.2).
    #[must_use]
    pub fn provider_overrides(&self) -> ProviderOverrideRepo<'_> {
        ProviderOverrideRepo::new(&self.conn)
    }

    /// Agent launch profiles repository (§13.4).
    #[must_use]
    pub fn agent_profiles(&self) -> AgentProfileRepo<'_> {
        AgentProfileRepo::new(&self.conn)
    }

    /// Per-project file-sharing rules repository (§14.2).
    #[must_use]
    pub fn shares(&self) -> ShareRepo<'_> {
        ShareRepo::new(&self.conn)
    }

    /// Project worktree-ignore rules repository (§14.4).
    #[must_use]
    pub fn ignores(&self) -> IgnoreRepo<'_> {
        IgnoreRepo::new(&self.conn)
    }

    /// Opaque application/layout state repository (ADR-003, §15.2).
    #[must_use]
    pub fn app_state(&self) -> AppStateRepo<'_> {
        AppStateRepo::new(&self.conn)
    }
}

#[cfg(test)]
mod tests {
    use super::Db;

    #[test]
    fn connection_pragmas_are_applied() {
        let db = Db::open_in_memory().unwrap();
        // §15.2 + the WAL contention note in `init`: a contended lock must retry
        // for 5 s rather than fail with `SQLITE_BUSY` on the first attempt.
        let busy_timeout: i64 = db
            .conn()
            .query_row("PRAGMA busy_timeout", [], |row| row.get(0))
            .unwrap();
        assert_eq!(busy_timeout, 5_000);

        let foreign_keys: i64 = db
            .conn()
            .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
            .unwrap();
        assert_eq!(foreign_keys, 1);
    }
}
