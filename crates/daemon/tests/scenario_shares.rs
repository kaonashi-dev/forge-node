//! Files shared between the workspaces of one project, end to end (§14.2).
//!
//! What these cover that `shares::plan`'s unit tests cannot: the *daemon's*
//! behaviour around the rules — that a rule set survives the protocol, that a
//! worktree is provisioned after it is handed over rather than before, that a
//! worktree created in a terminal is provisioned when it is adopted, that a
//! second apply changes nothing, and that removing a rule does exactly what
//! the caller asked with the files it already wrote.
//!
//! Real repositories throughout, and they fail rather than skip without `git`.

mod common;

use std::path::Path;

use client::Client;
use domain::{ShareCleanup, ShareRule, ShareRuleId, ShareState, ShareStrategy, Timestamp};
use protocol::{DaemonEvent, Request, Response};

fn git(repo: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .expect("run git");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn project_id(client: &Client) -> domain::ProjectId {
    let Response::Snapshot { projects, .. } =
        client.request(Request::GetSnapshot).expect("GetSnapshot")
    else {
        panic!("expected Snapshot");
    };
    projects[0].id
}

fn rule(project_id: domain::ProjectId, path: &str, strategy: ShareStrategy) -> ShareRule {
    ShareRule {
        id: ShareRuleId::new(),
        project_id,
        path: path.to_owned(),
        strategy,
        enabled: true,
        position: 0,
        created_at: Timestamp::now(),
    }
}

/// Create a worktree and wait until its provisioning run has landed.
fn worktree_with_shares(
    client: &Client,
    events: &flume::Receiver<DaemonEvent>,
    project_id: domain::ProjectId,
    branch: &str,
) -> domain::Workspace {
    client
        .request(Request::CreateWorktree {
            project_id,
            branch: branch.to_owned(),
            base: None,
            name: None,
        })
        .expect("CreateWorktree");
    let created = common::wait_for(
        events,
        common::DEADLINE,
        |event| matches!(event, DaemonEvent::WorkspaceCreated(w) if w.managed_by_app),
    )
    .expect("WorkspaceCreated");
    let DaemonEvent::WorkspaceCreated(workspace) = created else {
        panic!("expected WorkspaceCreated");
    };
    common::wait_for(events, common::DEADLINE, |event| {
        matches!(event, DaemonEvent::SharesApplied { workspace_id, .. } if *workspace_id == workspace.id)
    })
    .expect("SharesApplied");
    workspace
}

#[test]
fn a_projects_rules_survive_the_protocol_and_reach_a_new_worktree() {
    let harness = common::Harness::new();
    let repo = test_support::init_repo().expect("git repo (git must be installed)");
    std::fs::write(repo.path().join(".env"), "SECRET=1\n").unwrap();
    std::fs::create_dir_all(repo.path().join("deps")).unwrap();
    std::fs::write(repo.path().join("deps/installed.txt"), "yes\n").unwrap();

    let daemon = harness.boot();
    let client = daemon.connect("shares");
    let events = client.events();
    common::add_main_workspace(&client, repo.path());
    let project_id = project_id(&client);

    let rules = vec![
        rule(project_id, ".env", ShareStrategy::Copy),
        rule(project_id, "deps", ShareStrategy::Clone),
    ];
    assert_eq!(
        client
            .request(Request::SetProjectShares {
                project_id,
                rules: rules.clone(),
            })
            .expect("SetProjectShares"),
        Response::Ack
    );

    // The set comes back through the snapshot, in application order.
    let Response::Snapshot {
        worktree_shares, ..
    } = client.request(Request::GetSnapshot).expect("GetSnapshot")
    else {
        panic!("expected Snapshot");
    };
    assert_eq!(worktree_shares.len(), 2);
    assert_eq!(worktree_shares[0].path, ".env");
    assert_eq!(worktree_shares[1].path, "deps");

    let workspace = worktree_with_shares(&client, &events, project_id, "feature/shared");
    assert_eq!(
        std::fs::read_to_string(workspace.path.join(".env")).ok(),
        Some("SECRET=1\n".to_string()),
        "a copy rule brings the untracked file git could not"
    );
    assert!(
        workspace.path.join("deps/installed.txt").exists(),
        "a clone rule brings the whole directory, cloned or copied"
    );
}

#[test]
fn a_rule_for_a_path_that_is_not_there_is_skipped_rather_than_failed() {
    let harness = common::Harness::new();
    let repo = test_support::init_repo().expect("git repo (git must be installed)");

    let daemon = harness.boot();
    let client = daemon.connect("shares");
    let events = client.events();
    common::add_main_workspace(&client, repo.path());
    let project_id = project_id(&client);

    client
        .request(Request::SetProjectShares {
            project_id,
            // A list is a wish, not a requirement: the file may appear later.
            rules: vec![rule(project_id, ".env", ShareStrategy::Copy)],
        })
        .expect("SetProjectShares");

    let workspace = worktree_with_shares(&client, &events, project_id, "feature/no-env");
    assert!(workspace.path.join(".git").exists());

    let Response::ShareStatus { entries, .. } = client
        .request(Request::GetShareStatus {
            workspace_id: workspace.id,
        })
        .expect("GetShareStatus")
    else {
        panic!("expected ShareStatus");
    };
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].state, ShareState::Missing);
}

