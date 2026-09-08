//! Juva: draft a commit message from a dirty worktree and create the commit.
//!
//! Covers the new protocol surface end to end through a real daemon + client,
//! without needing the GUI or `gh`.

mod common;

use protocol::{DaemonEvent, Request, Response};

#[test]
fn juva_drafts_and_creates_a_local_commit() {
    let harness = common::Harness::new();
    let repo = test_support::init_repo().expect("git repo");
    std::fs::write(repo.path().join("note.txt"), "hello juva\n").expect("write");

    let daemon = harness.boot();
    let client = daemon.connect("scenario-juva");
    let events = client.events();

    let workspace = common::add_main_workspace(&client, repo.path());

    let Response::ChangeContext(ctx) = client
        .request(Request::GetChangeContext {
            workspace_id: workspace,
        })
        .expect("GetChangeContext")
    else {
        panic!("expected ChangeContext");
    };
    assert!(ctx.dirty, "fixture wrote an untracked file");
    assert!(
        ctx.files.iter().any(|f| f.path == "note.txt"),
        "change context lists note.txt: {:?}",
        ctx.files
    );

    // Juva acks when the draft *starts*: with `[juva]` configured it opens a
    // socket, so the text arrives as an event like every other request that
    // does. With it off — which is the default, and what this runs under — the
    // local draft still travels the same path rather than a second one.
    assert!(matches!(
        client
            .request(Request::DraftWithJuva {
                workspace_id: workspace,
                kind: domain::JuvaKind::CommitMessage,
            })
            .expect("DraftWithJuva"),
        Response::Ack
    ));

    let ready = common::wait_for(&events, common::DEADLINE, |event| {
        matches!(event, DaemonEvent::JuvaDraftReady { workspace_id, .. } if *workspace_id == workspace)
    })
    .expect("JuvaDraftReady");
    let DaemonEvent::JuvaDraftReady { draft, .. } = ready else {
        panic!("expected JuvaDraftReady");
    };
    assert_eq!(draft.kind, domain::JuvaKind::CommitMessage);
    assert!(!draft.title.trim().is_empty(), "Juva produced a title");

    let message = if draft.body.trim().is_empty() {
        draft.title.clone()
    } else {
        format!("{}\n\n{}", draft.title, draft.body)
    };
    client
        .request(Request::CreateCommit {
            workspace_id: workspace,
            message,
        })
        .expect("CreateCommit");

    let Response::ChangeContext(after) = client
        .request(Request::GetChangeContext {
            workspace_id: workspace,
        })
        .expect("GetChangeContext after commit")
    else {
        panic!("expected ChangeContext");
    };
    assert!(!after.dirty, "working tree clean after Juva commit");
}
