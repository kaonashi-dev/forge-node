//! Integration-style tests for the persistence layer, run against fresh
//! in-memory databases (§15). Each test opens its own [`Db`], so WAL,
//! `foreign_keys = ON` and the migrations all run per test.

use std::path::PathBuf;

use domain::{
    AgentProfile, AgentProfileId, AgentProviderId, ContextArtifactKind, ContextArtifactRef,
    ContextEnvelope, ContextId, GitContextRef, IgnoreScope, Project, ProjectGroup, ProjectGroupId,
    ProjectId, Session, SessionId, SessionKind, SessionRole, SessionState, SessionTitle, ShareRule,
    ShareRuleId, ShareStrategy, Timestamp, Workspace, WorkspaceId, WorkspaceKind, WorkspaceStatus,
    WorktreeIgnore,
};

use crate::Db;

// ---- builders --------------------------------------------------------------

fn mk_project(root: &str) -> Project {
    Project {
        id: ProjectId::new(),
        project_group_id: None,
        name: "demo".to_owned(),
        icon: None,
        root_path: PathBuf::from(root),
        git_root: None,
        created_at: Timestamp::now(),
        last_opened_at: Timestamp::now(),
    }
}

fn mk_workspace(project_id: ProjectId, path: &str) -> Workspace {
    Workspace {
        id: WorkspaceId::new(),
        project_id,
        kind: WorkspaceKind::Main,
        path: PathBuf::from(path),
        branch: None,
        display_name: None,
        managed_by_app: false,
        created_at: Timestamp::now(),
        status: WorkspaceStatus::default(),
    }
}

/// A `Db` seeded with one project and one workspace; returns their ids so
/// session tests satisfy the foreign keys.
fn db_with_workspace() -> (Db, ProjectId, WorkspaceId) {
    let db = Db::open_in_memory().unwrap();
    let project = mk_project("/tmp/forge-demo");
    db.projects().upsert(&project).unwrap();
    let workspace = mk_workspace(project.id, "/tmp/forge-demo");
    db.workspaces().upsert(&workspace).unwrap();
    (db, project.id, workspace.id)
}

fn mk_session(workspace_id: WorkspaceId, state: SessionState) -> Session {
    let id = SessionId::new();
    let created_at = Timestamp::now();
    Session {
        id,
        workspace_id,
        kind: SessionKind::Shell,
        role: SessionRole::Generic,
        parent_session_id: None,
        root_session_id: id,
        terminal_id: None,
        agent_provider_id: None,
        agent_profile_id: None,
        title: SessionTitle::default(),
        state,
        created_at,
        launch_command: None,
        last_activity_at: created_at,
        ended_at: None,
        base_commit: None,
    }
}

// ---- migrations ------------------------------------------------------------

#[test]
fn migrations_apply_and_user_version_advances() {
    let db = Db::open_in_memory().unwrap();
    let version: i64 = db
        .conn()
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    // Bump this with every appended migration (§15.2): `user_version` is the
    // count of applied migrations, and the assertion is what catches a migration
    // that was silently reordered or dropped.
    assert_eq!(
        version, 11,
        "eleven migrations applied => user_version == 11"
    );

    // Every §15.2 table is queryable.
    for table in [
        "projects",
        "project_groups",
        "workspaces",
        "sessions",
        "context_envelopes",
        "provider_overrides",
        "app_state",
        "agent_profiles",
        "worktree_shares",
        "worktree_ignores",
    ] {
        let count: i64 = db
            .conn()
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
            .unwrap_or_else(|e| panic!("table {table} not queryable: {e}"));
        assert_eq!(count, 0);
    }
}

#[test]
fn version_two_database_upgrades_existing_projects_into_general() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("forge-v2.db");
    let project_id = ProjectId::new();
    let now = Timestamp::now().to_rfc3339();
    {
        let mut conn = rusqlite::Connection::open(&path).unwrap();
        let legacy = rusqlite_migration::Migrations::new(vec![
            rusqlite_migration::M::up(crate::migrations::INITIAL_SCHEMA),
            rusqlite_migration::M::up(crate::migrations::REFERENTIAL_ACTIONS),
        ]);
        legacy.to_latest(&mut conn).unwrap();
        conn.execute(
            "INSERT INTO projects \
             (id, name, root_path, git_root, created_at, last_opened_at) \
             VALUES (?1, 'legacy', '/tmp/legacy', NULL, ?2, ?2)",
            rusqlite::params![project_id.to_string(), now],
        )
        .unwrap();
    }

    let db = Db::open(&path).unwrap();
    let project = db.projects().get(project_id).unwrap().unwrap();
    assert_eq!(project.name, "legacy");
    assert_eq!(project.project_group_id, None);
    assert!(db.project_groups().list().unwrap().is_empty());
    let version: i64 = db
        .conn()
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, 11);
}

/// Migration 6 over a database that already has workspaces: the display_name
/// column is nullable and nothing is backfilled, so every existing checkout
/// keeps leading with its branch.
#[test]
fn version_five_database_upgrades_and_its_workspaces_have_no_display_name() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("forge-v5.db");
    let project_id = ProjectId::new();
    let workspace_id = WorkspaceId::new();
    let now = Timestamp::now().to_rfc3339();
    {
        let mut conn = rusqlite::Connection::open(&path).unwrap();
        let legacy = rusqlite_migration::Migrations::new(vec![
            rusqlite_migration::M::up(crate::migrations::INITIAL_SCHEMA),
            rusqlite_migration::M::up(crate::migrations::REFERENTIAL_ACTIONS),
            rusqlite_migration::M::up(crate::migrations::PROJECT_GROUPS),
            rusqlite_migration::M::up(crate::migrations::AGENT_PROFILES),
            rusqlite_migration::M::up(crate::migrations::PROJECT_ICONS),
        ]);
        legacy.to_latest(&mut conn).unwrap();
        conn.execute(
            "INSERT INTO projects \
             (id, name, root_path, git_root, created_at, last_opened_at) \
             VALUES (?1, 'legacy', '/tmp/legacy-names', NULL, ?2, ?2)",
            rusqlite::params![project_id.to_string(), now],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO workspaces \
             (id, project_id, kind, path, branch, managed_by_app, created_at) \
             VALUES (?1, ?2, 'Main', '/tmp/legacy-names', 'main', 0, ?3)",
            rusqlite::params![workspace_id.to_string(), project_id.to_string(), now],
        )
        .unwrap();
    }

    let db = Db::open(&path).unwrap();
    let workspace = db.workspaces().get(workspace_id).unwrap().unwrap();
    assert_eq!(workspace.branch.as_deref(), Some("main"));
    assert_eq!(workspace.display_name, None, "nothing is backfilled");

    let mut named = workspace.clone();
    named.display_name = Some("Portal redesign".to_owned());
    db.workspaces().upsert(&named).unwrap();
    assert_eq!(
        db.workspaces()
            .get(workspace_id)
            .unwrap()
            .unwrap()
            .display_name
            .as_deref(),
        Some("Portal redesign"),
        "the upgraded row takes a display name like any other"
    );
}

