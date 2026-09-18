use client::Client;
use tauri::AppHandle;

use super::{emit, fail};

pub(super) fn load_usage_analytics(app: &AppHandle, client: &Client, window_days: Option<u16>) {
    match client.usage_analytics(window_days) {
        Ok(analytics) => emit(app, "workbench:usage", &analytics),
        Err(error) => fail(app, "workbench:usage_failed", None, &error),
    }
}
