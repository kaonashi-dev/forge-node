use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::{DeleteError, ExternalAgentSession, Root, Scan, Timestamp, TranscriptStore, Turn};

const MAX_ENTRIES: usize = 20_000;
pub(super) const MAX_RECORD_BYTES: u64 = 1024 * 1024;
const MAX_SCAN_BYTES: u64 = 16 * 1024 * 1024;
const MAX_TURN_CHARS: usize = 16_384;

pub(super) fn files(
    dir: &Path,
    depth: usize,
    accepts: fn(&Path) -> bool,
    limit: usize,
) -> Vec<PathBuf> {
    fn walk(
        dir: &Path,
        depth: usize,
        accepts: fn(&Path) -> bool,
        remaining: &mut usize,
        out: &mut Vec<PathBuf>,
    ) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            if *remaining == 0 {
                break;
            }
            *remaining -= 1;
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let path = entry.path();
            if kind.is_dir() && depth > 0 {
                walk(&path, depth - 1, accepts, remaining, out);
            } else if kind.is_file() && accepts(&path) {
                out.push(path);
            }
        }
    }
    let mut remaining = MAX_ENTRIES;
    let mut found = Vec::new();
    walk(dir, depth, accepts, &mut remaining, &mut found);
    found.sort_by_cached_key(|path| std::cmp::Reverse(super::file_mtime(path)));
    if remaining == 0 || found.len() > limit {
        tracing::debug!(dir = %dir.display(), limit, "history candidate scan truncated");
    }
    found.truncate(limit);
    found
}

pub(super) fn json(path: &Path) -> Option<Value> {
    let file = File::open(path).ok()?;
    if file.metadata().ok()?.len() > MAX_RECORD_BYTES {
        return None;
    }
    serde_json::from_reader(file.take(MAX_RECORD_BYTES)).ok()
}

/// Read a bounded tail, skipping a partial first record and oversized lines whole.
pub(super) fn jsonl(path: &Path, mut visit: impl FnMut(&Value)) -> bool {
    let Ok(mut file) = File::open(path) else {
        return false;
    };
    let size = file.metadata().map(|m| m.len()).unwrap_or(0);
    let start = size.saturating_sub(MAX_SCAN_BYTES);
    if file.seek(SeekFrom::Start(start)).is_err() {
        return true;
    }
    let mut reader = BufReader::new(file.take(MAX_SCAN_BYTES));
    let mut truncated = start > 0;
    if start > 0 {
        let _ = reader.skip_until(b'\n');
    }
    let mut line = Vec::new();
    loop {
        line.clear();
        match reader
            .by_ref()
            .take(MAX_RECORD_BYTES + 1)
            .read_until(b'\n', &mut line)
        {
            Ok(0) => break,
            Ok(_) => {}
            Err(_) => {
                truncated = true;
                break;
            }
        }
        if line.len() as u64 > MAX_RECORD_BYTES {
            truncated = true;
            if line.last() != Some(&b'\n') {
                let _ = reader.skip_until(b'\n');
            }
            continue;
        }
        match serde_json::from_slice(&line) {
            Ok(value) => visit(&value),
            Err(_) => truncated = true,
        }
    }
    truncated
}

pub(super) fn root_for<'a>(cwd: &str, roots: &'a [Root]) -> Option<&'a Root> {
    let path = Path::new(cwd);
    roots.iter().find(|root| super::same_path(path, &root.path))
}

pub(super) fn session(
    provider: &str,
    id: String,
    path: PathBuf,
    root: &Root,
    scan: Scan,
) -> ExternalAgentSession {
    let started_at = scan
        .first_ts
        .or_else(|| super::file_mtime(&path))
        .unwrap_or_else(super::now);
    ExternalAgentSession {
        title: scan
            .ai_title
            .as_deref()
            .and_then(super::non_empty)
            .or(scan.first_user)
            .map(|s| super::truncate_title(&s))
            .unwrap_or_else(|| id.clone()),
        session_id: id,
        profile_id: None,
        project_id: root.project_id,
        workspace_id: root.workspace_id,
        provider: provider.to_owned(),
        branch: scan.branch,
        preview: scan.last_assistant.as_deref().map(super::fold_preview),
        model: scan.model,
        message_count: scan.messages,
        subagent_count: 0,
        transcript_path: path,
        store: TranscriptStore::File,
        started_at: Timestamp(started_at),
        last_activity: Timestamp(scan.last_ts.unwrap_or(started_at)),
    }
}

pub(super) fn text(content: &Value) -> Option<String> {
    match content {
        Value::String(text) => super::non_empty(text),
        Value::Array(parts) => {
            let mut text = String::new();
            for part in parts {
                if !matches!(
                    part.get("type").and_then(Value::as_str),
                    Some("text" | "input_text" | "output_text")
                ) {
                    continue;
                }
                if let Some(chunk) = part.get("text").and_then(Value::as_str) {
                    if !text.is_empty() {
                        text.push('\n');
                    }
                    text.push_str(chunk);
                }
            }
            super::non_empty(&text)
        }
        _ => None,
    }
}

#[derive(Default)]
pub(super) struct Conversation {
    pub turns: Vec<Turn>,
    pub truncated: bool,
}

pub(super) fn conversation(
    path: &Path,
    limit: usize,
    parse: fn(&Value) -> Option<Turn>,
) -> Conversation {
    let mut turns = std::collections::VecDeque::new();
    let mut dropped = false;
    let truncated = jsonl(path, |value| {
        if let Some(mut turn) = parse(value) {
            if turn.text.chars().count() > MAX_TURN_CHARS {
                turn.text = super::cap(&turn.text, MAX_TURN_CHARS);
                dropped = true;
            }
            turns.push_back(turn);
            if turns.len() > limit {
                turns.pop_front();
                dropped = true;
            }
        }
    });
    Conversation {
        turns: turns.into(),
        truncated: truncated || dropped,
    }
}

pub(super) fn fold_turn(scan: &mut Scan, turn: Turn) {
    scan.messages = scan.messages.saturating_add(1);
    if turn.speaker == super::USER_SPEAKER && scan.first_user.is_none() {
        scan.first_user = Some(super::truncate_title(&turn.text));
    } else if turn.speaker == super::AGENT_SPEAKER {
        scan.last_assistant = Some(super::fold_preview(&turn.text));
    }
}

pub(super) fn timestamp(value: Option<&Value>) -> Option<time::OffsetDateTime> {
    time::OffsetDateTime::parse(
        value?.as_str()?,
        &time::format_description::well_known::Rfc3339,
    )
    .ok()
}

pub(super) fn delete_session_directory(session: &ExternalAgentSession) -> Result<(), DeleteError> {
    let dir = session
        .transcript_path
        .parent()
        .filter(|dir| dir.file_name().and_then(|s| s.to_str()) == Some(session.session_id.as_str()))
        .ok_or_else(|| DeleteError::Io("Transcript is outside its session directory".to_owned()))?;
    std::fs::remove_dir_all(dir).map_err(|error| DeleteError::Io(error.to_string()))
}
