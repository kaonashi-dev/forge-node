//! Settings › Storage, as Tauri's `StorageSection`: where the daemon keeps
//! its config, state and managed worktrees. The paths are the daemon's own
//! resolution (`GetPaths`), immutable per daemon, so one read per connection.
//! Moving data out of a legacy layout stops and restarts the daemon, which
//! only Tauri's host does today; here it is named, not offered.

use std::path::{Path, PathBuf};

use protocol::{ForgePaths, Layout};
use rune::{El, col, row, text};
use rune_kit::{Button, ButtonVariant, ControlSize, Kit};

use crate::app::AppState;

#[derive(Default)]
pub struct Storage {
    /// The connection generation the paths were asked on; a reconnect may be
    /// a different daemon, so it asks again.
    pub asked: Option<u64>,
    pub paths: Option<(ForgePaths, PathBuf)>,
    pub error: Option<String>,
}

/// Managed worktrees still under a legacy `…/worktrees/`, as `legacyWorktreeCount`:
/// `/x/worktrees2` is not under `/x/worktrees`, and an unmanaged checkout is not Forge's.
#[must_use]
pub fn legacy_worktree_count(workspaces: &[domain::Workspace], legacy: Option<&Path>) -> usize {
    let Some(legacy) = legacy else {
        return 0;
    };
    let root = legacy.join("worktrees");
    workspaces
        .iter()
        .filter(|workspace| {
            workspace.managed_by_app
                && Path::new(&workspace.path)
                    .strip_prefix(&root)
                    .is_ok_and(|rest| !rest.as_os_str().is_empty())
        })
        .count()
}

/// Opens `path` in Finder. `open` returns at once; it is reaped on its own
/// thread so neither the UI nor the daemon worker waits on it.
pub fn reveal(path: &Path) {
    let spawned = std::process::Command::new("/usr/bin/open")
        .arg(path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    match spawned {
        Ok(mut child) => {
            let _ = std::thread::Builder::new()
                .name("forge-ui-reveal".into())
                .spawn(move || child.wait());
        }
        Err(error) => eprintln!("[storage] could not open {}: {error}", path.display()),
    }
}

fn path_row(k: &Kit, name: &str, note: Option<&str>, value: Option<&Path>) -> El {
    let mut label = col().w(180.0).child(
        text(name.to_string())
            .font(k.ty.small_medium)
            .color(k.c.text_base),
    );
    if let Some(note) = note {
        label = label.child(
            text(note.to_string())
                .font(k.ty.text_xsmall)
                .color(k.c.text_faint),
        );
    }
    row().gap(12.0).w_full().child(label).child(
        text(value.map_or_else(
            || "unavailable".to_string(),
            |path| path.display().to_string(),
        ))
        .font(k.ty.small)
        .color(k.c.text_muted)
        .grow(1.0)
        .min_w(0.0)
        .truncate(),
    )
}

fn reveal_button(k: &Kit, label: String, key: &str, path: &Path) -> El {
    let path = path.to_path_buf();
    Button::new(label)
        .variant(ButtonVariant::Outline)
        .control_size(ControlSize::Small)
        .on_press(move |_s: &mut AppState| reveal(&path))
        .build(k)
        .key(key.to_string())
}

fn group(k: &Kit, key: &str, title: &str, description: &str) -> El {
    col()
        .key(key.to_string())
        .gap(6.0)
        .child(
            text(title.to_string())
                .font(k.ty.small_medium)
                .color(k.c.text_base),
        )
        .child(
            text(description.to_string())
                .font(k.ty.text_xsmall)
                .color(k.c.text_faint),
        )
}

pub fn section(s: &AppState, k: &Kit) -> El {
    let mut view = col().key("storage").gap(14.0);
    let Some((paths, worktrees_root)) = &s.storage.paths else {
        let note = s.storage.error.clone().unwrap_or_else(|| {
            if s.connection.is_some() {
                "Reading paths…".to_string()
            } else {
                "Connect to the daemon to see where Forge stores data.".to_string()
            }
        });
        return view.child(text(note).font(k.ty.small).color(k.c.text_faint));
    };
    let home_name = paths.home.file_name().map_or_else(
        || "home".to_string(),
        |name| name.to_string_lossy().into_owned(),
    );
    view = view.child(
        group(
            k,
            "storage-data",
            "Forge data",
            "Hidden. Config, database and logs; not a folder to open in an editor.",
        )
        .child(path_row(
            k,
            "home",
            Some("hidden · config, database, logs, runs"),
            Some(&paths.home),
        ))
        .child(path_row(k, "config.toml", None, Some(&paths.config_file)))
        .child(path_row(k, "database", None, Some(&paths.db)))
        .child(path_row(k, "logs", None, Some(&paths.logs)))
        .child(path_row(k, "runs", None, Some(&paths.runs)))
        .child(row().child(reveal_button(
            k,
            format!("Show {home_name} in Finder"),
            "storage-reveal-home",
            &paths.home,
        ))),
    );
    let custom_root = worktrees_root != &paths.default_worktrees_root;
    view = view.child(
        group(
            k,
            "storage-worktrees",
            "Worktrees",
            "Visible to you and to your editor.",
        )
        .child(path_row(
            k,
            "worktrees",
            custom_root.then_some("set by [worktrees] root in config.toml"),
            Some(worktrees_root),
        ))
        .child(row().child(reveal_button(
            k,
            "Show worktrees in Finder".into(),
            "storage-reveal-worktrees",
            worktrees_root,
        ))),
    );
    let workspaces = s
        .store
        .as_ref()
        .map_or(&[][..], |store| store.workspaces.as_slice());
    let left = legacy_worktree_count(workspaces, paths.legacy.as_deref());
    let legacy_note = match paths.layout {
        Layout::Legacy => Some(
            "Your data still lives in the old hidden folder, and new managed worktrees are \
             created there. Move it from Forge Node's Settings › Storage to finish the upgrade."
                .to_string(),
        ),
        Layout::Home if paths.legacy.is_some() && left > 0 => Some(format!(
            "{left} managed worktree{} in the old folder. Move them from Forge Node's \
             Settings › Storage to finish the upgrade.",
            if left == 1 {
                " still lives"
            } else {
                "s still live"
            }
        )),
        _ => None,
    };
    if let Some(note) = legacy_note {
        view = view.child(
            text(note)
                .key("storage-legacy")
                .font(k.ty.small)
                .color(k.c.warning.fg),
        );
    }
    view
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace(path: &str, managed: bool) -> domain::Workspace {
        domain::Workspace {
            id: domain::WorkspaceId::new(),
            project_id: domain::ProjectId::new(),
            kind: domain::WorkspaceKind::GitWorktree,
            path: path.into(),
            branch: None,
            display_name: None,
            managed_by_app: managed,
            created_at: domain::Timestamp::now(),
            status: domain::WorkspaceStatus::default(),
        }
    }

    #[test]
    fn only_forges_own_worktrees_under_the_old_root_count() {
        let all = vec![
            workspace("/x/worktrees/a", true),
            workspace("/x/worktrees2/b", true),
            workspace("/x/worktrees/c", false),
            workspace("/y/worktrees/d", true),
        ];
        assert_eq!(legacy_worktree_count(&all, Some(Path::new("/x/"))), 1);
        assert_eq!(legacy_worktree_count(&all, None), 0);
    }
}
