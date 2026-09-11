//! ADR-012: list / read / write / search through the daemon, with a revision
//! precondition so an agent write cannot be overwritten in silence.

mod common;

use std::fs;

use protocol::{ErrorCode, Request, Response};

#[test]
fn list_read_write_and_revision_conflict() {
    let harness = common::Harness::new();
    let repo = test_support::init_repo().expect("git repo");
    fs::write(repo.path().join("src.rs"), "fn main() {}\n").unwrap();
    fs::create_dir_all(repo.path().join("lib")).unwrap();
    fs::write(repo.path().join("lib/mod.rs"), "pub fn x() {}\n").unwrap();

    let daemon = harness.boot();
    let client = daemon.connect("files");
    let workspace = common::add_main_workspace(&client, repo.path());

    let Response::FileTree(tree) = client
        .request(Request::ListFiles {
            workspace_id: workspace,
        })
        .expect("ListFiles")
    else {
        panic!("expected FileTree");
    };
    let paths: Vec<_> = tree.entries.iter().map(|e| e.path.as_str()).collect();
    assert!(paths.contains(&"src.rs"), "got {paths:?}");
    assert!(paths.contains(&"lib/mod.rs"), "got {paths:?}");

    let Response::FileContents(first) = client
        .request(Request::ReadFile {
            workspace_id: workspace,
            path: "src.rs".into(),
        })
        .expect("ReadFile")
    else {
        panic!("expected FileContents");
    };
    assert_eq!(first.text, "fn main() {}\n");
    assert_eq!(first.language, "rust");
    assert!(!first.revision.is_empty());

    client
        .request(Request::WriteFile {
            workspace_id: workspace,
            path: "src.rs".into(),
            text: "fn main() { /* edited */ }\n".into(),
            expected_revision: first.revision.clone(),
        })
        .expect("WriteFile");
    assert_eq!(
        fs::read_to_string(repo.path().join("src.rs")).unwrap(),
        "fn main() { /* edited */ }\n"
    );

    // An agent writes behind our back: the stale revision must be refused.
    fs::write(repo.path().join("src.rs"), "fn main() { /* agent */ }\n").unwrap();
    let err = client
        .request(Request::WriteFile {
            workspace_id: workspace,
            path: "src.rs".into(),
            text: "fn main() { /* mine */ }\n".into(),
            expected_revision: first.revision,
        })
        .expect_err("stale write must fail");
    match err {
        client::ClientError::Protocol(p) => {
            assert_eq!(p.code, ErrorCode::PreconditionFailed);
        }
        other => panic!("expected ProtocolError, got {other:?}"),
    }

    let Response::SearchResults(by_name) = client
        .request(Request::SearchFiles {
            workspace_id: workspace,
            query: "modrs".into(),
            kind: domain::SearchKind::Name,
            limit: Some(20),
        })
        .expect("SearchFiles name")
    else {
        panic!("expected SearchResults");
    };
    assert_eq!(by_name.matches.len(), 1);
    assert_eq!(by_name.matches[0].path, "lib/mod.rs");

    let Response::SearchResults(by_content) = client
        .request(Request::SearchFiles {
            workspace_id: workspace,
            query: "agent".into(),
            kind: domain::SearchKind::Content,
            limit: Some(20),
        })
        .expect("SearchFiles content")
    else {
        panic!("expected SearchResults");
    };
    assert!(
        by_content
            .matches
            .iter()
            .any(|m| m.path == "src.rs" && m.text.contains("agent")),
        "got {:?}",
        by_content.matches
    );
}

#[test]
fn path_escape_is_refused() {
    let harness = common::Harness::new();
    let repo = test_support::init_repo().expect("git repo");
    let daemon = harness.boot();
    let client = daemon.connect("files-escape");
    let workspace = common::add_main_workspace(&client, repo.path());

    let err = client
        .request(Request::ReadFile {
            workspace_id: workspace,
            path: "../outside".into(),
        })
        .expect_err("escape must fail");
    match err {
        client::ClientError::Protocol(p) => {
            assert_eq!(p.code, ErrorCode::InvalidRequest);
        }
        other => panic!("expected ProtocolError, got {other:?}"),
    }
}

#[test]
fn image_read_serves_images_and_nothing_else() {
    let harness = common::Harness::new();
    let repo = test_support::init_repo().expect("git repo");
    fs::create_dir_all(repo.path().join("docs")).unwrap();
    fs::write(repo.path().join("docs/logo.png"), [0x89, b'P', b'N', b'G']).unwrap();
    fs::write(repo.path().join(".env"), "SECRET=1\n").unwrap();
    let daemon = harness.boot();
    let client = daemon.connect("files-image");
    let workspace = common::add_main_workspace(&client, repo.path());

    let image = client
        .read_image(workspace, "docs/logo.png")
        .expect("ReadImage");
    assert_eq!(image.path, "docs/logo.png");
    assert_eq!(image.mime, "image/png");
    assert_eq!(image.data, "iVBORw==");

    let err = client
        .read_image(workspace, ".env")
        .expect_err("a non-image must be refused");
    assert_eq!(refusal_code(err), ErrorCode::InvalidRequest);
}

/// Every refusal in this file arrives as a `ProtocolError` inside a
/// `ClientError`; only its code is ever asserted on.
fn refusal_code(error: client::ClientError) -> ErrorCode {
    match error {
        client::ClientError::Protocol(p) => p.code,
        other => panic!("expected ProtocolError, got {other:?}"),
    }
}

