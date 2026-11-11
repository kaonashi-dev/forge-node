//! Projects and their checkouts: the main checkout and each worktree, with the
//! status the daemon measured, under the groups the person made. Every change
//! is a daemon request; what it changes comes back as events into the replica.

use domain::{ProjectGroupId, ProjectId, SessionId, Workspace, WorkspaceId, WorkspaceKind};
use rune::{El, KeyEvent, ScrollSpec, Tx, col, el, row, text};
use rune_kit::{Button, ButtonVariant, ControlSize, Kit};

use crate::app::AppState;
use crate::ds::{self, Emphasis, IconButtonSize, Row, Tone};
use crate::menus::ButtonMenu;
use crate::rail::{Rail, RowKind, Step};
use crate::workbench::CenterMode;

/// A worktree removal being decided: `None` is the first confirmation, a
/// reason is the daemon's refusal, which asks before forcing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Removal {
    pub workspace: WorkspaceId,
    pub reason: Option<String>,
}

#[derive(Default)]
pub struct Projects {
    pub picker: Option<crate::branch_picker::Picker>,
    pub removal: Option<Removal>,
    /// Why the daemon kept a worktree even when forced.
    pub refused: Option<(WorkspaceId, Vec<String>)>,
    /// The project whose ignored worktrees are listed.
    pub ignores: Option<ProjectId>,
    pub share_form: crate::shares::Form,
    pub shares: crate::project_ops::ShareReads,
    pub actions: Option<crate::actions_editor::Editor>,
    pub rail: Rail,
}

impl Projects {
    /// Whether a field in one of these dialogs has the keyboard.
    #[must_use]
    pub fn field_focused(&self) -> bool {
        self.share_form.focused()
            || self
                .actions
                .as_ref()
                .is_some_and(crate::actions_editor::Editor::focused)
    }
}

const PRECHECK_REASONS: [(&str, &str); 3] = [
    (
        "running_sessions=true",
        "One or more sessions are still running in it.",
    ),
    ("dirty=true", "It has uncommitted changes."),
    ("merge_or_rebase=true", "A merge or rebase is in progress."),
];

/// The daemon's removal precheck as sentences:
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

/// `#12 · draft · Title`.
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
    // A side at zero is left out.
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

/// A checkout's state as one chip. Nothing until git has answered, since
/// unmeasured is not clean.
#[must_use]
pub fn checkout_chip(status: &domain::WorkspaceStatus) -> Option<(String, Tone)> {
    status.measured_at?;
    let sync: Vec<String> = [
        status.ahead.filter(|n| *n > 0).map(|n| format!("↑{n}")),
        status.behind.filter(|n| *n > 0).map(|n| format!("↓{n}")),
    ]
    .into_iter()
    .flatten()
    .collect();
    let sync = sync.join(" ");
    Some(match (status.dirty, sync.is_empty()) {
        (true, true) => ("changes".to_string(), Tone::Warning),
        (true, false) => (format!("changes {sync}"), Tone::Warning),
        (false, true) => ("clean".to_string(), Tone::Neutral),
        (false, false) => (sync, Tone::Info),
    })
}

/// A project's checkouts in their default order:
/// the repository's own checkout first, then its worktrees by name.
#[must_use]
pub fn checkouts_of(workspaces: &[Workspace], project: ProjectId) -> Vec<&Workspace> {
    let mut owned: Vec<&Workspace> = workspaces
        .iter()
        .filter(|workspace| workspace.project_id == project)
        .collect();
    owned.sort_by_key(|workspace| (workspace.kind != WorkspaceKind::Main, workspace.label()));
    owned
}

/// Reads the folds and checkout order a snapshot carries. The order follows
/// every snapshot; the folds only the first, so a later write never re-folds
/// a row the person has opened since.
pub fn seed(s: &mut AppState) {
    let Some(store) = &s.store else {
        return;
    };
    let rail = &mut s.projects.rail;
    rail.order = crate::rail::parse_order(store.app_state_value(crate::rail::ORDER_KEY));
    if !rail.seeded {
        rail.folds = crate::rail::parse_folds(store.app_state_value(crate::rail::FOLDS_KEY));
        rail.seeded = true;
    }
}

pub fn toggle_fold(s: &mut AppState, id: &str) {
    let rail = &mut s.projects.rail;
    rail.seeded = true;
    if !rail.folds.remove(id) {
        rail.folds.insert(id.to_string());
    }
    let payload = crate::rail::folds_payload(&rail.folds);
    s.write_pref(crate::rail::FOLDS_KEY, &payload);
}

pub fn move_checkout(s: &mut AppState, workspace: WorkspaceId, by: isize) {
    let Some(store) = &s.store else {
        return;
    };
    let Some((project, ids)) =
        crate::rail::moved_checkout(&store.workspaces, workspace, by, &s.projects.rail.order)
    else {
        return;
    };
    s.projects.rail.order.insert(project.to_string(), ids);
    let payload = crate::rail::order_payload(&s.projects.rail.order);
    s.write_pref(crate::rail::ORDER_KEY, &payload);
}

