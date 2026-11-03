use tauri::{AppHandle, Emitter as _, State};
use tauri_plugin_dialog::DialogExt as _;

use crate::clipboard;
use crate::runtime::input;
use crate::runtime::{
    ConnectedPayload, HostStatus, NoticePayload, Runtime, RuntimeCommand, WorkbenchCommand,
};

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
pub fn send_workbench_command(
    runtime: State<'_, Runtime>,
    command: WorkbenchCommand,
) -> Result<(), String> {
    runtime.send_workbench(command)
}

/// Paste the system clipboard into an open Code editor's buffer.
///
/// Answers whether the host could read the pasteboard; `false` sends the
/// caller to its WebView fallback. The text never crosses the WebView: this
/// reads it here, drops the read when a reconnect moved the connection while
/// it waited, and enqueues the ordinary `PasteEditor` — the same clamped,
/// bracketed, single-undo path the `paste` event takes.
///
/// The read runs on the blocking pool, because a pasteboard type another
/// application produces on request can wait, and nothing here may run on the
/// bridge thread that also carries keystrokes.
#[tauri::command]
pub async fn paste_editor_clipboard(
    app: AppHandle,
    runtime: State<'_, Runtime>,
    session_id: domain::SessionId,
) -> Result<bool, String> {
    if !clipboard::can_read() {
        return Ok(false);
    }
    // `None` is "between connections": there is no editor to paste into, and
    // a generation captured there can never match the one after the read.
    let Some(generation) = runtime.connection_generation() else {
        return Ok(true);
    };
    let read = tauri::async_runtime::spawn_blocking(|| {
        clipboard::read_text(input::MAX_EDITOR_PASTE_BYTES)
    })
    .await
    .map_err(|error| error.to_string())?;
    let Some(read) = read else {
        return Ok(true);
    };
    if runtime.connection_generation() != Some(generation) {
        return Ok(true);
    }
    if let Some(total) = read.total {
        let _ = app.emit(
            "runtime:notice",
            NoticePayload {
                reason: input::paste_cut_notice(read.text.len(), total),
            },
        );
    }
    runtime.send(RuntimeCommand::PasteEditor {
        session_id,
        text: read.text,
        id: 0,
    })?;
    Ok(true)
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

///
/// Paths are strings because they are only ever *shown*, and one of them may
/// be missing entirely on a platform with no home directory — the screen says
/// so rather than the host failing to answer.
#[derive(Clone, Debug, serde::Serialize)]
pub struct ConfigPaths {
    /// Host version. The daemon reports its own; the two match in a release.
    pub app_version: String,
    /// `config.toml`, which the daemon reads; the host only shows where it is.
    pub config_file: Option<String>,
    /// Its directory, which is what "Reveal" opens: the file need not exist
    /// yet, and a file manager cannot show what is not there.
    pub config_dir: Option<String>,
    /// Whether `config.toml` exists right now.
    pub config_exists: bool,
    /// The daemon's `daemon.<date>.log` files live here.
    pub logs_dir: Option<String>,
}

/// Where Forge reads its configuration from.
///
/// Resolved per call rather than cached at startup: the file can appear or be
/// deleted while the window is open, and the screen is the place that says
/// which it is. This is the daemon's own pure resolver, not a second copy of it.
#[tauri::command]
pub fn config_paths() -> ConfigPaths {
    let inputs = client::paths::PathInputs::from_env();
    let resolved = client::paths::resolve(&inputs).ok();
    let file = resolved.as_ref().map(|paths| paths.config_file.clone());
    ConfigPaths {
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        config_exists: file.as_deref().is_some_and(std::path::Path::is_file),
        config_file: file.map(|path| path.display().to_string()),
        config_dir: resolved
            .as_ref()
            .and_then(|paths| paths.config_file.parent())
            .map(|path| path.display().to_string()),
        logs_dir: resolved
            .as_ref()
            .map(|paths| paths.logs.display().to_string()),
    }
}

/// What the migration would do with `--move-worktrees`, without writing
/// anything: the preview names what moves and why the rest is skipped.
#[tauri::command]
pub async fn migrate_home_preview() -> Result<serde_json::Value, String> {
    tauri::async_runtime::spawn_blocking(|| {
        run_migration(
            &["migrate-home", "--dry-run", "--move-worktrees", "--json"],
            std::time::Duration::from_secs(60),
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

/// Delete the old database and agent plugins once no worktree remains there.
#[tauri::command]
pub async fn remove_legacy_data() -> Result<serde_json::Value, String> {
    tauri::async_runtime::spawn_blocking(|| {
        run_migration(
            &["migrate-home", "--remove-legacy", "--json"],
            std::time::Duration::from_secs(60),
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

fn run_migration(args: &[&str], timeout: std::time::Duration) -> Result<serde_json::Value, String> {
    let executable = crate::daemon::locator::Locator::from_env().daemon_executable()?;
    let mut command = std::process::Command::new(executable);
    command.args(args);
    let output = crate::daemon::process::run_bounded(command, timeout)?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if stderr.is_empty() {
            "migrate-home failed".to_string()
        } else {
            stderr
        });
    }
    serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("migrate-home returned invalid JSON: {error}"))
}

/// Force a check, for the `Check for Updates…` menu item.
///
/// The background schedule stays quiet when there is nothing; a check the user
/// asked for reports either way, which is what the `Ok(None)` here means.
#[tauri::command]
pub async fn check_for_update(
    app: AppHandle,
) -> Result<Option<crate::updates::UpdateInfo>, String> {
    crate::updates::check(app, true).await
}

/// Download the pending update, swap the bundle and relaunch. Never returns on
/// success: the process is replaced.
#[tauri::command]
pub async fn install_update(app: AppHandle) -> Result<(), String> {
    crate::updates::install(app).await
}
