//! Branches, remotes and on-demand reconciliation, end to end through the
//! protocol (§14.1, §14.3; branches plan phases 2–4 and 6).
//!
//! What these cover that `git-service`'s own tests cannot: the *daemon's*
//! behaviour around git — that `ListBranches` knows which worktree already
//! holds a branch, that `FetchRemote` acks before it finishes and reports
//! through an event, that `RefreshProject` reconciles a worktree made outside
//! the app, and that a new worktree is provisioned before it is handed over.
//!
//! Every test uses a real repository and a real second repository as `origin`
//! (a local path is a first-class git transport), so the fetch path is the
//! genuine one and nothing here touches the network. They fail rather than
//! skip without `git`, like the rest of the daemon E2E suite.

mod common;

use std::path::Path;
use std::time::Duration;

use client::Client;
use domain::{RefScope, WorkspaceKind};
use protocol::{DaemonEvent, Request, Response};

/// Run a setup git command in `repo`, panicking on failure.
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

/// A repository to serve as `origin`, carrying `branches` beyond `main`.
fn origin_with(branches: &[&str]) -> test_support::temp_repo::TempRepo {
    let repo = test_support::init_repo().expect("git repo (git must be installed)");
    for branch in branches {
        git(repo.path(), &["checkout", "-b", branch]);
        std::fs::write(repo.path().join("work.txt"), *branch).unwrap();
        git(repo.path(), &["add", "."]);
        git(repo.path(), &["commit", "-m", "work"]);
    }
    git(repo.path(), &["checkout", "main"]);
    repo
}

/// The project id of the only project the daemon knows.
fn project_id(client: &Client) -> domain::ProjectId {
    let Response::Snapshot { projects, .. } =
        client.request(Request::GetSnapshot).expect("GetSnapshot")
    else {
        panic!("expected Snapshot");
    };
    projects[0].id
}

fn list_branches(client: &Client, project_id: domain::ProjectId) -> Vec<domain::BranchRef> {
    let Response::Branches { branches, .. } = client
        .request(Request::ListBranches { project_id })
        .expect("ListBranches")
    else {
        panic!("expected Branches");
    };
    branches
}

#[test]
fn list_branches_reports_local_refs_and_the_default_branch() {
    let harness = common::Harness::new();
    let repo = test_support::init_repo().expect("git repo (git must be installed)");
    git(repo.path(), &["branch", "feature/auth"]);

    let daemon = harness.boot();
    let client = daemon.connect("branches");
    common::add_main_workspace(&client, repo.path());
    let project_id = project_id(&client);

    let Response::Branches {
        branches,
        remotes,
        default_branch,
    } = client
        .request(Request::ListBranches { project_id })
        .expect("ListBranches")
    else {
        panic!("expected Branches");
    };

    let names: Vec<&str> = branches.iter().map(|b| b.name.as_str()).collect();
    assert!(names.contains(&"main"), "got {names:?}");
    assert!(names.contains(&"feature/auth"), "got {names:?}");
    assert!(branches.iter().all(|b| !b.is_remote()));
    // No remote configured: the GUI reads this as "hide the fetch controls".
    assert!(remotes.is_empty());
    // With no `origin/HEAD` the current branch is the fallback base.
    assert_eq!(default_branch.as_deref(), Some("main"));
}

