use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read};
use std::path::Path;

use serde_json::Value;

use super::{
    bounded, ExternalAgentSession, Root, Scan, Turn, AGENT_SPEAKER, SCAN_LIMIT, USER_SPEAKER,
};

pub(super) fn discover(store: &Path, roots: &[Root], out: &mut Vec<ExternalAgentSession>) {
    let files = bounded::files(
        &store.join("sessions"),
        3,
        |p| super::has_extension(p, "jsonl"),
        4096,
    );
    let mut counts = HashMap::new();
    for path in files {
        let Some(meta) = metadata(&path) else {
            continue;
        };
        if meta.pointer("/source/subagent").is_some() {
            continue;
        }
        let Some(root) = meta
            .get("cwd")
            .and_then(Value::as_str)
            .and_then(|cwd| bounded::root_for(cwd, roots))
        else {
            continue;
        };
        let count = counts.entry(root.path.clone()).or_insert(0);
        if *count >= SCAN_LIMIT {
            continue;
        }
        let Some(id) = meta
            .get("id")
            .or_else(|| meta.get("session_id"))
            .and_then(Value::as_str)
        else {
            continue;
        };
        let mut scan = Scan {
            branch: meta
                .pointer("/git/branch")
                .and_then(Value::as_str)
                .map(str::to_owned),
            first_ts: bounded::timestamp(meta.get("timestamp")),
            ..Scan::default()
        };
        let truncated = bounded::jsonl(&path, |value| {
            if let Some(ts) = bounded::timestamp(value.get("timestamp")) {
                scan.last_ts = Some(ts);
            }
            if value.get("type").and_then(Value::as_str) == Some("turn_context") {
                scan.model = value
                    .pointer("/payload/model")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
            }
            if let Some(turn) = turn(value) {
                bounded::fold_turn(&mut scan, turn);
            }
        });
        if truncated {
            tracing::debug!(path = %path.display(), "codex history transcript truncated");
        }
        out.push(bounded::session("codex", id.to_owned(), path, root, scan));
        *count += 1;
    }
}

fn metadata(path: &Path) -> Option<Value> {
    let file = std::fs::File::open(path).ok()?;
    let mut line = String::new();
    BufReader::new(file)
        .take(bounded::MAX_RECORD_BYTES)
        .read_line(&mut line)
        .ok()?;
    let value: Value = serde_json::from_str(&line).ok()?;
    (value.get("type")?.as_str()? == "session_meta").then(|| value["payload"].clone())
}

fn turn(value: &Value) -> Option<Turn> {
    if value.get("type")?.as_str()? != "response_item" {
        return None;
    }
    let message = value.get("payload")?;
    if message.get("type")?.as_str()? != "message" {
        return None;
    }
    let speaker = match message.get("role")?.as_str()? {
        "user" => USER_SPEAKER,
        "assistant" => AGENT_SPEAKER,
        _ => return None,
    };
    let text = bounded::text(message.get("content")?)?;
    if speaker == USER_SPEAKER
        && (text.starts_with('<') || text.starts_with("# AGENTS.md instructions"))
    {
        return None;
    }
    Some(Turn { speaker, text })
}

pub(super) fn turns(path: &Path, limit: usize) -> bounded::Conversation {
    bounded::conversation(path, limit, turn)
}
