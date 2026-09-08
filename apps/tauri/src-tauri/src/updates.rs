//! In-app updates: check, download, apply.
//!
//! This module owns the update *policy*; the WebView owns none of it and is
//! told what happened through the `shell:update` event, the same way it learns
//! about menu clicks. What it does not own: the daemon. See [`UpdateKind`].

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tauri_plugin_updater::UpdaterExt as _;

/// How long a check stays fresh. A background check that finds nothing is not
/// news, so the cost of being wrong here is one HTTP request.
const CHECK_INTERVAL_SECS: u64 = 6 * 60 * 60;

/// Delay before the launch check, so it never competes with the daemon
/// handshake and the first snapshot for the user's first second.
const LAUNCH_DELAY_SECS: u64 = 30;

/// Unix seconds of the last completed check, `0` until the first one.
static LAST_CHECK: AtomicU64 = AtomicU64::new(0);

/// What a release costs the user.
///
/// The GUI and the daemon are two processes and only the GUI is replaced by an
/// update: a relaunched GUI reconnects to the daemon that is still running, so
/// live sessions, PTYs and scrollback survive. That holds exactly as long as
/// the two still speak the same protocol — `PROTOCOL_VERSION` requires
/// equality, and a daemon cannot be replaced without killing every PTY it owns
/// (`docs/architecture.md`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum UpdateKind {
    /// Same protocol: relaunch the GUI and nothing else is disturbed.
    Soft,
    /// The protocol moved: the running daemon cannot serve the new GUI.
    Hard,
}

#[derive(Clone, Debug, Serialize)]
pub struct UpdateInfo {
    pub version: String,
    pub current_version: String,
    pub notes: String,
    pub kind: UpdateKind,
}

/// What the WebView renders. One event, one payload, no partial states.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum UpdateState {
    Checking,
    UpToDate,
    Available(UpdateInfo),
    Downloading { percent: u8 },
    Installing,
    Failed { message: String },
}

