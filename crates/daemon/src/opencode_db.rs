//! Reading opencode's session database (opencode ≥ 1.17).
//!
//! opencode used to keep one JSON file per session under
//! `<data>/opencode/storage`. Current versions write a single SQLite database
//! next to it — `<data>/opencode/opencode.db` — and stop updating that tree, so
//! a scanner that only reads files reports a history frozen at the upgrade:
//! present, plausible, and months out of date. This module is the other half of
//! [`crate::external_agents`]'s opencode support, reading the same facts out of
//! the database.
//!
//! Two rules hold throughout. The database belongs to another program, so it is
//! opened **read-only** with `query_only` on top — Forge must not be the reason
//! someone's history breaks. And every failure — no database, a schema this
//! build does not recognise, a locked file, a malformed JSON blob — resolves to
//! "no sessions from here", never to an error: the JSON tree is still scanned,
//! and the panel degrades to what it can read.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};
use serde_json::Value;

/// How many messages back from the end of a session we look for the last agent
/// reply. opencode records one row per conversation turn — tool traffic lives
/// in `part`, not `message` — so a couple of dozen covers any run that ended in
/// a tail of tool calls without turning the scan into a full read.
const MESSAGE_TAIL: usize = 24;

/// One top-level session, as the database records it.
///
/// Timestamps stay in epoch milliseconds, the unit opencode stores, and are
/// converted by the caller alongside the ones read from JSON.
pub(crate) struct DbSession {
    pub id: String,
    pub title: Option<String>,
    pub created_ms: i64,
    pub updated_ms: i64,
    /// The model that wrote the last agent message, as opencode names it.
    pub model: Option<String>,
    /// Conversation turns: prompts and replies, not tool traffic.
    pub messages: u32,
    /// Sessions recorded with this one as their parent.
    pub subagents: u32,
    /// The last agent message that was prose, verbatim; the caller folds it.
    pub last_reply: Option<String>,
}

/// Every opencode database in `data_dir`, in a stable order.
///
/// opencode ships one `opencode.db`, but names a database it is migrating or
/// testing `opencode-<something>.db`, and a machine that has both should not
/// have half its history disappear because we guessed the name.
pub(crate) fn databases(data_dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(data_dir) else {
        return Vec::new();
    };
    let mut found: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_file() && is_database_name(path))
        .collect();
    found.sort();
    found
}

/// Whether a file name is one of opencode's databases: `opencode.db`, or
/// `opencode-<suffix>.db`. Anything else in the data directory is not ours to
/// open.
fn is_database_name(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let Some(stem) = name.strip_suffix(".db") else {
        return false;
    };
    stem == "opencode"
        || stem
            .strip_prefix("opencode-")
            .is_some_and(|s| !s.is_empty())
}

/// The most recent top-level sessions `db` records for any of `directories`,
/// newest first and at most `limit` of them.
///
/// `directories` is the same directory in the forms it might have been written
/// in — as Forge knows it and as the filesystem canonicalizes it — because the
/// column holds whatever path opencode was started with and SQL compares text,
/// not paths.
pub(crate) fn sessions(db: &Path, directories: &[String], limit: usize) -> Vec<DbSession> {
    if directories.is_empty() {
        return Vec::new();
    }
    let Some(conn) = open(db) else {
        return Vec::new();
    };
    let columns = columns(&conn, "session");
    // `id`, `directory` and the two instants are the schema this reads; without
    // them the file is not an opencode session database we understand.
    if !["id", "directory", "time_created", "time_updated"]
        .iter()
        .all(|needed| columns.contains(*needed))
    {
        return Vec::new();
    }

    let mut rows = match list(&conn, &columns, directories, limit) {
        Ok(rows) => rows,
        Err(error) => {
            tracing::debug!(db = %db.display(), %error, "opencode session list failed");
            return Vec::new();
        }
    };
    if rows.is_empty() {
        return rows;
    }

    let ids: Vec<String> = rows.iter().map(|row| row.id.clone()).collect();
    let subagents = if columns.contains("parent_id") {
        subagent_counts(&conn, &ids)
    } else {
        HashMap::new()
    };
    let messages = message_counts(&conn, &ids);

    for row in &mut rows {
        row.subagents = subagents.get(&row.id).copied().unwrap_or(0);
        row.messages = messages.get(&row.id).copied().unwrap_or(0);
        row.last_reply = last_reply(&conn, &row.id);
        // The `model` column arrived late and is null on most existing rows, so
        // the message the model actually wrote is the reliable source; the
        // column is only the fallback.
        if let Some(model) = last_model(&conn, &row.id) {
            row.model = Some(model);
        }
    }
    rows
}