/// A11: create, rename and delete a path through the daemon (ADR-012).
///
/// The GUI never touches a checkout with `std::fs`, so every one of these has
/// to work over the socket — and every refusal has to come back as a refusal
/// rather than as a half-done mutation.
#[test]
fn create_rename_and_delete_paths() {
    let harness = common::Harness::new();
    let repo = test_support::init_repo().expect("git repo");
    fs::write(repo.path().join("keep.rs"), "keep\n").unwrap();

    let daemon = harness.boot();
    let client = daemon.connect("paths");
    let workspace = common::add_main_workspace(&client, repo.path());

    // Create makes the directories above it, so "new file in a folder that
    // does not exist yet" is one request rather than two.
    client
        .request(Request::CreatePath {
            workspace_id: workspace,
            path: "src/nested/new.rs".into(),
            directory: false,
        })
        .expect("CreatePath");
    assert!(repo.path().join("src/nested/new.rs").is_file());

    client
        .request(Request::CreatePath {
            workspace_id: workspace,
            path: "docs".into(),
            directory: true,
        })
        .expect("CreatePath directory");
    assert!(repo.path().join("docs").is_dir());

    // Refuses to overwrite: there is no revision to condition a create on, so
    // this is the only answer that cannot lose an agent's work.
    let err = client
        .request(Request::CreatePath {
            workspace_id: workspace,
            path: "keep.rs".into(),
            directory: false,
        })
        .expect_err("create over an existing file must be refused");
    assert_eq!(refusal_code(err), ErrorCode::InvalidRequest);
    assert_eq!(
        fs::read_to_string(repo.path().join("keep.rs")).unwrap(),
        "keep\n"
    );

    // A path that leaves the checkout never reaches the filesystem.
    let escaped = client
        .request(Request::CreatePath {
            workspace_id: workspace,
            path: "../outside.rs".into(),
            directory: false,
        })
        .expect_err("path escaping the workspace must be refused");
    assert_eq!(refusal_code(escaped), ErrorCode::InvalidRequest);

    client
        .request(Request::RenamePath {
            workspace_id: workspace,
            from: "src/nested/new.rs".into(),
            to: "src/renamed.rs".into(),
        })
        .expect("RenamePath");
    assert!(!repo.path().join("src/nested/new.rs").exists());
    assert!(repo.path().join("src/renamed.rs").is_file());

    let occupied = client
        .request(Request::RenamePath {
            workspace_id: workspace,
            from: "src/renamed.rs".into(),
            to: "keep.rs".into(),
        })
        .expect_err("rename onto an existing path must be refused");
    assert_eq!(refusal_code(occupied), ErrorCode::InvalidRequest);
    assert_eq!(
        fs::read_to_string(repo.path().join("keep.rs")).unwrap(),
        "keep\n"
    );

    // A directory goes recursively; the checkout itself never does.
    client
        .request(Request::DeletePath {
            workspace_id: workspace,
            path: "src".into(),
        })
        .expect("DeletePath directory");
    assert!(!repo.path().join("src").exists());

    for root in ["", ".", "/"] {
        let err = client
            .request(Request::DeletePath {
                workspace_id: workspace,
                path: root.into(),
            })
            .expect_err("deleting the workspace root must be refused");
        assert_eq!(refusal_code(err), ErrorCode::InvalidRequest, "for {root:?}");
    }
    assert!(repo.path().join("keep.rs").is_file());
}

/// A tracked path that has been deleted is gone from the listing.
///
/// `git ls-files --cached` answers from the index, and nothing that removes a
/// file updates the index: `DeletePath` on a committed folder left every file
/// under it named by the next `ListFiles`, so the Files panel went on painting
/// a folder that was not on disk. The same held for a `plan.md` an agent had
/// removed in its own terminal.
#[test]
fn listing_drops_tracked_paths_deleted_from_the_worktree() {
    let harness = common::Harness::new();
    let repo = test_support::init_repo().expect("git repo");
    fs::create_dir_all(repo.path().join("docs/orca")).unwrap();
    fs::write(repo.path().join("docs/orca/worktrees.md"), "notes\n").unwrap();
    fs::write(repo.path().join("plan.md"), "notes\n").unwrap();
    repo.commit_file("keep.rs", "fn main() {}\n")
        .expect("commit keep");
    // `commit_file` only stages the one name it wrote, so the nested pair are
    // staged here; what matters is that all three are in the index.
    assert!(std::process::Command::new("git")
        .args(["-C", repo.path().to_str().unwrap(), "add", "-A"])
        .status()
        .unwrap()
        .success());
    assert!(std::process::Command::new("git")
        .args([
            "-C",
            repo.path().to_str().unwrap(),
            "commit",
            "-m",
            "seed the tree"
        ])
        .status()
        .unwrap()
        .success());

    let daemon = harness.boot();
    let client = daemon.connect("deleted-paths");
    let workspace = common::add_main_workspace(&client, repo.path());

    client
        .request(Request::DeletePath {
            workspace_id: workspace,
            path: "plan.md".into(),
        })
        .expect("DeletePath file");
    client
        .request(Request::DeletePath {
            workspace_id: workspace,
            path: "docs".into(),
        })
        .expect("DeletePath directory");

    let Response::FileTree(tree) = client
        .request(Request::ListFiles {
            workspace_id: workspace,
        })
        .expect("ListFiles")
    else {
        panic!("expected FileTree");
    };
    let paths: Vec<_> = tree.entries.iter().map(|e| e.path.as_str()).collect();
    assert!(!paths.contains(&"plan.md"), "got {paths:?}");
    assert!(
        !paths.iter().any(|path| path.starts_with("docs/")),
        "got {paths:?}"
    );
    assert!(paths.contains(&"keep.rs"), "got {paths:?}");
}
