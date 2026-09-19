use client::Client;
use domain::{SearchKind, WorkspaceId};
use serde::Serialize;
use tauri::{AppHandle, Emitter as _};

use super::{emit, fail, fail_text};

pub(super) fn watch_files(
    app: &AppHandle,
    client: &Client,
    workspace: WorkspaceId,
    directories: Vec<String>,
    generation: u64,
) {
    match client.watch_files(workspace, directories) {
        Ok(()) => emit(
            app,
            "workbench:watch_ready",
            &WatchReady {
                workspace,
                generation,
            },
        ),
        Err(error) => emit(
            app,
            "workbench:watch_failed",
            &WatchFailure {
                workspace,
                generation,
                error: error.to_string(),
            },
        ),
    }
}

pub(super) fn load_file_tree(
    app: &AppHandle,
    client: &Client,
    workspace: WorkspaceId,
    request_id: String,
) {
    match client.list_files(workspace) {
        Ok(tree) => emit(
            app,
            "workbench:file_tree",
            &IndexPayload {
                workspace,
                request_id,
                tree,
            },
        ),
        Err(error) => emit(
            app,
            "workbench:file_tree_failed",
            &IndexFailure {
                workspace,
                request_id,
                error: error.to_string(),
            },
        ),
    }
}

pub(super) fn load_file_directory(
    app: &AppHandle,
    client: &Client,
    workspace: WorkspaceId,
    path: String,
    request_id: String,
    generation: u64,
) {
    let result = client.list_directory(workspace, path.clone());
    let request = DirectoryRequest {
        workspace,
        path,
        request_id,
        generation,
    };
    match result {
        Ok(listing) => emit(
            app,
            "workbench:directory",
            &DirectoryPayload {
                request,
                entries: listing.entries,
                truncated: listing.truncated,
            },
        ),
        Err(error) => emit(
            app,
            "workbench:directory_failed",
            &DirectoryFailure {
                request,
                message: error.to_string(),
            },
        ),
    }
}

pub(super) fn open_file(app: &AppHandle, client: &Client, workspace: WorkspaceId, path: String) {
    match client.read_file(workspace, path.clone()) {
        Ok(contents) => emit(app, "workbench:file", &(workspace, contents)),
        Err(error) => fail(app, "workbench:file_failed", Some(workspace), &error),
    }
}

pub(super) fn load_image(app: &AppHandle, client: &Client, workspace: WorkspaceId, path: String) {
    match client.read_image(workspace, path.clone()) {
        Ok(image) => emit(app, "workbench:image", &(workspace, image)),
        Err(error) => fail_image(app, workspace, path, &error),
    }
}

pub(super) fn save_file(
    app: &AppHandle,
    client: &Client,
    workspace: WorkspaceId,
    path: String,
    text: String,
    revision: String,
) {
    match client.write_file(workspace, path.clone(), text, revision) {
        // The write answers `Ack`, and the pane needs the new revision to
        // save again — so the re-read follows it here rather than making
        // the WebView ask twice.
        Ok(()) => match client.read_file(workspace, path) {
            Ok(contents) => emit(app, "workbench:file_saved", &(workspace, contents)),
            Err(error) => fail(app, "workbench:file_failed", Some(workspace), &error),
        },
        // A stale revision comes back as `PreconditionFailed`: an agent
        // wrote the same path. The pane re-reads rather than the error
        // carrying the content.
        Err(error) => fail(app, "workbench:save_failed", Some(workspace), &error),
    }
}

pub(super) fn create_path(
    app: &AppHandle,
    client: &Client,
    operation_id: String,
    workspace: WorkspaceId,
    path: String,
    directory: bool,
) {
    let result = client.create_path(workspace, path.clone(), directory);
    emit(
        app,
        "workbench:path_result",
        &PathResult::new(
            operation_id,
            workspace,
            PathOperationKind::Create,
            None,
            Some(path),
            result,
        ),
    );
}

pub(super) fn rename_path(
    app: &AppHandle,
    client: &Client,
    operation_id: String,
    workspace: WorkspaceId,
    from: String,
    to: String,
) {
    let result = client.rename_path(workspace, from.clone(), to.clone());
    emit(
        app,
        "workbench:path_result",
        &PathResult::new(
            operation_id,
            workspace,
            PathOperationKind::Rename,
            Some(from),
            Some(to),
            result,
        ),
    );
}

