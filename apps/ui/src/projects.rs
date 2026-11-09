//! Projects and their checkouts: the main checkout and each worktree, with the
//! status the daemon measured. Every change is a daemon request; what it
//! changes comes back as events into the replica.

use domain::{ProjectId, Workspace, WorkspaceId, WorkspaceKind};
use rune::{El, FileDialogOptions, ScrollSpec, TextEdit, col, row, text};
use rune_kit::{Button, ButtonVariant, ControlSize, Kit};

use crate::app::AppState;
use crate::workbench::CenterMode;

#[derive(Default)]
pub struct Projects {
    /// The project a branch is being typed for.
    pub new_worktree: Option<ProjectId>,
    pub branch: TextEdit,
    /// A worktree whose removal waits for a second press.
    pub confirm_remove: Option<WorkspaceId>,
}

/// One line of what the daemon measured, or that it has not.
#[must_use]
pub fn status_line(workspace: &Workspace) -> String {
    let status = &workspace.status;
    // Unmeasured is not clean: saying "clean" before git answered would be a guess.
    if status.measured_at.is_none() {
        return "not measured yet".to_string();
    }
    let mut parts = vec![
        if status.dirty {
            "uncommitted changes"
        } else {
            "clean"
        }
        .to_string(),
    ];
    match (status.ahead, status.behind) {
        (Some(ahead), Some(behind)) if ahead > 0 || behind > 0 => {
            parts.push(format!("↑{ahead} ↓{behind}"));
        }
        _ => {}
    }
    parts.join(" · ")
}

fn small(label: &'static str, k: &Kit, key: String, press: impl Fn(&mut AppState) + 'static) -> El {
    Button::new(label)
        .variant(ButtonVariant::GhostMuted)
        .control_size(ControlSize::Small)
        .on_press(press)
        .build(k)
        .key(key)
}

pub fn view(s: &AppState, k: &Kit) -> El {
    let add = Button::new("Add project…")
        .variant(ButtonVariant::Outline)
        .control_size(ControlSize::Small)
        .on_press_tx(|_s: &mut AppState, tx| {
            tx.open_folder(
                FileDialogOptions {
                    title: "Add a project".into(),
                    ..FileDialogOptions::default()
                },
                |s: &mut AppState, answer, _tx| {
                    if let Ok(Some(path)) = answer {
                        s.add_project(path);
                    }
                },
            );
        })
        .build(k)
        .key("add-project");
    let mut list = col().key("project-list").w_full().gap(18.0).padding(16.0);
    let Some(store) = &s.store else {
        return col()
            .key("projects")
            .child(text("No replica yet.").font(k.ty.small));
    };
    for project in &store.projects {
        let mut workspaces: Vec<&Workspace> = store
            .workspaces
            .iter()
            .filter(|workspace| workspace.project_id == project.id)
            .collect();
        workspaces
            .sort_by_key(|workspace| (workspace.kind != WorkspaceKind::Main, workspace.label()));
        let project_id = project.id;
        let mut block = col()
            .key(format!("project-{}", project.id))
            .role("group")
            .label(project.name.clone())
            .gap(6.0)
            .child(
                row()
                    .gap(10.0)
                    .items_center()
                    .child(
                        text(project.name.clone())
                            .font(k.ty.body_medium)
                            .color(k.c.text_base)
                            .grow(1.0),
                    )
                    .child(small(
                        "New worktree…",
                        k,
                        format!("new-worktree-{project_id}"),
                        move |s: &mut AppState| {
                            s.projects.new_worktree = Some(project_id);
                            s.projects.branch = TextEdit::new("");
                            s.projects.branch.focused = true;
                        },
                    )),
            );
        for workspace in workspaces {
            block = block.child(workspace_row(s, k, workspace));
        }
        list = list.child(block);
    }
    if store.projects.is_empty() {
        list = list.child(
            text("No projects. Add a folder to start.")
                .font(k.ty.small)
                .color(k.c.text_muted),
        );
    }
    col()
        .key("projects")
        .w_full()
        .h_full()
        .min_h(0.0)
        .child(
            row()
                .key("projects-bar")
                .w_full()
                .h(36.0)
                .px(12.0)
                .items_center()
                .child(
                    text("Projects")
                        .font(k.ty.small_medium)
                        .color(k.c.text_base)
                        .grow(1.0),
                )
                .child(add),
        )
        .child(
            col()
                .key("projects-scroll")
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .scroll(ScrollSpec {
                    stick_bottom: false,
                })
                .child(list),
        )
}

