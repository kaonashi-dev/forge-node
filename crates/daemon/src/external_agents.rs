//! Discovery of agent sessions the daemon did not launch.
//!
//! Agent CLIs keep their own per-directory transcript history on disk. We
//! surface that history in the GUI's session panel without ever attaching to
//! it: for every directory Forge knows — each project root *and* each of its
//! worktrees — we locate the provider's transcript store, read the cheap
//! metadata out of the recent transcripts, and return a read-only
//! [`ExternalAgentSession`] per run.
//!
//! Two providers are supported:
//!
//! - **Claude Code**: transcripts at `~/.claude/projects/<slug>/<sessionId>.jsonl`,
//!   where `<slug>` is the directory with every `/` and `.` replaced by `-`.
//!   Each `.jsonl` is a stream of JSON records; we scan for the recorded
//!   `cwd`/`gitBranch`, the min/max `timestamp`, the latest `aiTitle`, the
//!   turn count, and the last assistant message with the model that wrote it.
//!   Subagent runs live beside it in `<sessionId>/subagents/*.jsonl`.
//! - **opencode**: two layouts, both read. Current versions (≥ 1.17) record
//!   every session in `~/.local/share/opencode/opencode.db`, which
//!   [`crate::opencode_db`] queries directly. Older ones wrote the tree below,
//!   which upgraded machines still carry with its last pre-upgrade contents —
//!   so it is scanned too, and a session found in both is listed once.
//!
//! - **opencode (legacy tree)**: metadata at `~/.local/share/opencode/storage`. A
//!   `project/<hash>.json` records its `worktree`; that project's sessions are
//!   one JSON file each under `session/<hash>/`, carrying an `id`, `title`,
//!   `directory` and `time.created`/`time.updated` (epoch millis). The
//!   conversation itself is split across `message/<sessionID>/*.json` and
//!   `part/<messageID>/*.json`, which is where the turn count and the last
//!   agent message come from. A session with a `parentID` is a subagent run:
//!   it is counted against its parent rather than listed on its own.
//!
//! Every store is read once per **account**: the default one, plus one per
//! launch profile that moved the provider's config directory (§13.4). A
//! profile is how a user runs a second login, and its transcripts live under
//! its own directory — scanning only the default one is why a `Personal`
//! profile's history used to be invisible here.
//!
//! Everything here is best-effort: an unreadable file, a malformed line or a
//! missing directory yields fewer results, never an error. It runs off the
//! daemon lock (see `Daemon::snapshot`) so transcript IO never blocks state,
//! and only the [`SCAN_LIMIT`] most recently touched transcripts per directory
//! are read — a year of history must not make opening the panel slower.

use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::{Hash, Hasher};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use domain::{
    AgentProfile, AgentProfileId, ExternalAgentSession, ExternalTranscript, Project, ProjectId,
    Timestamp, TranscriptStore, Workspace, WorkspaceId,
};
use serde_json::Value;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

/// Provider slug reported on Claude Code sessions.
const CLAUDE_PROVIDER: &str = "claude";
/// Provider slug reported on opencode sessions.
const OPENCODE_PROVIDER: &str = "opencode";
/// Transcripts read per provider and directory, most recently modified first.
/// Older runs stay on disk and stay resumable in the CLI; they just do not
/// earn the IO of being listed.
const SCAN_LIMIT: usize = 60;
/// Longest conversation preview kept, in characters.
const PREVIEW_CHARS: usize = 220;
/// Longest card title kept, in characters.
const TITLE_CHARS: usize = 80;
/// How many messages back from the end of an opencode session we look for the
/// last agent reply before giving up. A run that ends in a long tail of tool
/// calls gets no preview rather than an expensive one.
const OPENCODE_TAIL: usize = 24;

/// How long a completed scan is reused before the next one runs.
///
/// `GetSnapshot` used to rescan unconditionally, so every reconnect — and the
/// GUI reconnects on every daemon restart and every dropped socket — re-read up
/// to [`SCAN_LIMIT`] JSONL transcripts per provider per directory, line by line,
/// while the caller waited. Agent transcripts change on the timescale of a
/// conversation turn, so a few seconds of staleness is invisible and the
/// repeated IO is not.
const CACHE_TTL: Duration = Duration::from_secs(10);

/// A completed discovery pass, reusable until it goes stale.
///
/// Keyed by a fingerprint of the directories that fed it: adding a project or
/// creating a worktree changes the answer immediately rather than after the
/// TTL, because a scan of different roots is a different question, not a stale
/// answer to the same one.
#[derive(Default)]
pub struct Cache {
    scanned_at: Option<Instant>,
    fingerprint: u64,
    sessions: Vec<ExternalAgentSession>,
}

impl Cache {
    /// Discover external sessions, reusing the last pass when it is still fresh
    /// and was taken over the same directories.
    ///
    /// The scan itself runs while this is locked, so two concurrent snapshots
    /// cost one scan rather than two — the second waits and gets the fresh
    /// result. The daemon's core lock must *not* be held across this call: the
    /// whole point is that transcript IO never blocks daemon state.
    pub fn discover(
        &mut self,
        projects: &[Project],
        workspaces: &[Workspace],
        profiles: &[AgentProfile],
    ) -> Vec<ExternalAgentSession> {
        let fingerprint = fingerprint(projects, workspaces, profiles);
        let fresh = self.fingerprint == fingerprint
            && self.scanned_at.is_some_and(|at| at.elapsed() < CACHE_TTL);
        if !fresh {
            self.sessions = discover(projects, workspaces, profiles);
            self.scanned_at = Some(Instant::now());
            self.fingerprint = fingerprint;
        }
        self.sessions.clone()
    }

    /// The discovered run matching an identity, or `None`.
    ///
    /// Answers from the last pass rather than rescanning, so a client can only
    /// name a run the daemon itself found — which is what keeps a path off the
    /// wire. A cache that has never run answers `None`, and the caller reports
    /// it as a run that is not there.
    #[must_use]
    pub fn find(
        &self,
        session_id: &str,
        provider: &str,
        profile_id: Option<AgentProfileId>,
    ) -> Option<ExternalAgentSession> {
        self.sessions
            .iter()
            .find(|session| {
                session.session_id == session_id
                    && session.provider == provider
                    && session.profile_id == profile_id
            })
            .cloned()
    }

    /// Drop the cached pass, so the next call rescans.
    ///
    /// The fingerprint already covers the roots changing; this is for the cases
    /// where the *contents* are known to have moved — a session the daemon
    /// itself just ended, say — and waiting out the TTL would show the user
    /// stale history right after they acted.
    pub fn invalidate(&mut self) {
        self.scanned_at = None;
    }
}

/// A stable digest of the directories a scan covers — the ones it looks *for*
/// and the accounts it looks *in*, so saving a profile shows its history at
/// once rather than after the TTL.
fn fingerprint(projects: &[Project], workspaces: &[Workspace], profiles: &[AgentProfile]) -> u64 {
    // Sorted so the hash does not depend on `HashMap` iteration order, which
    // varies run to run and would defeat the cache entirely.
    let mut keys: Vec<&Path> = projects
        .iter()
        .map(|p| p.root_path.as_path())
        .chain(workspaces.iter().map(|w| w.path.as_path()))
        .chain(profiles.iter().filter_map(|p| p.config_dir.as_deref()))
        .collect();
    keys.sort_unstable();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    keys.len().hash(&mut hasher);
    for key in keys {
        key.hash(&mut hasher);
    }
    hasher.finish()
}

/// A directory whose agent history is attributed to one project, and to one
/// workspace when Forge knows which.
struct Root {
    project_id: ProjectId,
    workspace_id: Option<WorkspaceId>,
    path: PathBuf,
}

/// Discover every external agent session for the given projects.
///
/// `workspaces` supplies the worktree directories: an agent run started inside
/// a worktree records that path, not the project root, so without them the
/// history of every worktree would be invisible. The returned list is unsorted
/// (the GUI sorts the history panel by time).
#[must_use]
pub fn discover(
    projects: &[Project],
    workspaces: &[Workspace],
    profiles: &[AgentProfile],
) -> Vec<ExternalAgentSession> {
    let Some(home) = home_dir() else {
        return Vec::new();
    };
    // `CLAUDE_CONFIG_DIR` for Claude Code and `XDG_DATA_HOME` for opencode are
    // what a profile moves, so each account is a store of its own (§13.4).
    let claude = accounts(&home.join(".claude"), CLAUDE_PROVIDER, profiles, &home, "");
    let opencode = accounts(
        &opencode_data_dir(&home),
        OPENCODE_PROVIDER,
        profiles,
        &home,
        "opencode",
    );

    let mut out = Vec::new();
    for root in roots(projects, workspaces) {
        for account in &claude {
            let found = out.len();
            discover_claude(&account.dir, &root, &mut out);
            stamp_account(&mut out[found..], account);
        }
        for account in &opencode {
            let found = out.len();
            discover_opencode(&account.dir, &root, &mut out);
            stamp_account(&mut out[found..], account);
        }
    }
    // The same session can only be found twice if two accounts resolve to one
    // directory — a profile pointing at the default one. List it once.
    let mut seen = HashSet::new();
    out.retain(|session| seen.insert((session.provider.clone(), session.session_id.clone())));
    out
}

