//! Create a worktree, launch an agent and a shell in it, and
//! confirm both really run there while the main checkout keeps its branch.
//!
//! `tests/integration.rs::worktree_lifecycle` already covers create/remove on
//! disk. What it does not cover — and what the scenario is actually about — is
//! that a session launched in the worktree has the worktree as its working
//! directory (`cwd = workspace.path`), and that none of it moves the main
//! checkout off its branch (`git worktree add`, never `checkout`).
//!
//! The agent half runs a generated fake CLI rather than a real OpenCode, so the
//! test needs no installed agent CLI; it
//! is still the complete daemon launch path, PTY included.

mod common;

use domain::WorkspaceKind;
use protocol::{DaemonEvent, Request, Response};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

/// A real git repository at `path`, so the basename the project-dir rule reads
/// can contain a space or a case variant.
fn init_repo_at(path: &Path) {
    std::fs::create_dir_all(path).expect("create repo directory");
    for args in [
        &["-c", "init.defaultBranch=main", "init"][..],
        &["config", "user.email", "forge-test@example.com"][..],
        &["config", "user.name", "Forge Test"][..],
        &["config", "commit.gpgsign", "false"][..],
    ] {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(path)
            .args(args)
            .output()
            .expect("run git");
        assert!(out.status.success(), "git {args:?} failed");
    }
    std::fs::write(path.join("README.md"), "# forge test repo\n").expect("write README");
    for args in [
        &["add", "README.md"][..],
        &["commit", "-m", "initial commit"][..],
    ] {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(path)
            .args(args)
            .output()
            .expect("run git");
        assert!(out.status.success(), "git {args:?} failed");
    }
}

fn mode_of(path: &Path) -> u32 {
    std::fs::metadata(path).expect("stat").permissions().mode() & 0o777
}

fn project_snapshot(client: &client::Client) -> (domain::ProjectId, std::path::PathBuf) {
    let Response::Snapshot { projects, .. } =
        client.request(Request::GetSnapshot).expect("GetSnapshot")
    else {
        panic!("expected Snapshot");
    };
    (projects[0].id, projects[0].root_path.clone())
}

#[test]
fn get_paths_returns_the_resolved_harness_paths() {
    let harness = common::Harness::new();
    let daemon = harness.boot();
    let client = daemon.connect("paths");

    let (paths, worktrees_root) = client.paths().expect("GetPaths");
    assert_eq!(paths.layout, client::Layout::Home);
    assert_eq!(paths.home, harness.forge_home());
    assert_eq!(paths.db, harness.forge_home().join("app.db"));
    assert_eq!(
        paths.default_worktrees_root,
        harness.forge_home().join("worktrees")
    );
    assert_eq!(worktrees_root, harness.worktrees_root());

    common::stop_daemon(&client);
}

#[test]
fn managed_worktrees_live_under_a_readable_project_directory() {
    const BRANCH: &str = "feature/auth";

    let harness = common::Harness::new();
    let repo_root = harness.root().join("my repo");
    init_repo_at(&repo_root);
    let daemon = harness.boot();
    let client = daemon.connect("readable-worktree");
    let events = client.events();
    common::add_main_workspace(&client, &repo_root);
    let (project_id, _) = project_snapshot(&client);

    client
        .request(Request::CreateWorktree {
            project_id,
            branch: BRANCH.to_string(),
            base: None,
            name: None,
        })
        .expect("CreateWorktree");
    let created = common::wait_for(
        &events,
        common::DEADLINE,
        |event| matches!(event, DaemonEvent::WorkspaceCreated(w) if w.managed_by_app),
    )
    .expect("WorkspaceCreated");
    let DaemonEvent::WorkspaceCreated(worktree) = created else {
        unreachable!()
    };

    let project_dir = worktree.path.parent().expect("project directory");
    let name = project_dir
        .file_name()
        .expect("project dir name")
        .to_string_lossy();
    let id = project_id.to_string();
    assert!(name.starts_with("my-repo-"), "{name}");
    assert!(name.ends_with(&id[id.len() - 6..]), "{name}");
    assert!(!name.contains(' '), "{name}");
    assert_eq!(
        worktree.path.file_name().expect("worktree name"),
        "feature-auth"
    );
    assert_eq!(project_dir.parent(), Some(harness.worktrees_root()));

    common::stop_daemon(&client);
}