/// Migration 10 over profiles written when a profile carried a whole
/// environment: the config directory is the one value that survives, and the
/// variable it was spelled with no longer matters.
#[test]
fn version_nine_profiles_keep_their_config_directory() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("forge-v9.db");
    let claude = AgentProfileId::new();
    let codex = AgentProfileId::new();
    let plain = AgentProfileId::new();
    let now = Timestamp::now().to_rfc3339();
    {
        let mut conn = rusqlite::Connection::open(&path).unwrap();
        let legacy = rusqlite_migration::Migrations::new(vec![
            rusqlite_migration::M::up(crate::migrations::INITIAL_SCHEMA),
            rusqlite_migration::M::up(crate::migrations::REFERENTIAL_ACTIONS),
            rusqlite_migration::M::up(crate::migrations::PROJECT_GROUPS),
            rusqlite_migration::M::up(crate::migrations::AGENT_PROFILES),
            rusqlite_migration::M::up(crate::migrations::PROJECT_ICONS),
            rusqlite_migration::M::up(crate::migrations::WORKSPACE_DISPLAY_NAMES),
            rusqlite_migration::M::up(crate::migrations::SESSION_LAUNCH_COMMAND),
            rusqlite_migration::M::up(crate::migrations::WORKTREE_SHARES),
            rusqlite_migration::M::up(crate::migrations::SESSION_BASE_COMMIT),
        ]);
        legacy.to_latest(&mut conn).unwrap();
        let insert = |id: &AgentProfileId, provider: &str, name: &str, env: &str| {
            conn.execute(
                "INSERT INTO agent_profiles \
                 (id, provider_id, name, executable_path, args_json, env_json, created_at) \
                 VALUES (?1, ?2, ?3, NULL, '[\"--model\",\"opus\"]', ?4, ?5)",
                rusqlite::params![id.to_string(), provider, name, env, now],
            )
            .unwrap();
        };
        insert(
            &claude,
            "claude",
            "Personal",
            r#"[["HTTP_PROXY","http://localhost:8080"],["CLAUDE_CONFIG_DIR","/home/me/.claude-personal"]]"#,
        );
        insert(&codex, "codex", "Work", r#"[["CODEX_HOME",".codex-work"]]"#);
        insert(&plain, "claude", "Bare", "[]");
    }

    let db = Db::open(&path).unwrap();
    let by_id = |id: AgentProfileId| db.agent_profiles().get(id).unwrap().unwrap();

    // Whichever variable named it, it is now just the directory — and the rest
    // of the environment is gone, which is the point of the migration.
    assert_eq!(
        by_id(claude).config_dir,
        Some(std::path::PathBuf::from("/home/me/.claude-personal"))
    );
    assert_eq!(
        by_id(codex).config_dir,
        Some(std::path::PathBuf::from(".codex-work"))
    );
    assert_eq!(by_id(plain).config_dir, None);
    assert_eq!(by_id(plain).args, ["--model", "opus"]);
}

/// Migration 5 over a database that already has projects: the icon column is
/// nullable and nothing is backfilled, so every existing project keeps the
/// initials it was already being drawn with.
#[test]
fn version_four_database_upgrades_and_its_projects_have_no_icon() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("forge-v4.db");
    let project_id = ProjectId::new();
    let now = Timestamp::now().to_rfc3339();
    {
        let mut conn = rusqlite::Connection::open(&path).unwrap();
        let legacy = rusqlite_migration::Migrations::new(vec![
            rusqlite_migration::M::up(crate::migrations::INITIAL_SCHEMA),
            rusqlite_migration::M::up(crate::migrations::REFERENTIAL_ACTIONS),
            rusqlite_migration::M::up(crate::migrations::PROJECT_GROUPS),
            rusqlite_migration::M::up(crate::migrations::AGENT_PROFILES),
        ]);
        legacy.to_latest(&mut conn).unwrap();
        conn.execute(
            "INSERT INTO projects \
             (id, name, root_path, git_root, created_at, last_opened_at) \
             VALUES (?1, 'legacy', '/tmp/legacy-icons', NULL, ?2, ?2)",
            rusqlite::params![project_id.to_string(), now],
        )
        .unwrap();
    }

    let db = Db::open(&path).unwrap();
    let project = db.projects().get(project_id).unwrap().unwrap();
    assert_eq!(project.name, "legacy");
    assert_eq!(project.icon, None, "nothing is backfilled");

    let mut iconed = project.clone();
    iconed.icon = Some("🦀".to_owned());
    db.projects().upsert(&iconed).unwrap();
    assert_eq!(
        db.projects()
            .get(project_id)
            .unwrap()
            .unwrap()
            .icon
            .as_deref(),
        Some("🦀"),
        "the upgraded row takes an icon like any other"
    );
}

