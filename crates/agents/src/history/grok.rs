use std::collections::HashMap;
use std::path::Path;

use serde_json::Value;

use super::{
    bounded, ExternalAgentSession, Root, Scan, Turn, AGENT_SPEAKER, SCAN_LIMIT, USER_SPEAKER,
};

pub(super) fn discover(store: &Path, roots: &[Root], out: &mut Vec<ExternalAgentSession>) {
    let files = bounded::files(
        &store.join("sessions"),
        2,
        |p| p.file_name().is_some_and(|n| n == "summary.json"),
        4096,
    );
    let mut counts = HashMap::new();
    for path in files {
        let Some(meta) = bounded::json(&path) else {
            continue;
        };
        if meta.get("session_kind").and_then(Value::as_str) == Some("subagent") {
            continue;
        }
        let Some(root) = meta
            .pointer("/info/cwd")
            .and_then(Value::as_str)
            .and_then(|cwd| bounded::root_for(cwd, roots))
        else {
            continue;
        };
        let Some(id) = meta.pointer("/info/id").and_then(Value::as_str) else {
            continue;
        };
        let Some(dir) = path
            .parent()
            .filter(|dir| dir.file_name().and_then(|s| s.to_str()) == Some(id))
        else {
            continue;
        };
        let count = counts.entry(root.path.clone()).or_insert(0);
        if *count >= SCAN_LIMIT {
            continue;
        }
        let transcript = dir.join("chat_history.jsonl");
        let mut scan = Scan {
            ai_title: ["title", "generated_title", "session_summary"]
                .iter()
                .find_map(|key| {
                    meta.get(key)
                        .and_then(Value::as_str)
                        .and_then(super::non_empty)
                }),
            branch: meta
                .get("head_branch")
                .and_then(Value::as_str)
                .map(str::to_owned),
            model: meta
                .get("current_model_id")
                .and_then(Value::as_str)
                .map(str::to_owned),
            first_ts: bounded::timestamp(meta.get("created_at")),
            last_ts: bounded::timestamp(
                meta.get("last_active_at")
                    .or_else(|| meta.get("updated_at")),
            ),
            ..Scan::default()
        };
        let truncated = bounded::jsonl(&transcript, |value| {
            if let Some(turn) = turn(value) {
                bounded::fold_turn(&mut scan, turn);
            }
        });
        if truncated {
            tracing::debug!(path = %transcript.display(), "grok history transcript truncated");
        }
        let mut session = bounded::session("grok", id.to_owned(), transcript, root, scan);
        session.subagent_count = bounded::files(
            &dir.join("subagents"),
            1,
            |p| p.file_name().is_some_and(|n| n == "meta.json"),
            4096,
        )
        .len()
        .try_into()
        .unwrap_or(u32::MAX);
        out.push(session);
        *count += 1;
    }
}

fn turn(value: &Value) -> Option<Turn> {
    if value.get("synthetic_reason").is_some() {
        return None;
    }
    let speaker = match value.get("type")?.as_str()? {
        "user" => USER_SPEAKER,
        "assistant" => AGENT_SPEAKER,
        _ => return None,
    };
    let mut text = bounded::text(value.get("content")?)?;
    if speaker == USER_SPEAKER {
        if let Some((_, query)) = text.split_once("<user_query>") {
            text = query
                .split_once("</user_query>")
                .map_or(query, |(q, _)| q)
                .trim()
                .to_owned();
        } else if text.starts_with('<') {
            return None;
        }
    }
    super::non_empty(&text).map(|text| Turn { speaker, text })
}

pub(super) fn turns(path: &Path, limit: usize) -> bounded::Conversation {
    bounded::conversation(path, limit, turn)
}
