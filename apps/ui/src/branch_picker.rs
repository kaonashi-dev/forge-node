//! New worktree: the project's local and remote
//! branches over `ListBranches`, a free-text branch to create, an optional
//! folder name, and what to open once the daemon's `WorkspaceCreated` lands.

use domain::{AgentProfileId, AgentProviderId, ProjectId, Workspace};
use rune::{El, TextEdit, col, el, row, text};
use rune_kit::{Button, ButtonVariant, ControlSize, Dialog, Kit, TextInput};

use crate::app::AppState;
use crate::project_ops::{Op, send};

/// `git-service::worktree::MAX_SLUG_LEN`.
const MAX_SLUG_LEN: usize = 64;
const FALLBACK_SLUG: &str = "worktree";
/// Shown rows; the query narrows a long list rather than a scroll.
const SHOWN: usize = 12;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choice {
    pub branch: String,
    pub base: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub label: String,
    pub group: &'static str,
    pub note: Option<String>,
    pub enabled: bool,
    pub choice: Option<Choice>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Launch {
    Nothing,
    Shell,
    Agent(AgentProviderId, Option<AgentProfileId>),
}

pub struct Picker {
    pub project: ProjectId,
    pub query: TextEdit,
    pub name: TextEdit,
    pub branches: Option<Result<client::Branches, String>>,
    pub cursor: usize,
    pub launch: Launch,
    /// The branch a create is in flight for.
    pub pending: Option<String>,
    pub error: Option<String>,
    pub fetching: bool,
}

/// A mirror of `git_service::worktree::slugify`, only for the placeholder:
/// the daemon still derives the real name and may number it.
#[must_use]
pub fn slug_from_branch(branch: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for ch in branch.chars() {
        let kept = ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-');
        let mapped = if kept { ch } else { '-' };
        if mapped == '-' {
            if dash {
                continue;
            }
            dash = true;
        } else {
            dash = false;
        }
        out.push(mapped);
        if out.len() >= MAX_SLUG_LEN {
            break;
        }
    }
    if out.is_empty() || out == "." || out == ".." {
        FALLBACK_SLUG.to_string()
    } else {
        out
    }
}

#[must_use]
pub fn plausible_branch_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('-')
        && !name.starts_with('.')
        && !name.ends_with('/')
        && !name.ends_with(".lock")
        && !name.contains("..")
        && !name.contains("//")
        && !name.chars().any(|c| {
            c.is_whitespace() || matches!(c, '~' | '^' | ':' | '?' | '*' | '[' | ']' | '\\')
        })
}

/// A branch to create when the query names none,
/// then local branches (one already checked out is offered disabled, saying
/// where), then remote ones that have no local twin.
#[must_use]
pub fn entries(query: &str, branches: &client::Branches, workspaces: &[Workspace]) -> Vec<Entry> {
    let trimmed = query.trim();
    let needle = crate::fuzzy::needle_of(trimmed);
    let matches =
        |name: &str| trimmed.is_empty() || crate::fuzzy::score_with(&needle, name).is_some();
    let mut out = Vec::new();
    if !trimmed.is_empty() && !branches.branches.iter().any(|b| b.name == trimmed) {
        let base = branches.default_branch.clone();
        out.push(Entry {
            label: trimmed.to_string(),
            group: "Create",
            note: Some(match &base {
                Some(base) => format!("new branch from {base}"),
                None => "new branch from HEAD".to_string(),
            }),
            enabled: plausible_branch_name(trimmed),
            choice: Some(Choice {
                branch: trimmed.to_string(),
                base,
            }),
        });
    }
    for branch in branches.branches.iter().filter(|b| !b.is_remote()) {
        if !matches(&branch.name) {
            continue;
        }
        let holder = branch.checked_out_in.map(|id| {
            workspaces
                .iter()
                .find(|w| w.id == id)
                .map(|w| {
                    w.display_name
                        .clone()
                        .or_else(|| w.branch.clone())
                        .unwrap_or_else(|| w.path.display().to_string())
                })
                .map_or_else(
                    || "already open".to_string(),
                    |at| format!("already open in {at}"),
                )
        });
        out.push(Entry {
            label: branch.name.clone(),
            group: "Local branches",
            note: holder.clone().or_else(|| branch.subject.clone()),
            enabled: holder.is_none(),
            choice: holder.is_none().then(|| Choice {
                branch: branch.name.clone(),
                base: None,
            }),
        });
    }
    for branch in branches.branches.iter().filter(|b| b.is_remote()) {
        if !matches(&branch.name)
            || branches
                .branches
                .iter()
                .any(|other| !other.is_remote() && other.name == branch.name)
        {
            continue;
        }
        let start = branch.start_point();
        out.push(Entry {
            label: branch.name.clone(),
            group: "Remote branches",
            note: Some(match &branch.subject {
                Some(subject) => format!("{start} · {subject}"),
                None => start.clone(),
            }),
            enabled: true,
            choice: Some(Choice {
                branch: branch.name.clone(),
                base: Some(start),
            }),
        });
    }
    out
}