/// Migration 4 over a database that already has sessions: the new column is
/// nullable, so existing rows keep working and simply carry no profile.
#[test]
fn version_three_database_upgrades_and_keeps_its_sessions() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("forge-v3.db");
    let project_id = ProjectId::new();
    let workspace_id = WorkspaceId::new();
    let session_id = SessionId::new();
    let now = Timestamp::now().to_rfc3339();
    {
        let mut conn = rusqlite::Connection::open(&path).unwrap();
        let legacy = rusqlite_migration::Migrations::new(vec![
            rusqlite_migration::M::up(crate::migrations::INITIAL_SCHEMA),
            rusqlite_migration::M::up(crate::migrations::REFERENTIAL_ACTIONS),
            rusqlite_migration::M::up(crate::migrations::PROJECT_GROUPS),
        ]);
        legacy.to_latest(&mut conn).unwrap();
        conn.execute(
            "INSERT INTO projects \
             (id, name, root_path, git_root, created_at, last_opened_at) \
             VALUES (?1, 'legacy', '/tmp/legacy-v3', NULL, ?2, ?2)",
            rusqlite::params![project_id.to_string(), now],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO workspaces \
             (id, project_id, kind, path, branch, managed_by_app, created_at) \
             VALUES (?1, ?2, 'Main', '/tmp/legacy-v3', NULL, 0, ?3)",
            rusqlite::params![workspace_id.to_string(), project_id.to_string(), now],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sessions \
             (id, workspace_id, kind, role, parent_session_id, root_session_id, \
              agent_provider_id, user_title, terminal_title, last_state, last_exit_code, \
              created_at, ended_at) \
             VALUES (?1, ?2, 'Agent', 'Generic', NULL, ?1, 'claude', NULL, NULL, \
                     'Orphaned', NULL, ?3, NULL)",
            rusqlite::params![session_id.to_string(), workspace_id.to_string(), now],
        )
        .unwrap();
    }

    let db = Db::open(&path).unwrap();
    let session = db.sessions().get(session_id).unwrap().unwrap();
    assert_eq!(session.agent_provider_id.unwrap().as_str(), "claude");
    assert_eq!(session.agent_profile_id, None);
    assert!(db.agent_profiles().list().unwrap().is_empty());
}

// ---- agent profiles (§13.4) -------------------------------------------------

fn mk_profile(provider: &str, name: &str) -> AgentProfile {
    AgentProfile {
        id: AgentProfileId::new(),
        provider_id: AgentProviderId::new(provider),
        name: name.to_owned(),
        executable: None,
        // Stored as typed: what a relative directory is relative to is the
        // launching user's home, which is not a fact this row knows.
        config_dir: Some(std::path::PathBuf::from(".claude-personal")),
        args: vec!["--model".to_owned(), "opus".to_owned()],
        created_at: Timestamp::now(),
    }
}

#[test]
fn agent_profiles_round_trip_with_their_directory_and_args() {
    let db = Db::open_in_memory().unwrap();
    let profile = mk_profile("claude", "Work");
    db.agent_profiles().upsert(&profile).unwrap();

    let back = db.agent_profiles().get(profile.id).unwrap().unwrap();
    assert_eq!(back, profile);

    // Upsert replaces in place rather than inserting a second row.
    let renamed = AgentProfile {
        name: "Work 2".to_owned(),
        args: vec![],
        ..profile.clone()
    };
    db.agent_profiles().upsert(&renamed).unwrap();
    let all = db.agent_profiles().list().unwrap();
    assert_eq!(all, vec![renamed]);

    assert!(db.agent_profiles().delete(profile.id).unwrap());
    assert!(db.agent_profiles().list().unwrap().is_empty());
    assert!(!db.agent_profiles().delete(profile.id).unwrap());
}

#[test]
fn a_profile_name_is_unique_per_provider_ignoring_case() {
    let db = Db::open_in_memory().unwrap();
    db.agent_profiles()
        .upsert(&mk_profile("claude", "Work"))
        .unwrap();

    // Same provider, same name in another case: rejected.
    assert!(db
        .agent_profiles()
        .upsert(&mk_profile("claude", "work"))
        .is_err());

    // Another provider may reuse the name.
    db.agent_profiles()
        .upsert(&mk_profile("codex", "Work"))
        .unwrap();
    assert_eq!(db.agent_profiles().list().unwrap().len(), 2);
}

#[test]
fn profiles_are_listed_by_provider_then_name() {
    let db = Db::open_in_memory().unwrap();
    for (provider, name) in [
        ("codex", "Work"),
        ("claude", "work"),
        ("claude", "Personal"),
    ] {
        db.agent_profiles()
            .upsert(&mk_profile(provider, name))
            .unwrap();
    }
    let listed: Vec<(String, String)> = db
        .agent_profiles()
        .list()
        .unwrap()
        .into_iter()
        .map(|p| (p.provider_id.to_string(), p.name))
        .collect();
    assert_eq!(
        listed,
        [
            ("claude".to_owned(), "Personal".to_owned()),
            ("claude".to_owned(), "work".to_owned()),
            ("codex".to_owned(), "Work".to_owned()),
        ]
    );
}

#[test]
fn a_session_survives_the_deletion_of_its_profile() {
    let (db, _project_id, workspace_id) = db_with_workspace();
    let profile = mk_profile("claude", "Work");
    db.agent_profiles().upsert(&profile).unwrap();

    let mut session = mk_session(workspace_id, SessionState::Running);
    session.agent_provider_id = Some(AgentProviderId::new("claude"));
    session.agent_profile_id = Some(profile.id);
    db.sessions().upsert(&session).unwrap();

    assert!(db.agent_profiles().delete(profile.id).unwrap());

    // History keeps the pointer; the GUI falls back to the provider name.
    let back = db.sessions().get(session.id).unwrap().unwrap();
    assert_eq!(back.agent_profile_id, Some(profile.id));
}

