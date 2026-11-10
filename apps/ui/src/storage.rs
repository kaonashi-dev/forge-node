//! Settings › Storage: where the daemon keeps its config, state and managed
//! worktrees. The paths are the daemon's own resolution (`GetPaths`),
//! immutable per daemon, so one read per connection. Moving data out of a
//! legacy layout stops the daemon, runs `forge-daemon migrate-home`, and
//! reconnects.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use client::{ForgePaths, Layout};
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
    pub migration: Migration,
}

/// What `forge-daemon migrate-home` was asked to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MigrationAction {
    /// The dry run that names what would move.
    Preview,
    /// Move the legacy home into `~/.forge-node`, optionally moving worktrees too.
    Run { move_worktrees: bool },
    /// Delete the old database and agent plugins once nothing lives there.
    RemoveLegacy,
}

impl MigrationAction {
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            MigrationAction::Preview => "migrate-home --dry-run",
            MigrationAction::Run { move_worktrees } if move_worktrees => {
                "migrate-home --move-worktrees"
            }
            MigrationAction::Run { .. } => "migrate-home",
            MigrationAction::RemoveLegacy => "migrate-home --remove-legacy",
        }
    }

    #[must_use]
    pub fn stops_daemon(self) -> bool {
        matches!(self, MigrationAction::Run { .. })
    }
}

/// One worktree's place in the plan, as the daemon's `--json` writes it.
#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize)]
pub struct WorktreeReport {
    pub path: PathBuf,
    pub destination: PathBuf,
    pub outcome: Outcome,
    #[serde(default)]
    pub reason: Option<String>,
    pub transcripts: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Outcome {
    Stay,
    Move,
    #[serde(rename = "moved")]
    AlreadyMoved,
    Skip,
}

/// The daemon's report, bounded by its own output; the UI reads four fields.
#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize)]
pub struct MigrationReport {
    pub dry_run: bool,
    pub from: PathBuf,
    pub to: PathBuf,
    pub config: String,
    pub runs_copied: usize,
    pub worktrees: Vec<WorktreeReport>,
    pub rollback: Vec<String>,
}

impl MigrationReport {
    #[must_use]
    pub fn moving(&self) -> usize {
        self.worktrees
            .iter()
            .filter(|item| item.outcome == Outcome::Move)
            .count()
    }

    #[must_use]
    pub fn skipped(&self) -> usize {
        self.worktrees
            .iter()
            .filter(|item| item.outcome == Outcome::Skip)
            .count()
    }