/// One line naming what provisioning will do, or nothing when it will not.
#[must_use]
pub fn share_summary(rules: &[domain::ShareRule], project: ProjectId) -> Option<String> {
    let mut counts = [0usize; 4];
    for rule in rules
        .iter()
        .filter(|r| r.project_id == project && r.enabled)
    {
        let at = match rule.strategy {
            domain::ShareStrategy::Link => 0,
            domain::ShareStrategy::Copy => 1,
            domain::ShareStrategy::Clone => 2,
            _ => 3,
        };
        counts[at] += 1;
    }
    let words: Vec<String> = [
        ("one file linked", "files linked"),
        ("one file copied", "files copied"),
        ("one path cloned", "paths cloned"),
        ("one command run", "commands run"),
    ]
    .iter()
    .zip(counts)
    .filter(|(_, n)| *n > 0)
    .map(|((one, many), n)| {
        if n == 1 {
            (*one).to_string()
        } else {
            format!("{n} {many}")
        }
    })
    .collect();
    (!words.is_empty()).then(|| format!("This checkout will be set up with {}.", words.join(", ")))
}

impl Picker {
    fn rows(&self, workspaces: &[Workspace]) -> Vec<Entry> {
        match &self.branches {
            Some(Ok(branches)) => entries(&self.query.text, branches, workspaces),
            _ => Vec::new(),
        }
    }
}

/// Opens the picker on `project` and asks for its branches.
pub fn open(s: &mut AppState, project: ProjectId) {
    let mut query = TextEdit::new("");
    query.focused = true;
    s.projects.picker = Some(Picker {
        project,
        query,
        name: TextEdit::new(""),
        branches: None,
        cursor: 0,
        launch: Launch::Shell,
        pending: None,
        error: None,
        fetching: false,
    });
    send(s, Op::ListBranches(project));
}

pub fn apply_branches(
    s: &mut AppState,
    project: ProjectId,
    result: Result<crate::project_ops::Answer, String>,
) {
    let Some(picker) = s.projects.picker.as_mut().filter(|p| p.project == project) else {
        return;
    };
    picker.branches = Some(match result {
        Ok(crate::project_ops::Answer::Branches(branches)) => Ok(branches),
        Ok(_) => Err("unexpected answer".to_string()),
        Err(error) => Err(error),
    });
    picker.cursor = 0;
}

pub fn failed(s: &mut AppState, project: ProjectId, branch: &str, error: String) {
    if let Some(picker) = s
        .projects
        .picker
        .as_mut()
        .filter(|p| p.project == project && p.pending.as_deref() == Some(branch))
    {
        picker.pending = None;
        picker.error = Some(error);
    }
}

/// The checkout the picker waited for has arrived: open what was chosen in
/// it and close. Git checks a branch out once per repository, so the branch
/// names the row.
pub fn arrived(s: &mut AppState, workspace: &Workspace) {
    let Some(picker) = s.projects.picker.as_ref() else {
        return;
    };
    if picker.project != workspace.project_id
        || picker.pending.is_none()
        || picker.pending.as_deref() != workspace.branch.as_deref()
    {
        return;
    }
    let launch = picker.launch.clone();
    s.projects.picker = None;
    match launch {
        Launch::Nothing => {}
        Launch::Shell => s.shell_in(workspace.id),
        Launch::Agent(provider, profile) => {
            if let Some(connection) = &s.connection
                && connection
                    .create_agent(workspace.id, provider, profile, None, false)
                    .is_ok()
            {
                s.workbench.show(crate::workbench::CenterMode::Session);
            }
        }
    }
}