#[test]
fn a_branch_already_checked_out_is_reported_with_the_workspace_holding_it() {
    // This is what lets the picker grey the row out instead of letting the
    // user discover git's refusal as a `Conflict` after filling in a dialog.
    let harness = common::Harness::new();
    let repo = test_support::init_repo().expect("git repo (git must be installed)");

    let daemon = harness.boot();
    let client = daemon.connect("branches");
    let events = client.events();
    common::add_main_workspace(&client, repo.path());
    let project_id = project_id(&client);

    client
        .request(Request::CreateWorktree {
            project_id,
            branch: "feature/busy".to_string(),
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
    let DaemonEvent::WorkspaceCreated(workspace) = created else {
        panic!("expected WorkspaceCreated");
    };

    let branches = list_branches(&client, project_id);
    let busy = branches
        .iter()
        .find(|b| b.name == "feature/busy")
        .expect("the branch the worktree checked out");
    assert_eq!(busy.checked_out_in, Some(workspace.id));

    // The main checkout holds `main`, and it is a workspace like any other (P4).
    let main = branches.iter().find(|b| b.name == "main").expect("main");
    assert!(main.checked_out_in.is_some());
}

#[test]
fn fetching_a_remote_acks_immediately_and_reports_through_an_event() {
    // The ack means "started". A synchronous fetch would block the GUI's
    // command loop, which also carries every keystroke.
    let harness = common::Harness::new();
    let origin = origin_with(&["feature/remote-only"]);
    let repo = test_support::init_repo().expect("git repo (git must be installed)");
    git(
        repo.path(),
        &["remote", "add", "origin", origin.path().to_str().unwrap()],
    );

    let daemon = harness.boot();
    let client = daemon.connect("branches");
    let events = client.events();
    common::add_main_workspace(&client, repo.path());
    let project_id = project_id(&client);

    // Before the fetch the remote branch does not exist here at all — the
    // reason a picker without a fetch would silently show a stale list.
    let before = list_branches(&client, project_id);
    assert!(!before.iter().any(|b| b.name == "feature/remote-only"));

    let started = std::time::Instant::now();
    let response = client
        .request(Request::FetchRemote {
            project_id,
            remote: None,
        })
        .expect("FetchRemote");
    assert_eq!(response, Response::Ack);
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "the ack must not wait for the fetch to finish"
    );

    let event = common::wait_for(&events, common::DEADLINE, |event| {
        matches!(event, DaemonEvent::RemoteRefsUpdated { .. })
    })
    .expect("RemoteRefsUpdated");
    let DaemonEvent::RemoteRefsUpdated { remote, error, .. } = event else {
        panic!("expected RemoteRefsUpdated");
    };
    assert_eq!(remote, "origin");
    assert_eq!(error, None, "the fetch should have succeeded");

    let after = list_branches(&client, project_id);
    let remote_ref = after
        .iter()
        .find(|b| b.name == "feature/remote-only")
        .expect("the remote branch is listed after the fetch");
    assert_eq!(
        remote_ref.scope,
        RefScope::Remote {
            remote: "origin".to_string()
        }
    );
}

#[test]
fn a_failing_fetch_reports_git_own_message_instead_of_hanging() {
    // The batch-mode hardening is what makes this fail fast: without it an
    // unreachable remote can sit on the connection until the timeout.
    let harness = common::Harness::new();
    let repo = test_support::init_repo().expect("git repo (git must be installed)");
    git(
        repo.path(),
        &["remote", "add", "origin", "/definitely/not/a/repository"],
    );

    let daemon = harness.boot();
    let client = daemon.connect("branches");
    let events = client.events();
    common::add_main_workspace(&client, repo.path());
    let project_id = project_id(&client);

    client
        .request(Request::FetchRemote {
            project_id,
            remote: None,
        })
        .expect("FetchRemote");

    let event = common::wait_for(&events, common::DEADLINE, |event| {
        matches!(event, DaemonEvent::RemoteRefsUpdated { .. })
    })
    .expect("RemoteRefsUpdated even on failure");
    let DaemonEvent::RemoteRefsUpdated { error, .. } = event else {
        panic!("expected RemoteRefsUpdated");
    };
    let error = error.expect("a failed fetch must carry a message");
    assert!(
        !error.is_empty() && !error.contains("timed out"),
        "expected git's own words, got {error:?}"
    );
}

#[test]
fn a_worktree_created_from_a_remote_branch_tracks_it() {
    // The path the picker takes for a "Remote branches" row: the branch does
    // not exist locally, and the start point is the remote-tracking ref.
    let harness = common::Harness::new();
    let origin = origin_with(&["feature/tracked"]);
    let repo = test_support::init_repo().expect("git repo (git must be installed)");
    git(
        repo.path(),
        &["remote", "add", "origin", origin.path().to_str().unwrap()],
    );

    let daemon = harness.boot();
    let client = daemon.connect("branches");
    let events = client.events();
    common::add_main_workspace(&client, repo.path());
    let project_id = project_id(&client);

    client
        .request(Request::FetchRemote {
            project_id,
            remote: None,
        })
        .expect("FetchRemote");
    common::wait_for(&events, common::DEADLINE, |event| {
        matches!(event, DaemonEvent::RemoteRefsUpdated { error: None, .. })
    })
    .expect("a successful fetch");

    client
        .request(Request::CreateWorktree {
            project_id,
            branch: "feature/tracked".to_string(),
            base: Some("origin/feature/tracked".to_string()),
            name: None,
        })
        .expect("CreateWorktree from a remote branch");
    let created = common::wait_for(
        &events,
        common::DEADLINE,
        |event| matches!(event, DaemonEvent::WorkspaceCreated(w) if w.managed_by_app),
    )
    .expect("WorkspaceCreated");
    let DaemonEvent::WorkspaceCreated(workspace) = created else {
        panic!("expected WorkspaceCreated");
    };
    assert_eq!(workspace.branch.as_deref(), Some("feature/tracked"));

    // Git configures the upstream itself when the start point is a
    // remote-tracking branch; no `--track` is passed anywhere in the daemon.
    let upstream = std::process::Command::new("git")
        .arg("-C")
        .arg(&workspace.path)
        .args(["rev-parse", "--abbrev-ref", "@{upstream}"])
        .output()
        .expect("run git");
    assert!(
        upstream.status.success(),
        "no upstream: {}",
        String::from_utf8_lossy(&upstream.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&upstream.stdout).trim(),
        "origin/feature/tracked"
    );
}

#[test]
fn refresh_project_picks_up_a_worktree_created_outside_the_app() {
    // Before this, reconciliation only ran at daemon start, so a worktree made
    // in a terminal stayed invisible until a restart — and `RefreshProject`,
    // the button a user reaches for in exactly that situation, did nothing
    // about it.
    let harness = common::Harness::new();
    let repo = test_support::init_repo().expect("git repo (git must be installed)");

    let daemon = harness.boot();
    let client = daemon.connect("branches");
    let events = client.events();
    common::add_main_workspace(&client, repo.path());
    let project_id = project_id(&client);

    assert_eq!(
        common::workspaces(&client).len(),
        1,
        "only the main checkout"
    );

    // Created behind the daemon's back, the way a terminal would. It gets its
    // own temp dir rather than a fixed name next to the repo: the repository's
    // parent is the shared system temp directory, and a worktree left there
    // outlives the `TempRepo` that owns the repo, so a second run of this test
    // would find the path already taken.
    let outside_dir = tempfile::tempdir().expect("temp dir for the outside worktree");
    let outside = outside_dir.path().join("outside-worktree");
    git(
        repo.path(),
        &[
            "worktree",
            "add",
            "-b",
            "made/outside",
            outside.to_str().unwrap(),
        ],
    );

    client
        .request(Request::RefreshProject { project_id })
        .expect("RefreshProject");

    common::wait_for(&events, common::DEADLINE, |event| {
        matches!(event, DaemonEvent::WorkspaceCreated(w) if w.branch.as_deref() == Some("made/outside"))
    })
    .expect("RefreshProject must reconcile worktrees, not only the git root");

    let workspaces = common::workspaces(&client);
    let adopted = workspaces
        .iter()
        .find(|w| w.branch.as_deref() == Some("made/outside"))
        .expect("the outside worktree is now a workspace");
    assert_eq!(adopted.kind, WorkspaceKind::GitWorktree);
    // Forge did not create it, so Forge will never delete it from disk (§14.4).
    assert!(!adopted.managed_by_app);
}

/// A managed worktree deleted outside the app must leave the rail, and must
/// still be removable if it is somehow still there.
///
/// The status poll is the first thing that touches a vanished worktree, and it
/// used to swallow the git failure and answer `Ack`. The §15.3 step-4 diff that
/// would drop the row runs only at daemon start and from `RefreshProject`, so
/// the workspace stayed on screen — with a status dot, and with every action on
/// it failing — until the next restart.
#[test]
fn a_worktree_deleted_by_hand_leaves_the_model_on_the_next_status_poll() {
    let harness = common::Harness::new();
    let repo = test_support::init_repo().expect("git repo (git must be installed)");

    let daemon = harness.boot();
    let client = daemon.connect("branches");
    let events = client.events();
    common::add_main_workspace(&client, repo.path());
    let project_id = project_id(&client);

    client
        .request(Request::CreateWorktree {
            project_id,
            branch: "deleted/by-hand".to_string(),
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
    let DaemonEvent::WorkspaceCreated(ws) = created else {
        unreachable!()
    };
    assert!(ws.path.exists());

    // Deleted behind the daemon's back, the way `rm -rf` in a terminal would.
    std::fs::remove_dir_all(&ws.path).expect("delete the worktree directory");

    let workspace_id = ws.id;
    client
        .request(Request::RefreshWorkspaceStatus { workspace_id })
        .expect("RefreshWorkspaceStatus");

    common::wait_for(&events, common::DEADLINE, |event| {
        matches!(event, DaemonEvent::WorkspaceRemoved { workspace_id: id } if *id == workspace_id)
    })
    .expect("a workspace whose directory is gone must leave the model");

    assert!(
        !common::workspaces(&client)
            .iter()
            .any(|w| w.id == workspace_id),
        "the vanished worktree is no longer offered in the rail"
    );

    // The branch is still there: nothing here deletes one (§14.4).
    let Response::Branches { branches, .. } = client
        .request(Request::ListBranches { project_id })
        .expect("ListBranches")
    else {
        panic!("ListBranches");
    };
    assert!(branches
        .iter()
        .any(|b| b.name == "deleted/by-hand" && b.scope == RefScope::Local));
}

/// The same worktree, removed through the GUI's own path before any status poll
/// reconciled it. `precheck_remove` and `git worktree remove` both used to fail
/// on a path that is no longer there, so the one action that could clean the
/// workspace up was the one action guaranteed to fail.
#[test]
fn remove_worktree_succeeds_when_the_directory_is_already_gone() {
    let harness = common::Harness::new();
    let repo = test_support::init_repo().expect("git repo (git must be installed)");

    let daemon = harness.boot();
    let client = daemon.connect("branches");
    let events = client.events();
    common::add_main_workspace(&client, repo.path());
    let project_id = project_id(&client);

    client
        .request(Request::CreateWorktree {
            project_id,
            branch: "gone/already".to_string(),
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
    let DaemonEvent::WorkspaceCreated(ws) = created else {
        unreachable!()
    };

    std::fs::remove_dir_all(&ws.path).expect("delete the worktree directory");

    // Unforced: the pre-checks have nothing to report about a directory that
    // does not exist, so this must not come back `PreconditionFailed`.
    assert_eq!(
        client
            .request(Request::RemoveWorktree {
                workspace_id: ws.id,
                force: false,
            })
            .expect("RemoveWorktree must not fail on a vanished worktree"),
        Response::Ack
    );

    assert!(
        !common::workspaces(&client).iter().any(|w| w.id == ws.id),
        "the workspace is gone from the model"
    );
}

#[test]
fn refresh_project_reports_a_dirty_working_tree() {
    let harness = common::Harness::new();
    let repo = test_support::init_repo().expect("git repo (git must be installed)");

    let daemon = harness.boot();
    let client = daemon.connect("branches");
    common::add_main_workspace(&client, repo.path());
    let project_id = project_id(&client);

    // A workspace nobody has measured says so, rather than claiming to be clean.
    let before = common::workspaces(&client);
    assert!(!before[0].status.is_measured());

    std::fs::write(repo.path().join("scratch.txt"), "uncommitted\n").unwrap();
    client
        .request(Request::RefreshProject { project_id })
        .expect("RefreshProject");

    assert!(
        common::poll_until(common::DEADLINE, || {
            common::workspaces(&client)
                .first()
                .is_some_and(|w| w.status.is_measured() && w.status.dirty)
        }),
        "the status should have been measured and reported dirty"
    );
}

#[test]
fn a_commit_rewrites_the_reported_head() {
    // `WorkspaceStatus.head` is what clients read as "the checkout moved": a
    // `git pull` or a commit rewrites it while leaving `branch` unchanged.
    let harness = common::Harness::new();
    let repo = test_support::init_repo().expect("git repo (git must be installed)");

    let daemon = harness.boot();
    let client = daemon.connect("branches");
    common::add_main_workspace(&client, repo.path());

    client
        .request(Request::RefreshProject {
            project_id: project_id(&client),
        })
        .expect("RefreshProject");

    let mut before: Option<String> = None;
    assert!(
        common::poll_until(common::DEADLINE, || {
            before = common::workspaces(&client)
                .first()
                .and_then(|w| w.status.head.clone());
            before.is_some()
        }),
        "the first status should carry HEAD's oid"
    );

    repo.commit_file("scratch.txt", "moves HEAD\n").unwrap();
    client
        .request(Request::RefreshProject {
            project_id: project_id(&client),
        })
        .expect("RefreshProject");

    let mut after: Option<String> = None;
    assert!(
        common::poll_until(common::DEADLINE, || {
            after = common::workspaces(&client)
                .first()
                .and_then(|w| w.status.head.clone())
                .filter(|head| Some(head) != before.as_ref());
            after.is_some()
        }),
        "the committed oid should replace the old one"
    );
    assert_ne!(after, before);
}

#[test]
fn a_new_worktree_gets_the_configured_files_and_setup_script() {
    // A managed worktree lives away from the repository, so it starts without
    // the untracked files a project needs to run. Without this an agent
    // launched there fails on its first command.
    let harness = common::Harness::new();
    let repo = test_support::init_repo().expect("git repo (git must be installed)");
    std::fs::write(repo.path().join(".env"), "SECRET=1\n").unwrap();

    let daemon = harness.boot_with(|cfg| {
        cfg.worktrees.copy = vec![".env".to_string()];
        cfg.worktrees.setup_script = "printf ready > .setup-ran".to_string();
    });
    let client = daemon.connect("branches");
    let events = client.events();
    common::add_main_workspace(&client, repo.path());
    let project_id = project_id(&client);

    client
        .request(Request::CreateWorktree {
            project_id,
            branch: "feature/provisioned".to_string(),
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
    let DaemonEvent::WorkspaceCreated(workspace) = created else {
        panic!("expected WorkspaceCreated");
    };

    // Provisioning acks when it *starts* (§14.2): the worktree appears first
    // and the files land when `SharesApplied` says so. Asserting right after
    // the reply would be asserting against a race, not against behaviour.
    common::wait_for(&events, common::DEADLINE, |event| {
        matches!(event, DaemonEvent::SharesApplied { workspace_id, .. } if *workspace_id == workspace.id)
    })
    .expect("SharesApplied");

    // `.env` is untracked, so `git worktree add` would never have brought it.
    assert_eq!(
        std::fs::read_to_string(workspace.path.join(".env")).ok(),
        Some("SECRET=1\n".to_string()),
        "the configured file should have been copied in"
    );
    assert_eq!(
        std::fs::read_to_string(workspace.path.join(".setup-ran")).ok(),
        Some("ready".to_string()),
        "the setup script should have run in the worktree"
    );
}

#[test]
fn a_failing_setup_script_still_hands_over_a_usable_worktree() {
    // The checkout exists by the time provisioning runs. Refusing to hand it
    // over because a script exited 1 would leave the user worse off than
    // handing over a checkout that needs a second look.
    let harness = common::Harness::new();
    let repo = test_support::init_repo().expect("git repo (git must be installed)");

    let daemon = harness.boot_with(|cfg| {
        cfg.worktrees.setup_script = "exit 3".to_string();
        // A file that is not there is skipped, not an error: a copy list is a
        // wish, and `.env` legitimately does not exist in many checkouts.
        cfg.worktrees.copy = vec![".env".to_string()];
    });
    let client = daemon.connect("branches");
    let events = client.events();
    common::add_main_workspace(&client, repo.path());
    let project_id = project_id(&client);

    let response = client
        .request(Request::CreateWorktree {
            project_id,
            branch: "feature/bad-setup".to_string(),
            base: None,
            name: None,
        })
        .expect("CreateWorktree must succeed despite the script");
    assert_eq!(response, Response::Ack);

    let created = common::wait_for(
        &events,
        common::DEADLINE,
        |event| matches!(event, DaemonEvent::WorkspaceCreated(w) if w.managed_by_app),
    )
    .expect("WorkspaceCreated");
    let DaemonEvent::WorkspaceCreated(workspace) = created else {
        panic!("expected WorkspaceCreated");
    };
    assert!(
        workspace.path.join(".git").exists(),
        "the worktree is on disk and checked out"
    );
    // The failing script is reported, not fatal: the run still lands.
    common::wait_for(&events, common::DEADLINE, |event| {
        matches!(event, DaemonEvent::SharesApplied { workspace_id, .. } if *workspace_id == workspace.id)
    })
    .expect("SharesApplied");
}

#[test]
fn a_project_without_a_remote_refuses_a_fetch_instead_of_guessing() {
    let harness = common::Harness::new();
    let repo = test_support::init_repo().expect("git repo (git must be installed)");

    let daemon = harness.boot();
    let client = daemon.connect("branches");
    common::add_main_workspace(&client, repo.path());
    let project_id = project_id(&client);

    let error = client
        .request(Request::FetchRemote {
            project_id,
            remote: None,
        })
        .expect_err("there is nothing to fetch from");
    assert!(
        error.to_string().contains("remote"),
        "the refusal should name the missing remote: {error}"
    );
}
