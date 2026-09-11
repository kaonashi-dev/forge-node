//! Tauri host for the Forge Node shell.
//!
//! The WebView never talks to the daemon. This crate owns a dedicated runtime
//! thread that holds `client::Client` (blocking UDS + reader thread) and emits
//! coalesced snapshots.

mod commands;
mod daemon;
mod open;
mod paths;
mod runtime;
mod updates;

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{Emitter, Manager};

use runtime::Runtime;

pub use daemon::locator::{connect_or_spawn, Locator, CLIENT_VERSION};
/// The terminal wire encoder, so the probe binary can measure what the canvas
/// is actually sent rather than a re-implementation of it.
pub use runtime::cells;

fn item(app: &tauri::AppHandle, id: &str, label: &str) -> tauri::Result<MenuItem<tauri::Wry>> {
    MenuItem::with_id(app, id, label, true, None::<&str>)
}

pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(|app| {
            let menu = app_menus(app.handle())?;
            app.set_menu(menu)?;
            app.on_menu_event(|app, event| {
                let _ = app.emit("shell:menu", event.id().0.as_str());
            });

            let runtime = Runtime::start(app.handle().clone());
            app.manage(runtime);

            updates::spawn_schedule(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::host_status,
            commands::connect,
            commands::reconnect,
            commands::send_runtime_command,
            commands::send_workbench_command,
            commands::pick_directory,
            commands::pick_file,
            commands::config_paths,
            commands::check_for_update,
            commands::install_update,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Forge Node host");
}

fn app_menus(app: &tauri::AppHandle) -> tauri::Result<Menu<tauri::Wry>> {
    let forge = Submenu::with_items(
        app,
        "Forge Node",
        true,
        &[
            &item(app, "about", "About Forge Node")?,
            &item(app, "check_for_update", "Check for Updates…")?,
            &PredefinedMenuItem::separator(app)?,
            &item(app, "open_settings", "Settings…")?,
            &PredefinedMenuItem::separator(app)?,
            #[cfg(target_os = "macos")]
            &PredefinedMenuItem::services(app, None)?,
            #[cfg(target_os = "macos")]
            &PredefinedMenuItem::separator(app)?,
            #[cfg(target_os = "macos")]
            &PredefinedMenuItem::hide(app, None)?,
            #[cfg(target_os = "macos")]
            &PredefinedMenuItem::hide_others(app, None)?,
            #[cfg(target_os = "macos")]
            &PredefinedMenuItem::show_all(app, None)?,
            #[cfg(target_os = "macos")]
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::quit(app, None)?,
        ],
    )?;

    let file = Submenu::with_items(
        app,
        "File",
        true,
        &[
            &item(app, "new_terminal", "New Terminal")?,
            &item(app, "new_agent", "New Agent")?,
            &item(app, "new_worktree", "New Worktree")?,
            &PredefinedMenuItem::separator(app)?,
            &item(app, "close_session", "Close Session")?,
        ],
    )?;

    // Native clipboard items let WKWebView deliver paste/copy events to the
    // focused input; the terminal handles its canvas selection in that event.
    let edit = Submenu::with_items(
        app,
        "Edit",
        true,
        &[
            &PredefinedMenuItem::undo(app, None)?,
            &PredefinedMenuItem::redo(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::cut(app, None)?,
            &PredefinedMenuItem::copy(app, None)?,
            &PredefinedMenuItem::paste(app, None)?,
            &PredefinedMenuItem::select_all(app, None)?,
        ],
    )?;

    let view = Submenu::with_items(
        app,
        "View",
        true,
        &[
            &item(app, "open_command_palette", "Command Palette…")?,
            &item(app, "open_file_palette", "Open File…")?,
            &PredefinedMenuItem::separator(app)?,
            &item(app, "toggle_sidebar", "Toggle Sidebar")?,
            &item(app, "toggle_projects", "Projects")?,
            &item(app, "toggle_pull_requests", "Pull Requests")?,
            &item(app, "toggle_files", "Files")?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::fullscreen(app, None)?,
        ],
    )?;

    let window = Submenu::with_items(
        app,
        "Window",
        true,
        &[
            &PredefinedMenuItem::minimize(app, None)?,
            #[cfg(target_os = "macos")]
            &PredefinedMenuItem::maximize(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &item(app, "next_session", "Next Session")?,
            &item(app, "previous_session", "Previous Session")?,
        ],
    )?;

    let help = Submenu::with_items(
        app,
        "Help",
        true,
        &[&item(app, "about", "About Forge Node")?],
    )?;

    Menu::with_items(app, &[&forge, &file, &edit, &view, &window, &help])
}