    #[must_use]
    pub fn transcripts(&self) -> usize {
        self.worktrees
            .iter()
            .filter(|item| item.outcome == Outcome::Move)
            .map(|item| item.transcripts)
            .sum()
    }
}

#[derive(Default)]
pub struct Migration {
    pub preview: Option<MigrationReport>,
    /// The generation whose preview was asked for, so one reconnect asks once.
    pub preview_for: Option<u64>,
    pub running: bool,
    pub error: Option<String>,
    pub notice: Option<String>,
    /// The action waiting in its confirmation dialog.
    pub confirm: Option<MigrationAction>,
}

impl Migration {
    pub fn apply(&mut self, action: MigrationAction, result: Result<MigrationReport, String>) {
        self.running = false;
        match (action, result) {
            (MigrationAction::Preview, Ok(report)) => {
                self.preview = Some(report);
                self.error = None;
            }
            (MigrationAction::Preview, Err(error)) => {
                self.preview = None;
                self.error = Some(error);
            }
            (MigrationAction::Run { .. }, Ok(report)) => {
                self.preview = Some(report);
                self.notice = Some("The layout moved. Reconnecting to the new daemon…".into());
                self.error = None;
            }
            (MigrationAction::RemoveLegacy, Ok(_)) => {
                self.notice = Some("Old Forge data was removed.".into());
                self.error = None;
            }
            (_, Err(error)) => {
                self.error = Some(error);
            }
        }
    }
}

/// Managed worktrees still under a legacy `…/worktrees/`:
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
    row().gap(crate::ds::SPACE_5).w_full().child(label).child(
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
        .gap(crate::ds::SPACE_3)
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
    if let Some(migration) = migration_group(s, k, paths, left) {
        view = view.child(migration);
    }
    view
}

/// The move buttons: data alone leaves worktrees where they are, because
/// moving one changes its path and the transcripts recorded under it.
fn move_choices(layout: Layout, left: usize) -> Vec<(&'static str, &'static str, MigrationAction)> {
    if layout != Layout::Legacy {
        return vec![(
            "Move worktrees…",
            "storage-migration-move",
            MigrationAction::Run {
                move_worktrees: true,
            },
        )];
    }
    let mut choices = vec![(
        "Move data…",
        "storage-migration-move",
        MigrationAction::Run {
            move_worktrees: false,
        },
    )];
    if left > 0 {
        choices.push((
            "Move data and worktrees…",
            "storage-migration-move-all",
            MigrationAction::Run {
                move_worktrees: true,
            },
        ));
    }
    choices
}

fn migration_group(s: &AppState, k: &Kit, paths: &ForgePaths, left: usize) -> Option<El> {
    let migration = &s.storage.migration;
    let needs_move = paths.layout == Layout::Legacy || left > 0;
    let needs_remove = paths.layout == Layout::Home && paths.legacy.is_some();
    if !needs_move
        && !needs_remove
        && migration.preview.is_none()
        && migration.notice.is_none()
        && migration.error.is_none()
    {
        return None;
    }
    let mut view = group(
        k,
        "storage-migration",
        "Upgrade",
        "Move Forge's own data out of the old hidden folder. Repositories and branches are never touched.",
    );
    if let Some(error) = &migration.error {
        view = view.child(
            text(error.clone())
                .key("storage-migration-error")
                .role("status")
                .font(k.ty.text_xsmall)
                .color(k.c.danger.fg),
        );
    }
    if let Some(notice) = &migration.notice {
        view = view.child(
            text(notice.clone())
                .key("storage-migration-notice")
                .role("status")
                .font(k.ty.text_xsmall)
                .color(k.c.success.fg),
        );
    }
    if let Some(preview) = &migration.preview {
        view = view.child(
            col()
                .key("storage-migration-plan")
                .gap(crate::ds::SPACE_1)
                .child(
                    text(format!(
                        "{} worktree{} to move · {} skipped · {} transcript{} would stop matching",
                        preview.moving(),
                        if preview.moving() == 1 { "" } else { "s" },
                        preview.skipped(),
                        preview.transcripts(),
                        if preview.transcripts() == 1 { "" } else { "s" },
                    ))
                    .font(k.ty.text_xsmall)
                    .color(k.c.text_muted),
                )
                .child(
                    text(format!(
                        "{} → {}",
                        preview.from.display(),
                        preview.to.display()
                    ))
                    .font(k.ty.text_xsmall)
                    .color(k.c.text_faint)
                    .truncate(),
                ),
        );
    } else if migration.running {
        view = view.child(
            text("Reading the migration plan…")
                .font(k.ty.text_xsmall)
                .color(k.c.text_faint),
        );
    }
    let button = |label: &'static str, key: &'static str, action: MigrationAction| {
        Button::new(label)
            .variant(ButtonVariant::Outline)
            .control_size(ControlSize::Small)
            .disabled(migration.running)
            .on_press(move |s: &mut AppState| s.request_migration(action))
            .build(k)
            .key(key)
    };
    let mut actions = row().gap(crate::ds::SPACE_4).items_center();
    if needs_move {
        for (label, key, action) in move_choices(paths.layout, left) {
            actions = actions.child(button(label, key, action));
        }
    }
    if needs_remove {
        actions = actions.child(button(
            "Remove old data…",
            "storage-migration-remove",
            MigrationAction::RemoveLegacy,
        ));
    }
    actions = actions.child(button(
        "Read the plan",
        "storage-migration-preview",
        MigrationAction::Preview,
    ));
    Some(view.child(actions))
}

impl AppState {
    /// The plan is read once per connection and legacy-worktree count, never
    /// on every frame: the daemon copies a database and rescans transcripts.
    pub fn maybe_preview_migration(&mut self) {
        let Some((paths, _)) = &self.storage.paths else {
            return;
        };
        let left = legacy_worktree_count(
            self.store
                .as_ref()
                .map_or(&[][..], |store| &store.workspaces),
            paths.legacy.as_deref(),
        );
        let wants = paths.layout == Layout::Legacy || left > 0;
        if !wants
            || self.storage.migration.preview_for == Some(self.generation)
            || self.storage.migration.running
        {
            return;
        }
        self.storage.migration.preview_for = Some(self.generation);
        self.start_migration(MigrationAction::Preview);
    }

    pub fn request_migration(&mut self, action: MigrationAction) {
        self.storage.migration.confirm = Some(action);
    }

    pub fn start_migration(&mut self, action: MigrationAction) {
        if self.storage.migration.running {
            return;
        }
        let Some(connection) = &self.connection else {
            self.storage.migration.error = Some("Connect to the daemon first.".into());
            return;
        };
        match connection.migrate_storage(self.generation, action) {
            Ok(()) => {
                self.storage.migration.running = true;
                self.storage.migration.error = None;
                self.storage.migration.notice = None;
            }
            Err(error) => self.storage.migration.error = Some(error),
        }
    }

