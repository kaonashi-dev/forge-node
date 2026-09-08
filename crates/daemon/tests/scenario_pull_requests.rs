//! Pull-request refresh and creation through a real daemon and fake `gh` CLI.

mod common;

use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use domain::{PullRequestSourceStatus, PullRequestState};
use protocol::{DaemonEvent, Request, Response};

fn write_gh(path: &Path, counter: &Path, response: &str, exit: i32, delay: bool) {
    assert!(!response.contains('\''), "fixture must be shell-quotable");
    let delay = if delay { "/bin/sleep 1\n" } else { "" };
    let script = format!(
        "#!/bin/sh\nprintf x >> \"{}\"\n{delay}printf '%s\\n' '{response}'\nexit {exit}\n",
        counter.display()
    );
    fs::write(path, script).expect("write fake gh");
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("chmod fake gh");
}

fn command_ok(command: &mut Command) {
    let output = command.output().expect("run command");
    assert!(
        output.status.success(),
        "command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn github_repo(root: &Path, name: &str, remote: &str) -> PathBuf {
    let repo = root.join(name);
    fs::create_dir(&repo).expect("create repository directory");
    command_ok(Command::new("git").arg("-C").arg(&repo).args([
        "-c",
        "init.defaultBranch=main",
        "init",
    ]));
    command_ok(
        Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(["remote", "add", "origin", remote]),
    );
    repo
}

fn local_repo(root: &Path, name: &str) -> PathBuf {
    let repo = root.join(name);
    fs::create_dir(&repo).expect("create repository directory");
    command_ok(Command::new("git").arg("-C").arg(&repo).args([
        "-c",
        "init.defaultBranch=main",
        "init",
    ]));
    repo
}

fn pull_request_event(
    events: &flume::Receiver<DaemonEvent>,
    timeout: Duration,
) -> PullRequestState {
    let event = common::wait_for(events, timeout, |event| {
        matches!(event, DaemonEvent::PullRequestsUpdated { .. })
    })
    .expect("PullRequestsUpdated event");
    let DaemonEvent::PullRequestsUpdated { state } = event else {
        unreachable!()
    };
    state
}

fn invocation_count(counter: &Path) -> usize {
    fs::read(counter).map_or(0, |contents| contents.len())
}

fn complete_response(repository: &str) -> String {
    format!(
        r#"{{"data":{{"viewer":{{"login":"ForgeUser"}},"p0":{{"nameWithOwner":"{repository}","pullRequests":{{"totalCount":1,"nodes":[{{"number":17,"title":"Ship cached PRs","url":"https://github.com/{repository}/pull/17","body":"Ready to ship","isDraft":false,"createdAt":"2026-08-24T10:00:00Z","updatedAt":"2026-08-25T12:00:00Z","author":{{"login":"forgeuser"}},"repository":{{"nameWithOwner":"{repository}"}},"baseRefName":"main","headRefName":"cache","reviewDecision":"APPROVED","additions":12,"deletions":3,"changedFiles":2,"comments":{{"totalCount":4}},"labels":{{"nodes":[{{"name":"ready","color":"00ff00"}}]}},"assignees":{{"nodes":[{{"login":"FORGEUSER"}}]}},"reviewRequests":{{"nodes":[{{"requestedReviewer":{{"login":"ForgeUser"}}}}]}}}}]}}}},"rateLimit":{{"cost":1,"remaining":4999,"resetAt":"2026-08-25T13:00:00Z"}}}}}}"#
    )
}

#[test]
fn refresh_coalesces_reuses_cache_and_snapshot_never_invokes_gh() {
    let harness = common::Harness::new();
    let gh = harness.root().join("fake-gh");
    let counter = harness.root().join("gh-count");
    write_gh(&gh, &counter, &complete_response("Acme/Forge"), 0, true);
    let repo = github_repo(
        harness.root(),
        "forge-project",
        "git@github.com:Acme/Forge.git",
    );
    let local = local_repo(harness.root(), "local-project");
    let unsupported = github_repo(
        harness.root(),
        "gitlab-project",
        "git@gitlab.com:Acme/Other.git",
    );
    let daemon = harness.boot_with(|config| {
        config.github.executable = gh.to_string_lossy().into_owned();
    });
    let client = daemon.connect("scenario-pull-requests");
    let events = client.events();
    common::add_main_workspace(&client, &repo);
    common::add_main_workspace(&client, &local);
    common::add_main_workspace(&client, &unsupported);

    let Response::Snapshot { pull_requests, .. } = client
        .request(Request::GetSnapshot)
        .expect("snapshot before refresh")
    else {
        panic!("expected snapshot");
    };
    assert_eq!(pull_requests, PullRequestState::default());
    assert_eq!(invocation_count(&counter), 0, "snapshot must not invoke gh");

    assert_eq!(
        client
            .request(Request::RefreshPullRequests)
            .expect("first refresh"),
        Response::Ack
    );
    assert_eq!(
        client
            .request(Request::RefreshPullRequests)
            .expect("coalesced refresh"),
        Response::Ack
    );
    let state = pull_request_event(&events, common::DEADLINE);
    assert_eq!(invocation_count(&counter), 1, "one worker invokes gh once");
    assert_eq!(state.pull_requests.len(), 1);
    assert_eq!(state.viewers[0].login, "ForgeUser");
    assert!(state
        .sources
        .iter()
        .any(|source| source.status == PullRequestSourceStatus::Ready));
    assert!(state
        .sources
        .iter()
        .any(|source| source.status == PullRequestSourceStatus::NoRemote));
    assert!(state
        .sources
        .iter()
        .any(|source| source.status == PullRequestSourceStatus::UnsupportedHost));
    let pull_request = &state.pull_requests[0];
    assert!(pull_request.project_id.is_some());
    assert!(pull_request.relations.assigned);
    assert!(pull_request.relations.review_requested);
    assert!(pull_request.relations.authored);

    client
        .request(Request::RefreshPullRequests)
        .expect("fresh cached refresh");
    let cached = pull_request_event(&events, common::DEADLINE);
    assert_eq!(cached, state, "fresh refresh rebroadcasts the cached state");
    assert_eq!(invocation_count(&counter), 1, "fresh cache bypasses gh");

    let Response::Snapshot { pull_requests, .. } = client
        .request(Request::GetSnapshot)
        .expect("snapshot after refresh")
    else {
        panic!("expected snapshot");
    };
    assert_eq!(pull_requests, state);
    assert_eq!(invocation_count(&counter), 1, "snapshot remains clone-only");
}

#[test]
fn missing_gh_is_readable_and_does_not_kill_the_daemon() {
    let harness = common::Harness::new();
    let missing = harness.root().join("definitely-missing-gh");
    let repo = github_repo(
        harness.root(),
        "missing-gh-project",
        "https://github.com/Acme/Missing.git",
    );
    let daemon = harness.boot_with(|config| {
        config.github.executable = missing.to_string_lossy().into_owned();
    });
    let client = daemon.connect("scenario-missing-gh");
    let events = client.events();
    common::add_main_workspace(&client, &repo);

    client
        .request(Request::RefreshPullRequests)
        .expect("refresh ack");
    let state = pull_request_event(&events, common::DEADLINE);
    let error = state.error.expect("global error when every host fails");
    assert!(error.contains("GitHub CLI was not found"), "{error}");
    assert_eq!(state.sources[0].status, PullRequestSourceStatus::Failed);
    assert!(
        matches!(
            client.request(Request::GetSnapshot),
            Ok(Response::Snapshot { .. })
        ),
        "daemon remains responsive"
    );
}

#[test]
fn exit_one_with_partial_graphql_data_keeps_successful_repositories() {
    let harness = common::Harness::new();
    let gh = harness.root().join("partial-gh");
    let counter = harness.root().join("partial-count");
    write_gh(&gh, &counter, "{}", 1, false);
    let first = github_repo(harness.root(), "partial-one", "git@github.com:Acme/One.git");
    let second = github_repo(harness.root(), "partial-two", "git@github.com:Acme/Two.git");
    let daemon = harness.boot_with(|config| {
        config.github.executable = gh.to_string_lossy().into_owned();
    });
    let client = daemon.connect("scenario-partial-gh");
    let events = client.events();
    common::add_main_workspace(&client, &first);
    common::add_main_workspace(&client, &second);

    let Response::Snapshot { mut projects, .. } = client
        .request(Request::GetSnapshot)
        .expect("project snapshot")
    else {
        panic!("expected snapshot");
    };
    projects.sort_unstable_by_key(|project| project.id);
    let identities: Vec<&str> = projects
        .iter()
        .map(|project| {
            if project.root_path.file_name().and_then(|name| name.to_str()) == Some("partial-one") {
                "Acme/One"
            } else {
                "Acme/Two"
            }
        })
        .collect();
    let response = format!(
        r#"{{"data":{{"viewer":{{"login":"forge"}},"p0":{{"nameWithOwner":"{}","pullRequests":{{"totalCount":1,"nodes":[{{"number":1,"title":"Good","url":"https://github.com/{}/pull/1","body":"","isDraft":false,"createdAt":"2026-08-24T10:00:00Z","updatedAt":"2026-08-25T10:00:00Z","author":{{"login":"other"}},"repository":{{"nameWithOwner":"{}"}},"baseRefName":"main","headRefName":"good","additions":0,"deletions":0,"changedFiles":0,"comments":{{"totalCount":0}},"labels":{{"nodes":[]}},"assignees":{{"nodes":[]}},"reviewRequests":{{"nodes":[]}}}}]}}}},"p1":null,"rateLimit":{{"cost":1,"remaining":4999,"resetAt":"2026-08-25T13:00:00Z"}}}},"errors":[{{"type":"NOT_FOUND","path":["p1"],"message":"repository is unavailable"}}]}}"#,
        identities[0], identities[0], identities[0]
    );
    write_gh(&gh, &counter, &response, 1, false);

    client
        .request(Request::RefreshPullRequests)
        .expect("partial refresh ack");
    let state = pull_request_event(&events, common::DEADLINE);
    assert_eq!(state.error, None, "parseable host data is a host success");
    assert_eq!(state.pull_requests.len(), 1);
    assert_eq!(state.pull_requests[0].repository, identities[0]);
    assert!(state
        .failures
        .iter()
        .any(|failure| failure.repository.as_deref() == Some(identities[1])));
    let good = state
        .sources
        .iter()
        .find(|source| source.repository.as_deref() == Some(identities[0]))
        .expect("good source");
    let failed = state
        .sources
        .iter()
        .find(|source| source.repository.as_deref() == Some(identities[1]))
        .expect("failed source");
    assert_eq!(good.status, PullRequestSourceStatus::Ready);
    assert_eq!(failed.status, PullRequestSourceStatus::Failed);
    assert_eq!(invocation_count(&counter), 1);
}

#[test]
fn successful_creation_uses_configured_gh_and_immediately_refreshes() {
    let harness = common::Harness::new();
    let gh = harness.root().join("create-gh");
    let counter = harness.root().join("create-count");
    write_gh(
        &gh,
        &counter,
        "https://github.com/Acme/Created/pull/9",
        0,
        false,
    );
    let repo = test_support::init_repo().expect("git repo");
    repo.create_branch("feature").expect("feature branch");
    repo.commit_file("feature.txt", "created\n")
        .expect("feature commit");
    let bare = harness.root().join("remote.git");
    command_ok(Command::new("git").args(["init", "--bare"]).arg(&bare));
    command_ok(
        Command::new("git")
            .arg("-C")
            .arg(repo.path())
            .args(["remote", "add", "origin"])
            .arg(&bare),
    );

    let daemon = harness.boot_with(|config| {
        config.github.executable = gh.to_string_lossy().into_owned();
    });
    let client = daemon.connect("scenario-create-pr");
    let events = client.events();
    let workspace = common::add_main_workspace(&client, repo.path());

    assert_eq!(
        client
            .request(Request::CreatePullRequest {
                workspace_id: workspace,
                title: "Created".into(),
                body: "From configured gh".into(),
                base: Some("main".into()),
            })
            .expect("create ack"),
        Response::Ack
    );
    let opened = common::wait_for(&events, common::DEADLINE, |event| {
        matches!(
            event,
            DaemonEvent::PullRequestOpened {
                url: Some(url),
                ..
            } if url.ends_with("/pull/9")
        )
    })
    .expect("PullRequestOpened");
    assert!(matches!(opened, DaemonEvent::PullRequestOpened { .. }));
    let refreshed = pull_request_event(&events, common::DEADLINE);
    assert_eq!(
        refreshed.sources[0].status,
        PullRequestSourceStatus::InvalidRemote,
        "the local bare remote is resolved by the immediate refresh"
    );
    assert_eq!(
        invocation_count(&counter),
        1,
        "configured gh created the PR"
    );
}
