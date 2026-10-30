//! The database handle, connection setup, error type and daemon-startup
//! reconciliation (ADR-009).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use domain::Timestamp;
use rusqlite::{params, Connection};

use crate::migrations::migrations;
use crate::repositories::{
    ActionRepo, AgentProfileRepo, AppStateRepo, ContextRepo, IgnoreRepo, OrchestrationRepo,
    ProjectGroupRepo, ProjectRepo, ProviderOverrideRepo, SessionRepo, ShareRepo, WorkspaceRepo,
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
/// and the schema is migrated to the latest version. Repositories are
/// reached through the accessor methods ([`Db::projects`], [`Db::sessions`], ...);
/// each borrows the underlying connection for the duration of the call.
pub struct Db {
    conn: Connection,
    location: PathBuf,
}

#[derive(Clone)]
pub struct ContextWriter(Arc<Mutex<Connection>>);

impl ContextWriter {
    pub fn insert(&self, envelope: &domain::ContextEnvelope) -> Result<(), DbError> {
        let conn = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        ContextRepo::new(&conn).insert(envelope)
    }

    /// Persist a session's terminal title alone, from outside the core lock.
    ///
    /// An `UPDATE` and never an upsert: by the time the PTY thread writes, the
    /// session may have been closed and its row deleted, and an upsert would
    /// bring it back.
    pub fn set_terminal_title(
        &self,
        session: domain::SessionId,
        title: Option<&str>,
    ) -> Result<(), DbError> {
        let conn = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        conn.prepare_cached("UPDATE sessions SET terminal_title = ?2 WHERE id = ?1")?
            .execute(params![session.to_string(), title])?;
        Ok(())
    }

    pub fn unread_count(&self, session: domain::SessionId) -> Result<usize, DbError> {
        let conn = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Ok(conn.query_row("SELECT count(*) FROM context_envelopes WHERE target_session_id = ?1 AND kind IS NOT NULL AND acked_at IS NULL", params![session.to_string()], |row| row.get(0))?)
    }
}

impl Db {
    /// Open (creating if needed) the database at `path` and migrate it.
    ///
    /// # Errors
    /// Returns [`DbError`] if the file cannot be opened, the pragmas cannot be
    /// set, or a migration fails.
    pub fn open(path: &Path) -> Result<Db, DbError> {
        let conn = Connection::open(path)?;
        Self::init(conn, path.to_path_buf())
    }

    /// Open a fresh in-memory database and migrate it. Intended for tests; the
    /// database vanishes when its last connection is dropped.
    ///
    /// # Errors
    /// Returns [`DbError`] if the pragmas cannot be set or a migration fails.
    pub fn open_in_memory() -> Result<Db, DbError> {
        let location = PathBuf::from(format!(
            "file:forge-{}?mode=memory&cache=shared",
            domain::ContextId::new()
        ));
        let conn = Connection::open(&location)?;
        Self::init(conn, location)
    }

    /// Apply connection pragmas (WAL + `foreign_keys = ON` +
    /// `busy_timeout`) and run migrations. Foreign keys are enforced from the
    /// first application write; see the note on the migration window below.
    fn init(mut conn: Connection, location: PathBuf) -> Result<Db, DbError> {
        // `journal_mode = WAL` returns a row ("wal"), so it must not go through
        // `execute`; `execute_batch` discards result rows and runs each pragma
        // outside a transaction, which `foreign_keys` requires.
        // WAL lets readers and one writer coexist, but a second writer still
        // contends: without `busy_timeout` that returns `SQLITE_BUSY`
        // immediately instead of retrying, and the daemon writes from both the
        // request handlers and the PTY threads.
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
        // fully enforced.
        migrations().to_latest(&mut conn)?;
        conn.execute_batch("PRAGMA foreign_keys = ON;")?;
        Ok(Db { conn, location })
    }

    /// Open a metadata writer before entering the daemon's core lock.
    pub fn context_writer(&self) -> Result<ContextWriter, DbError> {
        let conn = Connection::open(&self.location)?;
        conn.execute_batch(
            "PRAGMA foreign_keys = ON; PRAGMA synchronous = NORMAL; PRAGMA busy_timeout = 5000;",
        )?;
        Ok(ContextWriter(Arc::new(Mutex::new(conn))))
    }

    /// The raw connection, for callers that need bespoke queries.
    #[must_use]
    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    /// Any session that was `Starting` or
    /// `Running` when the daemon last stopped could not have kept its PTY alive
    /// so it is marked `Orphaned` with an `ended_at` of now. Returns the
    /// number of rows updated.
    ///
    /// This is the sole safe transition for those states after a restart — a
    /// PTY never survives the daemon, so we never pretend otherwise.
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
    /// [`Db::reconcile_orphaned`], selected by
    /// `sessions.persist_history = false`.
    ///
    /// A PTY never survives the daemon, so on startup every persisted
    /// session is already dead. Keeping them only grows a tree of `Orphaned`
    /// rows nobody restarts, so by default the daemon starts on an empty
    /// session list instead. Handoff envelopes are owned by their source
    /// session and go with it, while run messages stay for the run; projects, workspaces, provider
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
            "DELETE FROM orchestration_receipts;
             DELETE FROM run_state;
             DELETE FROM attempts;
             DELETE FROM task_deps;
             DELETE FROM tasks;
             DELETE FROM runs;
             DELETE FROM context_envelopes;
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

    #[must_use]
    pub fn projects(&self) -> ProjectRepo<'_> {
        ProjectRepo::new(&self.conn)
    }

    /// Organizational project groups repository.
    #[must_use]
    pub fn project_groups(&self) -> ProjectGroupRepo<'_> {
        ProjectGroupRepo::new(&self.conn)
    }

    #[must_use]
    pub fn workspaces(&self) -> WorkspaceRepo<'_> {
        WorkspaceRepo::new(&self.conn)
    }

    #[must_use]
    pub fn sessions(&self) -> SessionRepo<'_> {
        SessionRepo::new(&self.conn)
    }

    #[must_use]
    pub fn context(&self) -> ContextRepo<'_> {
        ContextRepo::new(&self.conn)
    }

    #[must_use]
    pub fn orchestration(&self) -> OrchestrationRepo<'_> {
        OrchestrationRepo::new(&self.conn)
    }

    #[must_use]
    pub fn provider_overrides(&self) -> ProviderOverrideRepo<'_> {
        ProviderOverrideRepo::new(&self.conn)
    }

    #[must_use]
    pub fn agent_profiles(&self) -> AgentProfileRepo<'_> {
        AgentProfileRepo::new(&self.conn)
    }

    #[must_use]
    pub fn actions(&self) -> ActionRepo<'_> {
        ActionRepo::new(&self.conn)
    }

    #[must_use]
    pub fn shares(&self) -> ShareRepo<'_> {
        ShareRepo::new(&self.conn)
    }

    #[must_use]
    pub fn ignores(&self) -> IgnoreRepo<'_> {
        IgnoreRepo::new(&self.conn)
    }

    #[must_use]
    pub fn app_state(&self) -> AppStateRepo<'_> {
        AppStateRepo::new(&self.conn)
    }
}

