use std::path::{Path, PathBuf};

use domain::{
    AgentProfile, AgentProfileId, AgentProviderId, Project, ProjectId, Workspace, WorkspaceId,
    WorkspaceKind, WorkspaceStatus,
};
use serde_json::{json, Value};

use super::*;

fn write(path: &Path, value: Value) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
}

fn write_lines(path: &Path, values: &[Value]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let text = values.iter().map(|v| format!("{v}\n")).collect::<String>();
    std::fs::write(path, text).unwrap();
}

fn root(path: &Path) -> Root {
    Root {
        path: path.to_owned(),
        project_id: ProjectId::new(),
        workspace_id: Some(WorkspaceId::new()),
    }
}

fn codex_fixture(store: &Path, cwd: &Path, id: &str) -> PathBuf {
    let path = store.join(format!("sessions/2026/09/21/rollout-{id}.jsonl"));
    write_lines(
        &path,
        &[
            json!({"type":"session_meta", "timestamp":"2026-09-21T10:00:00Z", "payload":{
                "id":id, "cwd":cwd, "timestamp":"2026-09-21T10:00:00Z", "source":"cli", "git":{"branch":"main"}
            }}),
            json!({"type":"response_item", "payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"# AGENTS.md instructions for checkout"}]}}),
            json!({"type":"turn_context", "payload":{"model":"gpt-5-codex"}}),
            json!({"type":"event_msg", "payload":{"type":"user_message", "message":"Repair history"}}),
            json!({"type":"response_item", "payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Repair history"}]}}),
            json!({"type":"response_item", "payload":{"type":"function_call_output","output":"tool output"}}),
            json!({"type":"response_item", "timestamp":"2026-09-21T11:00:00Z", "payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"History repaired"}]}}),
            json!({"type":"event_msg", "payload":{"type":"agent_message", "message":"History repaired"}}),
        ],
    );
    path
}

fn cursor_fixture(store: &Path, cwd: &Path, id: &str) -> PathBuf {
    let path = store.join(format!("chats/opaque-workspace-hash/{id}/meta.json"));
    write(
        &path,
        json!({"schemaVersion":1, "cwd":cwd, "title":"Cursor history", "hasConversation":true, "createdAtMs":1789999200000i64,"updatedAtMs":1790002800000i64}),
    );
    write(
        &path.with_file_name("prompt_history.json"),
        json!(["Latest prompt", "/clear", "First prompt"]),
    );
    path
}

fn grok_fixture(store: &Path, cwd: &Path, id: &str) -> PathBuf {
    let path = store.join(format!("sessions/encoded-or-hashed-cwd/{id}/summary.json"));
    write(
        &path,
        json!({"info":{"id":id,"cwd":cwd}, "generated_title":"Grok history", "created_at":"2026-09-21T10:00:00Z", "updated_at":"2026-09-21T11:00:00Z", "head_branch":"topic", "current_model_id":"grok-4.7"}),
    );
    write_lines(
        &path.with_file_name("chat_history.jsonl"),
        &[
            json!({"type":"system","content":"system instructions"}),
            json!({"type":"user","content":"injected context","synthetic_reason":"system_reminder"}),
            json!({"type":"user","content":[{"type":"text","text":"<user_query>Repair history</user_query>"}]}),
            json!({"type":"tool_result","content":"tool output"}),
            json!({"type":"assistant","content":"History repaired"}),
        ],
    );
    path
}

#[test]
fn codex_history_maps_the_recorded_checkout_and_ignores_duplicate_events() {
    let tmp = tempfile::tempdir().unwrap();
    let root = root(&tmp.path().join("checkout"));
    let path = codex_fixture(tmp.path(), &root.path, "run-1");
    codex_fixture(tmp.path(), &tmp.path().join("other"), "other");
    let mut sessions = Vec::new();
    codex::discover(tmp.path(), std::slice::from_ref(&root), &mut sessions);
    assert_eq!(sessions.len(), 1);
    let session = &sessions[0];
    assert_eq!(session.provider, "codex");
    assert_eq!(session.session_id, "run-1");
    assert_eq!(session.workspace_id, root.workspace_id);
    assert_eq!(session.title, "Repair history");
    assert_eq!(session.message_count, 2);
    assert_eq!(session.branch.as_deref(), Some("main"));
    assert_eq!(session.model.as_deref(), Some("gpt-5-codex"));
    assert_eq!(session.preview.as_deref(), Some("History repaired"));
    let read = read_transcript(session, 80, 36000);
    assert_eq!(read.text, "You: Repair history\n\nAgent: History repaired");
    assert!(!read.truncated);
    delete_transcript(session).unwrap();
    assert!(!path.exists());
}

#[test]
fn cursor_metadata_lists_real_chats_and_partial_prompts_in_order() {
    let tmp = tempfile::tempdir().unwrap();
    let root = root(&tmp.path().join("checkout"));
    let path = cursor_fixture(tmp.path(), &root.path, "chat-1");
    cursor_fixture(tmp.path(), &tmp.path().join("other"), "other");
    let empty = cursor_fixture(tmp.path(), &root.path, "empty");
    write(&empty, json!({"cwd":root.path,"hasConversation":false}));
    let mut sessions = Vec::new();
    cursor::discover(tmp.path(), &[root], &mut sessions);
    assert_eq!(sessions.len(), 1);
    let session = &sessions[0];
    assert_eq!(session.session_id, "chat-1");
    assert_eq!(session.title, "Cursor history");
    assert_eq!(session.message_count, 2);
    let read = read_transcript(session, 80, 36000);
    assert_eq!(read.text, "You: First prompt\n\nYou: Latest prompt");
    assert!(
        read.truncated,
        "encrypted replies are not in prompt history"
    );
    assert_eq!(
        read_transcript(session, 1, 36000).text,
        "You: Latest prompt"
    );
    delete_transcript(session).unwrap();
    assert!(!path.parent().unwrap().exists());
    assert!(empty.exists());
}

#[test]
fn grok_history_reads_prose_and_skips_subagent_cards() {
    let tmp = tempfile::tempdir().unwrap();
    let root = root(&tmp.path().join("checkout"));
    let path = grok_fixture(tmp.path(), &root.path, "run-1");
    write(
        &path.parent().unwrap().join("subagents/child/meta.json"),
        json!({"session_id":"child"}),
    );
    let child = grok_fixture(tmp.path(), &root.path, "child");
    let mut meta = bounded::json(&child).unwrap();
    meta["session_kind"] = json!("subagent");
    write(&child, meta);
    grok_fixture(tmp.path(), &tmp.path().join("other"), "other");
    let mut sessions = Vec::new();
    grok::discover(tmp.path(), &[root], &mut sessions);
    assert_eq!(sessions.len(), 1);
    let session = &sessions[0];
    assert_eq!(session.title, "Grok history");
    assert_eq!(session.branch.as_deref(), Some("topic"));
    assert_eq!(session.model.as_deref(), Some("grok-4.7"));
    assert_eq!(session.message_count, 2);
    assert_eq!(session.subagent_count, 1);
    let read = read_transcript(session, 80, 36000);
    assert_eq!(read.text, "You: Repair history\n\nAgent: History repaired");
    assert!(!read.truncated);
    delete_transcript(session).unwrap();
    assert!(!path.exists());
    assert!(child.exists());
}

#[test]
fn discovery_includes_profiles_and_keeps_same_ids_in_distinct_accounts() {
    let tmp = tempfile::tempdir().unwrap();
    let checkout = tmp.path().join("checkout");
    let project = Project {
        id: ProjectId::new(),
        project_group_id: None,
        name: "p".into(),
        icon: None,
        root_path: checkout.clone(),
        git_root: None,
        created_at: Timestamp::now(),
        last_opened_at: Timestamp::now(),
    };
    let workspace = Workspace {
        id: WorkspaceId::new(),
        project_id: project.id,
        kind: WorkspaceKind::GitWorktree,
        path: checkout.clone(),
        branch: None,
        display_name: None,
        managed_by_app: true,
        created_at: Timestamp::now(),
        status: WorkspaceStatus::default(),
    };
    let profiles: Vec<_> = ["codex", "codex", "grok"]
        .iter()
        .enumerate()
        .map(|(index, provider)| AgentProfile {
            id: AgentProfileId::new(),
            provider_id: AgentProviderId::new(*provider),
            name: format!("account-{index}"),
            executable: None,
            config_dir: Some(tmp.path().join(format!("account-{index}"))),
            args: Vec::new(),
            created_at: Timestamp::now(),
        })
        .collect();
    codex_fixture(
        profiles[0].config_dir.as_ref().unwrap(),
        &checkout,
        "same-id",
    );
    codex_fixture(
        profiles[1].config_dir.as_ref().unwrap(),
        &checkout,
        "same-id",
    );
    grok_fixture(
        profiles[2].config_dir.as_ref().unwrap(),
        &checkout,
        "grok-id",
    );
    cursor_fixture(&tmp.path().join(".cursor"), &checkout, "cursor-id");
    let sessions = discover_in(
        tmp.path(),
        &[project],
        std::slice::from_ref(&workspace),
        &profiles,
    );
    assert_eq!(sessions.len(), 4);
    assert!(sessions
        .iter()
        .all(|session| session.workspace_id == Some(workspace.id)));
    for profile in &profiles {
        assert_eq!(
            sessions
                .iter()
                .filter(|s| s.profile_id == Some(profile.id))
                .count(),
            1
        );
    }
}

#[test]
fn discovery_caps_each_checkout_independently() {
    let tmp = tempfile::tempdir().unwrap();
    let first = root(&tmp.path().join("first"));
    let second = root(&tmp.path().join("second"));
    for i in 0..SCAN_LIMIT + 1 {
        cursor_fixture(tmp.path(), &first.path, &format!("chat-{i}"));
    }
    cursor_fixture(tmp.path(), &second.path, "second-chat");
    let mut sessions = Vec::new();
    cursor::discover(tmp.path(), &[first, second], &mut sessions);
    assert_eq!(sessions.len(), SCAN_LIMIT + 1);
    assert!(sessions.iter().any(|s| s.session_id == "second-chat"));
}

#[test]
fn bounded_transcripts_skip_oversized_records_and_keep_later_prose() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("chat_history.jsonl");
    write_lines(
        &path,
        &[
            json!({"type":"tool_result","content":"x".repeat(bounded::MAX_RECORD_BYTES as usize + 1)}),
            json!({"type":"assistant","content":"Last reply"}),
        ],
    );
    let read = grok::turns(&path, 10);
    assert!(read.truncated);
    assert_eq!(read.turns.len(), 1);
    assert_eq!(read.turns[0].text, "Last reply");
}