/// One store of transcripts, and the login it belongs to.
struct Account {
    /// The directory the provider keeps its data in.
    dir: PathBuf,
    /// The profile that moved it there, or `None` for the default account.
    profile: Option<AgentProfileId>,
}

/// Every directory one provider keeps its data in: `default`, plus a profile's
/// resolved config directory with `leaf` appended, for each profile of that
/// provider (§13.4). Duplicate directories are dropped by canonical path, so a
/// profile pointing at the default directory is the default account and not a
/// second one.
fn accounts(
    default: &Path,
    provider: &str,
    profiles: &[AgentProfile],
    home: &Path,
    leaf: &str,
) -> Vec<Account> {
    let mut seen = HashSet::new();
    let mut stores = Vec::new();
    let dirs = std::iter::once(Account {
        dir: default.to_path_buf(),
        profile: None,
    })
    .chain(profiles.iter().filter_map(|p| {
        let dir = p.config_dir.as_deref()?;
        (p.provider_id.as_str() == provider).then(|| Account {
            dir: AgentProfile::resolve_config_dir(dir, home).join(leaf),
            profile: Some(p.id),
        })
    }));
    for account in dirs {
        if seen.insert(dedup_key(&account.dir)) {
            stores.push(account);
        }
    }
    stores
}

/// Attribute the sessions a single account's scan just produced to that
/// account.
///
/// Stamped here rather than threaded through every parser: which login a
/// transcript belongs to is a fact about the *store* it was read from, and the
/// parsers only ever see one file.
fn stamp_account(found: &mut [ExternalAgentSession], account: &Account) {
    for session in found {
        session.profile_id = account.profile;
    }
}

/// Every directory worth scanning, each attributed to a project and — where
/// one exists — the workspace that owns it.
///
/// Workspaces come first so a directory Forge tracks as a workspace keeps that
/// attribution; a project root with no workspace row still gets scanned, which
/// is what covers a project added before its main workspace was created.
fn roots(projects: &[Project], workspaces: &[Workspace]) -> Vec<Root> {
    let known: HashSet<ProjectId> = projects.iter().map(|p| p.id).collect();
    let mut seen = HashSet::new();
    let mut roots = Vec::new();

    for workspace in workspaces {
        if !known.contains(&workspace.project_id) {
            continue;
        }
        if seen.insert(dedup_key(&workspace.path)) {
            roots.push(Root {
                project_id: workspace.project_id,
                workspace_id: Some(workspace.id),
                path: workspace.path.clone(),
            });
        }
    }
    for project in projects {
        if seen.insert(dedup_key(&project.root_path)) {
            roots.push(Root {
                project_id: project.id,
                workspace_id: None,
                path: project.root_path.clone(),
            });
        }
    }
    roots
}

/// Identity of a directory for de-duplication: the canonical path when the
/// directory exists, the path as given otherwise.
fn dedup_key(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

// ---------------------------------------------------------------------------
// Claude Code
// ---------------------------------------------------------------------------

/// Scan Claude Code transcripts for one directory, in one account's store
/// (`~/.claude`, or a profile's config directory).
fn discover_claude(store: &Path, root: &Root, out: &mut Vec<ExternalAgentSession>) {
    let dir = store.join("projects").join(claude_slug(&root.path));
    for path in recent_files(&dir, "jsonl") {
        if let Some(session) = parse_transcript(&path, root) {
            out.push(session);
        }
    }
}

/// Claude Code's project-directory slug: the absolute `cwd` with `/` and `.`
/// collapsed to `-` (`/Users/me/dev/app` → `-Users-me-dev-app`).
fn claude_slug(root: &Path) -> String {
    root.to_string_lossy()
        .chars()
        .map(|c| if c == '/' || c == '.' { '-' } else { c })
        .collect()
}

/// Cheap metadata scanned out of one transcript.
#[derive(Default)]
struct Scan {
    cwd: Option<PathBuf>,
    branch: Option<String>,
    ai_title: Option<String>,
    first_user: Option<String>,
    last_assistant: Option<String>,
    model: Option<String>,
    messages: u32,
    first_ts: Option<OffsetDateTime>,
    last_ts: Option<OffsetDateTime>,
}

/// Read one `.jsonl` transcript into an [`ExternalAgentSession`], or `None` if
/// it is unreadable, empty, or belongs to a different `cwd` than the directory
/// being scanned (the slug is lossy, so distinct paths can collide onto one
/// folder).
fn parse_transcript(path: &Path, root: &Root) -> Option<ExternalAgentSession> {
    let file = std::fs::File::open(path).ok()?;
    let mut scan = Scan::default();
    // Streamed rather than slurped: a long-running session's transcript runs to
    // megabytes, and only one line of it is ever live at a time.
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        fold_line(line.trim(), &mut scan);
    }

    // Reject transcripts recorded in a different directory that merely slug to
    // the same folder. A transcript with no recorded cwd is kept (best effort).
    if let Some(cwd) = &scan.cwd {
        if !same_path(cwd, &root.path) {
            return None;
        }
    }

    let session_id = path.file_stem()?.to_str()?.to_owned();
    let title = scan
        .ai_title
        .filter(|t| !t.trim().is_empty())
        .or_else(|| scan.first_user.map(|u| truncate_title(&u)))
        .unwrap_or_else(|| session_id.clone());

    let mtime = file_mtime(path);
    let started_at = scan.first_ts.or(mtime).unwrap_or_else(now);
    let last_activity = scan.last_ts.or(mtime).unwrap_or(started_at);

    Some(ExternalAgentSession {
        session_id: session_id.clone(),
        profile_id: None,
        project_id: root.project_id,
        workspace_id: root.workspace_id,
        provider: CLAUDE_PROVIDER.to_owned(),
        title,
        branch: scan.branch,
        preview: scan.last_assistant.as_deref().map(fold_preview),
        model: scan.model,
        message_count: scan.messages,
        subagent_count: claude_subagent_count(path, &session_id),
        transcript_path: path.to_owned(),
        store: TranscriptStore::File,
        started_at: Timestamp(started_at),
        last_activity: Timestamp(last_activity),
    })
}

/// Fold one raw transcript line into the running [`Scan`].
///
/// Most of a transcript's bulk is records this scan reads nothing out of —
/// file snapshots, attachments, tool results — so the line is only parsed as
/// JSON once a substring check says it could matter. The check can only ever
/// skip work: a key that appears nowhere in the line appears nowhere in the
/// record it encodes.
fn fold_line(line: &str, scan: &mut Scan) {
    if line.is_empty() {
        return;
    }
    if let Some(ts) = line_timestamp(line) {
        if scan.first_ts.is_none_or(|first| ts < first) {
            scan.first_ts = Some(ts);
        }
        if scan.last_ts.is_none_or(|last| ts > last) {
            scan.last_ts = Some(ts);
        }
    }
    if !is_interesting(line, scan) {
        return;
    }
    if let Ok(value) = serde_json::from_str::<Value>(line) {
        scan_record(&value, scan);
    }
}

