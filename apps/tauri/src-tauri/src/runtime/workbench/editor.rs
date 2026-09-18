use client::Client;
use domain::SessionId;
use tauri::AppHandle;

use super::{emit, fail_session};

pub(super) fn load_editor_conflict(app: &AppHandle, client: &Client, session: SessionId) {
    match client.editor_conflict(session) {
        Ok((path, disk, mine)) => emit(
            app,
            "workbench:editor_conflict",
            &(session, path, disk, mine),
        ),
        Err(error) => fail_session(app, "workbench:editor_conflict_failed", session, &error),
    }
}

pub(super) fn reload_editor_buffer(app: &AppHandle, client: &Client, session: SessionId) {
    if let Err(error) = client.reload_editor_buffer(session) {
        fail_session(app, "workbench:editor_conflict_failed", session, &error);
    }
}

pub(super) fn overwrite_editor_buffer(app: &AppHandle, client: &Client, session: SessionId) {
    if let Err(error) = client.overwrite_editor_buffer(session) {
        fail_session(app, "workbench:editor_conflict_failed", session, &error);
    }
}

pub(super) fn set_editor_autosave(
    app: &AppHandle,
    client: &Client,
    session: SessionId,
    autosave: bool,
) {
    if let Err(error) = client.set_editor_autosave(session, autosave) {
        fail_session(app, "workbench:editor_conflict_failed", session, &error);
    }
}
