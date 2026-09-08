//! Sessions repository (§7.3, §15.2).
//!
//! `terminal_id` is deliberately not a column: it is pure runtime state,
//! regenerated on every spawn/restart (§15.2). Loaded sessions therefore always
//! have `terminal_id == None`. The title's two parts map to the `user_title` and
//! `terminal_title` columns; the state maps to `last_state` + `last_exit_code`
//! (see [`crate::repositories`] for the exact encoding).

use domain::{
    AgentProfileId, AgentProviderId, Session, SessionId, SessionState, SessionTitle, Timestamp,
    WorkspaceId,
};
use rusqlite::{params, Connection, OptionalExtension, Row};

use crate::db::DbError;
use crate::repositories::{
    id_from_opt, id_from_str, role_from_str, role_to_str, session_kind_from_str,
    session_kind_to_str, state_discriminant, state_exit_code, state_from_parts, ts_from_opt,
    ts_from_str, ts_to_str,
};

const COLUMNS: &str = "id, workspace_id, kind, role, parent_session_id, root_session_id, \
     agent_provider_id, agent_profile_id, user_title, terminal_title, last_state, last_exit_code, \
     created_at, ended_at, launch_command, base_commit";

/// Repository over the `sessions` table.
pub struct SessionRepo<'a> {
    conn: &'a Connection,
}

impl<'a> SessionRepo<'a> {
    #[must_use]
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }

    /// Insert a session, or update every persisted column in place if its id
    /// already exists. `terminal_id` is never written (§15.2).
    ///
    /// # Errors
    /// Returns [`DbError`] on a failed statement (e.g. a `workspace_id` or
    /// `parent_session_id` that violates a foreign key).
    pub fn upsert(&self, session: &Session) -> Result<(), DbError> {
        // `prepare_cached`, not `execute`: a terminal title change upserts on the
        // hot PTY path (C9), and re-compiling this statement on every shell
        // `precmd` is wasted work the connection's statement cache erases.
        self.conn
            .prepare_cached(
                "INSERT INTO sessions (\
               id, workspace_id, kind, role, parent_session_id, root_session_id, \
               agent_provider_id, agent_profile_id, user_title, terminal_title, last_state, \
               last_exit_code, created_at, ended_at, launch_command, base_commit) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16) \
             ON CONFLICT(id) DO UPDATE SET \
               workspace_id = excluded.workspace_id, \
               kind = excluded.kind, \
               role = excluded.role, \
               parent_session_id = excluded.parent_session_id, \
               root_session_id = excluded.root_session_id, \
               agent_provider_id = excluded.agent_provider_id, \
               agent_profile_id = excluded.agent_profile_id, \
               user_title = excluded.user_title, \
               terminal_title = excluded.terminal_title, \
               last_state = excluded.last_state, \
               last_exit_code = excluded.last_exit_code, \
               created_at = excluded.created_at, \
               ended_at = excluded.ended_at, \
               launch_command = excluded.launch_command, \
               base_commit = excluded.base_commit",
            )?
            .execute(params![
                session.id.to_string(),
                session.workspace_id.to_string(),
                session_kind_to_str(session.kind)?,
                role_to_str(&session.role)?,
                session.parent_session_id.map(|id| id.to_string()),
                session.root_session_id.to_string(),
                session.agent_provider_id.as_ref().map(|p| p.as_str()),
                session.agent_profile_id.map(|id| id.to_string()),
                session.title.user,
                session.title.terminal,
                state_discriminant(&session.state)?,
                state_exit_code(&session.state),
                ts_to_str(&session.created_at),
                session.ended_at.as_ref().map(ts_to_str),
                session.launch_command,
                session.base_commit,
            ])?;
        Ok(())
    }

    /// Fetch a session by id, or `None`.
    ///
    /// # Errors
    /// Returns [`DbError`] on a failed query or an undecodable row.
    pub fn get(&self, id: SessionId) -> Result<Option<Session>, DbError> {
        let raw = self
            .conn
            .query_row(
                &format!("SELECT {COLUMNS} FROM sessions WHERE id = ?1"),
                params![id.to_string()],
                RawSession::from_row,
            )
            .optional()?;
        raw.map(RawSession::into_domain).transpose()
    }

    /// List all sessions, in creation order.
    ///
    /// # Errors
    /// Returns [`DbError`] on a failed query or an undecodable row.
    pub fn list(&self) -> Result<Vec<Session>, DbError> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {COLUMNS} FROM sessions ORDER BY created_at ASC"
        ))?;
        let rows = stmt.query_map([], RawSession::from_row)?;
        Self::collect(rows)
    }

    /// List the sessions of a workspace, in creation order.
    ///
    /// # Errors
    /// Returns [`DbError`] on a failed query or an undecodable row.
    pub fn list_by_workspace(&self, workspace_id: WorkspaceId) -> Result<Vec<Session>, DbError> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {COLUMNS} FROM sessions WHERE workspace_id = ?1 ORDER BY created_at ASC"
        ))?;
        let rows = stmt.query_map(params![workspace_id.to_string()], RawSession::from_row)?;
        Self::collect(rows)
    }

    /// Delete a session by id. Returns `true` if a row was removed.
    ///
    /// # Errors
    /// Returns [`DbError`] on a failed statement (e.g. a child or envelope still
    /// references it).
    pub fn delete(&self, id: SessionId) -> Result<bool, DbError> {
        let n = self.conn.execute(
            "DELETE FROM sessions WHERE id = ?1",
            params![id.to_string()],
        )?;
        Ok(n > 0)
    }

    /// Update only the state columns (`last_state`, `last_exit_code`) and
    /// `ended_at` of a session. Returns `true` if it existed. Used by the session
    /// lifecycle without rewriting the whole row.
    ///
    /// # Errors
    /// Returns [`DbError`] on a failed statement.
    pub fn update_state(
        &self,
        id: SessionId,
        state: &SessionState,
        ended_at: Option<Timestamp>,
    ) -> Result<bool, DbError> {
        let n = self.conn.execute(
            "UPDATE sessions SET last_state = ?2, last_exit_code = ?3, ended_at = ?4 WHERE id = ?1",
            params![
                id.to_string(),
                state_discriminant(state)?,
                state_exit_code(state),
                ended_at.as_ref().map(ts_to_str),
            ],
        )?;
        Ok(n > 0)
    }

    /// Drain a `query_map` iterator, decoding each row into a [`Session`].
    fn collect(
        rows: impl Iterator<Item = rusqlite::Result<RawSession>>,
    ) -> Result<Vec<Session>, DbError> {
        let mut out = Vec::new();
        for row in rows {
            out.push(row?.into_domain()?);
        }
        Ok(out)
    }
}

