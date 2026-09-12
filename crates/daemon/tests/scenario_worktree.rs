//! §19 scenario C: create a worktree, launch an agent and a shell in it, and
//! confirm both really run there while the main checkout keeps its branch.
//!
//! `tests/integration.rs::worktree_lifecycle` already covers create/remove on
//! disk. What it does not cover — and what the scenario is actually about — is
//! that a session launched in the worktree has the worktree as its working
//! directory (§13.3 `cwd = workspace.path`), and that none of it moves the main
//! checkout off its branch (§14.3 uses `git worktree add`, never `checkout`).
//!
//! The agent half runs a generated fake CLI rather than a real Codex, so the
//! test is deterministic and needs nothing installed (§21 "Determinismo"); it
//! is still the complete daemon launch path, PTY included.

mod common;

use domain::WorkspaceKind;
use protocol::{DaemonEvent, Request, Response};

#[test]
fn sessions_launched_in_a_worktree_run_there_and_main_keeps_its_branch() {
    const BRANCH: &str = "feature/auth";

    let harness = common::Harness::new();
    let repo = test_support::init_repo().expect("git repo (git must be installed)");
    // A stand-in for Codex on the daemon's (hermetic) PATH.
    common::write_fake_agent_cli(harness.bin(), "codex", "forge_codex", "codex-cli 1.0.0");

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

    // --- create the worktree (§14.3) ---
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
            .any(|p| p.descriptor.id.as_str() == "codex" && p.detection.status.is_installed())
    });
    let (agent_session, agent_terminal) =
        common::create_agent_session(&client, &events, worktree.id, "codex");
    let agent_verdict =
        common::attach_and_read_line(&client, &events, agent_terminal, "forge_codex_cwd_");
    assert_eq!(
        agent_verdict.as_deref(),
        Some("forge_codex_cwd_ok"),
        "the agent must be launched in the worktree the daemon advertises \
         through FORGE_WORKSPACE (§13.3)"
    );

    // --- §14.4: a worktree with live sessions is not removed without force ---
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

/// §14.4 lists exactly three reasons to refuse a `force: false` removal:
/// running sessions in that workspace, a dirty working tree, and a merge or
/// rebase in progress. A worktree whose sessions have *ended* passes all three,
/// so removing it must work.
///
/// Regression test. `RemoveWorktree` used to remove the directory from disk
/// first and only then delete the workspace row, which SQLite refuses while the
/// terminated sessions still reference it (`sessions.workspace_id ON DELETE
/// RESTRICT`, migration 2). The request failed with `ErrorCode::Internal`
/// *after* the worktree was already gone, the workspace stayed in the model and
/// in SQLite pointing at a directory that no longer existed, and no
/// `WorkspaceRemoved` was broadcast — so the sidebar kept offering a workspace
/// whose every future session would fail to spawn.
///
/// It now deletes sessions before the workspace (the foreign-key order
/// `remove_project` documents) and touches the disk last, so a failure can no
/// longer leave SQLite and the filesystem disagreeing.
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
fn factory_reset_removes_managed_state_but_keeps_repository_harness_files() {
    const BRANCH: &str = "feature/factory-reset";

    let harness = common::Harness::new();
    let repo = test_support::init_repo().expect("git repo (git must be installed)");
    let harness_dir = repo.path().join("harness");
    std::fs::create_dir(&harness_dir).expect("create repository harness directory");
    let marker = harness_dir.join("keep-me.txt");
    std::fs::write(&marker, "repository-owned\n").expect("write harness marker");

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
        jobs,
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
    assert!(jobs.is_empty());
    assert!(
        !worktree.path.exists(),
        "a dirty worktree created by Forge is force-removed"
    );
    assert!(marker.is_file(), "repository harness state is retained");
    assert_eq!(
        std::fs::read_to_string(marker).expect("read harness marker"),
        "repository-owned\n"
    );

    common::stop_daemon(&client);
}