#[test]
fn transcript_byte_budget_applies_even_to_one_large_unicode_turn() {
    let tmp = tempfile::tempdir().unwrap();
    let root = root(tmp.path());
    let path = codex_fixture(tmp.path(), &root.path, "large");
    let mut sessions = Vec::new();
    codex::discover(tmp.path(), &[root], &mut sessions);
    write_lines(
        &path,
        &[
            json!({"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"日本語".repeat(100)}]}}),
        ],
    );
    for budget in [0, 1, 7, 8, 9, 10, 11, 20] {
        let read = read_transcript(&sessions[0], 80, budget);
        assert!(read.text.len() <= budget);
        assert!(read.truncated);
    }
}

#[test]
fn cache_identity_includes_account_provider_and_workspace_identity() {
    let tmp = tempfile::tempdir().unwrap();
    let mut profile = AgentProfile {
        id: AgentProfileId::new(),
        provider_id: AgentProviderId::new("codex"),
        name: "p".into(),
        executable: None,
        config_dir: Some(tmp.path().to_owned()),
        args: Vec::new(),
        created_at: Timestamp::now(),
    };
    let first = fingerprint(&[], &[], std::slice::from_ref(&profile));
    profile.provider_id = AgentProviderId::new("grok");
    assert_ne!(first, fingerprint(&[], &[], std::slice::from_ref(&profile)));
    let second = fingerprint(&[], &[], std::slice::from_ref(&profile));
    profile.id = AgentProfileId::new();
    assert_ne!(second, fingerprint(&[], &[], &[profile]));
}