fn rail_sessions(s: &AppState) -> Vec<(WorkspaceId, SessionId)> {
    s.rows
        .iter()
        .map(|row| (row.workspace_id, row.id))
        .collect()
}

fn select_row(s: &mut AppState, id: String) {
    s.projects.rail.cursor = Some(id);
    s.projects.rail.keyed = false;
}

/// Enter on a row: a session shows its terminal and hands it the keyboard;
/// anything else folds or unfolds.
fn activate(s: &mut AppState, row: &crate::rail::RailRow, tx: &mut Tx) {
    match row.kind {
        RowKind::Session => {
            if let Ok(id) = row.target.parse::<SessionId>() {
                s.select(id);
                s.show_mode(CenterMode::Session);
                tx.blur();
            }
        }
        _ => toggle_fold(s, &row.target),
    }
}

/// The rail's own chords, taken only while it holds focus, so `j`, `k`,
/// `h` and `l` stay the terminal's everywhere else.
pub fn rail_key(s: &mut AppState, ev: &KeyEvent, tx: &mut Tx) -> bool {
    let Some(store) = &s.store else {
        return false;
    };
    let m = ev.mods;
    let plain = !m.cmd && !m.ctrl && !m.alt && !m.logo && !m.shift;
    let rows = crate::rail::rows(
        store,
        &s.projects.rail,
        &rail_sessions(s),
        s.workbench.workspace,
    );
    if rows.is_empty() {
        return false;
    }
    let at = s
        .projects
        .rail
        .cursor
        .as_ref()
        .and_then(|id| rows.iter().position(|row| row.id == *id));
    let cursor = at.unwrap_or(0);
    let pick = |s: &mut AppState, index: usize| select_row(s, rows[index].id.clone());
    if ev.key == rune::Key::Enter && m.cmd && !m.ctrl && !m.alt {
        let row = &rows[cursor];
        let workspace = match row.kind {
            RowKind::Workspace => Some(row.target.clone()),
            _ => row
                .parent
                .as_deref()
                .and_then(|parent| parent.strip_prefix("workspace:"))
                .map(str::to_string),
        };
        if let Some(workspace) = workspace.and_then(|w| w.parse::<WorkspaceId>().ok()) {
            s.shell_in(workspace);
            tx.blur();
        }
        return true;
    }
    if !plain {
        return false;
    }
    let letter = match &ev.key {
        rune::Key::Text(typed) => typed.as_str(),
        _ => "",
    };
    match (&ev.key, letter) {
        (rune::Key::Down, _) | (_, "j") => {
            pick(
                s,
                if at.is_some() {
                    (cursor + 1).min(rows.len() - 1)
                } else {
                    0
                },
            );
        }
        (rune::Key::Up, _) | (_, "k") => pick(s, cursor.saturating_sub(1)),
        (rune::Key::Home, _) => pick(s, 0),
        (rune::Key::End, _) => pick(s, rows.len() - 1),
        (rune::Key::Left, _) | (_, "h") => match crate::rail::collapse_target(rows.get(cursor)) {
            Some(Step::Fold(target)) => toggle_fold(s, &target),
            Some(Step::Select(id)) => select_row(s, id),
            None => {}
        },
        (rune::Key::Right, _) | (_, "l") => match crate::rail::expand_target(&rows, cursor) {
            Some(Step::Fold(target)) => toggle_fold(s, &target),
            Some(Step::Select(id)) => select_row(s, id),
            None => {}
        },
        (rune::Key::Enter, _) => {
            let row = rows[cursor].clone();
            activate(s, &row, tx);
        }
        _ => return false,
    }
    s.projects.rail.keyed = true;
    true
}

fn chevron(open: bool) -> &'static rune::Icon {
    if open {
        &crate::glyphs::CHEVRON_DOWN
    } else {
        &crate::glyphs::CHEVRON_RIGHT
    }
}

/// The cursor's mark: the row the rail's keys act on.
fn cursored(s: &AppState, k: &Kit, id: &str, row: El) -> El {
    let rail = &s.projects.rail;
    if rail.keyed && rail.cursor.as_deref() == Some(id) {
        row.border(1.0, k.c.border_focus)
    } else {
        row
    }
}

fn wants_you(s: &AppState, workspace: WorkspaceId) -> bool {
    s.rows.iter().any(|row| {
        row.workspace_id == workspace && row.status == Some(crate::app::SessionStatus::Attention)
    })
}

