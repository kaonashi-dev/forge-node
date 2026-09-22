use std::collections::HashMap;
use std::path::Path;

use serde_json::Value;

use super::{bounded, ExternalAgentSession, Root, Scan, Turn, SCAN_LIMIT, USER_SPEAKER};

pub(super) fn discover(store: &Path, roots: &[Root], out: &mut Vec<ExternalAgentSession>) {
    let files = bounded::files(
        &store.join("chats"),
        2,
        |p| p.file_name().is_some_and(|n| n == "meta.json"),
        4096,
    );
    let mut counts = HashMap::new();
    for path in files {
        let Some(meta) = bounded::json(&path) else {
            continue;
        };
        if meta.get("hasConversation").and_then(Value::as_bool) != Some(true) {
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
        let Some(id) = path
            .parent()
            .and_then(Path::file_name)
            .and_then(|s| s.to_str())
        else {
            continue;
        };
        let prompts = turns(&path, usize::MAX);
        let scan = Scan {
            ai_title: meta.get("title").and_then(Value::as_str).map(str::to_owned),
            first_user: prompts
                .first()
                .map(|turn| super::truncate_title(&turn.text)),
            messages: prompts.len().try_into().unwrap_or(u32::MAX),
            first_ts: meta
                .get("createdAtMs")
                .and_then(Value::as_i64)
                .and_then(super::millis_to_ts),
            last_ts: meta
                .get("updatedAtMs")
                .and_then(Value::as_i64)
                .and_then(super::millis_to_ts),
            ..Scan::default()
        };
        out.push(bounded::session("cursor", id.to_owned(), path, root, scan));
        *count += 1;
    }
}

pub(super) fn turns(meta: &Path, limit: usize) -> Vec<Turn> {
    // Cursor encrypts conversation blobs; prompt history is readable but is only a partial transcript.
    let Some(value) = meta
        .parent()
        .and_then(|p| bounded::json(&p.join("prompt_history.json")))
    else {
        return Vec::new();
    };
    let Some(prompts) = value.as_array() else {
        return Vec::new();
    };
    let mut turns: Vec<Turn> = prompts
        .iter()
        .filter_map(Value::as_str)
        .filter(|text| !text.trim().is_empty() && !text.starts_with('/'))
        .take(limit)
        .map(|text| Turn {
            speaker: USER_SPEAKER,
            text: text.to_owned(),
        })
        .collect();
    turns.reverse();
    turns
}
