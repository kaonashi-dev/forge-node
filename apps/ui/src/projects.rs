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
    /// The removal sent and not answered yet.
    pub removing: Option<WorkspaceId>,
    /// Why the daemon kept a worktree, in words for a person.
    pub refused: Option<(WorkspaceId, Vec<String>)>,
}

const PRECHECK_REASONS: [(&str, &str); 3] = [
    (
        "running_sessions=true",
        "One or more sessions are still running in it.",
    ),
    ("dirty=true", "It has uncommitted changes."),
    ("merge_or_rebase=true", "A merge or rebase is in progress."),
];

/// The daemon's removal precheck as sentences, as `describeWorktreeRemovalBlock`:
/// only the checks that failed, or the daemon's own words when none is known.
#[must_use]
pub fn removal_block(reason: &str) -> Vec<String> {
    let known: Vec<String> = PRECHECK_REASONS
        .iter()
        .filter(|(token, _)| reason.contains(token))
        .map(|(_, sentence)| (*sentence).to_string())
        .collect();
    if known.is_empty() {
        vec![reason.to_string()]
    } else {
        known
    }
}

/// The open pull request on this checkout's branch, as a suffix.
fn pr_note(s: &AppState, workspace: &Workspace) -> String {
    let Some(store) = &s.store else {
        return String::new();
    };
    crate::sync::pull_request_for(
        &store.pull_requests.pull_requests,
        workspace.project_id,
        workspace.branch.as_deref(),
    )
    .map_or_else(String::new, |pr| format!(" · {}", pr_label(pr)))
}