fn needs_you_block(s: &AppState, k: &Kit) -> Option<El> {
    let store = s.store.as_ref()?;
    let waiting = crate::rail::needs_you(&store.sessions);
    if waiting.is_empty() {
        return None;
    }
    let now = domain::Timestamp::now();
    let roles = crate::theme::roles(k);
    let mut block = col()
        .key("needs-you")
        .role("region")
        .label("Needs you")
        .w_full()
        .gap(crate::ds::SPACE_2);
    for session in waiting {
        let id = session.id;
        let place = store
            .workspaces
            .iter()
            .find(|w| w.id == session.workspace_id)
            .and_then(|w| {
                store
                    .projects
                    .iter()
                    .find(|p| p.id == w.project_id)
                    .map(|p| p.name.clone())
                    .or_else(|| w.branch.clone())
            })
            .unwrap_or_default();
        let title = s.rows.iter().find(|row| row.id == id).map_or_else(
            || crate::session_label::session_title(session),
            |row| row.title.clone(),
        );
        let waited = crate::rail::waited(session, &now);
        block = block.child(
            col()
                .key(format!("needs-you-{id}"))
                .role("button")
                .label(format!("{title}, waiting on you for {waited}"))
                .w_full()
                .px(8.0)
                .py(6.0)
                .radius(k.radius.md)
                .fill(roles.needs_you.soft)
                .hover(k.c.hover)
                .on_press(move |s: &mut AppState| s.focus_session(id))
                .child(
                    row()
                        .gap(crate::ds::SPACE_3)
                        .items_center()
                        .child(
                            rune::icon(&crate::glyphs::NEEDS_YOU, 12.0)
                                .color(roles.needs_you.solid),
                        )
                        .child(
                            text(title)
                                .font(k.ty.small_medium)
                                .color(k.c.text_base)
                                .truncate()
                                .min_w(0.0)
                                .grow(1.0),
                        )
                        .child(text(waited).font(k.ty.text_xsmall).color(k.c.text_faint)),
                )
                .child(
                    text(format!("{place} · waiting on you"))
                        .font(k.ty.text_xsmall)
                        .color(k.c.text_faint)
                        .truncate(),
                ),
        );
    }
    Some(block)
}

fn group_header(s: &AppState, k: &Kit, group: ProjectGroupId, projects: &[ProjectId]) -> El {
    let store = s.store.as_ref();
    let roles = crate::theme::roles(k);
    let name = store
        .and_then(|store| store.project_groups.iter().find(|g| g.id == group))
        .map_or_else(String::new, |g| g.name.clone());
    let target = group.to_string();
    let open = !s.projects.rail.folds.contains(&target);
    let asking = !open
        && store.is_some_and(|store| {
            store
                .workspaces
                .iter()
                .filter(|w| projects.contains(&w.project_id))
                .any(|w| wants_you(s, w.id))
        });
    let id = format!("group:{target}");
    let fold = target.clone();
    let mut line = row()
        .key(format!("group-row-{group}"))
        .role("treeitem")
        .label(name.clone())
        .w_full()
        .h(s.chrome_metrics().row)
        .pl(ds::SPACE_2)
        .pr(ds::SPACE_2)
        .gap(ds::SPACE_3)
        .radius(k.radius.md)
        .center_y()
        .hover_group_focus()
        .child(
            text(name.to_uppercase())
                .font(k.ty.text_xsmall_medium)
                .color(k.c.text_faint)
                .truncate()
                .min_w(0.0)
                .grow(1.0),
        );
    if asking {
        line = line.child(
            rune::icon(&crate::glyphs::NEEDS_YOU, ds::ICON_SMALL).color(roles.needs_you.solid),
        );
    }
    // A folded section keeps its chevron; an open one shows it only under the pointer.
    line = line.child(
        rune::icon(chevron(open), ds::ICON_SMALL)
            .color(k.c.icon_faint)
            .group_opacity(if open { 0.0 } else { 1.0 }, 1.0),
    );
    let line = k
        .interactive(line)
        .on_press(move |s: &mut AppState| {
            select_row(s, format!("group:{fold}"));
            toggle_fold(s, &fold);
        })
        .focusable(true)
        .no_focus_ring()
        .on_key(rail_key)
        .on_secondary_press(move |s: &mut AppState, point, _| {
            crate::menus::open(s, crate::menus::Target::Group(group), point);
        });
    cursored(s, k, &id, line)
}