struct RawSession {
    id: String,
    workspace_id: String,
    kind: String,
    role: String,
    parent_session_id: Option<String>,
    root_session_id: String,
    agent_provider_id: Option<String>,
    agent_profile_id: Option<String>,
    user_title: Option<String>,
    terminal_title: Option<String>,
    last_state: String,
    last_exit_code: Option<i32>,
    created_at: String,
    ended_at: Option<String>,
    launch_command: Option<String>,
    base_commit: Option<String>,
}

impl RawSession {
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            workspace_id: row.get(1)?,
            kind: row.get(2)?,
            role: row.get(3)?,
            parent_session_id: row.get(4)?,
            root_session_id: row.get(5)?,
            agent_provider_id: row.get(6)?,
            agent_profile_id: row.get(7)?,
            user_title: row.get(8)?,
            terminal_title: row.get(9)?,
            last_state: row.get(10)?,
            last_exit_code: row.get(11)?,
            created_at: row.get(12)?,
            ended_at: row.get(13)?,
            launch_command: row.get(14)?,
            base_commit: row.get(15)?,
        })
    }

    fn into_domain(self) -> Result<Session, DbError> {
        let created_at = ts_from_str(&self.created_at)?;
        let ended_at = ts_from_opt(self.ended_at)?;
        Ok(Session {
            id: id_from_str::<SessionId>("SessionId", &self.id)?,
            workspace_id: id_from_str::<WorkspaceId>("WorkspaceId", &self.workspace_id)?,
            kind: session_kind_from_str(&self.kind)?,
            role: role_from_str(&self.role)?,
            parent_session_id: id_from_opt::<SessionId>("SessionId", self.parent_session_id)?,
            root_session_id: id_from_str::<SessionId>("SessionId", &self.root_session_id)?,
            // Pure runtime state, never persisted (§15.2): always None on load.
            terminal_id: None,
            agent_provider_id: self.agent_provider_id.map(AgentProviderId::new),
            agent_profile_id: id_from_opt::<AgentProfileId>(
                "AgentProfileId",
                self.agent_profile_id,
            )?,
            title: SessionTitle {
                user: self.user_title,
                terminal: self.terminal_title,
            },
            state: state_from_parts(&self.last_state, self.last_exit_code)?,
            created_at,
            launch_command: self.launch_command,
            // Runtime state, never persisted (§15.2), like `terminal_id`. A row
            // on disk describes a session whose PTY is already gone, so the
            // best available answer is when it ended, else when it began.
            last_activity_at: ended_at.unwrap_or(created_at),
            ended_at,
            base_commit: self.base_commit,
        })
    }
}