/// Open `db` read-only, or `None` if it cannot be read.
///
/// Read-only is the contract with opencode, and `query_only` is the guard
/// against this file ever growing a statement that would write.
fn open(db: &Path) -> Option<Connection> {
    let conn = Connection::open_with_flags(
        db,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .inspect_err(|error| {
        tracing::debug!(db = %db.display(), %error, "opencode database not readable");
    })
    .ok()?;
    conn.pragma_update(None, "query_only", "ON").ok()?;
    Some(conn)
}

/// The column names of `table`, empty when the table does not exist.
fn columns(conn: &Connection, table: &str) -> HashSet<String> {
    // `table` is a literal from this module, never user input.
    let Ok(mut statement) = conn.prepare(&format!("PRAGMA table_info({table})")) else {
        return HashSet::new();
    };
    let Ok(names) = statement.query_map([], |row| row.get::<_, String>(1)) else {
        return HashSet::new();
    };
    names.flatten().collect()
}

/// The session rows themselves, without the per-session details.
fn list(
    conn: &Connection,
    columns: &HashSet<String>,
    directories: &[String],
    limit: usize,
) -> rusqlite::Result<Vec<DbSession>> {
    let optional = |name: &str| {
        if columns.contains(name) {
            name.to_owned()
        } else {
            "NULL".to_owned()
        }
    };
    // A session with a parent is a subagent run: counted against its parent
    // rather than listed. An archived one the user has already put away.
    let parent = if columns.contains("parent_id") {
        "AND parent_id IS NULL"
    } else {
        ""
    };
    let archived = if columns.contains("time_archived") {
        "AND time_archived IS NULL"
    } else {
        ""
    };
    let sql = format!(
        "SELECT id, {}, time_created, time_updated, {}
         FROM session
         WHERE directory IN ({}) {parent} {archived}
         ORDER BY CASE WHEN time_updated > 0 THEN time_updated ELSE time_created END DESC
         LIMIT ?",
        optional("title"),
        optional("model"),
        placeholders(directories.len()),
    );

    let mut statement = conn.prepare(&sql)?;
    let mut params: Vec<&dyn rusqlite::ToSql> = directories
        .iter()
        .map(|d| d as &dyn rusqlite::ToSql)
        .collect();
    let limit = i64::try_from(limit).unwrap_or(i64::MAX);
    params.push(&limit);

    let rows = statement.query_map(params.as_slice(), |row| {
        Ok(DbSession {
            id: row.get(0)?,
            title: row.get::<_, Option<String>>(1)?,
            created_ms: row.get(2)?,
            updated_ms: row.get(3)?,
            model: row
                .get::<_, Option<String>>(4)?
                .as_deref()
                .and_then(model_id),
            messages: 0,
            subagents: 0,
            last_reply: None,
        })
    })?;
    Ok(rows.flatten().collect())
}

/// How many subagent runs each of `ids` spawned.
fn subagent_counts(conn: &Connection, ids: &[String]) -> HashMap<String, u32> {
    let sql = format!(
        "SELECT parent_id, COUNT(*) FROM session
         WHERE parent_id IN ({}) GROUP BY parent_id",
        placeholders(ids.len())
    );
    counts(conn, &sql, ids)
}

/// How many conversation turns each of `ids` holds.
///
/// Roles live inside the JSON blob, so the count asks for them by name; a build
/// of SQLite without JSON support falls back to counting rows, which is the
/// same number for every opencode version seen so far (it records one row per
/// turn and nothing else).
fn message_counts(conn: &Connection, ids: &[String]) -> HashMap<String, u32> {
    let list = placeholders(ids.len());
    let sql = if reads_json(conn) {
        format!(
            "SELECT session_id, COUNT(*) FROM message
             WHERE session_id IN ({list})
               AND json_extract(data, '$.role') IN ('user','assistant')
             GROUP BY session_id"
        )
    } else {
        format!(
            "SELECT session_id, COUNT(*) FROM message
             WHERE session_id IN ({list}) GROUP BY session_id"
        )
    };
    counts(conn, &sql, ids)
}

/// Whether this build of SQLite can look inside a JSON column.
///
/// Asked once rather than inferred from an empty result: a session with no
/// turns yet is a real answer, not a missing feature.
fn reads_json(conn: &Connection) -> bool {
    conn.query_row(r#"SELECT json_extract('{"a":1}', '$.a')"#, [], |row| {
        row.get::<_, i64>(0)
    })
    .is_ok()
}