fn project_block(
    s: &AppState,
    k: &Kit,
    hues: &std::collections::BTreeMap<String, usize>,
    project: &domain::Project,
) -> El {
    let Some(store) = &s.store else {
        return el();
    };
    let roles = crate::theme::roles(k);
    let hue = hues.get(&project.id.to_string()).copied();
    let mark = project_mark(project);
    let project_id = project.id;
    let target = project_id.to_string();
    let open = !s.projects.rail.folds.contains(&target);
    let workspaces = crate::rail::ordered(&store.workspaces, project_id, &s.projects.rail.order);
    let asking = !open && workspaces.iter().any(|w| wants_you(s, w.id));
    let fold = target.clone();
    let menu = ButtonMenu::Project(project_id);
    let mut head = Row::new(format!("project-row-{project_id}"), project.name.clone())
        .role("treeitem")
        .height(s.chrome_metrics().row)
        .emphasis(Emphasis::Strong)
        .leading(rune::icon(chevron(open), ds::ICON_SMALL).color(k.c.icon_faint))
        .leading(ds::avatar(k, &roles, hue, &mark))
        .action(
            ds::icon_button(
                &crate::glyphs::PLUS,
                format!("New worktree in {}", project.name),
                IconButtonSize::Mini,
            )
            .on_press(move |s: &mut AppState| crate::branch_picker::open(s, project_id))
            .build(k)
            .key(format!("new-worktree-{project_id}")),
        )
        .action(
            ds::icon_button(
                &crate::glyphs::MORE_HORIZONTAL,
                format!("More for {}", project.name),
                IconButtonSize::Mini,
            )
            .expanded(s.menus.button.as_ref() == Some(&menu))
            .on_press(move |s: &mut AppState| {
                s.menus.button = Some(ButtonMenu::Project(project_id));
            })
            .build(k)
            .key(menu.trigger()),
        );
    if asking {
        head = head.trailing(
            rune::icon(&crate::glyphs::NEEDS_YOU, ds::ICON_SMALL).color(roles.needs_you.solid),
        );
    }
    let head = head
        .build(k, &roles)
        .on_press(move |s: &mut AppState| {
            select_row(s, format!("project:{fold}"));
            toggle_fold(s, &fold);
        })
        .focusable(true)
        .no_focus_ring()
        .on_key(rail_key)
        .on_secondary_press(move |s: &mut AppState, point, _| {
            crate::menus::open(s, crate::menus::Target::Project(project_id), point);
        });
    let mut block = col()
        .key(format!("project-{project_id}"))
        .role("group")
        .label(project.name.clone())
        .w_full()
        .gap(ds::SPACE_1)
        .child(cursored(s, k, &format!("project:{target}"), head));
    if open {
        let (shown, idle) =
            crate::rail::split_idle(workspaces, &rail_sessions(s), s.workbench.workspace);
        let mut checkouts: Vec<El> = shown
            .into_iter()
            .map(|workspace| workspace_row(s, k, workspace))
            .collect();
        if !idle.is_empty() {
            let target = crate::rail::idle_target(project_id);
            let listed = s.projects.rail.folds.contains(&target);
            checkouts.push(idle_row(s, k, &target, idle.len(), listed));
            if listed {
                checkouts.push(subtree(
                    k,
                    idle.into_iter()
                        .map(|workspace| workspace_row(s, k, workspace))
                        .collect(),
                ));
            }
        }
        block = block.child(subtree(k, checkouts));
    }
    block
}

/// Where a tree row's children hang: a guide under the parent's leading
/// mark, and one `INDENT` in from it.
const GUIDE_X: f32 = ds::SPACE_2 + ds::ICON_SMALL / 2.0;

fn subtree(k: &Kit, children: Vec<El>) -> El {
    col().w_full().pl(GUIDE_X).child(
        col()
            .w_full()
            .gap(ds::SPACE_1)
            .pl(ds::INDENT - GUIDE_X - ds::SPACE_2)
            .border_sides([0.0, 0.0, 0.0, 1.0], k.c.border_weak)
            .children(children),
    )
}

/// The fold for a project's idle checkouts.
fn idle_row(s: &AppState, k: &Kit, target: &str, count: usize, listed: bool) -> El {
    let roles = crate::theme::roles(k);
    let fold = target.to_string();
    let line = Row::new(
        format!("idle-row-{target}"),
        format!("{count} idle checkouts"),
    )
    .role("treeitem")
    .label(if listed {
        format!("Hide {count} idle checkouts")
    } else {
        format!("Show {count} idle checkouts")
    })
    .height(s.chrome_metrics().row)
    .emphasis(Emphasis::Quiet)
    .leading(rune::icon(chevron(listed), ds::ICON_SMALL).color(k.c.icon_faint))
    .build(k, &roles)
    .on_press(move |s: &mut AppState| {
        select_row(s, fold.clone());
        toggle_fold(s, &fold);
    })
    .focusable(true)
    .no_focus_ring()
    .on_key(rail_key);
    cursored(s, k, target, line)
}

pub fn view(s: &AppState, k: &Kit) -> El {
    let add = ds::icon_button(
        &crate::glyphs::PLUS,
        "Add a project or a group",
        IconButtonSize::Mini,
    )
    .expanded(s.menus.button == Some(ButtonMenu::RailAdd))
    .on_press(|s: &mut AppState| s.menus.button = Some(ButtonMenu::RailAdd))
    .build(k)
    .key(ButtonMenu::RailAdd.trigger());
    let Some(store) = &s.store else {
        return col()
            .key("projects")
            .child(ds::Empty::new("projects-empty", "No replica yet").build(k));
    };
    let mut list = col()
        .key("project-list")
        .role("tree")
        .label("Projects, checkouts and sessions")
        .w_full()
        .gap(ds::SPACE_1);
    let hues = crate::menus::parse_hues(store.app_state_value(crate::menus::PROJECT_HUES_KEY));
    for (group, projects) in crate::rail::grouped(store) {
        let mut section = col().w_full().gap(ds::SPACE_1);
        let open = match group {
            Some(group) => {
                section = section
                    .key(format!("group-{group}"))
                    .role("group")
                    .child(group_header(s, k, group, &projects));
                !s.projects.rail.folds.contains(&group.to_string())
            }
            None => {
                section = section.key("group-none");
                true
            }
        };
        if open {
            for project in projects
                .iter()
                .filter_map(|id| store.projects.iter().find(|p| p.id == *id))
            {
                section = section.child(project_block(s, k, &hues, project));
            }
        }
        list = list.child(section);
    }
    if store.projects.is_empty() {
        list = list.child(
            ds::Empty::new("projects-empty", "No projects")
                .why("Add a folder to start.")
                .build(k),
        );
    }
    let mut column = col()
        .key("projects")
        .w_full()
        .h_full()
        .min_h(0.0)
        .gap(ds::SPACE_4);
    if let Some(block) = needs_you_block(s, k) {
        column = column.child(block);
    }
    column
        .child(
            ds::section_header(k, "Projects", Some(store.projects.len()), Some(add))
                .key("projects-bar"),
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
                .on_wheel(crate::pages::wheel)
                .child(list),
        )
}