/// Whether a raw record mentions anything the [`Scan`] would read.
fn is_interesting(line: &str, scan: &Scan) -> bool {
    line.contains(r#""type":"user""#)
        || line.contains(r#""type":"assistant""#)
        || line.contains(r#""aiTitle""#)
        || (scan.cwd.is_none() && line.contains(r#""cwd""#))
        || (scan.branch.is_none() && line.contains(r#""gitBranch""#))
}

/// The RFC 3339 instant a record carries, read straight out of the raw line.
///
/// A `timestamp` value is plain ASCII with no escapes, so recovering it needs
/// no parser — and every record has one, including the ones
/// [`is_interesting`] rejects. Reading it here is what keeps the session's
/// first and last activity exact while most of the file goes unparsed.
fn line_timestamp(line: &str) -> Option<OffsetDateTime> {
    const KEY: &str = r#""timestamp":""#;
    let value = &line[line.find(KEY)? + KEY.len()..];
    let end = value.find('"')?;
    OffsetDateTime::parse(&value[..end], &Rfc3339).ok()
}

/// Fold one parsed transcript record into the running [`Scan`].
fn scan_record(value: &Value, scan: &mut Scan) {
    if scan.cwd.is_none() {
        if let Some(cwd) = value.get("cwd").and_then(Value::as_str) {
            scan.cwd = Some(PathBuf::from(cwd));
        }
    }
    if scan.branch.is_none() {
        if let Some(branch) = value.get("gitBranch").and_then(Value::as_str) {
            if !branch.is_empty() {
                scan.branch = Some(branch.to_owned());
            }
        }
    }
    // The title evolves; keep the latest non-empty one.
    if let Some(title) = value.get("aiTitle").and_then(Value::as_str) {
        if !title.trim().is_empty() {
            scan.ai_title = Some(title.to_owned());
        }
    }

    match value.get("type").and_then(Value::as_str) {
        Some("user") if is_typed_prompt(value) => {
            scan.messages += 1;
            if scan.first_user.is_none() {
                if let Some(text) = user_text(value) {
                    scan.first_user = Some(text);
                }
            }
        }
        Some("assistant") => {
            scan.messages += 1;
            if let Some(text) = assistant_text(value) {
                scan.last_assistant = Some(text);
            }
            if let Some(model) = value.pointer("/message/model").and_then(Value::as_str) {
                scan.model = Some(model.to_owned());
            }
        }
        _ => {}
    }
}

/// Whether a `user` record is something a person typed. Claude Code writes tool
/// results and injected context back into the transcript as user records;
/// counting those would report a conversation many times longer than the one
/// that happened.
fn is_typed_prompt(value: &Value) -> bool {
    value.get("toolUseResult").is_none()
        && value.get("isMeta").and_then(Value::as_bool) != Some(true)
}

/// Extract the first plain-text block of a `user` message, skipping the command
/// wrappers and tool results that carry no readable prompt.
fn user_text(value: &Value) -> Option<String> {
    let content = value.get("message")?.get("content")?;
    let text = match content {
        Value::String(s) => s.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .find_map(|b| b.get("text").and_then(Value::as_str))
            .map(str::to_owned)?,
        _ => return None,
    };
    let text = text.trim();
    if text.is_empty() || text.starts_with('<') {
        return None;
    }
    Some(text.to_owned())
}

/// The prose of an `assistant` record: its last `text` block. Thinking and
/// tool-use blocks are skipped — the preview is what the agent *said*.
fn assistant_text(value: &Value) -> Option<String> {
    let content = value.get("message")?.get("content")?;
    match content {
        Value::String(s) => non_empty(s),
        Value::Array(blocks) => blocks.iter().rev().find_map(|block| {
            if block.get("type").and_then(Value::as_str) != Some("text") {
                return None;
            }
            block
                .get("text")
                .and_then(Value::as_str)
                .and_then(non_empty)
        }),
        _ => None,
    }
}

/// Subagent transcripts recorded beside a Claude session, in
/// `<transcript dir>/<sessionId>/subagents/*.jsonl`.
fn claude_subagent_count(transcript: &Path, session_id: &str) -> u32 {
    let dir = match transcript.parent() {
        Some(parent) => parent.join(session_id).join("subagents"),
        None => return 0,
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let count = entries
        .flatten()
        .filter(|entry| has_extension(&entry.path(), "jsonl"))
        .count();
    u32::try_from(count).unwrap_or(u32::MAX)
}

// ---------------------------------------------------------------------------
// opencode
// ---------------------------------------------------------------------------

/// Scan opencode sessions for one directory.
///
/// opencode groups sessions by an opaque project hash, so we first find the
/// `project/<hash>.json` whose `worktree` is this directory, then read every
/// session file under `session/<hash>/`. Sessions carrying a `parentID` are
/// subagent runs: they are tallied against their parent instead of listed.
fn discover_opencode(data: &Path, root: &Root, out: &mut Vec<ExternalAgentSession>) {
    // The database is the current layout and the JSON tree the old one, and an
    // upgraded machine has both: the tree stops being written but stays on
    // disk, so a session recorded in each must be listed once, from the source
    // that is still current.
    let recorded = discover_opencode_databases(data, root, out);
    discover_opencode_storage(&data.join("storage"), root, &recorded, out);
}

/// Read every opencode database in `data` for this directory (opencode ≥ 1.17),
/// returning the session ids listed from them.
fn discover_opencode_databases(
    data: &Path,
    root: &Root,
    out: &mut Vec<ExternalAgentSession>,
) -> HashSet<String> {
    let mut recorded = HashSet::new();
    let directories = path_spellings(&root.path);
    for db in crate::opencode_db::databases(data) {
        for session in crate::opencode_db::sessions(&db, &directories, SCAN_LIMIT) {
            if !recorded.insert(session.id.clone()) {
                continue;
            }
            out.push(opencode_db_session(&db, session, root));
        }
    }
    recorded
}

/// The forms a directory may have been recorded in: as Forge knows it, and as
/// the filesystem canonicalizes it. opencode stores the path it was started
/// with, and `/tmp` against `/private/tmp` is the difference between a full
/// history and an empty panel.
fn path_spellings(path: &Path) -> Vec<String> {
    let mut spellings = vec![path.to_string_lossy().into_owned()];
    if let Ok(canonical) = std::fs::canonicalize(path) {
        let canonical = canonical.to_string_lossy().into_owned();
        if !spellings.contains(&canonical) {
            spellings.push(canonical);
        }
    }
    spellings
}

/// Turn one database row into the card the panel draws.
fn opencode_db_session(
    db: &Path,
    session: crate::opencode_db::DbSession,
    root: &Root,
) -> ExternalAgentSession {
    let started_at = millis_to_ts(session.created_ms).unwrap_or_else(now);
    let last_activity = millis_to_ts(session.updated_ms).unwrap_or(started_at);
    let title = session
        .title
        .as_deref()
        .and_then(non_empty)
        .map(|title| truncate_title(&title))
        .unwrap_or_else(|| session.id.clone());

    ExternalAgentSession {
        session_id: session.id,
        profile_id: None,
        project_id: root.project_id,
        workspace_id: root.workspace_id,
        provider: OPENCODE_PROVIDER.to_owned(),
        title,
        branch: None,
        preview: session.last_reply.as_deref().map(fold_preview),
        model: session.model,
        message_count: session.messages,
        subagent_count: session.subagents,
        // There is no per-session file any more; the database is where this run
        // is recorded, and it is what "reveal the transcript" has to point at.
        transcript_path: db.to_path_buf(),
        store: TranscriptStore::SharedDatabase,
        started_at: Timestamp(started_at),
        last_activity: Timestamp(last_activity),
    }
}

/// Read the legacy per-session JSON tree (opencode < 1.17), skipping sessions
/// already listed from a database.
fn discover_opencode_storage(
    base: &Path,
    root: &Root,
    recorded: &HashSet<String>,
    out: &mut Vec<ExternalAgentSession>,
) {
    let Some(hash) = opencode_project_hash(base, &root.path) else {
        return;
    };

    let mut top_level: Vec<(PathBuf, Value)> = Vec::new();
    let mut subagents: HashMap<String, u32> = HashMap::new();
    for path in json_files(&base.join("session").join(&hash)) {
        let Some(value) = read_json(&path) else {
            continue;
        };
        match value.get("parentID").and_then(Value::as_str) {
            Some(parent) => *subagents.entry(parent.to_owned()).or_default() += 1,
            None => top_level.push((path, value)),
        }
    }

    // Newest first, then capped: the conversation of each surviving session
    // costs a directory listing and a few small reads.
    top_level.sort_by_key(|(_, value)| std::cmp::Reverse(opencode_updated(value)));
    top_level.truncate(SCAN_LIMIT);

    for (path, value) in top_level {
        let Some(session) = parse_opencode_session(base, &path, &value, root, &subagents) else {
            continue;
        };
        if recorded.contains(&session.session_id) {
            continue;
        }
        out.push(session);
    }
}

/// The opencode project hash whose `worktree` matches `root`, if any.
fn opencode_project_hash(base: &Path, root: &Path) -> Option<String> {
    for path in json_files(&base.join("project")) {
        let Some(value) = read_json(&path) else {
            continue;
        };
        if let Some(worktree) = value.get("worktree").and_then(Value::as_str) {
            if same_path(Path::new(worktree), root) {
                return path.file_stem()?.to_str().map(str::to_owned);
            }
        }
    }
    None
}

/// Read one already-parsed opencode session record into an
/// [`ExternalAgentSession`], or `None` if it was recorded in a different
/// directory than the one being scanned (one opencode project spans a
/// repository and its worktrees).
fn parse_opencode_session(
    base: &Path,
    path: &Path,
    value: &Value,
    root: &Root,
    subagents: &HashMap<String, u32>,
) -> Option<ExternalAgentSession> {
    if let Some(directory) = value.get("directory").and_then(Value::as_str) {
        if !same_path(Path::new(directory), &root.path) {
            return None;
        }
    }

    let session_id = value.get("id").and_then(Value::as_str)?.to_owned();
    let title = value
        .get("title")
        .and_then(Value::as_str)
        .filter(|t| !t.trim().is_empty())
        .map(truncate_title)
        .unwrap_or_else(|| session_id.clone());

    let created = value
        .pointer("/time/created")
        .and_then(Value::as_i64)
        .and_then(millis_to_ts);
    let updated = opencode_updated(value).and_then(millis_to_ts);
    let mtime = file_mtime(path);
    let started_at = created.or(mtime).unwrap_or_else(now);
    let last_activity = updated.or(created).or(mtime).unwrap_or(started_at);

    let conversation = opencode_conversation(base, &session_id);

    Some(ExternalAgentSession {
        session_id: session_id.clone(),
        profile_id: None,
        project_id: root.project_id,
        workspace_id: root.workspace_id,
        provider: OPENCODE_PROVIDER.to_owned(),
        title,
        branch: None,
        preview: conversation.last_reply.as_deref().map(fold_preview),
        model: conversation.model,
        message_count: conversation.messages,
        subagent_count: subagents.get(&session_id).copied().unwrap_or(0),
        transcript_path: path.to_owned(),
        store: TranscriptStore::File,
        started_at: Timestamp(started_at),
        last_activity: Timestamp(last_activity),
    })
}

/// An opencode session's last-touched instant in epoch millis, falling back to
/// its creation time. Used both to rank sessions and to date them.
fn opencode_updated(value: &Value) -> Option<i64> {
    value
        .pointer("/time/updated")
        .and_then(Value::as_i64)
        .or_else(|| value.pointer("/time/created").and_then(Value::as_i64))
}

/// What one opencode session's message store says about the conversation.
#[derive(Default)]
struct Conversation {
    messages: u32,
    last_reply: Option<String>,
    model: Option<String>,
}

/// Read the turn count, the last agent reply and the model that wrote it out
/// of `message/<sessionID>/` and `part/<messageID>/`.
///
/// Message ids are time-ordered, so walking the directory listing backwards
/// walks the conversation backwards; we stop at the first assistant message
/// that actually said something, or after [`OPENCODE_TAIL`] messages.
fn opencode_conversation(base: &Path, session_id: &str) -> Conversation {
    let mut ids: Vec<String> = json_files(&base.join("message").join(session_id))
        .iter()
        .filter_map(|path| path.file_stem()?.to_str().map(str::to_owned))
        .collect();
    ids.sort();

    let mut conversation = Conversation {
        messages: u32::try_from(ids.len()).unwrap_or(u32::MAX),
        ..Conversation::default()
    };

    for id in ids.iter().rev().take(OPENCODE_TAIL) {
        let path = base
            .join("message")
            .join(session_id)
            .join(format!("{id}.json"));
        let Some(value) = read_json(&path) else {
            continue;
        };
        if value.get("role").and_then(Value::as_str) != Some("assistant") {
            continue;
        }
        if conversation.model.is_none() {
            conversation.model = value
                .get("modelID")
                .and_then(Value::as_str)
                .map(str::to_owned);
        }
        if let Some(text) = opencode_message_text(base, id) {
            conversation.last_reply = Some(text);
            break;
        }
    }
    conversation
}

/// The prose of one opencode message: the last non-synthetic `text` part.
/// Tool, reasoning and step parts carry no reply the user would recognise.
fn opencode_message_text(base: &Path, message_id: &str) -> Option<String> {
    let mut parts = json_files(&base.join("part").join(message_id));
    parts.sort();
    parts.iter().rev().find_map(|path| {
        let value = read_json(path)?;
        if value.get("type").and_then(Value::as_str) != Some("text") {
            return None;
        }
        if value.get("synthetic").and_then(Value::as_bool) == Some(true) {
            return None;
        }
        value
            .get("text")
            .and_then(Value::as_str)
            .and_then(non_empty)
    })
}

/// opencode's *default* data directory. It follows XDG on every platform
/// (including macOS), so it is `$XDG_DATA_HOME/opencode` or
/// `~/.local/share/opencode`; a profile's directory replaces it, because a
/// profile is what sets `XDG_DATA_HOME` for the launch (§13.4). The session
/// databases sit here; the legacy JSON tree is `storage/` inside.
fn opencode_data_dir(home: &Path) -> PathBuf {
    match std::env::var_os("XDG_DATA_HOME") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir).join("opencode"),
        _ => home.join(".local").join("share").join("opencode"),
    }
}

// ---------------------------------------------------------------------------
// Reading and removing one discovered run
// ---------------------------------------------------------------------------

/// Turns a transcript read returns when the caller names no number.
pub const DEFAULT_EXTERNAL_TURNS: u32 = 80;
/// Hard cap on one read, so a wire `u32` cannot fold a year of history.
pub const MAX_EXTERNAL_TURNS: u32 = 400;

/// Who spoke, as the folded text labels it. Matches the terminal capture's own
/// prose so a handoff prompt reads the same whichever side it came from.
const USER_SPEAKER: &str = "You";
const AGENT_SPEAKER: &str = "Agent";

/// One turn, before the byte budget decides how many survive.
struct Turn {
    speaker: &'static str,
    text: String,
}

/// Fold a discovered run's transcript to plain text, newest turns first to
/// survive the budget.
///
/// Best-effort like every other read in this module: an unreadable file is an
/// empty transcript with `turns: 0`, not an error — the GUI already handles the
/// "nothing to carry" case.
///
/// `max_bytes` bounds what is *built*, not what is trimmed: the turns are
/// walked backwards and the fold stops at the budget, the same way
/// `get_session_transcript` walks rows.
#[must_use]
pub fn read_transcript(
    session: &ExternalAgentSession,
    max_turns: u32,
    max_bytes: usize,
) -> ExternalTranscript {
    let limit = max_turns as usize;
    // One more than the caller asked for: a reader that stops exactly at the
    // limit cannot tell a conversation that fit from one that was cut, and
    // `truncated` would always read false.
    let probe = limit.saturating_add(1);
    let turns = match (session.provider.as_str(), session.store) {
        (CLAUDE_PROVIDER, _) => claude_turns(&session.transcript_path, probe),
        (OPENCODE_PROVIDER, TranscriptStore::SharedDatabase) => {
            opencode_db_turns(&session.transcript_path, &session.session_id, probe)
        }
        (OPENCODE_PROVIDER, _) => opencode_turns(&session.transcript_path, &session.session_id),
        _ => Vec::new(),
    };

    // Keep the *last* `limit`: a handoff carries where the conversation got to,
    // not where it started.
    let from = turns.len().saturating_sub(limit);
    let turns = &turns[from..];

    let mut kept = 0usize;
    let mut size = 0usize;
    for turn in turns.iter().rev() {
        let next = size + turn.speaker.len() + 2 + turn.text.len() + 2;
        if kept > 0 && next > max_bytes {
            break;
        }
        size = next;
        kept += 1;
    }

    let text = turns[turns.len() - kept..]
        .iter()
        .map(|turn| format!("{}: {}", turn.speaker, turn.text))
        .collect::<Vec<_>>()
        .join("\n\n");

    ExternalTranscript {
        session_id: session.session_id.clone(),
        text,
        turns: u32::try_from(kept).unwrap_or(u32::MAX),
        truncated: kept < turns.len() || from > 0,
    }
}

/// The last `limit` turns of a Claude `.jsonl`, oldest first.
fn claude_turns(path: &Path, limit: usize) -> Vec<Turn> {
    let Ok(file) = std::fs::File::open(path) else {
        return Vec::new();
    };
    let mut turns: VecDeque<Turn> = VecDeque::new();
    // Streamed and bounded to a sliding window: a long run's transcript is
    // megabytes, and only the tail is ever returned.
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let speaker = if line.contains(r#""type":"user""#) {
            USER_SPEAKER
        } else if line.contains(r#""type":"assistant""#) {
            AGENT_SPEAKER
        } else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let text = match speaker {
            USER_SPEAKER if is_typed_prompt(&value) => user_text(&value),
            AGENT_SPEAKER => assistant_text(&value),
            _ => None,
        };
        let Some(text) = text else { continue };
        turns.push_back(Turn { speaker, text });
        if turns.len() > limit {
            turns.pop_front();
        }
    }
    turns.into()
}

/// Every readable turn of a legacy opencode session, oldest first.
///
/// `transcript_path` is `<base>/session/<hash>/<id>.json`, so the message store
/// is three levels up — the same layout [`opencode_conversation`] walks.
fn opencode_turns(transcript: &Path, session_id: &str) -> Vec<Turn> {
    let Some(base) = opencode_base(transcript) else {
        return Vec::new();
    };
    let mut ids: Vec<String> = json_files(&base.join("message").join(session_id))
        .iter()
        .filter_map(|path| path.file_stem()?.to_str().map(str::to_owned))
        .collect();
    // Message ids are time-ordered, so sorting them sorts the conversation.
    ids.sort();

    let mut turns = Vec::new();
    for id in &ids {
        let path = base
            .join("message")
            .join(session_id)
            .join(format!("{id}.json"));
        let Some(value) = read_json(&path) else {
            continue;
        };
        let speaker = match value.get("role").and_then(Value::as_str) {
            Some("user") => USER_SPEAKER,
            Some("assistant") => AGENT_SPEAKER,
            _ => continue,
        };
        let Some(text) = opencode_message_text(&base, id) else {
            continue;
        };
        turns.push(Turn { speaker, text });
    }
    turns
}

/// Every readable turn recorded for a run in an opencode database, oldest
/// first. Read-only, like every other query against that file.
fn opencode_db_turns(db: &Path, session_id: &str, limit: usize) -> Vec<Turn> {
    crate::opencode_db::conversation(db, session_id, limit)
        .into_iter()
        .map(|turn| Turn {
            speaker: if turn.role == "assistant" {
                AGENT_SPEAKER
            } else {
                USER_SPEAKER
            },
            text: turn.text,
        })
        .collect()
}

/// The legacy opencode storage root a session file sits under.
fn opencode_base(transcript: &Path) -> Option<PathBuf> {
    Some(transcript.parent()?.parent()?.parent()?.to_path_buf())
}

/// Why a run's transcript cannot be removed.
#[derive(Debug, PartialEq, Eq)]
pub enum DeleteError {
    /// The run is recorded in a store holding every other run of its provider.
    Shared,
    /// The transcript could not be removed from disk.
    Io(String),
}

/// Remove a discovered run's transcript and the artifacts recorded beside it.
///
/// Never the checkout, never a daemon session row, and never a shared store:
/// [`crate::opencode_db`] opens opencode's database read-only and says why, so
/// a run recorded there is refused rather than deleted one row at a time.
///
/// Sibling artifacts are best effort — a run with no subagents has no directory
/// — but the transcript itself is not: failing to remove it and reporting
/// success would leave the row on the next scan with nothing to explain it.
pub fn delete_transcript(session: &ExternalAgentSession) -> Result<(), DeleteError> {
    if session.store == TranscriptStore::SharedDatabase {
        return Err(DeleteError::Shared);
    }
    let path = &session.transcript_path;
    match session.provider.as_str() {
        OPENCODE_PROVIDER => delete_opencode_siblings(path, &session.session_id),
        _ => delete_claude_siblings(path, &session.session_id),
    }
    std::fs::remove_file(path).map_err(|error| DeleteError::Io(error.to_string()))
}

/// `<parent>/<sessionId>/subagents/` and the now-empty directory holding it.
fn delete_claude_siblings(transcript: &Path, session_id: &str) {
    let Some(parent) = transcript.parent() else {
        return;
    };
    let run = parent.join(session_id);
    remove_tree(&run.join("subagents"));
    // Only when nothing else was recorded under it; `remove_dir` refuses a
    // directory that still holds something we did not put there.
    let _ = std::fs::remove_dir(&run);
}

/// `message/<sessionId>/` and the `part/<messageId>/` directories its messages
/// named. The ids are read *before* the message directory goes.
fn delete_opencode_siblings(transcript: &Path, session_id: &str) {
    let Some(base) = opencode_base(transcript) else {
        return;
    };
    let messages = base.join("message").join(session_id);
    let ids: Vec<String> = json_files(&messages)
        .iter()
        .filter_map(|path| path.file_stem()?.to_str().map(str::to_owned))
        .collect();
    remove_tree(&messages);
    for id in ids {
        remove_tree(&base.join("part").join(id));
    }
}

/// Remove a directory tree, ignoring one that was never there.
fn remove_tree(dir: &Path) {
    if let Err(error) = std::fs::remove_dir_all(dir) {
        if error.kind() != std::io::ErrorKind::NotFound {
            tracing::debug!(dir = %dir.display(), %error, "transcript artifact not removed");
        }
    }
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// The [`SCAN_LIMIT`] most recently modified files with `ext` directly in
/// `dir`, newest first. An unreadable directory yields nothing.
fn recent_files(dir: &Path, ext: &str) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<(OffsetDateTime, PathBuf)> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| has_extension(path, ext))
        .map(|path| {
            let mtime = file_mtime(&path).unwrap_or(OffsetDateTime::UNIX_EPOCH);
            (mtime, path)
        })
        .collect();
    files.sort_by(|a, b| b.0.cmp(&a.0));
    files.truncate(SCAN_LIMIT);
    files.into_iter().map(|(_, path)| path).collect()
}

/// Every `.json` file directly in `dir`, in directory order.
fn json_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| has_extension(path, "json"))
        .collect()
}

fn has_extension(path: &Path, ext: &str) -> bool {
    path.extension().and_then(|e| e.to_str()) == Some(ext)
}

/// Parse a small JSON file, or `None` if it is missing or malformed.
fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// `Some(trimmed)` when a string has readable content.
fn non_empty(text: &str) -> Option<String> {
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

/// First line of `text`, capped so a card title stays one line.
fn truncate_title(text: &str) -> String {
    let first_line = text.lines().next().unwrap_or(text).trim();
    cap(first_line, TITLE_CHARS)
}

/// A message folded into one capped paragraph: the card shows a couple of
/// lines of it, and the newlines and code fences of a real answer would waste
/// most of that space.
fn fold_preview(text: &str) -> String {
    let folded = text.split_whitespace().collect::<Vec<_>>().join(" ");
    cap(&folded, PREVIEW_CHARS)
}

/// `text` cut to `max` characters, with an ellipsis when anything was cut.
fn cap(text: &str, max: usize) -> String {
    let mut capped: String = text.chars().take(max).collect();
    if text.chars().count() > max {
        capped.push('…');
    }
    capped
}

/// Convert an epoch-milliseconds instant to [`OffsetDateTime`].
fn millis_to_ts(ms: i64) -> Option<OffsetDateTime> {
    OffsetDateTime::from_unix_timestamp_nanos(i128::from(ms) * 1_000_000).ok()
}

/// Compare two paths for identity, canonicalizing when possible so that
/// symlinked or non-normalized forms still match.
fn same_path(a: &Path, b: &Path) -> bool {
    let ca = std::fs::canonicalize(a);
    let cb = std::fs::canonicalize(b);
    match (ca, cb) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

fn file_mtime(path: &Path) -> Option<OffsetDateTime> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    Some(OffsetDateTime::from(modified))
}

fn now() -> OffsetDateTime {
    OffsetDateTime::now_utc()
}

fn home_dir() -> Option<PathBuf> {
    directories::BaseDirs::new().map(|d| d.home_dir().to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::{ProjectId, WorkspaceKind, WorkspaceStatus};

    fn root_of(path: &Path) -> Root {
        Root {
            project_id: ProjectId::new(),
            workspace_id: None,
            path: path.to_path_buf(),
        }
    }

    fn sample_project(root: &Path) -> Project {
        Project {
            id: ProjectId::new(),
            project_group_id: None,
            name: "p".into(),
            icon: None,
            root_path: root.to_path_buf(),
            git_root: None,
            created_at: Timestamp::now(),
            last_opened_at: Timestamp::now(),
        }
    }

    fn sample_profile(provider: &str, dir: &str) -> AgentProfile {
        AgentProfile {
            id: domain::AgentProfileId::new(),
            provider_id: domain::AgentProviderId::new(provider),
            name: "Personal".to_owned(),
            executable: None,
            config_dir: Some(PathBuf::from(dir)),
            args: Vec::new(),
            created_at: Timestamp::now(),
        }
    }

    fn sample_workspace(project: &Project, path: &Path) -> Workspace {
        Workspace {
            id: domain::WorkspaceId::new(),
            project_id: project.id,
            kind: WorkspaceKind::GitWorktree,
            path: path.to_path_buf(),
            branch: Some("feature".into()),
            display_name: None,
            managed_by_app: true,
            created_at: Timestamp::now(),
            status: WorkspaceStatus::default(),
        }
    }

    #[test]
    fn the_cache_reuses_a_fresh_pass_and_rescans_when_the_roots_change() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let project = sample_project(tmp.path());
        let mut cache = Cache::default();

        // First call scans; the second reuses it without touching the clock.
        let _ = cache.discover(std::slice::from_ref(&project), &[], &[]);
        let first = cache.scanned_at.expect("scanned");
        let _ = cache.discover(std::slice::from_ref(&project), &[], &[]);
        assert_eq!(cache.scanned_at, Some(first), "a fresh pass is reused");

        // A new worktree is a different question, not a stale answer: rescan
        // immediately rather than after the TTL.
        let workspace = sample_workspace(&project, &tmp.path().join("wt"));
        let _ = cache.discover(
            std::slice::from_ref(&project),
            std::slice::from_ref(&workspace),
            &[],
        );
        assert_ne!(cache.scanned_at, Some(first), "new roots force a rescan");

        // So is a profile: it adds an account to look in, and waiting out the
        // TTL would hide the history the user just pointed Forge at.
        let third = cache.scanned_at.expect("scanned");
        let profile = sample_profile("claude", ".claude-personal");
        let _ = cache.discover(
            std::slice::from_ref(&project),
            std::slice::from_ref(&workspace),
            std::slice::from_ref(&profile),
        );
        assert_ne!(
            cache.scanned_at,
            Some(third),
            "a new account forces a rescan"
        );

        // Explicit invalidation forces one too.
        let second = cache.scanned_at.expect("scanned");
        cache.invalidate();
        let _ = cache.discover(
            std::slice::from_ref(&project),
            std::slice::from_ref(&workspace),
            std::slice::from_ref(&profile),
        );
        assert_ne!(cache.scanned_at, Some(second));
    }

    #[test]
    fn the_fingerprint_ignores_the_order_roots_arrive_in() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let a = sample_project(&tmp.path().join("a"));
        let b = sample_project(&tmp.path().join("b"));
        assert_eq!(
            fingerprint(&[a.clone(), b.clone()], &[], &[]),
            fingerprint(&[b, a], &[], &[]),
            "projects come out of a HashMap, so the order varies run to run"
        );
    }

    /// A Claude run on disk, with `turns` alternating prompt and reply.
    fn claude_run(dir: &Path, session_id: &str, turns: usize) -> ExternalAgentSession {
        let transcript = dir.join(format!("{session_id}.jsonl"));
        let mut lines = String::new();
        for turn in 0..turns {
            lines.push_str(&format!(
                "{{\"type\":\"user\",\"message\":{{\"content\":\"ask {turn}\"}},\"timestamp\":\"2026-01-01T00:00:00Z\"}}\n"
            ));
            lines.push_str(&format!(
                "{{\"type\":\"assistant\",\"message\":{{\"content\":[{{\"type\":\"text\",\"text\":\"answer {turn}\"}}]}},\"timestamp\":\"2026-01-01T00:00:01Z\"}}\n"
            ));
        }
        std::fs::write(&transcript, lines).unwrap();
        ExternalAgentSession {
            session_id: session_id.to_owned(),
            project_id: ProjectId::new(),
            workspace_id: None,
            provider: CLAUDE_PROVIDER.to_owned(),
            profile_id: None,
            title: "t".into(),
            branch: None,
            preview: None,
            model: None,
            message_count: 0,
            subagent_count: 0,
            transcript_path: transcript,
            store: TranscriptStore::File,
            started_at: Timestamp::now(),
            last_activity: Timestamp::now(),
        }
    }

    #[test]
    fn a_claude_transcript_folds_to_labelled_turns_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let session = claude_run(dir.path(), "ses-1", 2);

        let read = read_transcript(&session, 80, 36_000);

        assert_eq!(read.session_id, "ses-1");
        assert_eq!(read.turns, 4);
        assert!(!read.truncated);
        assert_eq!(
            read.text,
            "You: ask 0\n\nAgent: answer 0\n\nYou: ask 1\n\nAgent: answer 1"
        );
    }

    #[test]
    fn the_turn_cap_keeps_the_end_of_the_conversation() {
        let dir = tempfile::tempdir().unwrap();
        let session = claude_run(dir.path(), "ses-1", 5);

        let read = read_transcript(&session, 2, 36_000);

        assert_eq!(read.turns, 2);
        assert!(read.truncated, "older turns were dropped");
        assert_eq!(read.text, "You: ask 4\n\nAgent: answer 4");
    }

    #[test]
    fn the_byte_budget_bounds_the_text_and_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let session = claude_run(dir.path(), "ses-1", 20);

        let read = read_transcript(&session, 80, 60);

        assert!(read.truncated);
        assert!(read.text.len() <= 60, "budget bounds what is built");
        // Whatever survived is the tail, not the head.
        assert!(read.text.ends_with("Agent: answer 19"));
    }

    #[test]
    fn an_unreadable_transcript_is_empty_rather_than_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let mut session = claude_run(dir.path(), "ses-1", 1);
        session.transcript_path = dir.path().join("gone.jsonl");

        let read = read_transcript(&session, 80, 36_000);

        assert_eq!(read.turns, 0);
        assert_eq!(read.text, "");
        assert!(!read.truncated);
    }

    #[test]
    fn deleting_a_claude_run_takes_its_subagents_and_leaves_its_siblings() {
        let dir = tempfile::tempdir().unwrap();
        let session = claude_run(dir.path(), "ses-1", 1);
        let neighbour = claude_run(dir.path(), "ses-2", 1);
        let subagents = dir.path().join("ses-1").join("subagents");
        std::fs::create_dir_all(&subagents).unwrap();
        std::fs::write(subagents.join("agent-a.jsonl"), "{}\n").unwrap();

        delete_transcript(&session).expect("removed");

        assert!(!session.transcript_path.exists());
        assert!(!dir.path().join("ses-1").exists(), "the run directory goes");
        assert!(
            neighbour.transcript_path.exists(),
            "another run's transcript is not ours to remove"
        );
    }

    #[test]
    fn a_shared_store_is_refused_and_left_byte_identical() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("opencode.db");
        std::fs::write(&db, b"not really sqlite, but bytes are bytes").unwrap();
        let before = std::fs::read(&db).unwrap();

        let mut session = claude_run(dir.path(), "ses-1", 1);
        session.provider = OPENCODE_PROVIDER.to_owned();
        session.store = TranscriptStore::SharedDatabase;
        session.transcript_path = db.clone();

        assert_eq!(delete_transcript(&session), Err(DeleteError::Shared));
        assert_eq!(std::fs::read(&db).unwrap(), before);
    }

    #[test]
    fn the_cache_only_resolves_a_run_it_discovered() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache {
            sessions: vec![claude_run(dir.path(), "ses-1", 1)],
            ..Cache::default()
        };

        assert!(cache.find("ses-1", CLAUDE_PROVIDER, None).is_some());
        assert!(
            cache.find("ses-1", OPENCODE_PROVIDER, None).is_none(),
            "a session id is only unique within a provider"
        );
        assert!(cache.find("never-scanned", CLAUDE_PROVIDER, None).is_none());
    }

    #[test]
    fn slug_collapses_slashes_and_dots() {
        assert_eq!(
            claude_slug(Path::new("/Users/me/dev/forge-node")),
            "-Users-me-dev-forge-node"
        );
        assert_eq!(
            claude_slug(Path::new("/Users/me/my.app")),
            "-Users-me-my-app"
        );
    }

    #[test]
    fn truncate_keeps_first_line() {
        assert_eq!(truncate_title("hola\nmundo"), "hola");
        let long = "x".repeat(100);
        let title = truncate_title(&long);
        assert_eq!(title.chars().count(), TITLE_CHARS + 1); // + ellipsis
    }

    #[test]
    fn preview_folds_a_multi_line_answer_into_one_paragraph() {
        assert_eq!(
            fold_preview("done.\n\n  two   spaces\n"),
            "done. two spaces"
        );
        let long = "word ".repeat(100);
        assert_eq!(fold_preview(&long).chars().count(), PREVIEW_CHARS + 1);
    }

    #[test]
    fn user_text_skips_command_wrappers() {
        let v: Value = serde_json::from_str(
            r#"{"type":"user","message":{"content":"<command-name>/x</command-name>"}}"#,
        )
        .unwrap();
        assert!(user_text(&v).is_none());

        let v: Value =
            serde_json::from_str(r#"{"type":"user","message":{"content":"real prompt"}}"#).unwrap();
        assert_eq!(user_text(&v).as_deref(), Some("real prompt"));
    }

    #[test]
    fn assistant_text_prefers_prose_over_thinking_and_tools() {
        let v: Value = serde_json::from_str(
            r#"{"type":"assistant","message":{"content":[
                {"type":"thinking","thinking":"hmm"},
                {"type":"text","text":"Here is the answer."},
                {"type":"tool_use","name":"Bash","input":{}}
            ]}}"#,
        )
        .unwrap();
        assert_eq!(assistant_text(&v).as_deref(), Some("Here is the answer."));

        let only_tools: Value = serde_json::from_str(
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash"}]}}"#,
        )
        .unwrap();
        assert!(assistant_text(&only_tools).is_none());
    }

    #[test]
    fn tool_results_do_not_count_as_conversation_turns() {
        let mut scan = Scan::default();
        for line in [
            r#"{"type":"user","message":{"content":"do it"},"timestamp":"2026-01-01T00:00:00Z"}"#,
            r#"{"type":"assistant","message":{"model":"claude-opus-5","content":[{"type":"text","text":"on it"}]},"timestamp":"2026-01-01T00:00:01Z"}"#,
            r#"{"type":"user","toolUseResult":{"stdout":"ok"},"message":{"content":"tool output"}}"#,
            r#"{"type":"user","isMeta":true,"message":{"content":"injected context"}}"#,
            r#"{"type":"assistant","message":{"model":"claude-opus-5","content":[{"type":"text","text":"done"}]},"timestamp":"2026-01-01T00:00:09Z"}"#,
        ] {
            fold_line(line, &mut scan);
        }
        assert_eq!(scan.messages, 3, "two replies and one typed prompt");
        assert_eq!(scan.last_assistant.as_deref(), Some("done"));
        assert_eq!(scan.model.as_deref(), Some("claude-opus-5"));
        assert_eq!(scan.first_user.as_deref(), Some("do it"));
        assert!(scan.first_ts.unwrap() < scan.last_ts.unwrap());
    }

    #[test]
    fn transcript_carries_the_conversation_and_its_subagents() {
        let dir = tempfile::tempdir().unwrap();
        let root = root_of(dir.path());
        let transcript = dir.path().join("ses-1.jsonl");
        std::fs::write(
            &transcript,
            format!(
                "{}\n{}\n",
                format_args!(
                    r#"{{"type":"user","cwd":"{}","gitBranch":"main","message":{{"content":"ship it"}},"timestamp":"2026-01-01T00:00:00Z"}}"#,
                    dir.path().display()
                ),
                r#"{"type":"assistant","message":{"model":"claude-opus-5","content":[{"type":"text","text":"Shipped.\nAll green."}]},"timestamp":"2026-01-01T00:01:00Z"}"#
            ),
        )
        .unwrap();

        let subagents = dir.path().join("ses-1").join("subagents");
        std::fs::create_dir_all(&subagents).unwrap();
        std::fs::write(subagents.join("agent-a.jsonl"), "{}\n").unwrap();
        std::fs::write(subagents.join("agent-b.jsonl"), "{}\n").unwrap();
        std::fs::write(subagents.join("agent-b.meta.json"), "{}\n").unwrap();

        let parsed = parse_transcript(&transcript, &root).expect("transcript parses");
        assert_eq!(parsed.session_id, "ses-1");
        assert_eq!(parsed.title, "ship it");
        assert_eq!(parsed.branch.as_deref(), Some("main"));
        assert_eq!(parsed.preview.as_deref(), Some("Shipped. All green."));
        assert_eq!(parsed.model.as_deref(), Some("claude-opus-5"));
        assert_eq!(parsed.message_count, 2);
        assert_eq!(
            parsed.subagent_count, 2,
            "only the .jsonl transcripts count"
        );
    }

    /// The account list a scan walks: the default store first, then one per
    /// profile of that provider, resolved against `$HOME` (§13.4).
    #[test]
    fn every_profile_of_a_provider_adds_an_account_to_scan() {
        let home = Path::new("/home/me");
        let profiles = [
            sample_profile("claude", ".claude-personal"),
            sample_profile("claude", "/Volumes/work/.claude"),
            // Another provider's profile, and a profile with no directory at
            // all (a wrapper on PATH carries the account instead).
            sample_profile("opencode", ".opencode-personal"),
            AgentProfile {
                config_dir: None,
                ..sample_profile("claude", "")
            },
        ];

        // Each store also carries the login it belongs to: the default
        // account has no profile, and every other one names the profile that
        // moved it, which is what a resume needs to re-enter the run.
        assert_eq!(
            listed(accounts(
                &home.join(".claude"),
                CLAUDE_PROVIDER,
                &profiles,
                home,
                ""
            )),
            [
                (PathBuf::from("/home/me/.claude"), None),
                (
                    PathBuf::from("/home/me/.claude-personal"),
                    Some(profiles[0].id)
                ),
                (PathBuf::from("/Volumes/work/.claude"), Some(profiles[1].id)),
            ]
        );
        // opencode's directory is `XDG_DATA_HOME`, so its store is one level in.
        assert_eq!(
            listed(accounts(
                &home.join(".local/share/opencode"),
                OPENCODE_PROVIDER,
                &profiles,
                home,
                "opencode"
            )),
            [
                (PathBuf::from("/home/me/.local/share/opencode"), None),
                (
                    PathBuf::from("/home/me/.opencode-personal/opencode"),
                    Some(profiles[2].id)
                ),
            ]
        );
    }

    /// The directories a scan walks, each with the profile that owns it.
    fn listed(accounts: Vec<Account>) -> Vec<(PathBuf, Option<AgentProfileId>)> {
        accounts
            .into_iter()
            .map(|account| (account.dir, account.profile))
            .collect()
    }

    /// A profile pointing at the default account is one account, not two, or
    /// every session in it would be listed twice.
    #[test]
    fn an_account_named_twice_is_scanned_once() {
        let home = tempfile::tempdir().unwrap();
        let default = home.path().join(".claude");
        std::fs::create_dir_all(&default).unwrap();
        let profiles = [sample_profile("claude", ".claude")];

        assert_eq!(
            listed(accounts(
                &default,
                CLAUDE_PROVIDER,
                &profiles,
                home.path(),
                ""
            )),
            [(default, None)]
        );
    }

    /// The transcripts of a profile's account are found where that profile put
    /// them: `<config dir>/projects/<slug>`, not `~/.claude/projects/<slug>`.
    #[test]
    fn a_profiles_store_is_scanned_for_the_directory_being_viewed() {
        let tmp = tempfile::tempdir().unwrap();
        let workdir = tmp.path().join("proj");
        std::fs::create_dir_all(&workdir).unwrap();
        let root = root_of(&workdir);

        let store = tmp.path().join(".claude-personal");
        let projects = store.join("projects").join(claude_slug(&workdir));
        std::fs::create_dir_all(&projects).unwrap();
        std::fs::write(
            projects.join("ses-personal.jsonl"),
            format!(
                r#"{{"type":"user","cwd":"{}","message":{{"content":"hello"}},"timestamp":"2026-01-01T00:00:00Z"}}"#,
                workdir.display()
            ) + "\n",
        )
        .unwrap();

        let account = Account {
            dir: store,
            profile: Some(AgentProfileId::new()),
        };
        let mut out = Vec::new();
        discover_claude(&account.dir, &root, &mut out);
        stamp_account(&mut out, &account);
        assert_eq!(out.len(), 1, "the profile's transcript is listed");
        assert_eq!(out[0].session_id, "ses-personal");
        assert_eq!(
            out[0].profile_id, account.profile,
            "the card says which login can resume it"
        );

        // ...and the default store, which holds nothing for this profile,
        // contributes nothing rather than erroring.
        let mut empty = Vec::new();
        discover_claude(&tmp.path().join(".claude"), &root, &mut empty);
        assert!(empty.is_empty());
    }

    #[test]
    fn transcript_from_another_directory_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let root = root_of(dir.path());
        let transcript = dir.path().join("ses-2.jsonl");
        std::fs::write(
            &transcript,
            "{\"type\":\"user\",\"cwd\":\"/somewhere/else\",\"message\":{\"content\":\"hi\"}}\n",
        )
        .unwrap();
        assert!(parse_transcript(&transcript, &root).is_none());
    }

    #[test]
    fn millis_convert_to_a_known_instant() {
        // 1769459636674 ms = 2026-01-26T21:53:56.674Z
        let ts = millis_to_ts(1_769_459_636_674).unwrap();
        assert_eq!(ts.year(), 2026);
        assert_eq!(ts.unix_timestamp(), 1_769_459_636);
    }

    /// Write an opencode storage tree with one session, one subagent session
    /// and a two-message conversation, and return its base directory.
    fn opencode_fixture(base: &Path, worktree: &Path, hash: &str) {
        let session_id = "ses_root";
        std::fs::create_dir_all(base.join("project")).unwrap();
        std::fs::write(
            base.join("project").join(format!("{hash}.json")),
            format!(r#"{{"id":"{hash}","worktree":"{}"}}"#, worktree.display()),
        )
        .unwrap();

        let sessions = base.join("session").join(hash);
        std::fs::create_dir_all(&sessions).unwrap();
        std::fs::write(
            sessions.join(format!("{session_id}.json")),
            format!(
                r#"{{"id":"{session_id}","title":"Split the modal","directory":"{}","time":{{"created":1769459636674,"updated":1769468294667}}}}"#,
                worktree.display()
            ),
        )
        .unwrap();
        std::fs::write(
            sessions.join("ses_child.json"),
            format!(
                r#"{{"id":"ses_child","parentID":"{session_id}","title":"tool call","time":{{"created":1769459636674}}}}"#
            ),
        )
        .unwrap();

        let messages = base.join("message").join(session_id);
        std::fs::create_dir_all(&messages).unwrap();
        std::fs::write(
            messages.join("msg_001.json"),
            r#"{"id":"msg_001","role":"user","time":{"created":1769459636674}}"#,
        )
        .unwrap();
        std::fs::write(
            messages.join("msg_002.json"),
            r#"{"id":"msg_002","role":"assistant","modelID":"gpt-5.3-codex","providerID":"openai"}"#,
        )
        .unwrap();

        let parts = base.join("part").join("msg_002");
        std::fs::create_dir_all(&parts).unwrap();
        std::fs::write(
            parts.join("prt_001.json"),
            r#"{"id":"prt_001","type":"reasoning","text":"thinking"}"#,
        )
        .unwrap();
        std::fs::write(
            parts.join("prt_002.json"),
            r#"{"id":"prt_002","type":"text","synthetic":false,"text":"The modal is split.\n\nTwo files changed."}"#,
        )
        .unwrap();
    }

    #[test]
    fn opencode_session_carries_its_conversation() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("storage");
        let worktree = dir.path().join("repo");
        std::fs::create_dir_all(&worktree).unwrap();
        opencode_fixture(&base, &worktree, "abc123");

        let root = root_of(&worktree);
        let mut out = Vec::new();
        // `discover_opencode` resolves storage from $HOME, so drive its parts
        // directly against the fixture's base.
        let hash = opencode_project_hash(&base, &worktree).expect("project matches by worktree");
        assert_eq!(hash, "abc123");

        let mut subagents = HashMap::new();
        let mut top_level = Vec::new();
        for path in json_files(&base.join("session").join(&hash)) {
            let value = read_json(&path).unwrap();
            match value.get("parentID").and_then(Value::as_str) {
                Some(parent) => *subagents.entry(parent.to_owned()).or_default() += 1,
                None => top_level.push((path, value)),
            }
        }
        assert_eq!(top_level.len(), 1, "sub-sessions are not listed");
        for (path, value) in &top_level {
            out.push(parse_opencode_session(&base, path, value, &root, &subagents).unwrap());
        }

        let session = &out[0];
        assert_eq!(session.session_id, "ses_root");
        assert_eq!(session.title, "Split the modal");
        assert_eq!(session.provider, OPENCODE_PROVIDER);
        assert_eq!(session.message_count, 2);
        assert_eq!(session.subagent_count, 1);
        assert_eq!(session.model.as_deref(), Some("gpt-5.3-codex"));
        assert_eq!(
            session.preview.as_deref(),
            Some("The modal is split. Two files changed.")
        );
        assert!(session.last_activity.0 > session.started_at.0);
    }

    #[test]
    fn opencode_session_from_another_directory_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("storage");
        let worktree = dir.path().join("repo");
        std::fs::create_dir_all(&worktree).unwrap();
        opencode_fixture(&base, &worktree, "abc123");

        let elsewhere = root_of(Path::new("/somewhere/else"));
        let path = base.join("session").join("abc123").join("ses_root.json");
        let value = read_json(&path).unwrap();
        assert!(
            parse_opencode_session(&base, &path, &value, &elsewhere, &HashMap::new()).is_none()
        );
    }

    /// Write a database with the current opencode schema holding `ids`, all
    /// recorded in `directory`.
    fn opencode_db_fixture(path: &Path, directory: &Path, ids: &[&str]) {
        let conn = rusqlite::Connection::open(path).unwrap();
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
        for (index, id) in ids.iter().enumerate() {
            conn.execute(
                "INSERT INTO session (id, directory, title, time_created, time_updated)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![
                    id,
                    directory.to_string_lossy(),
                    format!("From the database {index}"),
                    1_769_459_636_674_i64,
                    1_769_468_294_667_i64 + index as i64,
                ],
            )
            .unwrap();
        }
    }

    /// The scan opencode ≥ 1.17 needs: the sessions live in the database, and
    /// the JSON tree left behind by the upgrade must not hide them.
    #[test]
    fn opencode_sessions_come_from_the_database() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        std::fs::create_dir_all(&data).unwrap();
        let worktree = dir.path().join("repo");
        std::fs::create_dir_all(&worktree).unwrap();
        opencode_db_fixture(&data.join("opencode.db"), &worktree, &["ses_a", "ses_b"]);

        let root = root_of(&worktree);
        let mut out = Vec::new();
        let recorded = discover_opencode_databases(&data, &root, &mut out);

        assert_eq!(recorded.len(), 2);
        let titles: Vec<&str> = out.iter().map(|s| s.title.as_str()).collect();
        // Newest first.
        assert_eq!(titles, ["From the database 1", "From the database 0"]);
        for session in &out {
            assert_eq!(session.provider, OPENCODE_PROVIDER);
            assert_eq!(session.project_id, root.project_id);
            // With no per-session file left, the database is the transcript.
            assert_eq!(session.transcript_path, data.join("opencode.db"));
        }
    }

    /// An upgraded machine has the same session in both layouts. It is listed
    /// once, from the database — the tree stopped being written.
    #[test]
    fn a_session_in_both_layouts_is_listed_once() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        let base = data.join("storage");
        let worktree = dir.path().join("repo");
        std::fs::create_dir_all(&worktree).unwrap();
        opencode_fixture(&base, &worktree, "abc123");
        opencode_db_fixture(&data.join("opencode.db"), &worktree, &["ses_root"]);

        let root = root_of(&worktree);
        let mut out = Vec::new();
        let recorded = discover_opencode_databases(&data, &root, &mut out);
        discover_opencode_storage(&base, &root, &recorded, &mut out);

        assert_eq!(out.len(), 1, "the JSON copy is not listed again");
        assert_eq!(out[0].session_id, "ses_root");
        assert_eq!(out[0].title, "From the database 0");
    }

    /// A machine still on the old opencode has no database, and loses nothing.
    #[test]
    fn without_a_database_the_json_tree_is_still_read() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        let base = data.join("storage");
        let worktree = dir.path().join("repo");
        std::fs::create_dir_all(&worktree).unwrap();
        opencode_fixture(&base, &worktree, "abc123");

        let root = root_of(&worktree);
        let mut out = Vec::new();
        let recorded = discover_opencode_databases(&data, &root, &mut out);
        assert!(recorded.is_empty());
        discover_opencode_storage(&base, &root, &recorded, &mut out);

        assert_eq!(out.len(), 1);
        assert_eq!(out[0].title, "Split the modal");
    }

    #[test]
    fn opencode_project_hash_matches_by_worktree() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path();
        let worktree = dir.path().join("repo");
        std::fs::create_dir_all(base.join("project")).unwrap();
        std::fs::create_dir_all(&worktree).unwrap();
        std::fs::write(
            base.join("project").join("abc123.json"),
            format!(r#"{{"id":"abc123","worktree":"{}"}}"#, worktree.display()),
        )
        .unwrap();

        assert_eq!(
            opencode_project_hash(base, &worktree).as_deref(),
            Some("abc123")
        );
        assert_eq!(opencode_project_hash(base, Path::new("/nope")), None);
    }

    #[test]
    fn worktrees_are_scanned_and_keep_their_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("repo");
        let tree = dir.path().join("repo-feature");
        std::fs::create_dir_all(&main).unwrap();
        std::fs::create_dir_all(&tree).unwrap();

        let project = sample_project(&main);
        let workspace = sample_workspace(&project, &tree);
        let scanned = roots(
            std::slice::from_ref(&project),
            std::slice::from_ref(&workspace),
        );

        assert_eq!(scanned.len(), 2, "the worktree and the project root");
        let worktree_root = scanned
            .iter()
            .find(|root| root.path == tree)
            .expect("the worktree is scanned");
        assert_eq!(worktree_root.workspace_id, Some(workspace.id));
        assert_eq!(worktree_root.project_id, project.id);

        let orphan = sample_workspace(&sample_project(&main), &dir.path().join("other"));
        let scanned = roots(
            std::slice::from_ref(&project),
            std::slice::from_ref(&orphan),
        );
        assert!(
            scanned
                .iter()
                .all(|root| root.path != dir.path().join("other")),
            "a workspace of an unknown project is not scanned"
        );
    }

    #[test]
    fn recent_files_keeps_the_newest_and_caps_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        for index in 0..(SCAN_LIMIT + 5) {
            std::fs::write(dir.path().join(format!("s{index}.jsonl")), "{}\n").unwrap();
        }
        std::fs::write(dir.path().join("ignored.txt"), "x").unwrap();

        let files = recent_files(dir.path(), "jsonl");
        assert_eq!(files.len(), SCAN_LIMIT);
        assert!(files.iter().all(|path| has_extension(path, "jsonl")));
        assert!(recent_files(&dir.path().join("missing"), "jsonl").is_empty());
    }
}