/// Run a `(key, count)` query over `ids`.
fn counts(conn: &Connection, sql: &str, ids: &[String]) -> HashMap<String, u32> {
    let Ok(mut statement) = conn.prepare(sql) else {
        return HashMap::new();
    };
    let params = rusqlite::params_from_iter(ids.iter());
    let Ok(rows) = statement.query_map(params, |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    }) else {
        return HashMap::new();
    };
    rows.flatten()
        .map(|(key, count)| (key, u32::try_from(count).unwrap_or(u32::MAX)))
        .collect()
}

/// The last thing the agent said in `session_id` that a user would recognise as
/// prose: the newest non-synthetic text part of the newest assistant message.
fn last_reply(conn: &Connection, session_id: &str) -> Option<String> {
    let sql = "SELECT p.data
               FROM (SELECT id, data, time_created FROM message
                     WHERE session_id = ?1
                     ORDER BY time_created DESC, id DESC
                     LIMIT ?2) m
               JOIN part p ON p.message_id = m.id
               WHERE json_extract(m.data, '$.role') = 'assistant'
                 AND json_extract(p.data, '$.type') = 'text'
                 AND COALESCE(json_extract(p.data, '$.synthetic'), 0) <> 1
               ORDER BY m.time_created DESC, p.time_created DESC, p.id DESC
               LIMIT 1";
    let tail = i64::try_from(MESSAGE_TAIL).unwrap_or(i64::MAX);
    let blob: String = conn
        .query_row(sql, rusqlite::params![session_id, tail], |row| row.get(0))
        .ok()?;
    part_text(&blob)
}

/// One turn of a recorded conversation, oldest first.
pub(crate) struct DbTurn {
    /// `"user"` or `"assistant"`, as opencode records the role.
    pub role: String,
    pub text: String,
}

/// The last `limit` turns of `session_id`, oldest first.
///
/// Still read-only: reading a shared database is fine, only writing is not.
/// The `LIMIT` is applied to the *newest* rows and the result reversed, so a
/// long run costs the tail rather than the whole conversation.
pub(crate) fn conversation(db: &Path, session_id: &str, limit: usize) -> Vec<DbTurn> {
    let Some(conn) = open(db) else {
        return Vec::new();
    };
    let sql = "SELECT json_extract(m.data, '$.role'), p.data
               FROM message m
               JOIN part p ON p.message_id = m.id
               WHERE m.session_id = ?1
                 AND json_extract(p.data, '$.type') = 'text'
                 AND COALESCE(json_extract(p.data, '$.synthetic'), 0) <> 1
               ORDER BY m.time_created DESC, m.id DESC, p.time_created DESC, p.id DESC
               LIMIT ?2";
    let Ok(mut statement) = conn.prepare(sql) else {
        return Vec::new();
    };
    let cap = i64::try_from(limit).unwrap_or(i64::MAX);
    let Ok(rows) = statement.query_map(rusqlite::params![session_id, cap], |row| {
        Ok((row.get::<_, Option<String>>(0)?, row.get::<_, String>(1)?))
    }) else {
        return Vec::new();
    };
    let mut turns: Vec<DbTurn> = rows
        .flatten()
        .filter_map(|(role, blob)| {
            Some(DbTurn {
                role: role?,
                text: part_text(&blob)?,
            })
        })
        .collect();
    turns.reverse();
    turns
}

/// The model that wrote the most recent agent message of `session_id`.
fn last_model(conn: &Connection, session_id: &str) -> Option<String> {
    let sql = "SELECT json_extract(data, '$.modelID') FROM message
               WHERE session_id = ?1
                 AND json_extract(data, '$.role') = 'assistant'
                 AND json_extract(data, '$.modelID') IS NOT NULL
               ORDER BY time_created DESC, id DESC
               LIMIT 1";
    let model: Option<String> = conn
        .query_row(sql, rusqlite::params![session_id], |row| row.get(0))
        .ok()?;
    model.filter(|model| !model.trim().is_empty())
}

