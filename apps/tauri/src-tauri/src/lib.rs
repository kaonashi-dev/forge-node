//! Tauri host for the Forge Node shell.
//!
//! The WebView never talks to the daemon. This crate owns a dedicated runtime
//! thread that holds `client::Client` (blocking UDS + reader thread) and emits
//! coalesced snapshots.

mod commands;
mod daemon;
mod menus;
mod open;
mod paths;
mod runtime;
mod updates;

use tauri::{Emitter, Manager};

use runtime::Runtime;

pub use daemon::locator::{connect_or_spawn, Locator, CLIENT_VERSION};
/// The terminal wire encoder, so the probe binary can measure what the canvas
/// is actually sent rather than a re-implementation of it.
pub use runtime::cells;

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
            let menu = menus::app_menus(app.handle())?;
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