/// The actions pinned to a project, under its chosen checkout; the rest
/// are behind the row's Run menu.
fn pinned_actions(s: &AppState, k: &Kit, id: WorkspaceId) -> Option<El> {
    let mut pinned: Vec<_> = s
        .actions_for(id)
        .into_iter()
        .filter(|action| action.pinned)
        .collect();
    if pinned.is_empty() {
        return None;
    }
    pinned.sort_by_key(|action| action.position);
    let mut actions = row()
        .key(format!("pinned-actions-{id}"))
        .w_full()
        .gap(ds::SPACE_2)
        .flex_wrap()
        .pl(ds::SPACE_2)
        .py(ds::SPACE_2);
    for action in pinned {
        let running = s.action_running(id, &action.id);
        let action_id = action.id;
        actions = actions.child(
            Button::new(action.label.clone())
                .icon(if running {
                    &crate::glyphs::STOP
                } else {
                    &crate::glyphs::PLAY
                })
                .variant(if running {
                    ButtonVariant::Neutral
                } else {
                    ButtonVariant::Outline
                })
                .control_size(ControlSize::Small)
                .on_press(move |s: &mut AppState| s.run_action(id, action_id))
                .build(k)
                .key(format!("action-{id}-{}", action.id))
                .label(if running {
                    format!("{}, running", action.label)
                } else {
                    action.label.clone()
                }),
        );
    }
    Some(actions)
}

/// What a checkout opens, as icons the row reveals under the pointer and
/// keeps while it is chosen.
fn checkout_tools(s: &AppState, k: &Kit, id: WorkspaceId, name: &str) -> [El; 4] {
    let run = ButtonMenu::Run(id);
    [
        ds::icon_button(
            &crate::glyphs::TERMINAL,
            format!("New shell in {name}"),
            IconButtonSize::Mini,
        )
        .on_press(move |s: &mut AppState| s.shell_in(id))
        .build(k)
        .key(format!("shell-{id}")),
        ds::icon_button(
            &crate::glyphs::GIT,
            format!("Changes in {name}"),
            IconButtonSize::Mini,
        )
        .on_press(move |s: &mut AppState| {
            s.inspect(id, CenterMode::Changes);
        })
        .build(k)
        .key(format!("changes-{id}")),
        ds::icon_button(
            &crate::glyphs::FILES,
            format!("Files in {name}"),
            IconButtonSize::Mini,
        )
        .on_press(move |s: &mut AppState| {
            s.inspect(id, CenterMode::Files);
        })
        .build(k)
        .key(format!("files-{id}")),
        ds::icon_button(
            &crate::glyphs::PLAY,
            format!("Actions in {name}"),
            IconButtonSize::Mini,
        )
        .expanded(s.menus.button == Some(run.clone()))
        .on_press(move |s: &mut AppState| s.menus.button = Some(ButtonMenu::Run(id)))
        .build(k)
        .key(run.trigger()),
    ]
}