#[test]
fn applying_twice_changes_nothing_the_second_time() {
    let harness = common::Harness::new();
    let repo = test_support::init_repo().expect("git repo (git must be installed)");
    std::fs::write(repo.path().join(".env"), "SECRET=1\n").unwrap();

    let daemon = harness.boot();
    let client = daemon.connect("shares");
    let events = client.events();
    common::add_main_workspace(&client, repo.path());
    let project_id = project_id(&client);
    client
        .request(Request::SetProjectShares {
            project_id,
            rules: vec![rule(project_id, ".env", ShareStrategy::Copy)],
        })
        .expect("SetProjectShares");

    let workspace = worktree_with_shares(&client, &events, project_id, "feature/idempotent");
    // The worktree's own copy is edited: an agent's work, which a background
    // run must never overwrite.
    std::fs::write(workspace.path.join(".env"), "SECRET=edited\n").unwrap();

    client
        .request(Request::ApplyShares {
            workspace_id: workspace.id,
            only: None,
        })
        .expect("ApplyShares");
    common::wait_for(&events, common::DEADLINE, |event| {
        matches!(event, DaemonEvent::SharesApplied { workspace_id, .. } if *workspace_id == workspace.id)
    })
    .expect("SharesApplied");

    assert_eq!(
        std::fs::read_to_string(workspace.path.join(".env")).ok(),
        Some("SECRET=edited\n".to_string()),
        "a run nobody asked for does not overwrite what is already there"
    );
}

#[test]
fn a_worktree_made_in_a_terminal_is_provisioned_when_it_is_adopted() {
    let harness = common::Harness::new();
    let repo = test_support::init_repo().expect("git repo (git must be installed)");
    std::fs::write(repo.path().join(".env"), "SECRET=1\n").unwrap();

    let daemon = harness.boot();
    let client = daemon.connect("shares");
    let events = client.events();
    common::add_main_workspace(&client, repo.path());
    let project_id = project_id(&client);
    client
        .request(Request::SetProjectShares {
            project_id,
            rules: vec![rule(project_id, ".env", ShareStrategy::Copy)],
        })
        .expect("SetProjectShares");

    // Outside the app entirely, the way a person actually makes one. The name
    // is unique per run: the parent is the shared temp dir, and a leftover from
    // an earlier run would make `git worktree add` refuse.
    let outside = repo
        .path()
        .parent()
        .unwrap()
        .join(format!("outside-worktree-{}", domain::WorkspaceId::new()));
    git(
        repo.path(),
        &[
            "worktree",
            "add",
            "-b",
            "feature/outside",
            outside.to_str().unwrap(),
        ],
    );

    client
        .request(Request::RefreshProject { project_id })
        .expect("RefreshProject");
    // The main workspace's own `WorkspaceCreated` is still in the queue, and
    // it is `managed_by_app = false` like an adopted worktree — the kind is
    // what tells them apart.
    let created = common::wait_for(&events, common::DEADLINE, |event| {
        matches!(event, DaemonEvent::WorkspaceCreated(w) if w.kind == domain::WorkspaceKind::GitWorktree)
    })
    .expect("WorkspaceCreated for the adopted worktree");
    let DaemonEvent::WorkspaceCreated(workspace) = created else {
        panic!("expected WorkspaceCreated");
    };
    common::wait_for(&events, common::DEADLINE, |event| {
        matches!(event, DaemonEvent::SharesApplied { workspace_id, .. } if *workspace_id == workspace.id)
    })
    .expect("SharesApplied for the adopted worktree");

    assert_eq!(
        std::fs::read_to_string(workspace.path.join(".env")).ok(),
        Some("SECRET=1\n".to_string()),
        "who ran `git worktree add` is not a reason to skip provisioning"
    );
    let _ = std::fs::remove_dir_all(&outside);
}

