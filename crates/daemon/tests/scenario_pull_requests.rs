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
    assert_eq!(*pull_requests, PullRequestState::default());
    assert_eq!(invocation_count(&counter), 0, "snapshot must not invoke gh");

    assert_eq!(
        client
            .request(Request::RefreshPullRequests { force: false })
            .expect("first refresh"),
        Response::Ack
    );
    assert_eq!(
        client
            .request(Request::RefreshPullRequests { force: false })
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
        .request(Request::RefreshPullRequests { force: false })
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
    assert_eq!(*pull_requests, state);
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
        .request(Request::RefreshPullRequests { force: false })
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
        .request(Request::RefreshPullRequests { force: false })
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

fn git_stdout(repo: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .expect("run git");
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn draft(branch: &str) -> domain::PullRequestDraft {
    domain::PullRequestDraft {
        title: "Draft title".into(),
        body: "Draft body".into(),
        branch: branch.into(),
        commit_message: "Add the draft file".into(),
    }
}

/// A checkout on `main` with one uncommitted file and a bare `origin`.
fn dirty_main_with_remote(harness: &common::Harness) -> test_support::TempRepo {
    let repo = test_support::init_repo().expect("git repo");
    fs::write(repo.path().join("draft.txt"), "drafted\n").expect("write change");
    let bare = harness.root().join("draft-remote.git");
    command_ok(Command::new("git").args(["init", "--bare"]).arg(&bare));
    command_ok(
        Command::new("git")
            .arg("-C")
            .arg(repo.path())
            .args(["remote", "add", "origin"])
            .arg(&bare),
    );
    repo
}

#[test]
fn an_agent_draft_is_committed_on_a_new_branch_and_opened() {
    let harness = common::Harness::new();
    let gh = harness.root().join("draft-gh");
    let counter = harness.root().join("draft-count");
    write_gh(
        &gh,
        &counter,
        "https://github.com/Acme/Drafted/pull/11",
        0,
        false,
    );
    let repo = dirty_main_with_remote(&harness);
    let daemon = harness.boot_with(|config| {
        config.github.executable = gh.to_string_lossy().into_owned();
    });
    let client = daemon.connect("scenario-pr-draft");
    let events = client.events();
    let workspace = common::add_main_workspace(&client, repo.path());
    let (session_id, _) = common::create_shell_session(&client, &events, workspace);

    assert_eq!(
        client
            .request(Request::SubmitPullRequestDraft {
                session_id,
                draft: draft("agent/draft"),
            })
            .expect("draft ack"),
        Response::Ack
    );
    common::wait_for(&events, common::DEADLINE, |event| {
        matches!(
            event,
            DaemonEvent::PullRequestDraftReady { workspace_id, session_id: from, draft }
                if *workspace_id == workspace && *from == session_id && draft.title == "Draft title"
        )
    })
    .expect("PullRequestDraftReady");

    let stranger = client.request(Request::SubmitPullRequestDraft {
        session_id: domain::SessionId::new(),
        draft: draft("x"),
    });
    assert!(
        matches!(&stranger, Err(client::ClientError::Protocol(error)) if error.code == protocol::ErrorCode::NotFound),
        "a draft names its own session: {stranger:?}"
    );

    client
        .request(Request::CommitAndOpenPullRequest {
            workspace_id: workspace,
            commit: domain::CommitPlan {
                branch: Some("agent/draft".into()),
                message: "Add the draft file".into(),
            },
            title: "Draft title".into(),
            body: "Draft body".into(),
            base: None,
        })
        .expect("open ack");
    common::wait_for(&events, common::DEADLINE, |event| {
        matches!(
            event,
            DaemonEvent::PullRequestOpened { url: Some(url), .. } if url.ends_with("/pull/11")
        )
    })
    .expect("PullRequestOpened");

    assert_eq!(common::checked_out_branch(repo.path()), "agent/draft");
    assert_eq!(
        git_stdout(repo.path(), &["log", "-1", "--format=%s"]),
        "Add the draft file"
    );
    assert_eq!(git_stdout(repo.path(), &["status", "--porcelain"]), "");
    assert_eq!(
        git_stdout(repo.path(), &["rev-parse", "main"]),
        git_stdout(repo.path(), &["rev-parse", "agent/draft~1"]),
        "main did not move"
    );
    assert_eq!(invocation_count(&counter), 1, "gh created the pull request");
}

#[test]
fn a_draft_is_never_committed_to_the_default_branch() {
    let harness = common::Harness::new();
    let gh = harness.root().join("never-gh");
    let counter = harness.root().join("never-count");
    write_gh(
        &gh,
        &counter,
        "https://github.com/Acme/Never/pull/1",
        0,
        false,
    );
    let repo = dirty_main_with_remote(&harness);
    let daemon = harness.boot_with(|config| {
        config.github.executable = gh.to_string_lossy().into_owned();
    });
    let client = daemon.connect("scenario-pr-default");
    let events = client.events();
    let workspace = common::add_main_workspace(&client, repo.path());
    let before = git_stdout(repo.path(), &["rev-parse", "HEAD"]);

    client
        .request(Request::CommitAndOpenPullRequest {
            workspace_id: workspace,
            commit: domain::CommitPlan {
                branch: None,
                message: "Add the draft file".into(),
            },
            title: "Draft title".into(),
            body: String::new(),
            base: None,
        })
        .expect("open ack");
    let refused = common::wait_for(&events, common::DEADLINE, |event| {
        matches!(event, DaemonEvent::PullRequestOpened { .. })
    })
    .expect("PullRequestOpened");
    let DaemonEvent::PullRequestOpened { url, error, .. } = refused else {
        unreachable!()
    };
    assert_eq!(url, None);
    assert!(
        error
            .as_deref()
            .is_some_and(|text| text.contains("default branch")),
        "{error:?}"
    );
    assert_eq!(git_stdout(repo.path(), &["rev-parse", "HEAD"]), before);
    assert_eq!(common::checked_out_branch(repo.path()), "main");
    assert_eq!(invocation_count(&counter), 0, "gh never ran");
}

/// The background session `OpenPullRequestWithAgent` launched.
fn background_agent(events: &flume::Receiver<DaemonEvent>) -> domain::SessionId {
    match common::wait_for(
        events,
        common::DEADLINE,
        |event| matches!(event, DaemonEvent::SessionCreated(session) if session.background),
    ) {
        Some(DaemonEvent::SessionCreated(session)) => session.id,
        other => panic!("no background agent session: {other:?}"),
    }
}

#[test]
fn a_background_agent_drafts_the_pull_request_and_the_daemon_opens_it() {
    let harness = common::Harness::new();
    common::write_fake_agent_cli(harness.bin(), "claude", "forge_claude", "Claude Code 1.0.0");
    let gh = harness.root().join("agent-gh");
    let counter = harness.root().join("agent-count");
    write_gh(
        &gh,
        &counter,
        "https://github.com/Acme/Agent/pull/12",
        0,
        false,
    );
    let repo = dirty_main_with_remote(&harness);
    let daemon = harness.boot_with(|config| {
        config.github.executable = gh.to_string_lossy().into_owned();
    });
    let client = daemon.connect("scenario-pr-agent");
    let events = client.events();
    let workspace = common::add_main_workspace(&client, repo.path());
    let open = Request::OpenPullRequestWithAgent {
        workspace_id: workspace,
        provider_id: domain::AgentProviderId::new("claude"),
        profile_id: None,
        draft: false,
    };
    assert_eq!(
        client.request(open.clone()).expect("open ack"),
        Response::Ack
    );
    assert_eq!(
        client.request(open).expect("coalesced ack"),
        Response::Ack,
        "a second press rides the first run"
    );
    let agent = background_agent(&events);

    client
        .request(Request::SubmitPullRequestDraft {
            session_id: agent,
            draft: draft("agent/background"),
        })
        .expect("draft ack");
    let mut broadcast_draft = false;
    let mut second_agent = false;
    common::wait_for(&events, common::DEADLINE, |event| match event {
        DaemonEvent::PullRequestDraftReady { .. } => {
            broadcast_draft = true;
            false
        }
        DaemonEvent::SessionCreated(session) if session.background => {
            second_agent = true;
            false
        }
        DaemonEvent::PullRequestOpened {
            url: Some(url),
            session_id: Some(from),
            ..
        } => url.ends_with("/pull/12") && *from == agent,
        _ => false,
    })
    .expect("PullRequestOpened");
    assert!(!broadcast_draft, "the worker consumes its agent's draft");
    assert!(!second_agent, "the coalesced press launched nothing");

    assert_eq!(common::checked_out_branch(repo.path()), "agent/background");
    assert_eq!(git_stdout(repo.path(), &["status", "--porcelain"]), "");
    assert_eq!(invocation_count(&counter), 1, "gh created the pull request");
    let Response::Snapshot { sessions, .. } =
        client.request(Request::GetSnapshot).expect("snapshot")
    else {
        panic!("expected snapshot");
    };
    assert!(
        sessions.iter().all(|session| session.id != agent),
        "the agent's session is closed once it hands over"
    );
}

#[test]
fn a_background_agent_that_hands_nothing_over_fails_the_request() {
    let harness = common::Harness::new();
    common::write_fake_agent_cli(harness.bin(), "claude", "forge_claude", "Claude Code 1.0.0");
    let gh = harness.root().join("silent-gh");
    let counter = harness.root().join("silent-count");
    write_gh(
        &gh,
        &counter,
        "https://github.com/Acme/Silent/pull/1",
        0,
        false,
    );
    let repo = dirty_main_with_remote(&harness);
    let daemon = harness.boot_with(|config| {
        config.github.executable = gh.to_string_lossy().into_owned();
        config.pull_requests.timeout_secs = 1;
    });
    let client = daemon.connect("scenario-pr-agent-silent");
    let events = client.events();
    let workspace = common::add_main_workspace(&client, repo.path());
    client
        .request(Request::OpenPullRequestWithAgent {
            workspace_id: workspace,
            provider_id: domain::AgentProviderId::new("claude"),
            profile_id: None,
            draft: false,
        })
        .expect("open ack");
    let agent = background_agent(&events);
    common::wait_for(&events, common::DEADLINE, |event| {
        matches!(
            event,
            DaemonEvent::PullRequestOpened { url: None, error: Some(error), session_id: Some(from), .. }
                if error.contains("no draft") && *from == agent
        )
    })
    .expect("PullRequestOpened with the timeout");
    assert_eq!(invocation_count(&counter), 0, "nothing was opened");
    assert_ne!(
        git_stdout(repo.path(), &["status", "--porcelain"]),
        "",
        "nothing was committed"
    );
}

#[test]
fn a_clean_default_branch_has_nothing_to_propose() {
    let harness = common::Harness::new();
    let repo = test_support::init_repo().expect("git repo");
    let daemon = harness.boot();
    let client = daemon.connect("scenario-pr-agent-clean");
    let workspace = common::add_main_workspace(&client, repo.path());
    let refused = client.request(Request::OpenPullRequestWithAgent {
        workspace_id: workspace,
        provider_id: domain::AgentProviderId::new("claude"),
        profile_id: None,
        draft: false,
    });
    assert!(
        matches!(&refused, Err(client::ClientError::Protocol(error)) if error.code == protocol::ErrorCode::InvalidRequest),
        "{refused:?}"
    );
}

#[test]
fn a_checkout_that_changed_while_the_agent_drafted_is_not_published() {
    let harness = common::Harness::new();
    common::write_fake_agent_cli(harness.bin(), "claude", "forge_claude", "Claude Code 1.0.0");
    let gh = harness.root().join("moved-gh");
    let counter = harness.root().join("moved-count");
    write_gh(
        &gh,
        &counter,
        "https://github.com/Acme/Moved/pull/1",
        0,
        false,
    );
    let repo = dirty_main_with_remote(&harness);
    let daemon = harness.boot_with(|config| {
        config.github.executable = gh.to_string_lossy().into_owned();
    });
    let client = daemon.connect("scenario-pr-agent-moved");
    let events = client.events();
    let workspace = common::add_main_workspace(&client, repo.path());
    client
        .request(Request::OpenPullRequestWithAgent {
            workspace_id: workspace,
            provider_id: domain::AgentProviderId::new("claude"),
            profile_id: None,
            draft: false,
        })
        .expect("open ack");
    let agent = background_agent(&events);

    fs::write(repo.path().join("unrelated.txt"), "typed meanwhile\n").expect("edit meanwhile");
    client
        .request(Request::SubmitPullRequestDraft {
            session_id: agent,
            draft: draft("agent/moved"),
        })
        .expect("draft ack");
    common::wait_for(&events, common::DEADLINE, |event| {
        matches!(
            event,
            DaemonEvent::PullRequestOpened { url: None, error: Some(error), .. }
                if error.contains("checkout changed")
        )
    })
    .expect("PullRequestOpened refusing the changed checkout");
    assert_eq!(invocation_count(&counter), 0, "nothing was opened");
    assert_eq!(
        common::checked_out_branch(repo.path()),
        "main",
        "no branch was made"
    );
}

fn single_response(repository: &str, number: u32, state: &str) -> String {
    format!(
        r#"{{"data":{{"viewer":{{"login":"ForgeUser"}},"repository":{{"nameWithOwner":"{repository}","pullRequest":{{"state":"{state}","number":{number},"title":"Linked","url":"https://github.com/{repository}/pull/{number}","body":"From a link","isDraft":false,"createdAt":"2026-08-24T10:00:00Z","updatedAt":"2026-08-25T12:00:00Z","author":{{"login":"someone"}},"repository":{{"nameWithOwner":"{repository}"}},"baseRefName":"main","headRefName":"linked","additions":1,"deletions":0,"changedFiles":1,"comments":{{"totalCount":0}},"labels":{{"nodes":[]}},"assignees":{{"nodes":[]}},"reviewRequests":{{"nodes":[]}}}}}}}}}}"#
    )
}

fn fetched_event(events: &flume::Receiver<DaemonEvent>) -> Option<String> {
    let event = common::wait_for(events, common::DEADLINE, |event| {
        matches!(event, DaemonEvent::PullRequestFetched { .. })
    })
    .expect("PullRequestFetched event");
    let DaemonEvent::PullRequestFetched { error, .. } = event else {
        unreachable!()
    };
    error
}

#[test]
fn a_pull_request_read_by_url_is_pinned_across_refreshes_and_only_open_ones_are_kept() {
    let harness = common::Harness::new();
    let gh = harness.root().join("fake-gh");
    let counter = harness.root().join("gh-count");
    write_gh(
        &gh,
        &counter,
        &single_response("Acme/Forge", 99, "OPEN"),
        0,
        true,
    );
    let repo = github_repo(harness.root(), "forge", "git@github.com:Acme/Forge.git");
    let daemon = harness.boot_with(|config| {
        config.github.executable = gh.to_string_lossy().into_owned();
    });
    let client = daemon.connect("scenario-pr-by-url");
    let events = client.events();
    common::add_main_workspace(&client, &repo);

    let refused = client.request(Request::FetchPullRequest {
        host: "gitlab.com".into(),
        repository: "acme/forge".into(),
        number: 99,
    });
    assert!(
        refused.is_err(),
        "an unlisted host is refused before gh runs"
    );
    for _ in 0..2 {
        assert_eq!(
            client
                .request(Request::FetchPullRequest {
                    host: "github.com".into(),
                    repository: "acme/forge".into(),
                    number: 99,
                })
                .expect("fetch ack"),
            Response::Ack
        );
    }
    let state = pull_request_event(&events, common::DEADLINE);
    assert_eq!(fetched_event(&events), None);
    assert_eq!(invocation_count(&counter), 1, "the second read coalesced");
    let linked = state
        .pull_requests
        .iter()
        .find(|pr| pr.number == 99)
        .expect("pinned into the broadcast state");
    assert!(linked.project_id.is_some(), "matched to the Forge project");

    // The listing answers without #99; the pin keeps it.
    write_gh(&gh, &counter, &complete_response("Acme/Forge"), 0, false);
    client
        .request(Request::RefreshPullRequests { force: true })
        .expect("refresh ack");
    let refreshed = pull_request_event(&events, common::DEADLINE);
    let numbers: Vec<u32> = refreshed.pull_requests.iter().map(|pr| pr.number).collect();
    assert!(
        numbers.contains(&17) && numbers.contains(&99),
        "{numbers:?}"
    );
    let Response::Snapshot { pull_requests, .. } =
        client.request(Request::GetSnapshot).expect("snapshot")
    else {
        panic!("expected snapshot");
    };
    assert!(pull_requests.pull_requests.iter().any(|pr| pr.number == 99));

    write_gh(
        &gh,
        &counter,
        &single_response("Acme/Forge", 98, "MERGED"),
        0,
        false,
    );
    client
        .request(Request::FetchPullRequest {
            host: "github.com".into(),
            repository: "Acme/Forge".into(),
            number: 98,
        })
        .expect("fetch ack");
    let error = fetched_event(&events).expect("a merged pull request is refused");
    assert!(error.contains("merged"), "{error}");
}
