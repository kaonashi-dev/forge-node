//! Agent launch profiles repository (§13.4, §15.2).
//!
//! A profile is a named way to start a known provider: its own command,
//! arguments and environment. `args` and `env` are stored as JSON lists rather
//! than child tables — nothing queries inside them and the order of both is
//! meaningful (arguments are positional, and a later variable wins).
//!
//! Values are stored in clear text, like every other row here. A profile is
//! meant to point at a config directory (`CLAUDE_CONFIG_DIR`), not to carry an
//! API key; the GUI says so where the variables are edited.

use std::path::PathBuf;

use domain::{AgentProfile, AgentProfileId, AgentProviderId};
use rusqlite::{params, Connection, OptionalExtension, Row};

use crate::db::DbError;
use crate::repositories::{id_from_str, path_to_str, ts_from_str, ts_to_str};

const COLUMNS: &str = "id, provider_id, name, executable_path, args_json, env_json, created_at";

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
        let env = serde_json::to_string(&profile.env)
            .map_err(|e| DbError::Encode(format!("profile env: {e}")))?;
        self.conn.execute(
            "INSERT INTO agent_profiles (\
               id, provider_id, name, executable_path, args_json, env_json, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) \
             ON CONFLICT(id) DO UPDATE SET \
               provider_id = excluded.provider_id, \
               name = excluded.name, \
               executable_path = excluded.executable_path, \
               args_json = excluded.args_json, \
               env_json = excluded.env_json",
            params![
                profile.id.to_string(),
                profile.provider_id.as_str(),
                profile.name,
                profile.executable.as_deref().map(path_to_str),
                args,
                env,
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
    args_json: String,
    env_json: String,
    created_at: String,
}

impl RawProfile {
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            provider_id: row.get(1)?,
            name: row.get(2)?,
            executable_path: row.get(3)?,
            args_json: row.get(4)?,
            env_json: row.get(5)?,
            created_at: row.get(6)?,
        })
    }

    fn into_domain(self) -> Result<AgentProfile, DbError> {
        let args: Vec<String> = serde_json::from_str(&self.args_json)
            .map_err(|e| DbError::decode("profile args", &self.args_json, e))?;
        let env: Vec<(String, String)> = serde_json::from_str(&self.env_json)
            .map_err(|e| DbError::decode("profile env", &self.env_json, e))?;
        Ok(AgentProfile {
            id: id_from_str::<AgentProfileId>("AgentProfileId", &self.id)?,
            provider_id: AgentProviderId::new(self.provider_id),
            name: self.name,
            executable: self.executable_path.map(PathBuf::from),
            args,
            env,
            created_at: ts_from_str(&self.created_at)?,
        })
    }
}