#[test]
fn project_groups_roundtrip_and_projects_fall_back_to_general_on_delete() {
    let db = Db::open_in_memory().unwrap();
    let group = ProjectGroup {
        id: ProjectGroupId::new(),
        name: "Product X".to_string(),
        created_at: Timestamp::now(),
    };
    db.project_groups().upsert(&group).unwrap();
    assert_eq!(
        db.project_groups().get(group.id).unwrap(),
        Some(group.clone())
    );
    assert_eq!(db.project_groups().list().unwrap(), vec![group.clone()]);

    let mut project = mk_project("/tmp/grouped");
    project.project_group_id = Some(group.id);
    db.projects().upsert(&project).unwrap();
    assert_eq!(
        db.projects()
            .get(project.id)
            .unwrap()
            .unwrap()
            .project_group_id,
        Some(group.id)
    );

    assert!(db.project_groups().delete(group.id).unwrap());
    assert_eq!(
        db.projects()
            .get(project.id)
            .unwrap()
            .unwrap()
            .project_group_id,
        None
    );
}

#[test]
fn foreign_keys_are_enforced() {
    let db = Db::open_in_memory().unwrap();
    // A session referencing a workspace that does not exist must fail.
    let orphan = mk_session(WorkspaceId::new(), SessionState::Running);
    let err = db.sessions().upsert(&orphan);
    assert!(err.is_err(), "FK violation should surface as an error");
}

/// Migration 2: an envelope is owned by the session that produced it, so
/// deleting the session must delete the envelope rather than fail the delete.
/// Before the referential actions were added, `CloseSession` on any session that
/// had produced an envelope failed with a constraint error, permanently.
#[test]
fn deleting_a_session_cascades_to_its_context_envelopes() {
    let db = Db::open_in_memory().unwrap();
    let project = mk_project("/tmp/cascade");
    db.projects().upsert(&project).unwrap();
    let workspace = mk_workspace(project.id, "/tmp/cascade");
    db.workspaces().upsert(&workspace).unwrap();

    let source = mk_session(workspace.id, SessionState::Running);
    let target = mk_session(workspace.id, SessionState::Running);
    db.sessions().upsert(&source).unwrap();
    db.sessions().upsert(&target).unwrap();

    let envelope = ContextEnvelope {
        id: ContextId::new(),
        source_session_id: source.id,
        target_session_id: Some(target.id),
        summary: Some("hand-off".to_string()),
        instructions: None,
        artifacts: vec![],
        git_context: None,
        created_at: Timestamp::now(),
    };
    db.context().insert(&envelope).unwrap();

    // Deleting the *target* only clears the pointer (ON DELETE SET NULL).
    assert!(db.sessions().delete(target.id).unwrap());
    let kept = db.context().list_by_source(source.id).unwrap();
    assert_eq!(kept.len(), 1, "envelope survives a deleted recipient");
    assert_eq!(kept[0].target_session_id, None);

    // Deleting the *source* removes the envelope with it (ON DELETE CASCADE).
    assert!(db.sessions().delete(source.id).unwrap());
    assert!(db.context().list_by_source(source.id).unwrap().is_empty());
}

// ---- projects --------------------------------------------------------------

#[test]
fn project_roundtrip_and_unique_root_path() {
    let db = Db::open_in_memory().unwrap();

    // git_root = None.
    let p1 = mk_project("/tmp/a");
    db.projects().upsert(&p1).unwrap();
    assert_eq!(db.projects().get(p1.id).unwrap().as_ref(), Some(&p1));

    // git_root = Some.
    let mut p2 = mk_project("/tmp/b");
    p2.git_root = Some(PathBuf::from("/tmp/b/.git-root"));
    db.projects().upsert(&p2).unwrap();
    assert_eq!(db.projects().get(p2.id).unwrap().as_ref(), Some(&p2));

    // list returns both.
    let all = db.projects().list().unwrap();
    assert_eq!(all.len(), 2);

    // Upsert updates in place (same id, new name).
    let mut p1b = p1.clone();
    p1b.name = "renamed".to_owned();
    db.projects().upsert(&p1b).unwrap();
    assert_eq!(db.projects().get(p1.id).unwrap().unwrap().name, "renamed");
    assert_eq!(db.projects().list().unwrap().len(), 2, "still two projects");

    // A *different* project with the same root_path violates UNIQUE(root_path).
    let clash = mk_project("/tmp/a");
    assert!(db.projects().upsert(&clash).is_err());

    // touch_last_opened and delete.
    let later = Timestamp::now();
    assert!(db.projects().touch_last_opened(p1.id, later).unwrap());
    assert_eq!(
        db.projects().get(p1.id).unwrap().unwrap().last_opened_at,
        later
    );
    assert!(db.projects().delete(p2.id).unwrap());
    assert_eq!(db.projects().list().unwrap().len(), 1);
}

// ---- workspaces ------------------------------------------------------------

#[test]
fn workspace_roundtrip_with_kind_and_managed_flag() {
    let db = Db::open_in_memory().unwrap();
    let project = mk_project("/tmp/proj");
    db.projects().upsert(&project).unwrap();

    let main = mk_workspace(project.id, "/tmp/proj");
    db.workspaces().upsert(&main).unwrap();

    let worktree = Workspace {
        id: WorkspaceId::new(),
        project_id: project.id,
        kind: WorkspaceKind::GitWorktree,
        path: PathBuf::from("/tmp/proj-wt/feature"),
        branch: Some("feature/auth".to_owned()),
        display_name: Some("Auth refresh".to_owned()),
        managed_by_app: true,
        created_at: Timestamp::now(),
        status: WorkspaceStatus::default(),
    };
    db.workspaces().upsert(&worktree).unwrap();

    assert_eq!(db.workspaces().get(main.id).unwrap().as_ref(), Some(&main));
    let loaded_wt = db.workspaces().get(worktree.id).unwrap().unwrap();
    assert_eq!(loaded_wt, worktree);
    assert_eq!(loaded_wt.kind, WorkspaceKind::GitWorktree);
    assert!(loaded_wt.managed_by_app);
    assert_eq!(loaded_wt.display_name.as_deref(), Some("Auth refresh"));

    let list = db.workspaces().list_by_project(project.id).unwrap();
    assert_eq!(list.len(), 2);

    assert!(db.workspaces().delete(worktree.id).unwrap());
    assert_eq!(
        db.workspaces().list_by_project(project.id).unwrap().len(),
        1
    );
}

