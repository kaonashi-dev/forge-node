//! Agent launch profiles repository (§13.4, §15.2).
//!
//! A profile is a named way to start a known provider: its own binary, its own
//! config directory and its own arguments. `args` is stored as a JSON list
//! rather than a child table — nothing queries inside it and its order is
//! meaningful, because arguments are positional.
//!
//! `config_dir` is stored as the user typed it, relative paths included: what
//! it is relative *to* is the home directory of whoever launches, which is a
//! runtime fact and not a stored one (`domain::AgentProfile::resolve_config_dir`).

use std::path::PathBuf;

use domain::{AgentProfile, AgentProfileId, AgentProviderId};
use rusqlite::{params, Connection, OptionalExtension, Row};

use crate::db::DbError;
use crate::repositories::{id_from_str, path_to_str, ts_from_str, ts_to_str};

const COLUMNS: &str = "id, provider_id, name, executable_path, config_dir, args_json, created_at";

/// Repository over the `agent_profiles` table.
pub struct AgentProfileRepo<'a> {
    conn: &'a Connection,
}

impl<'a> AgentProfileRepo<'a> {
    #[must_use]
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }

    /// Insert a profile, or replace every column of an existing one.
    ///
    /// # Errors
    /// Returns [`DbError`] on a failed statement — including the unique index
    /// on `(provider_id, name)`, which the daemon turns into a `Conflict`.
    pub fn upsert(&self, profile: &AgentProfile) -> Result<(), DbError> {
        let args = serde_json::to_string(&profile.args)
            .map_err(|e| DbError::Encode(format!("profile args: {e}")))?;
        self.conn.execute(
            "INSERT INTO agent_profiles (\
               id, provider_id, name, executable_path, config_dir, args_json, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) \
             ON CONFLICT(id) DO UPDATE SET \
               provider_id = excluded.provider_id, \
               name = excluded.name, \
               executable_path = excluded.executable_path, \
               config_dir = excluded.config_dir, \
               args_json = excluded.args_json",
            params![
                profile.id.to_string(),
                profile.provider_id.as_str(),
                profile.name,
                profile.executable.as_deref().map(path_to_str),
                profile.config_dir.as_deref().map(path_to_str),
                args,
                ts_to_str(&profile.created_at),
            ],
        )?;
        Ok(())
    }

    /// Fetch one profile by id, or `None`.
    ///
    /// # Errors
    /// Returns [`DbError`] on a failed query or an undecodable row.
    pub fn get(&self, id: AgentProfileId) -> Result<Option<AgentProfile>, DbError> {
        let raw = self
            .conn
            .query_row(
                &format!("SELECT {COLUMNS} FROM agent_profiles WHERE id = ?1"),
                params![id.to_string()],
                RawProfile::from_row,
            )
            .optional()?;
        raw.map(RawProfile::into_domain).transpose()
    }

    /// List every profile, ordered by provider then name, which is the order a
    /// launch menu shows them in.
    ///
    /// # Errors
    /// Returns [`DbError`] on a failed query or an undecodable row.
    pub fn list(&self) -> Result<Vec<AgentProfile>, DbError> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {COLUMNS} FROM agent_profiles \
             ORDER BY provider_id ASC, name COLLATE NOCASE ASC"
        ))?;
        let rows = stmt.query_map([], RawProfile::from_row)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?.into_domain()?);
        }
        Ok(out)
    }

    /// Delete a profile by id. Returns `true` if a row was removed.
    ///
    /// # Errors
    /// Returns [`DbError`] on a failed statement.
    pub fn delete(&self, id: AgentProfileId) -> Result<bool, DbError> {
        let n = self.conn.execute(
            "DELETE FROM agent_profiles WHERE id = ?1",
            params![id.to_string()],
        )?;
        Ok(n > 0)
    }
}

struct RawProfile {
    id: String,
    provider_id: String,
    name: String,
    executable_path: Option<String>,
    config_dir: Option<String>,
    args_json: String,
    created_at: String,
}

impl RawProfile {
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            provider_id: row.get(1)?,
            name: row.get(2)?,
            executable_path: row.get(3)?,
            config_dir: row.get(4)?,
            args_json: row.get(5)?,
            created_at: row.get(6)?,
        })
    }

    fn into_domain(self) -> Result<AgentProfile, DbError> {
        let args: Vec<String> = serde_json::from_str(&self.args_json)
            .map_err(|e| DbError::decode("profile args", &self.args_json, e))?;
        Ok(AgentProfile {
            id: id_from_str::<AgentProfileId>("AgentProfileId", &self.id)?,
            provider_id: AgentProviderId::new(self.provider_id),
            name: self.name,
            executable: self.executable_path.map(PathBuf::from),
            config_dir: self.config_dir.map(PathBuf::from),
            args,
            created_at: ts_from_str(&self.created_at)?,
        })
    }
}