    pub fn apply_migration(
        &mut self,
        action: MigrationAction,
        result: Result<MigrationReport, String>,
    ) {
        self.storage.migration.apply(action, result);
        if action.stops_daemon() && self.storage.migration.error.is_none() {
            self.storage.migration.notice =
                Some("The layout moved. Reconnecting to the new daemon…".into());
        }
    }
}

/// The confirmation for a migration action; every one ends live sessions.
pub fn dialogs(s: &AppState, k: &Kit, layer: &mut Vec<El>) {
    let Some(action) = s.storage.migration.confirm else {
        return;
    };
    let (title, description, label) = match action {
        MigrationAction::RemoveLegacy => (
            "Remove old Forge data?",
            "Deletes the old app.db and agent plugins. Worktrees, config.toml, logs and runs are kept.",
            "Remove old data",
        ),
        MigrationAction::Run {
            move_worktrees: true,
        } => (
            "Move data and worktrees?",
            "Managed worktrees move to the visible folder with git worktree move. Live sessions end, as on any restart, and agent transcripts recorded in the old paths no longer match their worktree.",
            "Move data and worktrees",
        ),
        MigrationAction::Run {
            move_worktrees: false,
        } => (
            "Move data?",
            "Managed worktrees stay where they are and keep working; new ones go under the visible folder. Live sessions end, as on any restart.",
            "Move data",
        ),
        MigrationAction::Preview => return,
    };
    let destructive = action == MigrationAction::RemoveLegacy;
    let mut overlay = Vec::new();
    let cancel = Button::new("Cancel")
        .variant(ButtonVariant::GhostMuted)
        .control_size(ControlSize::Small)
        .on_press(|s: &mut AppState| s.storage.migration.confirm = None)
        .build(k)
        .key("storage-migration-cancel");
    let confirm = Button::new(label)
        .variant(if destructive {
            ButtonVariant::Danger
        } else {
            ButtonVariant::Submit
        })
        .control_size(ControlSize::Small)
        .on_press(move |s: &mut AppState| {
            s.storage.migration.confirm = None;
            s.start_migration(action);
        })
        .build(k)
        .key("storage-migration-confirm");
    crate::modal::Modal::new("storage-migration-confirm", title)
        .description(description)
        .width(crate::modal::WIDTH_STANDARD)
        .on_close(|s: &mut AppState| s.storage.migration.confirm = None)
        .child(
            row()
                .gap(crate::ds::SPACE_4)
                .items_center()
                .child(cancel)
                .child(confirm),
        )
        .build(s, k, &mut overlay);
    layer.extend(overlay);
}

/// Runs `forge-daemon migrate-home` for `action`. The caller stops the daemon
/// first for a move; the subprocess takes the old lock itself.
pub fn migrate_home(
    preferred: Option<&Path>,
    client: &client::Client,
    action: MigrationAction,
    metrics: &crate::connection::Metrics,
) -> Result<MigrationReport, String> {
    let executable = daemon_executable(preferred)?;
    if action.stops_daemon() {
        client
            .stop_daemon(client::StopSessions::Kill)
            .map_err(|error| format!("stop forge-daemon: {error}"))?;
    }
    let mut command = Command::new(&executable);
    let timeout = match action {
        MigrationAction::Preview => Duration::from_secs(120),
        MigrationAction::Run { .. } => Duration::from_secs(600),
        MigrationAction::RemoveLegacy => Duration::from_secs(120),
    };
    match action {
        MigrationAction::Preview => {
            command.args(["migrate-home", "--dry-run", "--move-worktrees", "--json"]);
        }
        MigrationAction::Run { move_worktrees } => {
            command.args(["migrate-home", "--json", "--wait", "30"]);
            if move_worktrees {
                command.arg("--move-worktrees");
            }
        }
        MigrationAction::RemoveLegacy => {
            command.args(["migrate-home", "--remove-legacy", "--json"]);
        }
    }
    let (status, stdout, stderr) = run_bounded(command, timeout, metrics)?;
    if !status.success() {
        let detail = String::from_utf8_lossy(&stderr).trim().to_string();
        return Err(if detail.is_empty() {
            format!("{} exited with {status}", action.label())
        } else {
            detail
        });
    }
    serde_json::from_slice(&stdout).map_err(|error| {
        let detail = String::from_utf8_lossy(&stderr).trim().to_string();
        format!(
            "{} returned unreadable JSON ({error}){}",
            action.label(),
            if detail.is_empty() {
                String::new()
            } else {
                format!(": {detail}")
            }
        )
    })
}

fn daemon_executable(preferred: Option<&Path>) -> Result<PathBuf, String> {
    if let Some(path) = preferred.filter(|path| path.is_file()) {
        return Ok(path.to_path_buf());
    }
    if let Some(path) = std::env::var_os("FORGE_DAEMON_BIN").map(PathBuf::from)
        && path.is_file()
    {
        return Ok(path);
    }
    if let Ok(current) = std::env::current_exe()
        && let Some(parent) = current.parent()
    {
        let sibling = parent.join("forge-daemon");
        if sibling.is_file() {
            return Ok(sibling);
        }
    }
    let path = std::env::var_os("PATH").unwrap_or_default();
    for directory in std::env::split_paths(&path) {
        let candidate = directory.join("forge-daemon");
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    Err(
        "could not find forge-daemon; set FORGE_DAEMON_BIN or launch Forge Node from its bundle"
            .into(),
    )
}

fn run_bounded(
    mut command: Command,
    timeout: Duration,
    metrics: &crate::connection::Metrics,
) -> Result<(std::process::ExitStatus, Vec<u8>, Vec<u8>), String> {
    use std::io::Read;
    use std::os::unix::process::CommandExt;
    use std::process::Stdio;

    metrics.note_subprocess_op();
    command
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().map_err(|error| error.to_string())?;
    let stdout = child.stdout.take().map(|mut pipe| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = pipe.read_to_end(&mut bytes);
            bytes
        })
    });
    let stderr = child.stderr.take().map(|mut pipe| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = pipe.read_to_end(&mut bytes);
            bytes
        })
    });
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let stdout = stdout
                    .and_then(|handle| handle.join().ok())
                    .unwrap_or_default();
                let stderr = stderr
                    .and_then(|handle| handle.join().ok())
                    .unwrap_or_default();
                return Ok((status, stdout, stderr));
            }
            Ok(None) if Instant::now() >= deadline => {
                // SAFETY: `kill` reads only its two integer arguments; the
                // negative pid is the child's own process group.
                unsafe {
                    libc::kill(-(child.id() as i32), libc::SIGKILL);
                }
                let _ = child.kill();
                let _ = child.wait();
                if let Some(handle) = stdout {
                    let _ = handle.join();
                }
                if let Some(handle) = stderr {
                    let _ = handle.join();
                }
                return Err(format!("timed out after {} seconds", timeout.as_secs()));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(25)),
            Err(error) => {
                // SAFETY: `kill` reads only its two integer arguments; the
                // negative pid is the child's own process group.
                unsafe {
                    libc::kill(-(child.id() as i32), libc::SIGKILL);
                }
                let _ = child.kill();
                let _ = child.wait();
                return Err(error.to_string());
            }
        }
    }
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
    fn moving_legacy_data_alone_leaves_the_worktrees_in_place() {
        let actions = |layout, left| -> Vec<MigrationAction> {
            move_choices(layout, left)
                .into_iter()
                .map(|(_, _, action)| action)
                .collect()
        };
        assert_eq!(
            actions(Layout::Legacy, 2),
            [
                MigrationAction::Run {
                    move_worktrees: false
                },
                MigrationAction::Run {
                    move_worktrees: true
                },
            ]
        );
        assert_eq!(
            actions(Layout::Legacy, 0),
            [MigrationAction::Run {
                move_worktrees: false
            }]
        );
        assert_eq!(
            actions(Layout::Home, 3),
            [MigrationAction::Run {
                move_worktrees: true
            }]
        );
    }

    #[test]
    fn a_plan_counts_only_the_worktrees_it_moves_and_their_transcripts() {
        let report: MigrationReport = serde_json::from_str(
            r#"{
                "dry_run": true,
                "from": "/old",
                "to": "/new",
                "config": "copied",
                "runs_copied": 0,
                "worktrees": [
                    {"path": "/old/worktrees/a", "destination": "/new/worktrees/a",
                     "outcome": "move", "transcripts": 3},
                    {"path": "/old/worktrees/b", "destination": "/new/worktrees/b",
                     "outcome": "skip", "reason": "locked", "transcripts": 0},
                    {"path": "/old/worktrees/c", "destination": "/new/worktrees/c",
                     "outcome": "moved", "transcripts": 9}
                ],
                "rollback": []
            }"#,
        )
        .expect("the daemon's preview");
        assert_eq!(report.moving(), 1);
        assert_eq!(report.skipped(), 1);
        assert_eq!(
            report.transcripts(),
            3,
            "an already-moved tree is not moved"
        );
        assert!(report.dry_run);
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