// ---- sessions --------------------------------------------------------------

#[test]
fn session_roundtrip_custom_role_states_and_graph() {
    let (db, _project, workspace_id) = db_with_workspace();

    // Root session: Custom role, Exited with a code, both title fields set,
    // agent kind + provider, terminal_id set (must NOT survive persistence).
    let root_id = SessionId::new();
    let root = Session {
        id: root_id,
        workspace_id,
        kind: SessionKind::Agent,
        role: SessionRole::Custom("Auth Refactor".to_owned()),
        parent_session_id: None,
        root_session_id: root_id,
        terminal_id: Some(domain::TerminalId::new()),
        agent_provider_id: Some("claude".into()),
        agent_profile_id: None,
        title: SessionTitle {
            user: Some("auth refactor".to_owned()),
            terminal: Some("~/code".to_owned()),
        },
        state: SessionState::Exited {
            code: Some(42),
            signal: Some(9),
        },
        created_at: Timestamp::now(),
        launch_command: None,
        // Runtime-only, like terminal_id: deliberately far from `ended_at` so
        // the assertion below cannot pass by coincidence.
        last_activity_at: Timestamp::from_offset(
            Timestamp::now().as_offset() + time::Duration::hours(3),
        ),
        ended_at: Some(Timestamp::now()),
        base_commit: None,
    };
    db.sessions().upsert(&root).unwrap();

    let loaded = db.sessions().get(root_id).unwrap().unwrap();
    // terminal_id is runtime-only: None on load (§15.2).
    assert_eq!(loaded.terminal_id, None);
    // last_activity_at is runtime-only too: a loaded row describes a session
    // whose PTY is gone, so it reads back as when the session ended.
    assert_eq!(loaded.last_activity_at, root.ended_at.unwrap());
    // signal has no column: reconstructed as None (§15.2).
    assert_eq!(
        loaded.state,
        SessionState::Exited {
            code: Some(42),
            signal: None
        }
    );
    // Everything else round-trips exactly.
    let expected = Session {
        terminal_id: None,
        last_activity_at: root.ended_at.unwrap(),
        state: SessionState::Exited {
            code: Some(42),
            signal: None,
        },
        ..root.clone()
    };
    assert_eq!(loaded, expected);
    assert_eq!(loaded.role, SessionRole::Custom("Auth Refactor".to_owned()));

    // Child session referencing the root (parent/root ids, Orphaned state).
    let child_id = SessionId::new();
    let child_created = Timestamp::now();
    let child = Session {
        id: child_id,
        workspace_id,
        kind: SessionKind::Shell,
        role: SessionRole::Planner,
        parent_session_id: Some(root_id),
        root_session_id: root_id,
        terminal_id: None,
        agent_provider_id: None,
        agent_profile_id: None,
        title: SessionTitle::default(),
        state: SessionState::Orphaned,
        created_at: child_created,
        launch_command: Some("npm run dev".to_owned()),
        // A live row has no `ended_at`, so it loads back at `created_at`.
        last_activity_at: child_created,
        ended_at: None,
        // Round-tripped like any other column; the round-trip assert below is
        // what proves migration 9 landed.
        base_commit: Some("9f1c0de4c0ffee0000000000000000000000cafe".to_owned()),
    };
    db.sessions().upsert(&child).unwrap();
    assert_eq!(db.sessions().get(child_id).unwrap().unwrap(), child);

    // Failed reason is not persisted: reconstructed generically.
    let failed = mk_session(
        workspace_id,
        SessionState::Failed {
            reason: "boom".into(),
        },
    );
    db.sessions().upsert(&failed).unwrap();
    let loaded_failed = db.sessions().get(failed.id).unwrap().unwrap();
    assert!(matches!(loaded_failed.state, SessionState::Failed { .. }));
    if let SessionState::Failed { reason } = loaded_failed.state {
        assert_ne!(reason, "boom", "reason is not persisted (§15.2)");
        assert!(!reason.is_empty());
    }

    // list / list_by_workspace.
    assert_eq!(db.sessions().list().unwrap().len(), 3);
    assert_eq!(
        db.sessions().list_by_workspace(workspace_id).unwrap().len(),
        3
    );
}

#[test]
fn session_update_state() {
    let (db, _project, workspace_id) = db_with_workspace();
    let session = mk_session(workspace_id, SessionState::Starting);
    db.sessions().upsert(&session).unwrap();

    let ended = Timestamp::now();
    let new_state = SessionState::Exited {
        code: Some(0),
        signal: None,
    };
    assert!(db
        .sessions()
        .update_state(session.id, &new_state, Some(ended))
        .unwrap());

    let loaded = db.sessions().get(session.id).unwrap().unwrap();
    assert_eq!(loaded.state, new_state);
    assert_eq!(loaded.ended_at, Some(ended));

    // Updating a missing session reports false.
    assert!(!db
        .sessions()
        .update_state(SessionId::new(), &new_state, None)
        .unwrap());
}

// ---- context envelopes -----------------------------------------------------

