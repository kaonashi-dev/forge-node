//! Settings › General, as Tauri's `General`: what is running, where its
//! configuration lives, and the daemon's controls.

use rune::{El, col, el, row, text};
use rune_kit::{Button, ButtonVariant, ControlSize, Kit};

use super::{group, notice, setting_row};
use crate::app::{AppState, LinkStatus};

fn plural(count: usize, noun: &str) -> String {
    format!("{count} {noun}{}", if count == 1 { "" } else { "s" })
}

/// What a factory reset takes, as Tauri's confirmation counts it.
#[must_use]
pub fn reset_description(projects: usize, live: usize, worktrees: usize) -> String {
    format!(
        "{}, {}, and {} managed by Forge Node will be removed. Dirty managed worktrees are \
         deleted. Forge Node then reconnects as a fresh install.",
        plural(projects, "project"),
        plural(live, "live session"),
        plural(worktrees, "worktree"),
    )
}

fn fact(k: &Kit, name: &str, value: String) -> El {
    row()
        .gap(12.0)
        .child(
            text(name.to_string())
                .font(k.ty.small)
                .color(k.c.text_faint)
                .w(120.0),
        )
        .child(text(value).font(k.ty.small).color(k.c.text_base))
}

fn path_line(k: &Kit, name: &str, note: &str, value: Option<String>) -> El {
    col()
        .gap(2.0)
        .child(
            row()
                .gap(8.0)
                .items_center()
                .child(
                    text(name.to_string())
                        .font(k.ty.small_medium)
                        .color(k.c.text_base),
                )
                .child(
                    text(note.to_string())
                        .font(k.ty.text_xsmall)
                        .color(k.c.text_faint),
                ),
        )
        .child(
            text(value.unwrap_or_else(|| "unavailable until the daemon answers".into()))
                .font(k.ty.mono)
                .color(k.c.text_muted)
                .truncate()
                .min_w(0.0),
        )
}

fn button(
    k: &Kit,
    label: &str,
    key: &str,
    variant: ButtonVariant,
    press: impl Fn(&mut AppState) + 'static,
) -> El {
    Button::new(label.to_string())
        .variant(variant)
        .control_size(ControlSize::Small)
        .on_press(press)
        .build(k)
        .key(key.to_string())
}