/// `#12 · draft · Title`, as Tauri's `prLabel` words it.
#[must_use]
pub fn pr_label(pr: &domain::PullRequest) -> String {
    let state = if pr.is_draft {
        Some("draft")
    } else {
        match pr.review_decision {
            Some(domain::ReviewDecision::Approved) => Some("approved"),
            Some(domain::ReviewDecision::ChangesRequested) => Some("changes requested"),
            Some(domain::ReviewDecision::ReviewRequired) => Some("review required"),
            _ => None,
        }
    };
    match state {
        Some(state) => format!("#{} · {state} · {}", pr.number, pr.title),
        None => format!("#{} · {}", pr.number, pr.title),
    }
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
    // A side at zero is left out, as Tauri's `syncLabel` does.
    let sync: Vec<String> = [
        status.ahead.filter(|n| *n > 0).map(|n| format!("↑{n}")),
        status.behind.filter(|n| *n > 0).map(|n| format!("↓{n}")),
    ]
    .into_iter()
    .flatten()
    .collect();
    if !sync.is_empty() {
        parts.push(sync.join(" "));
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

/// A project's checkouts as Tauri's `workspaceOrder` lists them: the
/// repository's own checkout first, then its worktrees by name.
#[must_use]
pub fn checkouts_of(workspaces: &[Workspace], project: ProjectId) -> Vec<&Workspace> {
    let mut owned: Vec<&Workspace> = workspaces
        .iter()
        .filter(|workspace| workspace.project_id == project)
        .collect();
    owned.sort_by_key(|workspace| (workspace.kind != WorkspaceKind::Main, workspace.label()));
    owned
}

pub fn view(s: &AppState, k: &Kit) -> El {
    let add = Button::new("Add…")
        .variant(ButtonVariant::GhostMuted)
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
    let mut list = col().key("project-list").w_full().gap(14.0);
    let Some(store) = &s.store else {
        return col()
            .key("projects")
            .child(text("No replica yet.").font(k.ty.small));
    };
    for project in &store.projects {
        let workspaces = checkouts_of(&store.workspaces, project.id);
        let project_id = project.id;
        let mut block = col()
            .key(format!("project-{}", project.id))
            .role("group")
            .label(project.name.clone())
            .gap(4.0)
            .child(
                row()
                    .key(format!("project-row-{project_id}"))
                    .gap(10.0)
                    .items_center()
                    .on_secondary_press(move |s: &mut AppState, point, _| {
                        crate::menus::open(s, crate::menus::Target::Project(project_id), point);
                    })
                    .child(
                        text(project.name.clone())
                            .font(k.ty.small_medium)
                            .color(k.c.text_muted)
                            .grow(1.0)
                            .truncate()
                            .min_w(0.0),
                    )
                    .child(small(
                        "+ Worktree",
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
        .gap(8.0)
        .child(
            row()
                .key("projects-bar")
                .w_full()
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
        .flex_wrap()
        .child(small(
            "Shell",
            k,
            format!("shell-{id}"),
            move |s: &mut AppState| s.shell_in(id),
        ))
        .child(small(
            "Git",
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
    // Pinned actions are the ones a checkout keeps in reach; the rest are in
    // the palette ("Run: …").
    let mut pinned: Vec<_> = s
        .actions_for(id)
        .into_iter()
        .filter(|action| action.pinned)
        .collect();
    // In the project's own order, as Tauri's `actionState` lays out the chips.
    pinned.sort_by_key(|action| action.position);
    for action in pinned {
        let running = s.action_running(id, &action.id);
        let action_id = action.id;
        actions = actions.child(
            Button::new(if running {
                format!("● {}", action.label)
            } else {
                action.label.clone()
            })
            .variant(if running {
                ButtonVariant::Outline
            } else {
                ButtonVariant::GhostMuted
            })
            .control_size(ControlSize::Small)
            .on_press(move |s: &mut AppState| s.run_action(id, action_id))
            .build(k)
            .key(format!("action-{id}-{}", action.id)),
        );
    }
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
    let current = s.workbench.workspace == Some(id);
    let sessions = s
        .rows
        .iter()
        .enumerate()
        .filter(|(_, row)| row.workspace_id == id)
        .map(|(index, row)| crate::ui::session_row(s, row, index, k));
    col()
        .key(format!("workspace-{id}"))
        .role("group")
        .label(format!("{} · {branch}", workspace.label()))
        .w_full()
        .gap(4.0)
        .child(
            col()
                .key(format!("checkout-{id}"))
                .role("button")
                .label(format!("Show {}", workspace.label()))
                .w_full()
                .px(8.0)
                .py(5.0)
                .radius(6.0)
                .fill(if current {
                    k.c.surface_hover
                } else {
                    rune::Color::TRANSPARENT
                })
                .hover(k.c.hover)
                .on_press(move |s: &mut AppState| s.focus_checkout(id))
                .on_secondary_press(move |s: &mut AppState, point, _| {
                    crate::menus::open(s, crate::menus::Target::Checkout(id), point);
                })
                .child(
                    rune::row()
                        .gap(6.0)
                        .items_center()
                        .child(
                            rune::icon(
                                if workspace.kind == WorkspaceKind::Main {
                                    &crate::glyphs::FILES
                                } else {
                                    &crate::glyphs::GIT
                                },
                                13.0,
                            )
                            .color(k.c.icon_muted),
                        )
                        .child(
                            text(format!("{} · {branch}", workspace.label()))
                                .font(k.ty.small_medium)
                                .color(k.c.text_base)
                                .truncate()
                                .min_w(0.0),
                        ),
                )
                .child(
                    text(format!(
                        "{kind} · {}{}",
                        status_line(workspace),
                        pr_note(s, workspace)
                    ))
                    .font(k.ty.text_xsmall)
                    .color(if workspace.status.dirty {
                        k.c.warning.fg
                    } else {
                        k.c.text_faint
                    }),
                )
                .children(
                    s.projects
                        .refused
                        .iter()
                        .filter(|(refused, _)| *refused == id)
                        .flat_map(|(_, reasons)| reasons.iter())
                        .map(|reason| {
                            text(format!("Not removed: {reason}"))
                                .role("status")
                                .font(k.ty.text_xsmall)
                                .color(k.c.warning.fg)
                        }),
                ),
        )
        .children(sessions)
        .child(actions.px(4.0))
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
            "uncommitted changes · ↑2",
            "a side at zero is left out"
        );
    }

    /// A pinned action is a chip on its checkout, marked while a session runs it.
    #[test]
    fn a_pinned_action_is_a_chip_and_shows_when_it_runs() {
        use std::path::Path;
        use std::sync::Arc;

        use domain::{
            ActionKind, ActionReady, ActionRun, ActionSource, ProjectAction, ProjectActionId,
        };
        use rune::{Env, Runtime};

        use crate::connection::{Metrics, fixture_store};
        use crate::terminal::SharedTerminal;

        let mut store =
            fixture_store(&Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/snapshot.json"))
                .expect("fixture snapshot decodes");
        let workspace = store.workspaces[0].clone();
        let action = ProjectAction {
            id: ProjectActionId::new(),
            project_id: workspace.project_id,
            label: "dev".into(),
            command: "npm run dev".into(),
            kind: ActionKind::Dev,
            cwd: None,
            single_instance: true,
            pinned: true,
            ready: ActionReady::None,
            port: None,
            depends_on: Vec::new(),
            source: ActionSource::Manual,
            position: 1,
            created_at: domain::Timestamp::now(),
        };
        let chip = format!("action-{}-{}", workspace.id, action.id);
        let first = ProjectAction {
            id: ProjectActionId::new(),
            label: "lint".into(),
            position: 0,
            ..action.clone()
        };
        let first_chip = format!("action-{}-{}", workspace.id, first.id);
        store.project_actions.push(action.clone());
        store.project_actions.push(first.clone());
        let running = store
            .sessions
            .iter_mut()
            .find(|session| session.workspace_id == workspace.id && session.state.is_active())
            .expect("a live session in the first workspace");
        running.action = Some(Box::new(ActionRun {
            action_id: action.id,
            label: "dev".into(),
            port: None,
            readiness: domain::ActionReadiness::Probing,
            started_at: None,
        }));
        let mut state =
            AppState::for_bench(store, Arc::new(Metrics::default()), SharedTerminal::new());
        state.show_mode(crate::workbench::CenterMode::Projects);
        assert!(state.action_running(workspace.id, &action.id));
        assert!(
            !state.action_running(workspace.id, &first.id),
            "a session running another action does not mark this one"
        );
        if let Some(other) = state
            .store
            .as_ref()
            .and_then(|store| store.workspaces.iter().find(|w| w.id != workspace.id))
        {
            assert!(
                !state.action_running(other.id, &action.id),
                "a run is keyed by its checkout"
            );
        }
        let mut rt = Runtime::new(
            state,
            crate::ui::view,
            (1180.0, 760.0),
            Env {
                system_dark: true,
                scale: 1.0,
            },
        );
        rt.frame(0.1);
        let at = |key: &str| {
            rt.elements()
                .into_iter()
                .find(|element| element.key.as_deref() == Some(key))
                .map(|element| element.rect)
        };
        let found = at(&chip).expect("the pinned action is on its checkout");
        let before = at(&first_chip).expect("both pinned actions are chips");
        assert!(
            (before[1], before[0]) < (found[1], found[0]),
            "chips follow the actions' position"
        );
    }

    #[test]
    fn a_pull_request_is_labelled_as_workspace_card_labels_it() {
        let mut pr: domain::PullRequest = serde_json::from_value(serde_json::json!({
            "project_id": null, "repository": "o/r", "host": "github.com", "number": 12,
            "title": "Title", "body": "", "body_truncated": false, "url": "https://x/12",
            "author": "a", "base_ref": "main", "head_ref": "b", "is_draft": false,
            "review_decision": "Approved", "labels": [], "assignees": [],
            "review_requests": [], "additions": 1, "deletions": 0, "changed_files": 1,
            "comment_count": 0, "created_at": "2026-11-09T00:00:00Z",
            "updated_at": "2026-11-09T00:00:00Z",
            "relations": { "assigned": false, "review_requested": false, "authored": true }
        }))
        .expect("a pull request");
        assert_eq!(pr_label(&pr), "#12 · approved · Title");
        pr.is_draft = true;
        assert_eq!(
            pr_label(&pr),
            "#12 · draft · Title",
            "draft wins over the decision"
        );
        pr.is_draft = false;
        pr.review_decision = None;
        assert_eq!(pr_label(&pr), "#12 · Title");
    }

    #[test]
    fn a_refused_removal_says_only_the_checks_that_failed() {
        assert_eq!(
            removal_block(
                "cannot remove: running_sessions=false, dirty=true, merge_or_rebase=false"
            ),
            vec!["It has uncommitted changes.".to_string()]
        );
        assert_eq!(
            removal_block("removal is temporarily unavailable"),
            vec!["removal is temporarily unavailable".to_string()]
        );
    }

    #[test]
    fn the_main_checkout_leads_then_worktrees_by_name() {
        let project = ProjectId::new();
        let checkout = |kind, name: &str| Workspace {
            id: domain::WorkspaceId::new(),
            project_id: project,
            kind,
            path: format!("/tmp/{name}").into(),
            branch: Some(name.into()),
            display_name: None,
            managed_by_app: false,
            created_at: domain::Timestamp::now(),
            status: domain::WorkspaceStatus::default(),
        };
        let all = vec![
            checkout(WorkspaceKind::GitWorktree, "zeta"),
            checkout(WorkspaceKind::Main, "main"),
            checkout(WorkspaceKind::GitWorktree, "alpha"),
            Workspace {
                project_id: ProjectId::new(),
                ..checkout(WorkspaceKind::Main, "elsewhere")
            },
        ];
        let names: Vec<String> = checkouts_of(&all, project)
            .iter()
            .map(|workspace| workspace.label())
            .collect();
        assert_eq!(names, vec!["main", "alpha", "zeta"]);
    }
}