#[test]
fn removing_a_rule_does_only_what_the_caller_asked() {
    let harness = common::Harness::new();
    let repo = test_support::init_repo().expect("git repo (git must be installed)");
    std::fs::write(repo.path().join(".env"), "SECRET=1\n").unwrap();

    let daemon = harness.boot();
    let client = daemon.connect("shares");
    let events = client.events();
    common::add_main_workspace(&client, repo.path());
    let project_id = project_id(&client);

    let keeper = rule(project_id, ".env", ShareStrategy::Copy);
    client
        .request(Request::SetProjectShares {
            project_id,
            rules: vec![keeper.clone()],
        })
        .expect("SetProjectShares");
    let workspace = worktree_with_shares(&client, &events, project_id, "feature/removal");
    assert!(workspace.path.join(".env").exists());

    // `Leave` is the default and it means what it says.
    client
        .request(Request::RemoveShareRule {
            project_id,
            rule_id: keeper.id,
            cleanup: ShareCleanup::Leave,
        })
        .expect("RemoveShareRule");
    assert!(
        workspace.path.join(".env").exists(),
        "removing a rule with Leave must not touch a single file"
    );

    let Response::Snapshot {
        worktree_shares, ..
    } = client.request(Request::GetSnapshot).expect("GetSnapshot")
    else {
        panic!("expected Snapshot");
    };
    assert!(worktree_shares.is_empty(), "the rule itself is gone");
}

#[test]
fn remove_injected_takes_back_its_own_copy_and_leaves_an_edited_one() {
    let harness = common::Harness::new();
    let repo = test_support::init_repo().expect("git repo (git must be installed)");
    std::fs::write(repo.path().join(".env"), "SECRET=1\n").unwrap();

    let daemon = harness.boot();
    let client = daemon.connect("shares");
    let events = client.events();
    common::add_main_workspace(&client, repo.path());
    let project_id = project_id(&client);

    let keeper = rule(project_id, ".env", ShareStrategy::Copy);
    client
        .request(Request::SetProjectShares {
            project_id,
            rules: vec![keeper.clone()],
        })
        .expect("SetProjectShares");
    let untouched = worktree_with_shares(&client, &events, project_id, "feature/untouched");
    let edited = worktree_with_shares(&client, &events, project_id, "feature/edited");
    std::fs::write(edited.path.join(".env"), "SECRET=mine\n").unwrap();

    client
        .request(Request::RemoveShareRule {
            project_id,
            rule_id: keeper.id,
            cleanup: ShareCleanup::RemoveInjected,
        })
        .expect("RemoveShareRule");

    assert!(
        !untouched.path.join(".env").exists(),
        "what Forge wrote and nobody changed is Forge's to take back"
    );
    assert_eq!(
        std::fs::read_to_string(edited.path.join(".env")).ok(),
        Some("SECRET=mine\n".to_string()),
        "a file that was changed since is somebody's work, not our leftover"
    );
}