#[cfg(test)]
mod tests {
    use super::Db;

    #[test]
    fn context_writer_shares_metadata_without_sharing_the_core_connection() {
        for memory in [true, false] {
            let dir = tempfile::tempdir().unwrap();
            let db = if memory {
                Db::open_in_memory().unwrap()
            } else {
                Db::open(&dir.path().join("metadata.db")).unwrap()
            };
            let other = Db::open_in_memory().unwrap();
            let writer = db.context_writer().unwrap();
            let envelope = domain::ContextEnvelope {
                id: domain::ContextId::new(),
                source_session_id: None,
                target_session_id: None,
                summary: Some("audit".into()),
                instructions: Some("bounded prompt".into()),
                artifacts: vec![],
                git_context: None,
                created_at: domain::Timestamp::now(),
                run_id: None,
                task_id: None,
                kind: None,
                in_reply_to: None,
                acked_at: None,
            };
            std::thread::spawn(move || writer.insert(&envelope))
                .join()
                .unwrap()
                .unwrap();
            let count = |db: &Db| {
                db.conn()
                    .query_row("SELECT count(*) FROM context_envelopes", [], |row| {
                        row.get::<_, i64>(0)
                    })
                    .unwrap()
            };
            assert_eq!(count(&db), 1);
            assert_eq!(count(&other), 0);
            let keys: i64 = db
                .context_writer()
                .unwrap()
                .0
                .lock()
                .unwrap()
                .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
                .unwrap();
            assert_eq!(keys, 1);
        }
    }

    #[test]
    fn connection_pragmas_are_applied() {
        let db = Db::open_in_memory().unwrap();
        // A contended WAL lock must retry
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