fn workspace_row(s: &AppState, k: &Kit, workspace: &Workspace) -> El {
    let id = workspace.id;
    let roles = crate::theme::roles(k);
    let kind = match workspace.kind {
        WorkspaceKind::Main => "main checkout",
        _ => "worktree",
    };
    let branch = workspace
        .branch
        .clone()
        .unwrap_or_else(|| "detached".into());
    let name = workspace.label();
    let title = workspace.branch.clone().unwrap_or_else(|| name.clone());
    let fold = id.to_string();
    let open = !s.projects.rail.folds.contains(&fold);
    let current = s.workbench.workspace == Some(id);
    let session_here = s
        .active
        .and_then(|active| s.rows.iter().find(|row| row.id == active))
        .is_some_and(|row| row.workspace_id == id);
    let mut line = Row::new(format!("checkout-{id}"), title)
        .label(format!("Show {name}, {kind}"))
        .height(s.chrome_metrics().row)
        .selected(current && !session_here)
        .leading(
            rune::icon(
                if workspace.kind == WorkspaceKind::Main {
                    &crate::glyphs::GIT
                } else {
                    &crate::glyphs::WORKTREE
                },
                ds::ICON_ROW,
            )
            .color(k.c.icon_muted),
        );
    if let Some(store) = &s.store
        && let Some(pr) = crate::sync::pull_request_for(
            &store.pull_requests.pull_requests,
            workspace.project_id,
            workspace.branch.as_deref(),
        )
    {
        line = line.trailing(
            ds::chip(k, &roles, format!("#{}", pr.number), Tone::Accent).label(pr_label(pr)),
        );
    }
    if let Some((label, tone)) = checkout_chip(&workspace.status) {
        line = line.trailing(ds::chip(k, &roles, label, tone));
    }
    let chevron = el()
        .key(format!("fold-{id}"))
        .role("button")
        .label(if open { "Fold" } else { "Unfold" })
        .padding(ds::SPACE_1)
        .radius(k.radius.sm)
        .hover(k.c.hover)
        .on_press(move |s: &mut AppState| toggle_fold(s, &fold))
        .child(rune::icon(chevron(open), ds::ICON_SMALL).color(k.c.icon_faint));
    for tool in checkout_tools(s, k, id, &name) {
        line = line.action(tool);
    }
    line = if open {
        line.action(chevron)
    } else {
        line.trailing(chevron)
    };
    let card = line
        .build(k, &roles)
        .description(status_line(workspace))
        .on_press(move |s: &mut AppState| {
            select_row(s, format!("workspace:{id}"));
            s.focus_checkout(id);
        })
        .focusable(true)
        .no_focus_ring()
        .on_key(rail_key)
        .on_secondary_press(move |s: &mut AppState, point, _| {
            crate::menus::open(s, crate::menus::Target::Checkout(id), point);
        });
    let mut block = col()
        .key(format!("workspace-{id}"))
        .role("group")
        .label(format!("{name} · {branch}"))
        .w_full()
        .gap(ds::SPACE_1)
        .child(cursored(s, k, &format!("workspace:{id}"), card))
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
                        .pl(ds::INDENT)
                }),
        );
    if open {
        let mut children: Vec<El> = s
            .rows
            .iter()
            .enumerate()
            .filter(|(_, row)| row.workspace_id == id)
            .map(|(index, row)| {
                cursored(
                    s,
                    k,
                    &format!("session:{}", row.id),
                    crate::ui::session_row(s, row, index, 0, k),
                )
            })
            .collect();
        if current {
            children.extend(pinned_actions(s, k, id));
        }
        if !children.is_empty() {
            block = block.child(subtree(k, children));
        }
    }
    block
}

/// Every dialog the rail and its menus open, as overlay layers.
pub fn dialogs(s: &AppState, k: &Kit, layer: &mut Vec<El>) {
    crate::branch_picker::dialog(s, k, layer);
    removal_dialog(s, k, layer);
    ignores_dialog(s, k, layer);
    crate::shares::dialogs(s, k, layer);
    crate::actions_editor::dialog(s, k, layer);
}

/// Sends the removal the dialog stands for: forced once the daemon has said
/// why not.
pub fn confirm_removal(s: &mut AppState) {
    let Some(removal) = s.projects.removal.take() else {
        return;
    };
    s.projects.refused = None;
    crate::project_ops::send(
        s,
        crate::project_ops::Op::RemoveWorktree {
            workspace: removal.workspace,
            force: removal.reason.is_some(),
        },
    );
}

/// A confirmation, then the daemon's reasons
/// and an explicit force when it refuses.
fn removal_dialog(s: &AppState, k: &Kit, layer: &mut Vec<El>) {
    let (Some(removal), Some(store)) = (&s.projects.removal, &s.store) else {
        return;
    };
    let workspace = store.workspaces.iter().find(|w| w.id == removal.workspace);
    let name = workspace.map_or_else(|| "this worktree".to_string(), Workspace::label);
    let managed = workspace.is_none_or(|w| w.managed_by_app);
    let mut body = col().key("remove-worktree-body").gap(crate::ds::SPACE_4);
    match &removal.reason {
        None => {
            body = body.child(
                text(if managed {
                    "Forge Node will remove this worktree from disk. The branch and its commits \
                     are kept."
                } else {
                    "Forge Node did not create this worktree, so it only forgets it: the \
                     directory and branch are kept, and Forge Node will not adopt it again."
                })
                .font(k.ty.small)
                .color(k.c.text_base)
                .wrap(),
            );
        }
        Some(reason) => {
            body = body.child(
                text("Forge Node found conditions that make removal unsafe:")
                    .font(k.ty.small)
                    .color(k.c.text_base),
            );
            for line in removal_block(reason) {
                body = body.child(
                    text(format!("• {line}"))
                        .role("status")
                        .font(k.ty.small)
                        .color(k.c.warning.fg)
                        .wrap(),
                );
            }
            body = body.child(
                text(if managed {
                    "Removing it anyway discards the worktree and the state listed above. The \
                     branch is kept."
                } else {
                    "Removing it anyway only forgets Forge Node's entry, permanently. The \
                     directory, branch, and local changes are kept."
                })
                .font(k.ty.small)
                .color(k.c.text_base)
                .wrap(),
            );
        }
    }
    crate::modal::Modal::new("remove-worktree", format!("Remove {name}?"))
        .width(crate::modal::WIDTH_PROMPT)
        .on_close(|s: &mut AppState| s.projects.removal = None)
        .child(body)
        .child(
            row()
                .pt(12.0)
                .gap(crate::ds::SPACE_4)
                .child(el().grow(1.0))
                .child(
                    Button::new("Cancel")
                        .variant(ButtonVariant::Ghost)
                        .on_press(|s: &mut AppState| s.projects.removal = None)
                        .build(k)
                        .key("remove-worktree-cancel"),
                )
                .child(
                    Button::new(if removal.reason.is_some() {
                        "Remove anyway"
                    } else {
                        "Remove worktree"
                    })
                    .variant(ButtonVariant::Danger)
                    .on_press(confirm_removal)
                    .build(k)
                    .key("remove-worktree-confirm"),
                ),
        )
        .build(s, k, layer);
}