#[test]
fn context_envelope_roundtrip_with_artifacts_and_git() {
    let (db, _project, workspace_id) = db_with_workspace();
    let source = mk_session(workspace_id, SessionState::Running);
    let target = mk_session(workspace_id, SessionState::Running);
    db.sessions().upsert(&source).unwrap();
    db.sessions().upsert(&target).unwrap();

    let envelope = ContextEnvelope {
        id: ContextId::new(),
        source_session_id: source.id,
        target_session_id: Some(target.id),
        summary: Some("did the thing".to_owned()),
        instructions: Some("now do the next thing".to_owned()),
        artifacts: vec![
            ContextArtifactRef {
                kind: ContextArtifactKind::Plan,
                value: "step 1, step 2".to_owned(),
            },
            ContextArtifactRef {
                kind: ContextArtifactKind::FileReference,
                value: "src/main.rs".to_owned(),
            },
        ],
        git_context: Some(GitContextRef {
            repo_path: PathBuf::from("/tmp/repo"),
            branch: Some("main".to_owned()),
            commit: Some("abc123".to_owned()),
        }),
        created_at: Timestamp::now(),
    };
    db.context().insert(&envelope).unwrap();

    let loaded = db.context().list_by_source(source.id).unwrap();
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0], envelope);

    // An envelope with no artifacts and no git context also round-trips.
    let bare = ContextEnvelope {
        id: ContextId::new(),
        source_session_id: source.id,
        target_session_id: None,
        summary: None,
        instructions: None,
        artifacts: vec![],
        git_context: None,
        created_at: Timestamp::now(),
    };
    db.context().insert(&bare).unwrap();
    let loaded = db.context().list_by_source(source.id).unwrap();
    assert_eq!(loaded.len(), 2);
    assert!(loaded
        .iter()
        .any(|e| e.id == bare.id && e.artifacts.is_empty()));
}

// ---- provider overrides & app_state ---------------------------------------

#[test]
fn provider_override_set_get_delete() {
    let db = Db::open_in_memory().unwrap();
    let id = domain::AgentProviderId::from("claude");

    assert_eq!(db.provider_overrides().get(&id).unwrap(), None);

    let path = PathBuf::from("/opt/homebrew/bin/claude");
    db.provider_overrides().set(&id, Some(&path)).unwrap();
    assert_eq!(
        db.provider_overrides().get(&id).unwrap(),
        Some(path.clone())
    );

    // Upsert to a new path.
    let path2 = PathBuf::from("/usr/local/bin/claude");
    db.provider_overrides().set(&id, Some(&path2)).unwrap();
    assert_eq!(db.provider_overrides().get(&id).unwrap(), Some(path2));

    assert_eq!(db.provider_overrides().list().unwrap().len(), 1);

    // None deletes.
    db.provider_overrides().set(&id, None).unwrap();
    assert_eq!(db.provider_overrides().get(&id).unwrap(), None);
    assert!(db.provider_overrides().list().unwrap().is_empty());
}

#[test]
fn app_state_set_get() {
    let db = Db::open_in_memory().unwrap();
    assert_eq!(db.app_state().get("sidebar_width").unwrap(), None);

    db.app_state().set("sidebar_width", "240").unwrap();
    assert_eq!(
        db.app_state().get("sidebar_width").unwrap(),
        Some("240".to_owned())
    );

    // Upsert.
    db.app_state().set("sidebar_width", "300").unwrap();
    assert_eq!(
        db.app_state().get("sidebar_width").unwrap(),
        Some("300".to_owned())
    );

    db.app_state().set("layout", "{\"dock\":true}").unwrap();
    assert_eq!(db.app_state().list().unwrap().len(), 2);

    assert!(db.app_state().delete("layout").unwrap());
    assert_eq!(db.app_state().list().unwrap().len(), 1);
}

// ---- reconciliation --------------------------------------------------------

#[test]
fn reconcile_orphaned_marks_only_live_sessions() {
    let (db, _project, workspace_id) = db_with_workspace();

    let running = mk_session(workspace_id, SessionState::Running);
    let starting = mk_session(workspace_id, SessionState::Starting);
    let exited = mk_session(
        workspace_id,
        SessionState::Exited {
            code: Some(0),
            signal: None,
        },
    );
    db.sessions().upsert(&running).unwrap();
    db.sessions().upsert(&starting).unwrap();
    db.sessions().upsert(&exited).unwrap();

    let updated = db.reconcile_orphaned().unwrap();
    assert_eq!(updated, 2, "Running + Starting are reconciled");

    let load = |id| db.sessions().get(id).unwrap().unwrap();
    assert_eq!(load(running.id).state, SessionState::Orphaned);
    assert_eq!(load(starting.id).state, SessionState::Orphaned);
    assert!(load(running.id).ended_at.is_some());
    assert!(load(starting.id).ended_at.is_some());

    // The already-terminal session is untouched.
    assert_eq!(
        load(exited.id).state,
        SessionState::Exited {
            code: Some(0),
            signal: None
        }
    );

    // A second run is a no-op (nothing left in a live state).
    assert_eq!(db.reconcile_orphaned().unwrap(), 0);
}

#[test]
fn purge_sessions_drops_the_whole_session_history_but_keeps_the_workspace() {
    let (db, project_id, workspace_id) = db_with_workspace();

    let parent = mk_session(workspace_id, SessionState::Running);
    db.sessions().upsert(&parent).unwrap();
    let mut child = mk_session(workspace_id, SessionState::Orphaned);
    child.parent_session_id = Some(parent.id);
    child.root_session_id = parent.id;
    db.sessions().upsert(&child).unwrap();
    db.context()
        .insert(&ContextEnvelope {
            id: ContextId::new(),
            source_session_id: parent.id,
            target_session_id: Some(child.id),
            summary: None,
            instructions: None,
            artifacts: vec![],
            git_context: None,
            created_at: Timestamp::now(),
        })
        .unwrap();

    // A graph edge and an envelope are exactly what a naive DELETE trips over:
    // the parent link is `SET NULL` and the envelope cascades with its source.
    assert_eq!(db.purge_sessions().unwrap(), 2);
    assert!(db.sessions().list().unwrap().is_empty());
    assert!(db.context().list_by_source(parent.id).unwrap().is_empty());

    // Only sessions go. The project and its workspace are what the next daemon
    // reopens the tree from.
    assert_eq!(db.projects().list().unwrap().len(), 1);
    assert_eq!(
        db.workspaces().list_by_project(project_id).unwrap().len(),
        1
    );

    // A second run on an empty table is a no-op.
    assert_eq!(db.purge_sessions().unwrap(), 0);
}