pub fn fetched(s: &mut AppState, project: ProjectId) {
    if let Some(picker) = s.projects.picker.as_mut().filter(|p| p.project == project) {
        picker.fetching = false;
        send(s, Op::ListBranches(project));
    }
}

pub fn choose(s: &mut AppState) {
    let workspaces = s
        .store
        .as_ref()
        .map(|store| store.workspaces.clone())
        .unwrap_or_default();
    let Some(picker) = s.projects.picker.as_mut() else {
        return;
    };
    if picker.pending.is_some() {
        return;
    }
    let rows = picker.rows(&workspaces);
    let Some(choice) = rows
        .get(picker.cursor)
        .filter(|entry| entry.enabled)
        .and_then(|entry| entry.choice.clone())
    else {
        return;
    };
    let folder = picker.name.text.trim().to_string();
    picker.error = None;
    picker.pending = Some(choice.branch.clone());
    let op = Op::CreateWorktree {
        project: picker.project,
        branch: choice.branch,
        base: choice.base,
        name: (!folder.is_empty()).then_some(folder),
    };
    if !send(s, op)
        && let Some(picker) = s.projects.picker.as_mut()
    {
        picker.pending = None;
    }
}

fn step(s: &mut AppState, by: isize) {
    let workspaces = s
        .store
        .as_ref()
        .map(|store| store.workspaces.clone())
        .unwrap_or_default();
    if let Some(picker) = s.projects.picker.as_mut() {
        let count = picker.rows(&workspaces).len().min(SHOWN);
        if count > 0 {
            picker.cursor = picker.cursor.saturating_add_signed(by).min(count - 1);
        }
    }
}

fn fetch(s: &mut AppState) {
    let Some(project) = s.projects.picker.as_ref().map(|p| p.project) else {
        return;
    };
    if let Some(connection) = &s.connection
        && connection.fetch(project).is_ok()
    {
        s.sync.fetch_started(project);
        if let Some(picker) = s.projects.picker.as_mut() {
            picker.fetching = true;
            picker.error = None;
        }
    }
}

fn launch_options(s: &AppState) -> Vec<(String, Launch)> {
    let mut out = vec![
        ("Nothing".to_string(), Launch::Nothing),
        ("New Terminal".to_string(), Launch::Shell),
    ];
    for row in s.installed_agents() {
        out.push((
            format!("New {}", row.label),
            Launch::Agent(row.provider, row.profile),
        ));
    }
    out
}