pub fn section(s: &AppState, k: &Kit) -> El {
    let (live, total, projects, checkouts) = s.store.as_ref().map_or((0, 0, 0, 0), |store| {
        (
            store
                .sessions
                .iter()
                .filter(|session| session.state.is_active())
                .count(),
            store.sessions.len(),
            store.projects.len(),
            store.workspaces.len(),
        )
    });
    let overview = group(k, "general-overview", "Overview", None)
        .child(fact(
            k,
            "Version",
            format!("forge-ui v{}", env!("CARGO_PKG_VERSION")),
        ))
        .child(fact(k, "Sessions", format!("{live} live of {total}")))
        .child(fact(
            k,
            "Projects",
            format!("{projects} in {}", plural(checkouts, "checkout")),
        ));

    let paths = s.storage.paths.as_ref().map(|(paths, _)| paths);
    let config_dir =
        paths.and_then(|paths| paths.config_file.parent().map(|dir| dir.to_path_buf()));
    let logs = paths.map(|paths| paths.logs.clone());
    let mut reveal = row().gap(6.0);
    if let Some(dir) = config_dir {
        reveal = reveal.child(button(
            k,
            "Reveal config folder",
            "general-reveal-config",
            ButtonVariant::Outline,
            move |_s| crate::storage::reveal(&dir),
        ));
    }
    if let Some(dir) = logs.clone() {
        reveal = reveal.child(button(
            k,
            "Reveal logs",
            "general-reveal-logs",
            ButtonVariant::Outline,
            move |_s| crate::storage::reveal(&dir),
        ));
    }
    let config = group(
        k,
        "general-config",
        "Configuration & logs",
        Some("Edit config.toml in your editor, then restart Forge Node to apply changes."),
    )
    .child(path_line(
        k,
        "config.toml",
        "terminal, sessions, worktrees, daemon",
        paths.map(|paths| paths.config_file.display().to_string()),
    ))
    .child(path_line(
        k,
        "logs",
        "daemon.<date>.log, last 5 days",
        logs.map(|dir| dir.display().to_string()),
    ))
    .child(reveal);

    let (dot, title) = match s.status {
        LinkStatus::Connected => (k.c.success.icon, "Connected"),
        LinkStatus::Connecting => (k.c.icon_faint, "Connecting…"),
        LinkStatus::Reconnecting => (k.c.danger.icon, "Disconnected"),
    };
    let daemon = group(
        k,
        "general-daemon",
        "Daemon",
        Some("The daemon owns every PTY, so stopping it takes all running sessions with it."),
    )
    .child(
        row()
            .key("daemon-status")
            .role("status")
            .label(format!("{title}: {}", s.status_detail))
            .gap(8.0)
            .items_center()
            .child(el().size(8.0, 8.0).radius(4.0).fill(dot))
            .child(
                col()
                    .min_w(0.0)
                    .child(text(title).font(k.ty.small_medium).color(k.c.text_base))
                    .child(
                        text(s.status_detail.clone())
                            .font(k.ty.text_xsmall)
                            .color(k.c.text_faint)
                            .truncate(),
                    ),
            ),
    )
    .child(
        row()
            .gap(6.0)
            .flex_wrap()
            .child(button(
                k,
                "Reconnect",
                "daemon-reconnect",
                ButtonVariant::Outline,
                AppState::request_reconnect,
            ))
            .child(button(
                k,
                "Reload from the daemon",
                "daemon-reload",
                ButtonVariant::Outline,
                AppState::request_refresh,
            ))
            .child(button(
                k,
                "Stop the daemon",
                "daemon-stop",
                ButtonVariant::Outline,
                |s| s.stop_daemon(false),
            ))
            .child(button(
                k,
                "Stop and kill sessions",
                "daemon-stop-kill",
                ButtonVariant::Danger,
                |s| s.stop_daemon(true),
            )),
    );

    let danger = group(
        k,
        "general-danger",
        "Danger zone",
        Some(
            "Reset Forge Node's own state without modifying your repositories, configuration \
             file, or logs.",
        ),
    )
    .child(setting_row(
        k,
        "Reset to factory defaults",
        Some(
            "Stops every session, forgets all projects and preferences, and force-removes \
             worktrees created by Forge Node. Branches, commits, config.toml and logs stay in \
             place.",
        ),
        button(
            k,
            "Reset Forge Node",
            "general-reset",
            ButtonVariant::Danger,
            |s| s.settings.confirm_reset = true,
        ),
    ));

    col()
        .key("general")
        .w_full()
        .gap(24.0)
        .child(overview)
        .child(config)
        .child(daemon)
        .children(notice(s, k))
        .child(danger)
}

impl AppState {
    /// Stops the daemon; `kill_sessions` takes the live sessions down with
    /// it instead of letting the daemon refuse while any run.
    pub fn stop_daemon(&mut self, kill_sessions: bool) {
        if let Some(connection) = &self.connection {
            let _ = connection.stop_daemon(kill_sessions);
        }
    }

    pub fn confirm_factory_reset(&mut self) {
        self.settings.confirm_reset = false;
        if let Some(connection) = &self.connection {
            let _ = connection.factory_reset();
        }
    }
}

pub fn reset_dialog(s: &AppState, k: &Kit, layer: &mut Vec<El>) {
    if !s.settings.confirm_reset {
        return;
    }
    let (projects, live, worktrees) = s.store.as_ref().map_or((0, 0, 0), |store| {
        (
            store.projects.len(),
            store
                .sessions
                .iter()
                .filter(|session| session.state.is_active())
                .count(),
            store
                .workspaces
                .iter()
                .filter(|workspace| workspace.managed_by_app)
                .count(),
        )
    });
    let cancel = Button::new("Cancel")
        .variant(ButtonVariant::Ghost)
        .on_press(|s: &mut AppState| s.settings.confirm_reset = false)
        .build(k)
        .key("reset-cancel");
    let reset = Button::new("Reset Forge Node")
        .variant(ButtonVariant::Danger)
        .on_press(AppState::confirm_factory_reset)
        .build(k)
        .key("reset-confirm");
    rune_kit::Dialog::new("factory-reset", "Reset Forge Node to factory defaults?")
        .description(reset_description(projects, live, worktrees))
        .open(true)
        .fit()
        .width(460.0)
        .on_close(|s: &mut AppState| s.settings.confirm_reset = false)
        .child(
            row()
                .gap(8.0)
                .child(el().grow(1.0))
                .child(cancel)
                .child(reset),
        )
        .build(k, layer);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_reset_confirmation_counts_what_goes() {
        assert_eq!(
            reset_description(1, 0, 2),
            "1 project, 0 live sessions, and 2 worktrees managed by Forge Node will be \
             removed. Dirty managed worktrees are deleted. Forge Node then reconnects as a \
             fresh install."
        );
    }
}