fn workspace_row(s: &AppState, k: &Kit, workspace: &Workspace) -> El {
    let id = workspace.id;
    let kind = match workspace.kind {
        WorkspaceKind::Main => "main checkout",
        _ => "worktree",
    };
    let branch = workspace
        .branch
        .clone()
        .unwrap_or_else(|| "detached".into());
    let mut actions = row()
        .gap(2.0)
        .child(small(
            "Shell",
            k,
            format!("shell-{id}"),
            move |s: &mut AppState| s.shell_in(id),
        ))
        .child(small(
            "Changes",
            k,
            format!("changes-{id}"),
            move |s: &mut AppState| {
                s.inspect(id, CenterMode::Changes);
            },
        ))
        .child(small(
            "Files",
            k,
            format!("files-{id}"),
            move |s: &mut AppState| {
                s.inspect(id, CenterMode::Files);
            },
        ));
    if workspace.kind == WorkspaceKind::GitWorktree {
        let confirming = s.projects.confirm_remove == Some(id);
        let label = if confirming {
            "Confirm remove"
        } else {
            "Remove"
        };
        actions = actions.child(
            Button::new(label)
                .variant(if confirming {
                    ButtonVariant::Danger
                } else {
                    ButtonVariant::GhostMuted
                })
                .control_size(ControlSize::Small)
                .on_press(move |s: &mut AppState| s.remove_worktree(id))
                .build(k)
                .key(format!("remove-{id}")),
        );
    }
    row()
        .key(format!("workspace-{id}"))
        .role("group")
        .label(format!("{} · {branch}", workspace.label()))
        .w_full()
        .gap(10.0)
        .px(10.0)
        .py(6.0)
        .radius(6.0)
        .fill(k.c.surface_hover)
        .items_center()
        .child(
            col()
                .grow(1.0)
                .min_w(0.0)
                .child(
                    text(format!("{} · {branch}", workspace.label()))
                        .font(k.ty.small)
                        .color(k.c.text_base)
                        .truncate(),
                )
                .child(
                    text(format!("{kind} · {}", status_line(workspace)))
                        .font(k.ty.text_xsmall)
                        .color(if workspace.status.dirty {
                            k.c.warning.fg
                        } else {
                            k.c.text_faint
                        }),
                ),
        )
        .child(actions)
}

/// The branch prompt for a new worktree, as a dialog layer.
pub fn dialogs(s: &AppState, k: &Kit, layer: &mut Vec<El>) {
    if s.projects.new_worktree.is_none() {
        return;
    }
    let field = rune_kit::TextInput::new(&s.projects.branch)
        .placeholder("feature/my-branch")
        .label("Branch")
        .on_edit(|s: &mut AppState| &mut s.projects.branch)
        .on_key(|s: &mut AppState, ev, _| match ev.key {
            rune::Key::Enter => {
                s.create_worktree();
                true
            }
            rune::Key::Escape => {
                s.projects.new_worktree = None;
                true
            }
            _ => false,
        })
        .build(k)
        .key("worktree-branch");
    let create = Button::new("Create")
        .variant(ButtonVariant::Contrast)
        .on_press(|s: &mut AppState| s.create_worktree())
        .build(k)
        .key("worktree-create");
    rune_kit::Dialog::new("new-worktree", "New worktree")
        .description("A new branch, checked out beside the project.")
        .open(true)
        .fit()
        .width(420.0)
        .click_outside(true)
        .on_close(|s: &mut AppState| s.projects.new_worktree = None)
        .child(field)
        .child(row().pt(12.0).child(create))
        .build(k, layer);
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::WorkspaceStatus;

    fn workspace(status: WorkspaceStatus) -> Workspace {
        Workspace {
            id: WorkspaceId::new(),
            project_id: ProjectId::new(),
            kind: WorkspaceKind::Main,
            path: "/tmp/x".into(),
            branch: Some("main".into()),
            display_name: None,
            managed_by_app: false,
            created_at: domain::Timestamp::now(),
            status,
        }
    }

    #[test]
    fn an_unmeasured_checkout_is_not_called_clean() {
        assert_eq!(
            status_line(&workspace(WorkspaceStatus::default())),
            "not measured yet"
        );
    }

    #[test]
    fn a_measured_checkout_says_dirty_and_how_far_it_is() {
        let status = WorkspaceStatus {
            dirty: true,
            ahead: Some(2),
            behind: Some(0),
            measured_at: Some(domain::Timestamp::now()),
            ..WorkspaceStatus::default()
        };
        assert_eq!(
            status_line(&workspace(status)),
            "uncommitted changes · ↑2 ↓0"
        );
    }
}