#[test]
fn attention_plugins_stay_in_the_forge_home_with_a_custom_worktrees_root() {
    let harness = common::Harness::new();
    let daemon = harness.boot();
    let client = daemon.connect("plugins-location");

    assert!(
        harness.forge_home().join("agent-plugins").is_dir(),
        "agent-plugins belongs under the forge home"
    );
    assert!(
        !harness.root().join("agent-plugins").exists(),
        "and never beside a custom worktrees root"
    );

    common::stop_daemon(&client);
}

#[test]
fn a_user_created_visible_folder_keeps_its_mode() {
    let harness = common::Harness::new();
    let visible = harness.home().join("forge-node");
    std::fs::create_dir_all(&visible).expect("create visible folder");
    std::fs::set_permissions(&visible, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    let repo = test_support::init_repo().expect("git repo (git must be installed)");

    let daemon = harness.boot_with(|cfg| {
        cfg.worktrees.root = "~/forge-node/worktrees".to_string();
    });
    let client = daemon.connect("visible-mode");
    let events = client.events();
    common::add_main_workspace(&client, repo.path());
    let (project_id, _) = project_snapshot(&client);
    client
        .request(Request::CreateWorktree {
            project_id,
            branch: "feature/mode".to_string(),
            base: None,
            name: None,
        })
        .expect("CreateWorktree");
    let created = common::wait_for(
        &events,
        common::DEADLINE,
        |event| matches!(event, DaemonEvent::WorkspaceCreated(w) if w.managed_by_app),
    )
    .expect("WorkspaceCreated");
    let DaemonEvent::WorkspaceCreated(worktree) = created else {
        unreachable!()
    };

    assert_eq!(mode_of(&visible), 0o755, "the user's folder keeps its mode");
    assert_eq!(
        mode_of(worktree.path.parent().expect("project directory")),
        0o700
    );

    common::stop_daemon(&client);
}

#[test]
fn an_existing_directory_on_disk_wins_the_slug_race() {
    let harness = common::Harness::new();
    let repo = test_support::init_repo().expect("git repo (git must be installed)");
    let daemon = harness.boot();
    let client = daemon.connect("slug-race");
    let events = client.events();
    common::add_main_workspace(&client, repo.path());
    let (project_id, root_path) = project_snapshot(&client);
    let project_dir = harness
        .worktrees_root()
        .join(daemon::worktrees::project_dir_name(&root_path, project_id));
    let stale = project_dir.join("feature-x");
    std::fs::create_dir_all(&stale).expect("pre-create the stale worktree directory");
    std::fs::write(stale.join("left-behind.txt"), "stale\n").expect("write a file");

    client
        .request(Request::CreateWorktree {
            project_id,
            branch: "feature/x".to_string(),
            base: None,
            name: None,
        })
        .expect("CreateWorktree must not fail on a directory it does not own");
    let created = common::wait_for(
        &events,
        common::DEADLINE,
        |event| matches!(event, DaemonEvent::WorkspaceCreated(w) if w.managed_by_app),
    )
    .expect("WorkspaceCreated");
    let DaemonEvent::WorkspaceCreated(worktree) = created else {
        unreachable!()
    };
    assert_eq!(
        worktree
            .path
            .file_name()
            .expect("worktree name")
            .to_string_lossy(),
        "feature-x-2"
    );
    assert!(
        stale.join("left-behind.txt").is_file(),
        "the stale dir is untouched"
    );

    common::stop_daemon(&client);
}

#[test]
fn removing_the_last_worktree_removes_the_emptied_project_directory() {
    let harness = common::Harness::new();
    let repo = test_support::init_repo().expect("git repo (git must be installed)");
    let daemon = harness.boot();
    let client = daemon.connect("rmdir-project");
    let events = client.events();
    common::add_main_workspace(&client, repo.path());
    let (project_id, _) = project_snapshot(&client);

    let mut created = Vec::new();
    for branch in ["feature/one", "feature/two"] {
        client
            .request(Request::CreateWorktree {
                project_id,
                branch: branch.to_string(),
                base: None,
                name: None,
            })
            .expect("CreateWorktree");
        let event = common::wait_for(
            &events,
            common::DEADLINE,
            |event| matches!(event, DaemonEvent::WorkspaceCreated(w) if w.managed_by_app),
        )
        .expect("WorkspaceCreated");
        let DaemonEvent::WorkspaceCreated(worktree) = event else {
            unreachable!()
        };
        created.push(worktree);
    }
    let project_dir = created[0]
        .path
        .parent()
        .expect("project directory")
        .to_path_buf();

    client
        .request(Request::RemoveWorktree {
            workspace_id: created[0].id,
            force: false,
        })
        .expect("remove the first worktree");
    assert!(
        project_dir.is_dir(),
        "the project dir still holds one worktree"
    );

    client
        .request(Request::RemoveWorktree {
            workspace_id: created[1].id,
            force: false,
        })
        .expect("remove the last worktree");
    assert!(!project_dir.exists(), "the emptied project dir is gone");
    assert!(
        harness.worktrees_root().is_dir(),
        "but the worktrees root stays"
    );

    common::stop_daemon(&client);
}

#[test]
fn sessions_launched_in_a_worktree_run_there_and_main_keeps_its_branch() {
    const BRANCH: &str = "feature/auth";

    let harness = common::Harness::new();
    let repo = test_support::init_repo().expect("git repo (git must be installed)");
    // A stand-in for OpenCode on the daemon's (hermetic) PATH.
    common::write_fake_agent_cli(
        harness.bin(),
        "opencode",
        "forge_opencode",
        "opencode 1.0.0",
    );

    let daemon = harness.boot();
    let client = daemon.connect("scenario-c");
    let events = client.events();

    let main_workspace = common::add_main_workspace(&client, repo.path());
    assert_eq!(
        common::checked_out_branch(repo.path()),
        "main",
        "the fixture repo starts on main"
    );

    let Response::Snapshot { projects, .. } =
        client.request(Request::GetSnapshot).expect("GetSnapshot")
    else {
        panic!("expected Snapshot");
    };
    let project_id = projects[0].id;

    client
        .request(Request::CreateWorktree {
            project_id,
            branch: BRANCH.to_string(),
            base: None,
            name: None,
        })
        .expect("CreateWorktree");
    let created = common::wait_for(
        &events,
        common::DEADLINE,
        |event| matches!(event, DaemonEvent::WorkspaceCreated(w) if w.managed_by_app),
    )
    .expect("WorkspaceCreated for the managed worktree");
    let DaemonEvent::WorkspaceCreated(worktree) = created else {
        unreachable!()
    };
    assert_eq!(worktree.kind, WorkspaceKind::GitWorktree);
    assert_eq!(worktree.branch.as_deref(), Some(BRANCH));
    assert!(worktree.path.is_dir(), "the worktree exists on disk");
    assert_ne!(worktree.id, main_workspace);
    assert_eq!(common::checked_out_branch(&worktree.path), BRANCH);
    assert_eq!(
        common::checked_out_branch(repo.path()),
        "main",
        "creating a worktree must not move the main checkout (§19 C)"
    );

    // The shell compares the two paths itself and prints a short verdict: a
    // `pwd` long enough to wrap would be split across grid rows, and the marker
    // has to stay on one line. Both sides are resolved with `pwd -P`, so the
    // macOS `/tmp` → `/private/tmp` symlink cannot fake a mismatch.
    let want = std::fs::canonicalize(&worktree.path).expect("canonicalize the worktree path");
    let check = format!(
        "[ \"$(pwd -P)\" = '{}' ] && echo forge_cwd\"\"_ok || echo forge_cwd\"\"_bad\n",
        want.display()
    );

    // --- a shell in the worktree ---
    let (shell_session, shell_terminal) =
        common::create_shell_session(&client, &events, worktree.id);
    common::attach(&client, shell_terminal, 80, 24);
    common::wait_for_shell_prompt(&client, &events, shell_terminal);
    common::write_input(&client, shell_terminal, &check);
    let verdict = common::wait_for_row(&events, common::DEADLINE, |text| {
        matches!(text.trim(), "forge_cwd_ok" | "forge_cwd_bad")
    })
    .expect("the shell should report where it is running");
    assert_eq!(
        verdict.trim(),
        "forge_cwd_ok",
        "`pwd` in the shell must be the worktree path (§19 C)"
    );

    // --- an agent in the same worktree ---
    common::wait_for_detection(&client, |providers| {
        providers
            .iter()
            .any(|p| p.descriptor.id.as_str() == "opencode" && p.detection.status.is_installed())
    });
    let (agent_session, agent_terminal) =
        common::create_agent_session(&client, &events, worktree.id, "opencode");
    let agent_verdict =
        common::attach_and_read_line(&client, &events, agent_terminal, "forge_opencode_cwd_");
    assert_eq!(
        agent_verdict.as_deref(),
        Some("forge_opencode_cwd_ok"),
        "the agent must be launched in the worktree the daemon advertises \
         through FORGE_WORKSPACE (§13.3)"
    );

    let refused = client.request(Request::RemoveWorktree {
        workspace_id: worktree.id,
        force: false,
    });
    assert!(
        refused.is_err(),
        "RemoveWorktree must refuse while sessions are running (§14.4)"
    );
    assert!(worktree.path.is_dir(), "and must not touch the disk");

    // Kill both sessions, then close them: this is the ordinary path a user
    // takes, and it must leave nothing behind. (Closing is not *required* — the
    // test below covers removing a worktree whose sessions were only killed.)
    for session_id in [shell_session, agent_session] {
        assert!(common::kill_and_wait(&client, &events, session_id));
        client
            .request(Request::CloseSession { session_id })
            .expect("CloseSession");
    }

    let removed = client
        .request(Request::RemoveWorktree {
            workspace_id: worktree.id,
            force: false,
        })
        .expect("RemoveWorktree once nothing runs there");
    assert_eq!(removed, Response::Ack);
    assert!(!worktree.path.exists(), "the worktree is gone from disk");
    assert!(
        !common::workspaces(&client)
            .iter()
            .any(|w| w.id == worktree.id),
        "and out of the model"
    );
    assert_eq!(
        common::checked_out_branch(repo.path()),
        "main",
        "and the main checkout is still on its own branch (§19 C)"
    );

    common::stop_daemon(&client);
}

// Ended sessions still reference the workspace; removal must respect foreign-key order.
#[test]
fn a_worktree_whose_sessions_have_ended_can_be_removed() {
    const BRANCH: &str = "feature/ended-sessions";

    let harness = common::Harness::new();
    let repo = test_support::init_repo().expect("git repo (git must be installed)");
    let daemon = harness.boot();
    let client = daemon.connect("worktree-remove");
    let events = client.events();

    common::add_main_workspace(&client, repo.path());
    let Response::Snapshot { projects, .. } =
        client.request(Request::GetSnapshot).expect("GetSnapshot")
    else {
        panic!("expected Snapshot");
    };
    client
        .request(Request::CreateWorktree {
            project_id: projects[0].id,
            branch: BRANCH.to_string(),
            base: None,
            name: None,
        })
        .expect("CreateWorktree");
    let created = common::wait_for(
        &events,
        common::DEADLINE,
        |event| matches!(event, DaemonEvent::WorkspaceCreated(w) if w.managed_by_app),
    )
    .expect("WorkspaceCreated");
    let DaemonEvent::WorkspaceCreated(worktree) = created else {
        unreachable!()
    };

    let (session_id, _terminal_id) = common::create_shell_session(&client, &events, worktree.id);
    assert!(common::kill_and_wait(&client, &events, session_id));

    let outcome = client.request(Request::RemoveWorktree {
        workspace_id: worktree.id,
        force: false,
    });
    let still_listed = common::workspaces(&client)
        .iter()
        .any(|w| w.id == worktree.id);
    let still_on_disk = worktree.path.exists();
    common::stop_daemon(&client);

    assert!(
        outcome.is_ok(),
        "a worktree whose sessions have ended passes every §14.4 pre-check, so \
         removing it must succeed; got {outcome:?} \
         (workspace still in the model: {still_listed}, directory still on disk: {still_on_disk})"
    );
    assert!(!still_listed, "the workspace leaves the model");
    assert!(!still_on_disk, "and the disk");
}

#[test]
fn a_forgotten_worktree_stays_gone_across_a_daemon_restart() {
    let harness = common::Harness::new();
    let repo = test_support::init_repo().expect("git repo (git must be installed)");
    let external = harness.root().join("agent-wts/arch-review");
    std::fs::create_dir_all(external.parent().expect("parent")).expect("mkdir");
    // Created the way a parallel agent would: plain git, no Forge.
    let status = std::process::Command::new("git")
        .arg("-C")
        .arg(repo.path())
        .args(["worktree", "add"])
        .arg(&external)
        .args(["-b", "agent/arch-review"])
        .status()
        .expect("run git worktree add");
    assert!(status.success(), "git worktree add failed");

    let first = harness.boot();
    let client = first.connect("forget-restart");
    common::add_main_workspace(&client, repo.path());

    let adopted = common::workspaces(&client)
        .into_iter()
        .find(|w| w.kind == WorkspaceKind::GitWorktree)
        .expect("the agent's worktree is adopted when the project is added");
    assert!(!adopted.managed_by_app);

    client
        .request(Request::RemoveWorktree {
            workspace_id: adopted.id,
            force: false,
        })
        .expect("forget the worktree");
    assert!(
        !common::workspaces(&client)
            .iter()
            .any(|w| w.id == adopted.id),
        "forgetting drops the row"
    );
    assert!(
        external.is_dir(),
        "and never touches an unmanaged directory"
    );

    // Kill the daemon and bring it back over the same database: the literal
    // reproduction the tombstone exists for. Before it, this restart's rescan
    // adopted the worktree again with a fresh id.
    first.crash();
    drop(client);
    let second = harness.boot();
    let client = second.connect("forget-restart-2");

    let workspaces = common::workspaces(&client);
    assert_eq!(
        workspaces.len(),
        1,
        "only Main after the restart: {workspaces:?}"
    );
    assert_eq!(workspaces[0].kind, WorkspaceKind::Main);

    // The rule rode through the restart with it, canonical path and all.
    match client
        .request(Request::ListWorktreeIgnores {
            project_id: workspaces[0].project_id,
        })
        .expect("ListWorktreeIgnores")
    {
        Response::WorktreeIgnores(rules) => {
            assert_eq!(rules.len(), 1, "the tombstone survived: {rules:?}");
            assert_eq!(rules[0].scope, domain::IgnoreScope::Exact);
            assert_eq!(
                rules[0].path,
                std::fs::canonicalize(&external).expect("canonicalize the worktree")
            );
        }
        other => panic!("expected WorktreeIgnores, got {other:?}"),
    }

    common::stop_daemon(&client);
}

#[test]
fn factory_reset_removes_managed_state_but_keeps_repository_files() {
    const BRANCH: &str = "feature/factory-reset";

    let harness = common::Harness::new();
    let repo = test_support::init_repo().expect("git repo (git must be installed)");
    let notes = repo.path().join("notes");
    std::fs::create_dir(&notes).expect("create repository notes directory");
    let marker = notes.join("keep-me.txt");
    std::fs::write(&marker, "repository-owned\n").expect("write repository marker");

    let daemon = harness.boot();
    let client = daemon.connect("factory-reset");
    let events = client.events();
    common::add_main_workspace(&client, repo.path());
    let Response::Snapshot { projects, .. } =
        client.request(Request::GetSnapshot).expect("GetSnapshot")
    else {
        panic!("expected Snapshot");
    };
    client
        .request(Request::CreateWorktree {
            project_id: projects[0].id,
            branch: BRANCH.to_owned(),
            base: None,
            name: None,
        })
        .expect("CreateWorktree");
    let created = common::wait_for(
        &events,
        common::DEADLINE,
        |event| matches!(event, DaemonEvent::WorkspaceCreated(w) if w.managed_by_app),
    )
    .expect("WorkspaceCreated");
    let DaemonEvent::WorkspaceCreated(worktree) = created else {
        unreachable!()
    };
    std::fs::write(worktree.path.join("dirty.txt"), "uncommitted\n")
        .expect("dirty the managed worktree");
    client
        .request(Request::SetAppState {
            key: "ui.theme_base".to_owned(),
            value: "light".to_owned(),
        })
        .expect("SetAppState");

    assert_eq!(
        client.request(Request::FactoryReset).expect("FactoryReset"),
        Response::Ack
    );

    let Response::Snapshot {
        project_groups,
        projects,
        workspaces,
        sessions,
        agent_profiles,
        app_state,
        ..
    } = client.request(Request::GetSnapshot).expect("GetSnapshot")
    else {
        panic!("expected Snapshot");
    };
    assert!(project_groups.is_empty());
    assert!(projects.is_empty());
    assert!(workspaces.is_empty());
    assert!(sessions.is_empty());
    assert!(agent_profiles.is_empty());
    assert!(app_state.is_empty());
    assert!(
        !worktree.path.exists(),
        "a dirty worktree created by Forge is force-removed"
    );
    assert!(marker.is_file(), "repository files are retained");
    assert_eq!(
        std::fs::read_to_string(marker).expect("read repository marker"),
        "repository-owned\n"
    );

    common::stop_daemon(&client);
}