fn emit(app: &AppHandle, state: UpdateState) {
    let _ = app.emit("shell:update", state);
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Split the machine-readable header off a release's notes.
///
/// `tauri-plugin-updater` hands us exactly one field we control — the
/// manifest's `notes` — so the protocol version rides in its first line as
/// `protocol: <n>`. Anything else, including a missing line, is read as a
/// protocol we cannot vouch for and classified [`UpdateKind::Hard`]: the cost
/// of guessing "soft" wrongly is a GUI that relaunches into a rejected
/// handshake, and the cost of guessing "hard" wrongly is one extra warning.
fn parse_notes(body: &str) -> (UpdateKind, String) {
    let mut lines = body.lines();
    let Some(first) = lines.next() else {
        return (UpdateKind::Hard, String::new());
    };
    let Some(value) = first.trim().strip_prefix("protocol:") else {
        return (UpdateKind::Hard, body.trim().to_string());
    };
    let rest = lines.collect::<Vec<_>>().join("\n").trim().to_string();
    match value.trim().parse::<u32>() {
        Ok(v) if v == client::PROTOCOL_VERSION => (UpdateKind::Soft, rest),
        _ => (UpdateKind::Hard, rest),
    }
}

/// Ask the endpoint whether a newer release exists.
///
/// `force` skips the [`CHECK_INTERVAL_SECS`] clock — the menu item asks for an
/// answer, so it gets one, including the "you are up to date" the background
/// checks deliberately stay quiet about.
pub async fn check(app: AppHandle, force: bool) -> Result<Option<UpdateInfo>, String> {
    if !force {
        let last = LAST_CHECK.load(Ordering::Relaxed);
        if last != 0 && now_secs().saturating_sub(last) < CHECK_INTERVAL_SECS {
            return Ok(None);
        }
    }
    emit(&app, UpdateState::Checking);

    let updater = app.updater().map_err(|e| e.to_string())?;
    let found = updater.check().await.map_err(|e| e.to_string());
    LAST_CHECK.store(now_secs(), Ordering::Relaxed);

    match found {
        Ok(Some(update)) => {
            let (kind, notes) = parse_notes(update.body.as_deref().unwrap_or_default());
            let info = UpdateInfo {
                version: update.version.clone(),
                current_version: update.current_version.clone(),
                notes,
                kind,
            };
            emit(&app, UpdateState::Available(info.clone()));
            Ok(Some(info))
        }
        Ok(None) => {
            emit(&app, UpdateState::UpToDate);
            Ok(None)
        }
        Err(message) => {
            emit(
                &app,
                UpdateState::Failed {
                    message: message.clone(),
                },
            );
            Err(message)
        }
    }
}

/// Download the pending update and swap the bundle, then relaunch.
///
/// Re-checks rather than carrying the `Update` across two commands: one extra
/// HTTP round trip buys a stateless path, and the alternative is a mutex over
/// a value that is only ever read once.
///
/// A [`UpdateKind::Hard`] release is refused here. Replacing the daemon means
/// stopping it, `forge-daemon` has no shutdown request yet, and a GUI that
/// relaunches into a protocol it cannot speak is worse than one that says so.
pub async fn install(app: AppHandle) -> Result<(), String> {
    let updater = app.updater().map_err(|e| e.to_string())?;
    let Some(update) = updater.check().await.map_err(|e| e.to_string())? else {
        emit(&app, UpdateState::UpToDate);
        return Ok(());
    };

    let (kind, _) = parse_notes(update.body.as_deref().unwrap_or_default());
    if kind == UpdateKind::Hard {
        let message = format!(
            "{} also replaces the runtime, which this build cannot do in place. \
             Download it and install it over the app instead.",
            update.version
        );
        emit(
            &app,
            UpdateState::Failed {
                message: message.clone(),
            },
        );
        return Err(message);
    }

    let total = AtomicU64::new(0);
    let seen = AtomicU64::new(0);
    let last_percent = std::sync::atomic::AtomicU8::new(u8::MAX);
    let bytes = update
        .download(
            |chunk, length| {
                if let Some(length) = length {
                    total.store(length, Ordering::Relaxed);
                }
                let seen = seen.fetch_add(chunk as u64, Ordering::Relaxed) + chunk as u64;
                let percent = match total.load(Ordering::Relaxed) {
                    0 => 0,
                    t => ((seen.min(t) * 100) / t) as u8,
                };
                // The WebView repaints the pill on every event; a percent that
                // has not moved is a repaint nobody asked for.
                if percent != last_percent.swap(percent, Ordering::Relaxed) {
                    emit(&app, UpdateState::Downloading { percent });
                }
            },
            || {},
        )
        .await
        .map_err(|e| e.to_string())?;

    emit(&app, UpdateState::Installing);
    update.install(bytes).map_err(|e| e.to_string())?;

    // The daemon is a detached process and outlives this: the relaunched GUI
    // reconnects to it and the user's sessions are where they left them.
    app.restart();
}

/// Start the background check schedule: once shortly after launch, then on the
/// [`CHECK_INTERVAL_SECS`] clock.
pub fn spawn_schedule(app: AppHandle) {
    // A plain thread rather than an async timer: this crate has no Tokio of its
    // own, and a sleeping thread costs less than a runtime dependency.
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(LAUNCH_DELAY_SECS));
        loop {
            let app = app.clone();
            let _ = tauri::async_runtime::block_on(check(app, false));
            std::thread::sleep(std::time::Duration::from_secs(CHECK_INTERVAL_SECS));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_matching_protocol_line_is_a_soft_update() {
        let (kind, notes) = parse_notes(&format!(
            "protocol: {}\nDrag-reorderable tabs.",
            client::PROTOCOL_VERSION
        ));
        assert_eq!(kind, UpdateKind::Soft);
        assert_eq!(notes, "Drag-reorderable tabs.");
    }

    #[test]
    fn a_different_protocol_is_hard() {
        let (kind, _) = parse_notes(&format!(
            "protocol: {}\nnotes",
            client::PROTOCOL_VERSION + 1
        ));
        assert_eq!(kind, UpdateKind::Hard);
    }

    // A release published without the header is the case that must not be
    // optimistic: unknown protocol reads as hard, and the whole body survives
    // as notes rather than losing its first line to a failed parse.
    #[test]
    fn notes_without_a_header_are_hard_and_keep_every_line() {
        let (kind, notes) = parse_notes("Just a human changelog.\nSecond line.");
        assert_eq!(kind, UpdateKind::Hard);
        assert_eq!(notes, "Just a human changelog.\nSecond line.");
    }

    #[test]
    fn empty_notes_are_hard() {
        assert_eq!(parse_notes("").0, UpdateKind::Hard);
    }
}