/// The project's rules without the one at `path`: the daemon replaces the
/// whole set.
#[must_use]
pub fn without_ignore(
    rules: &[domain::WorktreeIgnore],
    project: ProjectId,
    path: &std::path::Path,
) -> Vec<domain::WorktreeIgnore> {
    rules
        .iter()
        .filter(|rule| rule.project_id == project && rule.path != path)
        .cloned()
        .collect()
}

/// Each rule that keeps worktrees out of the
/// rail, with a way to stop it. Nothing on disk changed, so the next scan
/// finds what a dropped rule covered.
fn ignores_dialog(s: &AppState, k: &Kit, layer: &mut Vec<El>) {
    let (Some(project), Some(store)) = (s.projects.ignores, &s.store) else {
        return;
    };
    let name = store
        .projects
        .iter()
        .find(|p| p.id == project)
        .map_or_else(String::new, |p| p.name.clone());
    let rules: Vec<&domain::WorktreeIgnore> = store
        .worktree_ignores
        .iter()
        .filter(|rule| rule.project_id == project)
        .collect();
    let mut body = col().key("worktree-ignores").gap(crate::ds::SPACE_4).child(
        text(if rules.is_empty() {
            "Forge Node adopts every worktree this repository lists, wherever it lives."
        } else {
            "Forge Node never adopts a worktree at these paths. Nothing on disk was changed, so \
             stopping here lets the next scan find them again."
        })
        .font(k.ty.small)
        .color(k.c.text_base)
        .wrap(),
    );
    for (index, rule) in rules.iter().enumerate() {
        let path = rule.path.clone();
        body = body.child(
            row()
                .key(format!("worktree-ignore-{index}"))
                .w_full()
                .gap(crate::ds::SPACE_4)
                .items_center()
                .child(
                    col()
                        .grow(1.0)
                        .min_w(0.0)
                        .gap(crate::ds::SPACE_1)
                        .child(
                            text(rule.path.display().to_string())
                                .font(k.ty.mono)
                                .color(k.c.text_base)
                                .truncate(),
                        )
                        .child(
                            text(match rule.scope {
                                domain::IgnoreScope::Subtree => {
                                    "This folder and every worktree under it"
                                }
                                _ => "This worktree only",
                            })
                            .font(k.ty.text_xsmall)
                            .color(k.c.text_faint),
                        ),
                )
                .child(
                    Button::new("Stop ignoring")
                        .variant(ButtonVariant::Outline)
                        .control_size(ControlSize::Small)
                        .on_press(move |s: &mut AppState| {
                            let Some(store) = &s.store else {
                                return;
                            };
                            let rules = without_ignore(&store.worktree_ignores, project, &path);
                            crate::menus::set_ignores(s, project, rules);
                        })
                        .build(k)
                        .key(format!("worktree-ignore-stop-{index}")),
                ),
        );
    }
    crate::modal::Modal::new(
        "worktree-ignores-dialog",
        format!("Ignored worktrees · {name}"),
    )
    .width(crate::modal::WIDTH_STANDARD)
    .on_close(|s: &mut AppState| s.projects.ignores = None)
    .child(body)
    .child(
        row().pt(12.0).child(el().grow(1.0)).child(
            Button::new("Done")
                .variant(ButtonVariant::Outline)
                .on_press(|s: &mut AppState| s.projects.ignores = None)
                .build(k)
                .key("worktree-ignores-done"),
        ),
    )
    .build(s, k, layer);
}

