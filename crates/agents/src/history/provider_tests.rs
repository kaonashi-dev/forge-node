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

fn claude_fixture(store: &Path, cwd: &Path, id: &str) -> PathBuf {
    let path = store
        .join("projects")
        .join(claude_slug(cwd))
        .join(format!("{id}.jsonl"));
    write_lines(
        &path,
        &[
            json!({"type":"user", "cwd":cwd, "message":{"content":"hello"}, "timestamp":"2026-09-21T10:00:00Z"}),
        ],
    );
    path
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
    std::fs::create_dir_all(&checkout).unwrap();
    let profiles: Vec<_> = (0..2)
        .map(|index| AgentProfile {
            id: AgentProfileId::new(),
            provider_id: AgentProviderId::new(CLAUDE_PROVIDER),
            name: format!("account-{index}"),
            executable: None,
            config_dir: Some(tmp.path().join(format!("account-{index}"))),
            args: Vec::new(),
            created_at: Timestamp::now(),
        })
        .collect();
    for profile in &profiles {
        claude_fixture(profile.config_dir.as_ref().unwrap(), &checkout, "same-id");
    }
    cursor_fixture(&tmp.path().join(".cursor"), &checkout, "cursor-id");
    let sessions = discover_in(
        tmp.path(),
        &[project],
        std::slice::from_ref(&workspace),
        &profiles,
        &[],
    );
    assert_eq!(sessions.len(), 3);
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
fn transcript_byte_budget_applies_even_to_one_large_unicode_turn() {
    let tmp = tempfile::tempdir().unwrap();
    let root = root(tmp.path());
    let path = cursor_fixture(tmp.path(), &root.path, "large");
    write(
        &path.with_file_name("prompt_history.json"),
        json!(["日本語".repeat(100)]),
    );
    let mut sessions = Vec::new();
    cursor::discover(tmp.path(), &[root], &mut sessions);
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
        provider_id: AgentProviderId::new(CLAUDE_PROVIDER),
        name: "p".into(),
        executable: None,
        config_dir: Some(tmp.path().to_owned()),
        args: Vec::new(),
        created_at: Timestamp::now(),
    };
    let first = fingerprint(&[], &[], std::slice::from_ref(&profile));
    profile.provider_id = AgentProviderId::new(OPENCODE_PROVIDER);
    assert_ne!(first, fingerprint(&[], &[], std::slice::from_ref(&profile)));
    let second = fingerprint(&[], &[], std::slice::from_ref(&profile));
    profile.id = AgentProfileId::new();
    assert_ne!(second, fingerprint(&[], &[], &[profile]));
}
