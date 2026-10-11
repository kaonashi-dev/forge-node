use std::os::unix::net::UnixStream;
use std::sync::{Arc, Mutex};

use domain::SessionId;
use editor_control::{ControlError, DaemonMessage};

use crate::core::Daemon;

pub(super) struct Snapshot {
    request_id: u64,
    pub path: String,
    pub text: Option<String>,
    pub revision: Option<String>,
}

#[derive(Default)]
pub(super) struct Sync {
    pending: Option<Snapshot>,
    owed: bool,
}

impl Sync {
    pub fn refresh(
        &mut self,
        daemon: &Arc<Daemon>,
        session_id: SessionId,
        revision: &str,
        writer: &Arc<Mutex<UnixStream>>,
    ) -> Result<(), ControlError> {
        if self.pending.is_some() {
            self.owed = true;
            return Ok(());
        }
        let operation = daemon.editor_file_operation();
        let Ok((workspace, path)) = daemon.editor_target(session_id) else {
            return Ok(());
        };
        let Ok(root) = daemon.workspace_path(workspace) else {
            return Ok(());
        };
        let (text, loaded) = match fs_service::read_file(&root, &path) {
            Ok(file) if !file.binary && !file.too_large => (Some(file.text), Some(file.revision)),
            Err(fs_service::FsError::NotFound(_)) => (None, None),
            Ok(_) => {
                tracing::debug!(%session_id, "editor disk read skipped: not bounded text");
                return Ok(());
            }
            Err(error) => {
                tracing::debug!(%session_id, %error, "editor disk read skipped");
                return Ok(());
            }
        };
        drop(operation);
        if loaded.as_deref().unwrap_or_default() == revision {
            return Ok(());
        }
        let request_id = daemon.next_editor_request_id();
        super::send(
            writer,
            &DaemonMessage::DiskChanged {
                request_id,
                path: path.clone(),
                text: text.clone(),
                revision: loaded.clone(),
            },
        )?;
        self.pending = Some(Snapshot {
            request_id,
            path,
            text,
            revision: loaded,
        });
        Ok(())
    }

    pub fn finish(
        &mut self,
        daemon: &Daemon,
        session_id: SessionId,
        request_id: u64,
    ) -> Option<Snapshot> {
        if self.pending.as_ref()?.request_id != request_id {
            return None;
        }
        let snapshot = self.pending.take();
        if std::mem::take(&mut self.owed) {
            daemon.refresh_editor_disk(session_id);
        }
        snapshot
    }
}