pub(super) fn delete_path(
    app: &AppHandle,
    client: &Client,
    operation_id: String,
    workspace: WorkspaceId,
    path: String,
) {
    let result = client.delete_path(workspace, path.clone());
    emit(
        app,
        "workbench:path_result",
        &PathResult::new(
            operation_id,
            workspace,
            PathOperationKind::Delete,
            Some(path),
            None,
            result,
        ),
    );
}

pub(super) fn search_files(
    app: &AppHandle,
    client: &Client,
    workspace: WorkspaceId,
    query: String,
    kind: String,
    limit: Option<u32>,
) {
    match parse_search_kind(&kind) {
        Ok(kind) => {
            let (ok, failed) = if kind == SearchKind::Name {
                ("workbench:name_search", "workbench:name_search_failed")
            } else {
                ("workbench:search", "workbench:search_failed")
            };
            match client.search_files(workspace, query, kind, limit) {
                Ok(results) => emit(app, ok, &(workspace, results)),
                Err(error) => fail(app, failed, Some(workspace), &error),
            }
        }
        Err(error) => fail_text(app, "workbench:search_failed", Some(workspace), &error),
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum PathOperationKind {
    Create,
    Rename,
    Delete,
}

#[derive(Debug, Serialize)]
struct PathResult {
    operation_id: String,
    workspace: WorkspaceId,
    kind: PathOperationKind,
    from: Option<String>,
    to: Option<String>,
    success: bool,
    error: Option<String>,
    uncertain: bool,
}

impl PathResult {
    fn new(
        operation_id: String,
        workspace: WorkspaceId,
        kind: PathOperationKind,
        from: Option<String>,
        to: Option<String>,
        result: Result<(), client::ClientError>,
    ) -> Self {
        // Only an acknowledged result or structured refusal establishes the write's outcome.
        let uncertain =
            matches!(&result, Err(error) if !matches!(error, client::ClientError::Protocol(_)));
        Self {
            operation_id,
            workspace,
            kind,
            from,
            to,
            success: result.is_ok(),
            error: result.err().map(|error| error.to_string()),
            uncertain,
        }
    }
}

#[derive(Debug, Serialize)]
struct DirectoryRequest {
    workspace: WorkspaceId,
    path: String,
    request_id: String,
    generation: u64,
}

#[derive(Debug, Serialize)]
struct IndexPayload {
    workspace: WorkspaceId,
    request_id: String,
    tree: domain::FileTree,
}

#[derive(Debug, Serialize)]
struct IndexFailure {
    workspace: WorkspaceId,
    request_id: String,
    error: String,
}

#[derive(Debug, Serialize)]
struct DirectoryPayload {
    #[serde(flatten)]
    request: DirectoryRequest,
    entries: Vec<domain::FileEntry>,
    truncated: bool,
}

#[derive(Debug, Serialize)]
struct DirectoryFailure {
    #[serde(flatten)]
    request: DirectoryRequest,
    message: String,
}

#[derive(Debug, Serialize)]
struct WatchReady {
    workspace: WorkspaceId,
    generation: u64,
}

#[derive(Debug, Serialize)]
struct WatchFailure {
    workspace: WorkspaceId,
    generation: u64,
    error: String,
}

/// A preview asks for several images at once, so a failure names its path.
#[derive(Clone, Debug, Serialize)]
struct ImageFailure {
    workspace: WorkspaceId,
    path: String,
    error: String,
}

fn fail_image(app: &AppHandle, workspace: WorkspaceId, path: String, error: &client::ClientError) {
    tracing::warn!(%error, path, "workbench image read failed");
    let _ = app.emit(
        "workbench:image_failed",
        ImageFailure {
            workspace,
            path,
            error: error.to_string(),
        },
    );
}

fn parse_search_kind(kind: &str) -> Result<SearchKind, String> {
    if kind.eq_ignore_ascii_case("name") {
        Ok(SearchKind::Name)
    } else if kind.eq_ignore_ascii_case("content") {
        Ok(SearchKind::Content)
    } else if kind.eq_ignore_ascii_case("definition") {
        Ok(SearchKind::Definition)
    } else {
        Err(format!("unknown search kind: {kind}"))
    }
}

#[cfg(test)]
mod tests {
    use super::super::WorkbenchCommand;
    use super::*;
    use serde_json::{json, to_value};

    #[test]
    fn acknowledged_mutations_serialize_explicit_outcomes_and_paths() {
        let workspace = WorkspaceId::new();
        for (kind, name, from, to) in [
            (PathOperationKind::Create, "create", None, Some(".agents")),
            (
                PathOperationKind::Rename,
                "rename",
                Some("src"),
                Some("lib"),
            ),
            (PathOperationKind::Delete, "delete", Some("lib"), None),
        ] {
            let payload = PathResult::new(
                "operation-1".into(),
                workspace,
                kind,
                from.map(String::from),
                to.map(String::from),
                Ok(()),
            );
            assert_eq!(
                to_value(payload).unwrap(),
                json!({
                    "operation_id": "operation-1", "workspace": workspace, "kind": name,
                    "from": from, "to": to, "success": true, "error": null, "uncertain": false,
                })
            );
        }
    }

    #[test]
    fn lost_replies_are_uncertain_but_daemon_refusals_are_definite() {
        for (error, uncertain) in [
            (client::ClientError::Disconnected, true),
            (client::ClientError::Timeout, true),
            (
                client::ClientError::Io(std::io::Error::from(std::io::ErrorKind::BrokenPipe)),
                true,
            ),
            (
                client::ClientError::UnexpectedResponse { expected: "Ack" },
                true,
            ),
            (
                client::ClientError::Protocol(client::ProtocolError::invalid_request(
                    "destination exists",
                )),
                false,
            ),
        ] {
            let message = error.to_string();
            let payload = PathResult::new(
                "operation-2".into(),
                WorkspaceId::new(),
                PathOperationKind::Rename,
                Some("src".into()),
                Some("lib".into()),
                Err(error),
            );
            assert!(!payload.success);
            assert_eq!(payload.error.as_deref(), Some(message.as_str()));
            assert_eq!(payload.uncertain, uncertain);
            assert_eq!(payload.operation_id, "operation-2");
        }
    }

    #[test]
    fn directory_results_preserve_root_request_identity_on_both_outcomes() {
        let workspace = WorkspaceId::new();
        let command: WorkbenchCommand = serde_json::from_value(json!({
            "type": "load_file_directory", "workspace": workspace, "path": "",
            "request_id": "root-7", "generation": 7,
        }))
        .unwrap();
        let WorkbenchCommand::LoadFileDirectory {
            workspace,
            path,
            request_id,
            generation,
        } = command
        else {
            panic!("expected directory command");
        };
        let request = || DirectoryRequest {
            workspace,
            path: path.clone(),
            request_id: request_id.clone(),
            generation,
        };
        assert_eq!(
            to_value(DirectoryPayload {
                request: request(),
                entries: vec![],
                truncated: true
            })
            .unwrap(),
            json!({
                "workspace": workspace, "path": "", "request_id": "root-7", "generation": 7,
                "entries": [], "truncated": true,
            })
        );
        assert_eq!(
            to_value(DirectoryFailure {
                request: request(),
                message: "permission denied".into()
            })
            .unwrap(),
            json!({
                "workspace": workspace, "path": "", "request_id": "root-7", "generation": 7,
                "message": "permission denied",
            })
        );
    }

    #[test]
    fn watch_results_echo_the_interest_generation_on_both_outcomes() {
        let workspace = WorkspaceId::new();
        let command: WorkbenchCommand = serde_json::from_value(json!({
            "type": "watch_files", "workspace": workspace, "directories": ["", "src"], "generation": 42,
        })).unwrap();
        let WorkbenchCommand::WatchFiles {
            workspace,
            directories,
            generation,
        } = command
        else {
            panic!("expected watch command");
        };
        assert_eq!(directories, ["", "src"]);
        assert_eq!(
            to_value(WatchReady {
                workspace,
                generation
            })
            .unwrap(),
            json!({
                "workspace": workspace, "generation": 42,
            })
        );
        assert_eq!(
            to_value(WatchFailure {
                workspace,
                generation,
                error: "watch failed".into()
            })
            .unwrap(),
            json!({
                "workspace": workspace, "generation": 42, "error": "watch failed",
            })
        );
    }
}
