//! Opaque application/layout state repository (ADR-003, §15.2).
//!
//! A simple key/value store the daemon persists on behalf of the GUI without
//! interpreting it (`SetAppState`/`GetAppState`, §10.2): serialized dock layout,
//! sidebar width, last active session, and similar presentation state. The
//! daemon never reads meaning into these values.

use rusqlite::{params, Connection, OptionalExtension, Row};

use crate::db::DbError;

/// Repository over the `app_state` table.
pub struct AppStateRepo<'a> {
    conn: &'a Connection,
}

impl<'a> AppStateRepo<'a> {
    #[must_use]
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }

    /// Set (upsert) the value for a key.
    ///
    /// # Errors
    /// Returns [`DbError`] on a failed statement.
    pub fn set(&self, key: &str, value: &str) -> Result<(), DbError> {
        self.conn.execute(
            "INSERT INTO app_state (key, value) VALUES (?1, ?2) \
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    /// Fetch the value for a key, or `None` if unset.
    ///
    /// # Errors
    /// Returns [`DbError`] on a failed query.
    pub fn get(&self, key: &str) -> Result<Option<String>, DbError> {
        let value = self
            .conn
            .query_row(
                "SELECT value FROM app_state WHERE key = ?1",
                params![key],
                |row| row.get(0),
            )
            .optional()?;
        Ok(value)
    }

    /// Delete a key. Returns `true` if it existed.
    ///
    /// # Errors
    /// Returns [`DbError`] on a failed statement.
    pub fn delete(&self, key: &str) -> Result<bool, DbError> {
        let n = self
            .conn
            .execute("DELETE FROM app_state WHERE key = ?1", params![key])?;
        Ok(n > 0)
    }

    /// List all `(key, value)` pairs, ordered by key.
    ///
    /// # Errors
    /// Returns [`DbError`] on a failed query.
    pub fn list(&self) -> Result<Vec<(String, String)>, DbError> {
        let mut stmt = self
            .conn
            .prepare("SELECT key, value FROM app_state ORDER BY key ASC")?;
        let rows = stmt.query_map([], Self::row_pair)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    fn row_pair(row: &Row<'_>) -> rusqlite::Result<(String, String)> {
        Ok((row.get(0)?, row.get(1)?))
    }
}