pub fn dialog(s: &AppState, k: &Kit, layer: &mut Vec<El>) {
    let (Some(picker), Some(store)) = (&s.projects.picker, &s.store) else {
        return;
    };
    let busy = picker.pending.is_some();
    let project_name = store
        .projects
        .iter()
        .find(|p| p.id == picker.project)
        .map_or_else(String::new, |p| p.name.clone());
    let query = TextInput::new(&picker.query)
        .label("Branch name or filter")
        .placeholder("Branch name or filter…")
        .on_edit(|s: &mut AppState| match s.projects.picker.as_mut() {
            Some(picker) => {
                picker.cursor = 0;
                &mut picker.query
            }
            None => &mut s.scratch_edit,
        })
        .on_key(|s: &mut AppState, ev, _| match ev.key {
            rune::Key::Enter => {
                choose(s);
                true
            }
            rune::Key::Down => {
                step(s, 1);
                true
            }
            rune::Key::Up => {
                step(s, -1);
                true
            }
            rune::Key::Escape => {
                s.projects.picker = None;
                true
            }
            _ => false,
        })
        .build(k)
        .key("branch-picker-query");
    let rows = picker.rows(&store.workspaces);
    let mut list = col()
        .key("branch-picker-list")
        .role("listbox")
        .w_full()
        .gap(1.0);
    let mut last_group = "";
    for (index, entry) in rows.iter().take(SHOWN).enumerate() {
        if entry.group != last_group {
            last_group = entry.group;
            list = list.child(
                text(entry.group)
                    .font(k.ty.text_xsmall_medium)
                    .color(k.c.text_faint)
                    .pt(6.0)
                    .px(8.0),
            );
        }
        let current = index == picker.cursor;
        let mut line = row()
            .key(format!("branch-entry-{index}"))
            .role("option")
            .label(entry.label.clone())
            .w_full()
            .px(8.0)
            .py(4.0)
            .gap(8.0)
            .radius(5.0)
            .items_center()
            .fill(if current {
                k.c.surface_hover
            } else {
                rune::Color::TRANSPARENT
            })
            .child(
                text(entry.label.clone())
                    .font(k.ty.small_medium)
                    .color(if entry.enabled {
                        k.c.text_base
                    } else {
                        k.c.text_faint
                    })
                    .truncate()
                    .min_w(0.0),
            );
        if let Some(note) = &entry.note {
            line = line.child(
                text(note.clone())
                    .font(k.ty.text_xsmall)
                    .color(k.c.text_faint)
                    .truncate()
                    .min_w(0.0)
                    .grow(1.0),
            );
        }
        if entry.enabled {
            line = line.hover(k.c.hover).on_press(move |s: &mut AppState| {
                if let Some(picker) = s.projects.picker.as_mut() {
                    picker.cursor = index;
                }
                choose(s);
            });
        }
        list = list.child(line.checked(current));
    }
    if rows.is_empty() {
        let empty = match &picker.branches {
            None => "Loading branches…".to_string(),
            Some(Err(error)) => format!("Branches could not be read: {error}"),
            Some(Ok(_)) => "No branches match.".to_string(),
        };
        list = list.child(text(empty).font(k.ty.small).color(k.c.text_faint).px(8.0));
    }
    let derived = rows
        .get(picker.cursor)
        .and_then(|entry| entry.choice.as_ref())
        .map(|choice| slug_from_branch(&choice.branch));
    let name = TextInput::new(&picker.name)
        .label(match &derived {
            Some(slug) => format!("Folder name · optional, defaults to {slug}"),
            None => "Folder name · optional".to_string(),
        })
        .placeholder(derived.clone().unwrap_or_default())
        .on_edit(|s: &mut AppState| match s.projects.picker.as_mut() {
            Some(picker) => &mut picker.name,
            None => &mut s.scratch_edit,
        })
        .build(k)
        .key("branch-picker-name");
    let mut launches = row().key("branch-picker-launch").gap(4.0).flex_wrap();
    for (index, (label, launch)) in launch_options(s).into_iter().enumerate() {
        let chosen = picker.launch == launch;
        launches = launches.child(
            Button::new(label)
                .variant(if chosen {
                    ButtonVariant::Outline
                } else {
                    ButtonVariant::GhostMuted
                })
                .control_size(ControlSize::Small)
                .on_press(move |s: &mut AppState| {
                    if let Some(picker) = s.projects.picker.as_mut() {
                        picker.launch = launch.clone();
                    }
                })
                .build(k)
                .key(format!("branch-launch-{index}"))
                .checked(chosen),
        );
    }
    let mut body = col()
        .gap(10.0)
        .child(
            text(format!("New worktree · {project_name}"))
                .font(k.ty.small_medium)
                .color(k.c.text_muted),
        )
        .child(query)
        .child(list)
        .child(name)
        .child(
            col()
                .gap(4.0)
                .child(text("Open with").font(k.ty.small).color(k.c.text_muted))
                .child(launches),
        );
    if let Some(summary) = share_summary(&store.worktree_shares, picker.project) {
        body = body.child(
            text(summary)
                .key("branch-picker-shares")
                .font(k.ty.text_xsmall)
                .color(k.c.text_faint)
                .wrap(),
        );
    }
    if let Some(error) = &picker.error {
        body = body.child(
            text(error.clone())
                .key("branch-picker-error")
                .role("status")
                .font(k.ty.text_xsmall)
                .color(k.c.danger.fg)
                .wrap(),
        );
    }
    let has_remote = matches!(&picker.branches, Some(Ok(b)) if !b.remotes.is_empty());
    let can_create = rows.get(picker.cursor).is_some_and(|entry| entry.enabled) && !busy;
    let mut footer = row().pt(12.0).gap(8.0);
    if has_remote {
        footer = footer.child(
            Button::new(if picker.fetching {
                "Fetching…"
            } else {
                "Fetch from remote"
            })
            .variant(ButtonVariant::Ghost)
            .disabled(picker.fetching || busy)
            .on_press(fetch)
            .build(k)
            .key("branch-picker-fetch"),
        );
    }
    footer = footer
        .child(el().grow(1.0))
        .child(
            Button::new("Cancel")
                .variant(ButtonVariant::Ghost)
                .disabled(busy)
                .on_press(|s: &mut AppState| s.projects.picker = None)
                .build(k)
                .key("branch-picker-cancel"),
        )
        .child(
            Button::new(if busy {
                "Creating…"
            } else {
                "Create worktree"
            })
            .variant(ButtonVariant::Contrast)
            .disabled(!can_create)
            .on_press(choose)
            .build(k)
            .key("worktree-create"),
        );
    Dialog::new("new-worktree", "New worktree")
        .open(true)
        .fit()
        .width(520.0)
        .click_outside(!busy)
        .on_close(|s: &mut AppState| s.projects.picker = None)
        .child(body)
        .child(footer)
        .build(k, layer);
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::{BranchRef, RefScope, WorkspaceId, WorkspaceKind};

    fn branch(name: &str, scope: RefScope, holder: Option<WorkspaceId>) -> BranchRef {
        BranchRef {
            name: name.into(),
            scope,
            upstream: None,
            committed_at: None,
            subject: Some(format!("tip of {name}")),
            checked_out_in: holder,
        }
    }

    #[test]
    fn the_picker_lists_create_then_local_then_remote_without_twins() {
        let project = ProjectId::new();
        let main = Workspace {
            id: WorkspaceId::new(),
            project_id: project,
            kind: WorkspaceKind::Main,
            path: "/r".into(),
            branch: Some("main".into()),
            display_name: None,
            managed_by_app: false,
            created_at: domain::Timestamp::now(),
            status: domain::WorkspaceStatus::default(),
        };
        let origin = || RefScope::Remote {
            remote: "origin".into(),
        };
        let branches = client::Branches {
            branches: vec![
                branch("main", RefScope::Local, Some(main.id)),
                branch("feature/a", RefScope::Local, None),
                branch("main", origin(), None),
                branch("fix/b", origin(), None),
            ],
            remotes: Vec::new(),
            default_branch: Some("main".into()),
        };
        let all = entries("", &branches, std::slice::from_ref(&main));
        let shown: Vec<(&str, &str, bool)> = all
            .iter()
            .map(|e| (e.group, e.label.as_str(), e.enabled))
            .collect();
        assert_eq!(
            shown,
            [
                ("Local branches", "main", false),
                ("Local branches", "feature/a", true),
                ("Remote branches", "fix/b", true),
            ],
            "a remote with a local twin is left out"
        );
        assert_eq!(all[0].note.as_deref(), Some("already open in main"));
        assert_eq!(
            all[2].choice,
            Some(Choice {
                branch: "fix/b".into(),
                base: Some("origin/fix/b".into())
            })
        );

        let typed = entries("new thing", &branches, &[]);
        assert_eq!(typed[0].group, "Create");
        assert!(!typed[0].enabled, "a space is no branch name");
        let typed = entries("feat/x", &branches, &[]);
        assert_eq!(typed[0].note.as_deref(), Some("new branch from main"));
        assert!(typed[0].enabled);
        assert_eq!(
            entries("feature/a", &branches, &[])[0].group,
            "Local branches",
            "an existing name is not offered for creation"
        );
    }

    #[test]
    fn the_folder_placeholder_is_the_daemons_slug() {
        assert_eq!(slug_from_branch("feature/auth flow"), "feature-auth-flow");
        assert_eq!(slug_from_branch("a//b"), "a-b");
        assert_eq!(slug_from_branch("/"), "-");
        assert_eq!(slug_from_branch(""), "worktree");
        assert_eq!(slug_from_branch(&"x".repeat(80)).len(), 64);
    }
}