/// The avatar text: the icon the user set, else the name's first letter.
fn project_mark(project: &domain::Project) -> String {
    match project.icon.as_deref().map(str::trim) {
        Some(icon) if !icon.is_empty() => icon.chars().take(2).collect(),
        _ => project
            .name
            .chars()
            .next()
            .map(|c| c.to_uppercase().collect())
            .unwrap_or_default(),
    }
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
    fn the_checkout_chip_waits_for_git_and_names_what_it_measured() {
        assert_eq!(checkout_chip(&WorkspaceStatus::default()), None);
        let measured = |dirty, ahead, behind| WorkspaceStatus {
            dirty,
            ahead,
            behind,
            measured_at: Some(domain::Timestamp::now()),
            ..WorkspaceStatus::default()
        };
        assert_eq!(
            checkout_chip(&measured(false, None, None)),
            Some(("clean".into(), Tone::Neutral))
        );
        assert_eq!(
            checkout_chip(&measured(true, Some(2), Some(0))),
            Some(("changes ↑2".into(), Tone::Warning))
        );
        assert_eq!(
            checkout_chip(&measured(false, Some(0), Some(3))),
            Some(("↓3".into(), Tone::Info))
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

    /// The rail head's "+" drops its menu from the button; a group header
    /// folds its projects away and the fold is stored; a focused row walks
    /// the rail with `j` and `l`.
    #[test]
    fn the_rail_menu_folds_and_keys_work_on_the_drawn_rail() {
        use std::path::Path;
        use std::sync::Arc;

        use rune::{Env, Key, KeyEvent, Mods, Runtime};

        use crate::connection::{Command, ConnectionHandle, Metrics, fixture_store};
        use crate::terminal::SharedTerminal;

        let store =
            fixture_store(&Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/snapshot.json"))
                .expect("fixture snapshot decodes");
        let group = store.project_groups[0].id;
        let project = store.projects[0].id;
        let metrics = Arc::new(Metrics::default());
        let mut state = AppState::for_bench(store, Arc::clone(&metrics), SharedTerminal::new());
        let (handle, rx) = ConnectionHandle::test_handle(metrics);
        state.connection = Some(handle);
        state.show_panel(crate::workbench::Panel::Projects);
        let mut rt = Runtime::new(
            state,
            crate::ui::view,
            (1180.0, 760.0),
            Env {
                system_dark: true,
                scale: 1.0,
            },
        );
        let settle = |rt: &mut Runtime<AppState>| {
            rt.frame(0.0);
            for _ in 0..20 {
                rt.frame(1.0 / 60.0);
            }
        };
        let click = |rt: &mut Runtime<AppState>, key: &str| {
            let (x, y) = rt.find(key).unwrap_or_else(|| panic!("{key} is drawn"));
            rt.pointer_down(x, y);
            rt.pointer_up(x, y);
        };
        settle(&mut rt);
        click(&mut rt, &ButtonMenu::RailAdd.trigger());
        settle(&mut rt);
        let items: Vec<String> = rt
            .elements()
            .into_iter()
            .filter(|e| e.role == "menuitem")
            .filter_map(|e| e.label)
            .collect();
        assert_eq!(items, ["Add a project…", "New group…"]);
        rt.key(KeyEvent {
            key: Key::Escape,
            mods: Mods::default(),
        });
        settle(&mut rt);

        let project_row = format!("project-row-{project}");
        click(&mut rt, &format!("group-row-{group}"));
        settle(&mut rt);
        assert!(
            rt.find(&project_row).is_none(),
            "a folded group hides its projects"
        );
        assert!(rx.try_iter().any(|command| matches!(
            command,
            Command::SetAppState { key, value } if key == crate::rail::FOLDS_KEY && value.contains(&group.to_string())
        )));
        click(&mut rt, &format!("group-row-{group}"));
        settle(&mut rt);
        assert!(rt.find(&project_row).is_some());

        click(&mut rt, &project_row);
        settle(&mut rt);
        assert_eq!(
            rt.state.projects.rail.cursor,
            Some(format!("project:{project}"))
        );
        assert!(
            !rt.state.projects.rail.keyed,
            "a click moves the cursor without marking the row"
        );
        assert!(
            rt.state.projects.rail.folds.contains(&project.to_string()),
            "a click folds the project"
        );
        let typed = |rt: &mut Runtime<AppState>, letter: &str| {
            rt.key(KeyEvent {
                key: Key::Text(letter.to_string()),
                mods: Mods::default(),
            });
            settle(rt);
        };
        typed(&mut rt, "l");
        assert!(
            !rt.state.projects.rail.folds.contains(&project.to_string()),
            "l unfolds the focused row's project"
        );
        typed(&mut rt, "j");
        let cursor = rt.state.projects.rail.cursor.clone().unwrap_or_default();
        assert!(
            cursor.starts_with("workspace:"),
            "j steps to the first checkout: {cursor}"
        );
        assert!(rt.state.projects.rail.keyed, "a key marks the cursor");
        typed(&mut rt, "h");
        assert!(
            rt.state
                .projects
                .rail
                .folds
                .contains(cursor.trim_start_matches("workspace:")),
            "h folds the checkout"
        );
        typed(&mut rt, "k");
        assert_eq!(
            rt.state.projects.rail.cursor,
            Some(format!("project:{project}"))
        );
    }
}