#[test]
fn reset_clears_every_application_table_and_keeps_the_schema() {
    let (mut db, _project_id, workspace_id) = db_with_workspace();
    let session = mk_session(workspace_id, SessionState::Running);
    db.sessions().upsert(&session).unwrap();
    db.context()
        .insert(&ContextEnvelope {
            id: ContextId::new(),
            source_session_id: session.id,
            target_session_id: None,
            summary: None,
            instructions: None,
            artifacts: vec![],
            git_context: None,
            created_at: Timestamp::now(),
        })
        .unwrap();
    db.project_groups()
        .upsert(&ProjectGroup {
            id: ProjectGroupId::new(),
            name: "Reset me".to_owned(),
            created_at: Timestamp::now(),
        })
        .unwrap();
    db.agent_profiles()
        .upsert(&mk_profile("claude", "Reset me"))
        .unwrap();
    let provider = AgentProviderId::new("claude");
    db.provider_overrides()
        .set(&provider, Some(&PathBuf::from("/tmp/claude")))
        .unwrap();
    db.app_state().set("ui.theme_base", "light").unwrap();

    db.reset().unwrap();

    for table in [
        "projects",
        "project_groups",
        "workspaces",
        "sessions",
        "context_envelopes",
        "provider_overrides",
        "app_state",
        "agent_profiles",
        "worktree_shares",
        "worktree_ignores",
    ] {
        let count: i64 = db
            .conn()
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 0, "{table} should be empty after reset");
    }
    let version: i64 = db
        .conn()
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, 11, "reset retains the migrated schema");
}

// ---- on-disk open ----------------------------------------------------------

#[test]
fn open_on_disk_persists_across_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("app.db");

    let project = mk_project("/tmp/persist-me");
    {
        let db = Db::open(&path).unwrap();
        db.projects().upsert(&project).unwrap();
    }
    {
        let db = Db::open(&path).unwrap();
        assert_eq!(
            db.projects().get(project.id).unwrap().as_ref(),
            Some(&project)
        );
    }
}

/// Migration 2 gave every reference an explicit action. `RESTRICT` is what stops
/// a project or workspace from being deleted out from under live sessions, and
/// `SET NULL` is what keeps a child session alive when its parent is deleted —
/// the daemon reparents the subtree itself and must not find a dangling id.
#[test]
fn referential_actions_protect_the_tree_from_the_leaves_up() {
    let (db, project_id, workspace_id) = db_with_workspace();

    let parent = mk_session(workspace_id, SessionState::Running);
    db.sessions().upsert(&parent).unwrap();
    let mut child = mk_session(workspace_id, SessionState::Running);
    child.parent_session_id = Some(parent.id);
    child.root_session_id = parent.id;
    db.sessions().upsert(&child).unwrap();

    assert!(
        db.workspaces().delete(workspace_id).is_err(),
        "a workspace with sessions must not be deletable"
    );
    assert!(
        db.projects().delete(project_id).is_err(),
        "a project with workspaces must not be deletable"
    );

    // The parent goes; the child stays, with its pointer cleared rather than
    // dangling at a row that no longer exists.
    assert!(db.sessions().delete(parent.id).unwrap());
    let orphan = db.sessions().get(child.id).unwrap().unwrap();
    assert_eq!(orphan.parent_session_id, None);
    assert_eq!(
        orphan.root_session_id, parent.id,
        "root_session_id carries no foreign key, so it is left to the daemon to fix"
    );

    // Once the sessions are gone the tree unwinds from the leaves up.
    assert!(db.sessions().delete(child.id).unwrap());
    assert!(db.workspaces().delete(workspace_id).unwrap());
    assert!(db.projects().delete(project_id).unwrap());
    assert!(db.projects().get(project_id).unwrap().is_none());
}

// ---- shared files (§14.2) --------------------------------------------------

fn mk_rule(project: ProjectId, path: &str, strategy: ShareStrategy) -> ShareRule {
    ShareRule {
        id: ShareRuleId::new(),
        project_id: project,
        path: path.to_owned(),
        strategy,
        enabled: true,
        position: 0,
        created_at: Timestamp::now(),
    }
}

#[test]
fn a_projects_share_rules_round_trip_in_the_order_they_were_given() {
    let db = Db::open_in_memory().unwrap();
    let project = mk_project("/tmp/p-shares");
    db.projects().upsert(&project).unwrap();

    let rules = vec![
        mk_rule(project.id, ".env", ShareStrategy::Link),
        mk_rule(
            project.id,
            "node_modules",
            ShareStrategy::Run {
                command: "pnpm install".to_owned(),
                timeout_secs: 600,
            },
        ),
    ];
    db.shares().replace_for_project(project.id, &rules).unwrap();

    let back = db.shares().list_for_project(project.id).unwrap();
    assert_eq!(back.len(), 2);
    assert_eq!(back[0].path, ".env");
    assert_eq!(back[0].strategy, ShareStrategy::Link);
    assert_eq!(back[1].path, "node_modules");
    // `position` is rewritten from the slice order, not taken from the client.
    assert_eq!(back[0].position, 0);
    assert_eq!(back[1].position, 1);
}

#[test]
fn replacing_a_set_drops_what_is_no_longer_in_it() {
    let db = Db::open_in_memory().unwrap();
    let project = mk_project("/tmp/p-shares-2");
    db.projects().upsert(&project).unwrap();

    let first = vec![
        mk_rule(project.id, ".env", ShareStrategy::Copy),
        mk_rule(project.id, "target", ShareStrategy::Clone),
    ];
    db.shares().replace_for_project(project.id, &first).unwrap();
    db.shares()
        .replace_for_project(project.id, &first[..1])
        .unwrap();

    let back = db.shares().list_for_project(project.id).unwrap();
    assert_eq!(back.len(), 1);
    assert_eq!(back[0].path, ".env");
}