/// The prose of one `part` row.
fn part_text(blob: &str) -> Option<String> {
    let value: Value = serde_json::from_str(blob).ok()?;
    let text = value.get("text")?.as_str()?.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

/// The model id inside a `session.model` blob, which opencode writes as
/// `{"id":"gpt-5.6","providerID":"openai"}` and older builds wrote as
/// `{"modelID":"…"}`.
fn model_id(blob: &str) -> Option<String> {
    let value: Value = serde_json::from_str(blob).ok()?;
    let id = value
        .get("id")
        .or_else(|| value.get("modelID"))?
        .as_str()?
        .trim();
    (!id.is_empty()).then(|| id.to_owned())
}

/// `?,?,…` for an `IN` list of `count` bound values.
fn placeholders(count: usize) -> String {
    std::iter::repeat_n("?", count)
        .collect::<Vec<_>>()
        .join(",")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a database with the current opencode schema, reduced to the
    /// columns this module reads.
    fn write_db(path: &Path) -> Connection {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(
            "CREATE TABLE session (
                 id TEXT PRIMARY KEY,
                 parent_id TEXT,
                 directory TEXT NOT NULL,
                 title TEXT NOT NULL,
                 time_created INTEGER NOT NULL,
                 time_updated INTEGER NOT NULL,
                 time_archived INTEGER,
                 model TEXT
             );
             CREATE TABLE message (
                 id TEXT PRIMARY KEY,
                 session_id TEXT NOT NULL,
                 time_created INTEGER NOT NULL,
                 data TEXT NOT NULL
             );
             CREATE TABLE part (
                 id TEXT PRIMARY KEY,
                 message_id TEXT NOT NULL,
                 session_id TEXT NOT NULL,
                 time_created INTEGER NOT NULL,
                 data TEXT NOT NULL
             );",
        )
        .unwrap();
        conn
    }

    fn add_session(conn: &Connection, id: &str, directory: &str, title: &str, updated: i64) {
        conn.execute(
            "INSERT INTO session (id, directory, title, time_created, time_updated)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![id, directory, title, updated - 1_000, updated],
        )
        .unwrap();
    }

    fn add_message(conn: &Connection, id: &str, session: &str, role: &str, at: i64) {
        let data = format!(r#"{{"role":"{role}","modelID":"gpt-5.6-sol"}}"#);
        conn.execute(
            "INSERT INTO message (id, session_id, time_created, data) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![id, session, at, data],
        )
        .unwrap();
    }

    fn add_part(conn: &Connection, id: &str, message: &str, session: &str, data: &str, at: i64) {
        conn.execute(
            "INSERT INTO part (id, message_id, session_id, time_created, data)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![id, message, session, at, data],
        )
        .unwrap();
    }

    fn text_part(text: &str) -> String {
        format!(r#"{{"type":"text","text":"{text}"}}"#)
    }

    #[test]
    fn reads_a_session_with_its_turns_reply_and_model() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("opencode.db");
        let conn = write_db(&path);
        add_session(&conn, "ses_a", "/w/one", "A plan", 2_000);
        add_message(&conn, "msg_1", "ses_a", "user", 1_100);
        add_message(&conn, "msg_2", "ses_a", "assistant", 1_200);
        add_part(&conn, "prt_1", "msg_2", "ses_a", &text_part("Done."), 1_210);
        drop(conn);

        let found = sessions(&path, &["/w/one".to_owned()], 60);

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, "ses_a");
        assert_eq!(found[0].title.as_deref(), Some("A plan"));
        assert_eq!(found[0].messages, 2);
        assert_eq!(found[0].last_reply.as_deref(), Some("Done."));
        assert_eq!(found[0].model.as_deref(), Some("gpt-5.6-sol"));
        assert_eq!(found[0].updated_ms, 2_000);
    }

    /// The three rows that must never be listed: another directory's, a
    /// subagent's, and one the user archived.
    #[test]
    fn only_top_level_live_sessions_of_the_directory_are_listed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("opencode.db");
        let conn = write_db(&path);
        add_session(&conn, "ses_a", "/w/one", "Mine", 3_000);
        add_session(&conn, "ses_b", "/w/two", "Elsewhere", 4_000);
        add_session(&conn, "ses_c", "/w/one", "Archived", 5_000);
        conn.execute(
            "UPDATE session SET time_archived = 9 WHERE id = 'ses_c'",
            [],
        )
        .unwrap();
        add_session(&conn, "ses_d", "/w/one", "Subagent", 6_000);
        conn.execute(
            "UPDATE session SET parent_id = 'ses_a' WHERE id = 'ses_d'",
            [],
        )
        .unwrap();
        drop(conn);

        let found = sessions(&path, &["/w/one".to_owned()], 60);

        let ids: Vec<&str> = found.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["ses_a"]);
        // The subagent is not listed, but it is counted against its parent.
        assert_eq!(found[0].subagents, 1);
    }

    /// The same directory reaches us in more than one spelling; both must find
    /// the same history, and neither may list it twice.
    #[test]
    fn any_recorded_spelling_of_the_directory_matches_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("opencode.db");
        let conn = write_db(&path);
        add_session(&conn, "ses_a", "/private/w/one", "Mine", 3_000);
        drop(conn);

        let found = sessions(
            &path,
            &["/w/one".to_owned(), "/private/w/one".to_owned()],
            60,
        );

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, "ses_a");
    }

    #[test]
    fn newest_first_and_capped_by_the_limit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("opencode.db");
        let conn = write_db(&path);
        for index in 0..5 {
            add_session(
                &conn,
                &format!("ses_{index}"),
                "/w/one",
                "T",
                1_000 + i64::from(index),
            );
        }
        drop(conn);

        let found = sessions(&path, &["/w/one".to_owned()], 2);

        let ids: Vec<&str> = found.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["ses_4", "ses_3"]);
    }

    /// A tool-only run has no prose to preview, and a synthetic part is not
    /// something the agent said.
    #[test]
    fn synthetic_and_non_text_parts_are_not_a_reply() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("opencode.db");
        let conn = write_db(&path);
        add_session(&conn, "ses_a", "/w/one", "Tools only", 2_000);
        add_message(&conn, "msg_1", "ses_a", "assistant", 1_200);
        add_part(
            &conn,
            "prt_1",
            "msg_1",
            "ses_a",
            r#"{"type":"tool","text":"ran ls"}"#,
            1_210,
        );
        add_part(
            &conn,
            "prt_2",
            "msg_1",
            "ses_a",
            r#"{"type":"text","text":"caveat","synthetic":true}"#,
            1_220,
        );
        drop(conn);

        let found = sessions(&path, &["/w/one".to_owned()], 60);

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].last_reply, None);
    }

    /// A file that is not an opencode database yields nothing, and yields it
    /// without an error: the JSON scanner still runs.
    #[test]
    fn a_foreign_or_missing_database_is_simply_empty() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("nope.db");
        assert!(sessions(&missing, &["/w/one".to_owned()], 60).is_empty());

        let foreign = dir.path().join("other.db");
        let conn = Connection::open(&foreign).unwrap();
        conn.execute_batch("CREATE TABLE unrelated (id TEXT)")
            .unwrap();
        drop(conn);
        assert!(sessions(&foreign, &["/w/one".to_owned()], 60).is_empty());
    }

    #[test]
    fn the_data_directory_yields_opencode_databases_only() {
        let dir = tempfile::tempdir().unwrap();
        for name in [
            "opencode.db",
            "opencode-migration.db",
            "opencode.db-wal",
            "opencode.db-shm",
            "notes.db",
            "opencode.json",
        ] {
            std::fs::write(dir.path().join(name), b"").unwrap();
        }
        std::fs::create_dir(dir.path().join("opencode-dir.db")).unwrap();

        let found: Vec<String> = databases(dir.path())
            .iter()
            .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();

        assert_eq!(found, ["opencode-migration.db", "opencode.db"]);
    }

    #[test]
    fn the_database_is_opened_read_only() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("opencode.db");
        let conn = write_db(&path);
        add_session(&conn, "ses_a", "/w/one", "Mine", 1_000);
        drop(conn);

        let conn = open(&path).expect("opens");
        let error = conn
            .execute("DELETE FROM session", [])
            .expect_err("must refuse to write");
        assert!(
            error.to_string().contains("readonly"),
            "unexpected error: {error}"
        );
    }
}
