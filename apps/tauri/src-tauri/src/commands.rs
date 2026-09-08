use tauri::{AppHandle, State};
use tauri_plugin_dialog::DialogExt as _;

use crate::runtime::{ConnectedPayload, HostStatus, Runtime, RuntimeCommand, WorkbenchCommand};

#[tauri::command]
pub fn host_status(runtime: State<'_, Runtime>) -> HostStatus {
    runtime.host_status()
}

#[tauri::command]
pub fn connect(runtime: State<'_, Runtime>) -> Option<ConnectedPayload> {
    runtime.snapshot()
}

#[tauri::command]
pub fn reconnect(runtime: State<'_, Runtime>) -> Result<(), String> {
    runtime.send(RuntimeCommand::Reconnect)
}

#[tauri::command]
pub fn send_runtime_command(
    runtime: State<'_, Runtime>,
    command: RuntimeCommand,
) -> Result<(), String> {
    runtime.send(command)
}

/// Workbench reads go to their own worker, never to the thread that carries
/// terminal input: a `git diff` of a large checkout is seconds of subprocess.
#[tauri::command]
pub fn send_workbench_command(runtime: State<'_, Runtime>, command: WorkbenchCommand) {
    runtime.send_workbench(command);
}

/// Ask the platform for a directory, for "Add project".
///
/// The picker is modal and the WebView must not wait on it inside an event
/// handler, so this answers on its own channel and the caller turns the path
/// into an ordinary `AddProject` command — the AddProject command.
///
/// `None` means the user cancelled, which is not an error and gets no notice.
/// Ask the platform for a file, for "share this file between worktrees".
///
/// Same shape as [`pick_directory`], and for the same reason: the picker is
/// modal, so it answers on its own channel and the caller turns the path into
/// an ordinary command instead of awaiting it inside an event handler.
#[tauri::command]
pub async fn pick_file(
    app: AppHandle,
    title: Option<String>,
    directory: Option<String>,
) -> Option<String> {
    let (tx, rx) = flume::bounded(1);
    let mut builder = app
        .dialog()
        .file()
        .set_title(title.as_deref().unwrap_or("Share a file"));
    if let Some(directory) = directory {
        builder = builder.set_directory(std::path::PathBuf::from(directory));
    }
    builder.pick_file(move |path| {
        let _ = tx.send(path);
    });
    let picked = rx.recv_async().await.ok().flatten()?;
    picked
        .into_path()
        .ok()
        .map(|path| path.display().to_string())
}

#[tauri::command]
pub async fn pick_directory(app: AppHandle, title: Option<String>) -> Option<String> {
    let (tx, rx) = flume::bounded(1);
    app.dialog()
        .file()
        .set_title(title.as_deref().unwrap_or("Add project"))
        .pick_folder(move |path| {
            let _ = tx.send(path);
        });
    let picked = rx.recv_async().await.ok().flatten()?;
    // A `FilePath` is a URI on mobile and a path everywhere we ship; only the
    // second can be handed to the daemon, and a picker that returned the first
    // is a platform we do not build for.
    picked
        .into_path()
        .ok()
        .map(|path| path.display().to_string())
}

/// The version and the on-disk files the settings screen reports (§15.1,
/// §15.4), .
///
/// Paths are strings because they are only ever *shown*, and one of them may
/// be missing entirely on a platform with no home directory — the screen says
/// so rather than the host failing to answer.
#[derive(Clone, Debug, serde::Serialize)]
pub struct ConfigPaths {
    /// Host version. The daemon reports its own; the two match in a release.
    pub app_version: String,
    /// `config.toml`, read by both the host and the daemon.
    pub config_file: Option<String>,
    /// Its directory, which is what "Reveal" opens: the file need not exist
    /// yet, and a file manager cannot show what is not there.
    pub config_dir: Option<String>,
    /// Whether `config.toml` exists right now.
    pub config_exists: bool,
    /// `app.log` and `daemon.log` live here.
    pub logs_dir: Option<String>,
}

/// Where Forge reads its configuration from.
///
/// Resolved per call rather than cached at startup: the file can appear or be
/// deleted while the window is open, and the screen is the place that says
/// which it is.
#[tauri::command]
pub fn config_paths() -> ConfigPaths {
    let file = crate::paths::config_file();
    ConfigPaths {
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        config_exists: file.as_deref().is_some_and(std::path::Path::is_file),
        config_file: file.map(|path| path.display().to_string()),
        config_dir: crate::paths::config_dir().map(|path| path.display().to_string()),
        logs_dir: crate::paths::logs_dir().map(|path| path.display().to_string()),
    }
}