#[test]
fn two_rules_cannot_claim_the_same_path_and_a_failed_set_keeps_the_old_one() {
    let db = Db::open_in_memory().unwrap();
    let project = mk_project("/tmp/p-shares-3");
    db.projects().upsert(&project).unwrap();

    let good = vec![mk_rule(project.id, ".env", ShareStrategy::Copy)];
    db.shares().replace_for_project(project.id, &good).unwrap();

    let duplicated = vec![
        mk_rule(project.id, ".env", ShareStrategy::Copy),
        mk_rule(project.id, ".env", ShareStrategy::Link),
    ];
    assert!(db
        .shares()
        .replace_for_project(project.id, &duplicated)
        .is_err());

    // Half a list is not a list: the previous set is still the one on disk.
    let back = db.shares().list_for_project(project.id).unwrap();
    assert_eq!(back.len(), 1);
    assert_eq!(back[0].strategy, ShareStrategy::Copy);
}

#[test]
fn removing_a_project_takes_its_rules_with_it() {
    let db = Db::open_in_memory().unwrap();
    let project = mk_project("/tmp/p-shares-4");
    db.projects().upsert(&project).unwrap();
    db.shares()
        .replace_for_project(
            project.id,
            &[mk_rule(project.id, ".env", ShareStrategy::Copy)],
        )
        .unwrap();

    db.projects().delete(project.id).unwrap();
    assert!(db.shares().list_all().unwrap().is_empty());
}

// ---- worktree ignores (§14.4) ----------------------------------------------

fn mk_ignore(project: ProjectId, path: &str, scope: IgnoreScope) -> WorktreeIgnore {
    WorktreeIgnore {
        project_id: project,
        path: PathBuf::from(path),
        scope,
        created_at: Timestamp::now(),
    }
}

#[test]
fn a_projects_ignore_rules_round_trip() {
    let db = Db::open_in_memory().unwrap();
    let project = mk_project("/tmp/p-ignores");
    db.projects().upsert(&project).unwrap();

    let rules = vec![
        mk_ignore(project.id, "/tmp/one", IgnoreScope::Exact),
        mk_ignore(project.id, "/tmp/tree", IgnoreScope::Subtree),
    ];
    db.ignores()
        .replace_for_project(project.id, &rules)
        .unwrap();

    let back = db.ignores().list_all().unwrap();
    assert_eq!(back.len(), 2);
    assert_eq!(back[0].path, PathBuf::from("/tmp/one"));
    assert_eq!(back[0].scope, IgnoreScope::Exact);
    assert_eq!(back[1].scope, IgnoreScope::Subtree);
}

#[test]
fn replacing_an_ignore_set_drops_what_is_no_longer_in_it() {
    let db = Db::open_in_memory().unwrap();
    let project = mk_project("/tmp/p-ignores-2");
    db.projects().upsert(&project).unwrap();

    let first = vec![
        mk_ignore(project.id, "/tmp/one", IgnoreScope::Exact),
        mk_ignore(project.id, "/tmp/tree", IgnoreScope::Subtree),
    ];
    db.ignores()
        .replace_for_project(project.id, &first)
        .unwrap();
    db.ignores()
        .replace_for_project(project.id, &first[..1])
        .unwrap();

    let back = db.ignores().list_all().unwrap();
    assert_eq!(back.len(), 1);
    assert_eq!(back[0].path, PathBuf::from("/tmp/one"));
}

#[test]
fn upserting_a_rule_keeps_the_created_at_of_the_row_it_replaces() {
    let db = Db::open_in_memory().unwrap();
    let project = mk_project("/tmp/p-ignores-3");
    db.projects().upsert(&project).unwrap();

    let first = mk_ignore(project.id, "/tmp/one", IgnoreScope::Exact);
    db.ignores().upsert(&first).unwrap();
    let mut second = mk_ignore(project.id, "/tmp/one", IgnoreScope::Subtree);
    second.created_at =
        Timestamp::from_offset(first.created_at.as_offset() + time::Duration::seconds(60));
    db.ignores().upsert(&second).unwrap();

    let back = db.ignores().list_all().unwrap();
    assert_eq!(back.len(), 1, "the path is the identity");
    assert_eq!(back[0].scope, IgnoreScope::Subtree);
    assert_eq!(back[0].created_at, first.created_at);
}

#[test]
fn deleting_a_rule_reports_whether_a_row_did_not_exist() {
    let db = Db::open_in_memory().unwrap();
    let project = mk_project("/tmp/p-ignores-4");
    db.projects().upsert(&project).unwrap();
    let rule = mk_ignore(project.id, "/tmp/one", IgnoreScope::Exact);
    db.ignores().upsert(&rule).unwrap();

    assert!(db.ignores().delete(project.id, &rule.path).unwrap());
    assert!(!db.ignores().delete(project.id, &rule.path).unwrap());
}

#[test]
fn removing_a_project_takes_its_ignore_rules_with_it() {
    let db = Db::open_in_memory().unwrap();
    let project = mk_project("/tmp/p-ignores-5");
    db.projects().upsert(&project).unwrap();
    db.ignores()
        .replace_for_project(
            project.id,
            &[mk_ignore(project.id, "/tmp/one", IgnoreScope::Exact)],
        )
        .unwrap();

    db.projects().delete(project.id).unwrap();
    assert!(db.ignores().list_all().unwrap().is_empty());
}

#[test]
fn an_unknown_ignore_scope_is_a_decode_error_not_a_panic() {
    let db = Db::open_in_memory().unwrap();
    let project = mk_project("/tmp/p-ignores-6");
    db.projects().upsert(&project).unwrap();
    db.conn()
        .execute(
            "INSERT INTO worktree_ignores (project_id, path, scope, created_at) \
             VALUES (?1, '/tmp/one', 'sideways', ?2)",
            rusqlite::params![project.id.to_string(), Timestamp::now().to_rfc3339()],
        )
        .unwrap();

    let error = db.ignores().list_all().unwrap_err();
    assert!(
        matches!(error, crate::DbError::Decode(_)),
        "expected a decode error, got {error:?}"
    );
}