#[test]
fn an_edited_copy_reads_as_diverged_whatever_the_filesystem_does_to_mtime() {
    // `std::fs::copy` preserves mtime on macOS and not on Linux, so the state
    // of a copy cannot be a timestamp question: it is a content question.
    let harness = common::Harness::new();
    let repo = test_support::init_repo().expect("git repo (git must be installed)");
    std::fs::write(repo.path().join(".env"), "SECRET=1\n").unwrap();

    let daemon = harness.boot();
    let client = daemon.connect("shares");
    let events = client.events();
    common::add_main_workspace(&client, repo.path());
    let project_id = project_id(&client);
    client
        .request(Request::SetProjectShares {
            project_id,
            rules: vec![rule(project_id, ".env", ShareStrategy::Copy)],
        })
        .expect("SetProjectShares");
    let workspace = worktree_with_shares(&client, &events, project_id, "feature/diverged");

    let status = |workspace_id| {
        let Response::ShareStatus { entries, .. } = client
            .request(Request::GetShareStatus { workspace_id })
            .expect("GetShareStatus")
        else {
            panic!("expected ShareStatus");
        };
        entries[0].state.clone()
    };
    assert_eq!(status(workspace.id), ShareState::Applied);

    // Same length, different bytes: a timestamp check would miss this one and a
    // length check would too.
    std::fs::write(workspace.path.join(".env"), "SECRET=2\n").unwrap();
    assert_eq!(status(workspace.id), ShareState::Diverged);
}

#[test]
fn detection_proposes_the_ignored_paths_of_the_project() {
    let harness = common::Harness::new();
    let repo = test_support::init_repo().expect("git repo (git must be installed)");
    std::fs::write(repo.path().join(".gitignore"), ".env\nnode_modules/\n").unwrap();
    git(repo.path(), &["add", ".gitignore"]);
    git(repo.path(), &["commit", "-m", "ignore"]);
    std::fs::write(repo.path().join(".env"), "SECRET=1\n").unwrap();
    std::fs::create_dir_all(repo.path().join("node_modules/pkg")).unwrap();
    std::fs::write(repo.path().join("node_modules/pkg/index.js"), "//\n").unwrap();

    let daemon = harness.boot();
    let client = daemon.connect("shares");
    common::add_main_workspace(&client, repo.path());
    let project_id = project_id(&client);

    let Response::ShareCandidates { candidates, .. } = client
        .request(Request::DetectShareCandidates { project_id })
        .expect("DetectShareCandidates")
    else {
        panic!("expected ShareCandidates");
    };
    let paths: Vec<&str> = candidates.iter().map(|c| c.path.as_str()).collect();
    assert!(paths.contains(&".env"), "got {paths:?}");
    assert!(paths.contains(&"node_modules"), "got {paths:?}");

    let env = candidates.iter().find(|c| c.path == ".env").unwrap();
    assert_eq!(env.class, domain::ShareClass::Secret);
    assert_eq!(env.suggested, ShareStrategy::Link);
    let modules = candidates
        .iter()
        .find(|c| c.path == "node_modules")
        .unwrap();
    assert!(modules.is_dir);
    assert_eq!(modules.suggested, ShareStrategy::Clone);
}

#[test]
fn a_path_that_escapes_the_repository_is_refused() {
    let harness = common::Harness::new();
    let repo = test_support::init_repo().expect("git repo (git must be installed)");

    let daemon = harness.boot();
    let client = daemon.connect("shares");
    common::add_main_workspace(&client, repo.path());
    let project_id = project_id(&client);

    // `copy = ["../../.ssh/id_rsa"]` must not duplicate a secret.
    let refused = client.request(Request::SetProjectShares {
        project_id,
        rules: vec![rule(project_id, "../../.ssh/id_rsa", ShareStrategy::Copy)],
    });
    assert!(refused.is_err(), "a path with `..` must be refused");

    let refused = client.request(Request::SetProjectShares {
        project_id,
        rules: vec![rule(project_id, ".git/config", ShareStrategy::Copy)],
    });
    assert!(refused.is_err(), "the git directory is not shareable");
}
