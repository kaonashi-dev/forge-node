//! Provider executable overrides repository (§13.2, §15.2).
//!
//! Maps an [`AgentProviderId`] to a user-chosen executable path. `set(_, None)`
//! deletes the override — mirroring the protocol's `SetProviderExecutable`
//! request where `None` removes the override (§10.2).

use std::path::PathBuf;

use domain::AgentProviderId;
use rusqlite::{params, Connection, OptionalExtension, Row};

use crate::db::DbError;
use crate::repositories::path_to_str;

/// Repository over the `provider_overrides` table.
pub struct ProviderOverrideRepo<'a> {
    conn: &'a Connection,
}

impl<'a> ProviderOverrideRepo<'a> {
    #[must_use]
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }

    /// Set (`Some`) or clear (`None`) the executable override for a provider.
    /// Setting upserts on the `provider_id` primary key.
    ///
    /// # Errors
    /// Returns [`DbError`] on a failed statement.
    pub fn set(
        &self,
        provider_id: &AgentProviderId,
        path: Option<&PathBuf>,
    ) -> Result<(), DbError> {
        match path {
            Some(path) => {
                self.conn.execute(
                    "INSERT INTO provider_overrides (provider_id, executable_path) \
                     VALUES (?1, ?2) \
                     ON CONFLICT(provider_id) DO UPDATE SET executable_path = excluded.executable_path",
                    params![provider_id.as_str(), path_to_str(path)],
                )?;
            }
            None => {
                self.conn.execute(
                    "DELETE FROM provider_overrides WHERE provider_id = ?1",
                    params![provider_id.as_str()],
                )?;
            }
        }
        Ok(())
    }

    /// Fetch the override path for a provider, or `None` if there is none.
    ///
    /// # Errors
    /// Returns [`DbError`] on a failed query.
    pub fn get(&self, provider_id: &AgentProviderId) -> Result<Option<PathBuf>, DbError> {
        let path: Option<String> = self
            .conn
            .query_row(
                "SELECT executable_path FROM provider_overrides WHERE provider_id = ?1",
                params![provider_id.as_str()],
                |row| row.get(0),
            )
            .optional()?;
        Ok(path.map(PathBuf::from))
    }

    /// List all overrides as `(provider_id, path)` pairs, ordered by provider id.
    ///
    /// # Errors
    /// Returns [`DbError`] on a failed query.
    pub fn list(&self) -> Result<Vec<(AgentProviderId, PathBuf)>, DbError> {
        let mut stmt = self.conn.prepare(
            "SELECT provider_id, executable_path FROM provider_overrides ORDER BY provider_id ASC",
        )?;
        let rows = stmt.query_map([], Self::row_pair)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    fn row_pair(row: &Row<'_>) -> rusqlite::Result<(AgentProviderId, PathBuf)> {
        let id: String = row.get(0)?;
        let path: String = row.get(1)?;
        Ok((AgentProviderId::new(id), PathBuf::from(path)))
    }
}
