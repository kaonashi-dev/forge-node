//! Context menus, in Tauri's order: what a row offers is built here as data
//! (`items`), so a test can read it without drawing, and drawn by
//! `rune_kit::Menu` in the overlay layer. Every effect is a daemon request,
//! the clipboard, or a desktop launch; nothing here touches a checkout.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use domain::{
    AgentProviderId, ProjectActionId, ProjectGroupId, ProjectId, SessionId, SessionKind,
    ShareRuleId, WorkspaceId, WorkspaceKind,
};
use protocol::RemoveProjectPolicy;
use rune::{El, Icon, KeyEvent, PointerPress, TextEdit, Tx, col, el, row, text};
use rune_kit::{
    Button, ButtonVariant, Dialog, Kit, Menu, MenuEntry, MenuItem, RadioGroup, RadioItem, TextInput,
};

use crate::app::{AppState, CommandStatus};
use crate::workbench::{CenterMode, Panel};

/// The file tree's element key: the Menu key and Shift+F10 reach the tree's
/// own menu only while it is what holds focus.
pub const FILES_TREE: &str = "files-tree-view";
/// Shared with Tauri's `projectColors.ts`: `{ project id: hue slot }`.
pub const PROJECT_HUES_KEY: &str = "ui.project_hues";
/// Tauri's `HUE_NAMES`, in slot order.
pub const HUE_NAMES: [&str; 9] = [
    "Teal", "Blue", "Green", "Orchid", "Rose", "Lime", "Sky", "Gold", "Violet",
];
/// The slot a project wears until someone picks another.
pub const DEFAULT_HUE: usize = 8;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Project(ProjectId),
    Checkout(WorkspaceId),
    /// A session's row in the rail.
    Session(SessionId),
    /// A tab in the strip: a session's, or an editor's, which Tauri keeps in
    /// its Code strip and gives that strip's menu.
    Tab(SessionId),
    Files(WorkspaceId, FileTarget),
    /// A named group's header in the rail.
    Group(ProjectGroupId),
}

/// A menu that drops from a button rather than opening at a pointer. The
/// button carries `trigger()` as its key, which is what the menu anchors to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ButtonMenu {
    /// The rail head's "+".
    RailAdd,
    /// A checkout's ▶: its project's actions.
    Run(WorkspaceId),
    /// A share rule's ⋯.
    ShareRule(ProjectId, ShareRuleId),
}

impl ButtonMenu {
    fn key(&self) -> String {
        match self {
            ButtonMenu::RailAdd => "rail-add".into(),
            ButtonMenu::Run(workspace) => format!("run-{workspace}"),
            ButtonMenu::ShareRule(_, rule) => format!("share-rule-menu-{rule}"),
        }
    }

    #[must_use]
    pub fn trigger(&self) -> String {
        format!("{}-trigger", self.key())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileTarget {
    Root,
    File(String),
    Directory(String),
}

/// What an item looks like, by Tauri's icon name; `icon` draws it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Glyph {
    None,
    Terminal,
    Agent(Option<String>),
    PullRequest,
    GitBranch,
    Folder,
    FolderOpen,
    File,
    Refresh,
    Copy,
    Edit,
    Filter,
    Trash,
    Play,
    Appearance,
    Close,
    Plus,
    MessagePlus,
    SpawnChild,
    Columns,
    ListChecks,
    Cursor,
    Zed,
    Globe,
    Stop,
    Activity,
    ChevronUp,
    ChevronDown,
}

#[must_use]
pub fn icon(glyph: &Glyph) -> Option<&'static Icon> {
    use crate::glyphs as forge;
    Some(match glyph {
        Glyph::Terminal => &forge::TERMINAL,
        Glyph::Agent(provider) => provider
            .as_deref()
            .and_then(forge::brand)
            .unwrap_or(&forge::AGENT),
        Glyph::PullRequest => &forge::PULL_REQUEST,
        Glyph::GitBranch => &forge::GIT,
        Glyph::Folder => &forge::FILES,
        Glyph::FolderOpen => &forge::FOLDER_OPEN,
        Glyph::File => &forge::FILE,
        Glyph::Refresh => &forge::REFRESH,
        Glyph::Copy => &forge::COPY,
        Glyph::Edit => &forge::EDIT,
        Glyph::Filter => &forge::FILTER,
        Glyph::Trash => &forge::TRASH,
        Glyph::Close => &forge::CLOSE,
        Glyph::Plus => &forge::PLUS,
        Glyph::Appearance => &forge::APPEARANCE,
        Glyph::Play => &forge::PLAY,
        Glyph::MessagePlus => &forge::HANDOFF,
        Glyph::SpawnChild => &forge::SPAWN_CHILD,
        Glyph::Columns => &forge::SPLIT,
        Glyph::ListChecks => &forge::REVIEW,
        Glyph::Cursor => &forge::CURSOR,
        Glyph::Zed => &forge::ZED,
        Glyph::Globe => &forge::GLOBE,
        Glyph::Stop => &forge::STOP,
        Glyph::Activity => &forge::ACTIVITY,
        Glyph::ChevronUp => &forge::CHEVRON_UP,
        Glyph::ChevronDown => &forge::CHEVRON_DOWN,
        Glyph::None => return None,
    })
}

/// A Tauri item whose feature Rune does not have yet. It is built in its
/// place and left out by `ported`, so porting one is a change to its `Act`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unported {
    PreviewPr,
    OpenPrWithAgent,
    ContinueInNewSession,
    SpawnChild,
    SendContext,
    SessionChanges,
    Review,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Opener {
    Zed,
    Cursor,
    Finder,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Act {
    NewTerminal(WorkspaceId),
    NewAgent(WorkspaceId, AgentProviderId, Option<domain::AgentProfileId>),
    CommitWithJuva(WorkspaceId),
    NewWorktree(ProjectId),
    RefreshStatus(WorkspaceId),
    RefreshProject(ProjectId),
    Copy(String),
    RenameCheckout(WorkspaceId),
    IgnoreFolder(WorkspaceId),
    RemoveWorktree(WorkspaceId),
    SetIcon(ProjectId),
    Hue(ProjectId, Option<usize>),
    RemoveProject(ProjectId),
    OpenIn(Opener, PathBuf),
    RenameSession(SessionId),
    Kill(SessionId),
    Restart(SessionId),
    Close(SessionId),
    CloseEditors(Vec<SessionId>),
    OpenFile(String),
    EditFile(WorkspaceId, String),
    InsertReference(WorkspaceId, String),
    NewPath {
        workspace: WorkspaceId,
        parent: String,
        directory: bool,
    },
    Duplicate(WorkspaceId, String),
    RenamePath(WorkspaceId, String),
    MovePath(WorkspaceId, String),
    DeletePath {
        workspace: WorkspaceId,
        path: String,
        file: bool,
    },
    RefreshFiles,
    CollapseFolders,
    Unported(Unported),
    AddProject(Option<ProjectGroupId>),
    NewGroup,
    RenameGroup(ProjectGroupId),
    RemoveGroup(ProjectGroupId),
    MoveToGroup(ProjectId, Option<ProjectGroupId>),
    MoveCheckout(WorkspaceId, isize),
    SharedFiles(ProjectId, Option<WorkspaceId>),
    Actions(ProjectId, WorkspaceId),
    IgnoredWorktrees(ProjectId),
    RunAction(WorkspaceId, ProjectActionId),
    FocusSession(SessionId),
    OpenUrl(String),
    ReapplyRule(ProjectId, ShareRuleId),
    MoveRule(ProjectId, ShareRuleId, isize),
    RemoveRule(ProjectId, ShareRuleId),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub label: String,
    pub glyph: Glyph,
    pub act: Act,
    pub enabled: bool,
    pub danger: bool,
    pub detail: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Entry {
    Item(Item),
    Rule,
    Heading(&'static str),
    Submenu {
        label: &'static str,
        glyph: Glyph,
        entries: Vec<Entry>,
    },
}

fn item(label: impl Into<String>, glyph: Glyph, act: Act) -> Entry {
    Entry::Item(Item {
        label: label.into(),
        glyph,
        act,
        enabled: true,
        danger: false,
        detail: None,
    })
}

impl Entry {
    fn enabled(mut self, on: bool) -> Self {
        if let Entry::Item(item) = &mut self {
            item.enabled = on;
        }
        self
    }

    fn danger(mut self) -> Self {
        if let Entry::Item(item) = &mut self {
            item.danger = true;
        }
        self
    }

    fn detail(mut self, detail: Option<String>) -> Self {
        if let Entry::Item(item) = &mut self {
            item.detail = detail;
        }
        self
    }
}

/// The filter between Tauri's list and what Rune draws.
#[must_use]
pub fn ported(act: &Act) -> bool {
    !matches!(act, Act::Unported(_))
}

/// `entries` without unported items, and without the rules and headings
/// that leaves with nothing to separate or name.
#[must_use]
pub fn rendered(entries: Vec<Entry>) -> Vec<Entry> {
    let kept: Vec<Entry> = entries
        .into_iter()
        .filter_map(|entry| match entry {
            Entry::Item(item) if !ported(&item.act) => None,
            Entry::Submenu {
                label,
                glyph,
                entries,
            } => {
                let entries = rendered(entries);
                (!entries.is_empty()).then_some(Entry::Submenu {
                    label,
                    glyph,
                    entries,
                })
            }
            other => Some(other),
        })
        .collect();
    let mut out: Vec<Entry> = Vec::with_capacity(kept.len());
    for (at, entry) in kept.iter().enumerate() {
        let names_a_choice = matches!(
            kept.get(at + 1),
            Some(Entry::Item(_) | Entry::Submenu { .. })
        );
        match entry {
            Entry::Heading(_) if !names_a_choice => {}
            Entry::Rule if out.is_empty() || matches!(out.last(), Some(Entry::Rule)) => {}
            _ => out.push(entry.clone()),
        }
    }
    while matches!(out.last(), Some(Entry::Rule | Entry::Heading(_))) {
        out.pop();
    }
    out
}

/// Every item Tauri lists for `target`, ported or not.
#[must_use]
pub fn items(s: &AppState, target: &Target) -> Vec<Entry> {
    match target {
        Target::Project(project) => project_items(s, *project),
        Target::Checkout(workspace) => checkout_items(s, *workspace),
        Target::Session(session) => session_items(s, *session),
        Target::Tab(session) => {
            let editor = s
                .rows
                .iter()
                .any(|row| row.id == *session && row.kind == SessionKind::Editor);
            if editor {
                editor_tab_items(s, *session)
            } else {
                session_items(s, *session)
            }
        }
        Target::Files(workspace, file) => file_items(s, *workspace, file),
        Target::Group(group) => group_items(*group),
    }
}

/// `groupMenu`. Removing a group never touches its projects' directories.
fn group_items(group: ProjectGroupId) -> Vec<Entry> {
    vec![
        item(
            "Add project…",
            Glyph::FolderOpen,
            Act::AddProject(Some(group)),
        ),
        Entry::Rule,
        item("Rename group…", Glyph::Edit, Act::RenameGroup(group)),
        item("Remove group", Glyph::Trash, Act::RemoveGroup(group)).danger(),
    ]
}

/// What a button's menu offers.
#[must_use]
pub fn button_items(s: &AppState, menu: &ButtonMenu) -> Vec<Entry> {
    match menu {
        ButtonMenu::RailAdd => vec![
            item("Add a project…", Glyph::FolderOpen, Act::AddProject(None)),
            item("New group…", Glyph::Folder, Act::NewGroup),
        ],
        ButtonMenu::Run(workspace) => run_items(s, *workspace),
        ButtonMenu::ShareRule(project, rule) => vec![
            item(
                "Re-apply everywhere",
                Glyph::Refresh,
                Act::ReapplyRule(*project, *rule),
            ),
            item(
                "Move up",
                Glyph::ChevronUp,
                Act::MoveRule(*project, *rule, -1),
            ),
            item(
                "Move down",
                Glyph::ChevronDown,
                Act::MoveRule(*project, *rule, 1),
            ),
            item("Remove…", Glyph::Trash, Act::RemoveRule(*project, *rule)).danger(),
        ],
    }
}

/// Tauri's `urlBadge`: `:5173` for a URL with a port, else its host.
#[must_use]
pub fn url_badge(url: &str) -> Option<String> {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let authority = rest.split(['/', '?', '#']).next()?;
    if authority.is_empty() {
        return None;
    }
    Some(match authority.rsplit_once(':') {
        Some((_, port)) if !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) => {
            format!(":{port}")
        }
        _ => authority.to_string(),
    })
}

/// The status beside a live action's name, as Tauri's `liveStatus`.
fn live_status(phase: &crate::processes::Phase) -> String {
    use crate::processes::Phase;
    match phase {
        Phase::Ready(url) => url
            .as_deref()
            .and_then(url_badge)
            .unwrap_or_else(|| "ready".to_string()),
        Phase::Waiting => "waiting".into(),
        Phase::Probing => "starting".into(),
        Phase::TimedOut => "not ready".into(),
        Phase::PortInUse(port) => format!(":{port} taken"),
        _ => "running".into(),
    }
}

/// Where a live run serves: the URL it printed, else its port on localhost
/// once it is up.
fn served_at(session: &domain::Session, phase: &crate::processes::Phase) -> Option<String> {
    use crate::processes::Phase;
    if let Phase::Ready(Some(url)) = phase {
        return Some(url.clone());
    }
    let up = matches!(phase, Phase::Ready(_) | Phase::Running | Phase::TimedOut);
    session
        .action
        .as_ref()
        .and_then(|run| run.port)
        .filter(|_| up)
        .map(|port| format!("http://localhost:{port}"))
}

/// `actionMenuItems`: each action of the checkout's project — an idle one
/// runs (in its finished session, if it left one), a live one shows its
/// terminal with its browser, restart and stop beneath — then the editor.
fn run_items(s: &AppState, workspace: WorkspaceId) -> Vec<Entry> {
    let Some(store) = &s.store else {
        return Vec::new();
    };
    let Some(project) = store
        .workspaces
        .iter()
        .find(|w| w.id == workspace)
        .map(|w| w.project_id)
    else {
        return Vec::new();
    };
    let mut actions: Vec<&domain::ProjectAction> = store
        .project_actions
        .iter()
        .filter(|a| a.project_id == project)
        .collect();
    actions.sort_by_key(|a| a.position);
    let mut out = Vec::new();
    for action in &actions {
        let mut runs: Vec<&domain::Session> = store
            .sessions
            .iter()
            .filter(|session| {
                session.workspace_id == workspace
                    && session
                        .action
                        .as_ref()
                        .is_some_and(|run| run.action_id == action.id)
            })
            .collect();
        runs.sort_by_key(|run| std::cmp::Reverse(run.created_at));
        let latest = runs
            .iter()
            .find(|session| session.state.is_active())
            .or(runs.first())
            .copied();
        let phase = latest.map(crate::processes::phase_of);
        match (latest, phase) {
            (Some(session), Some(phase)) if phase.live() => {
                let id = session.id;
                out.push(
                    item(action.label.clone(), Glyph::Activity, Act::FocusSession(id))
                        .detail(Some(live_status(&phase))),
                );
                if let Some(url) = served_at(session, &phase) {
                    out.push(item(
                        format!("Open {url} in the browser"),
                        Glyph::Globe,
                        Act::OpenUrl(url),
                    ));
                }
                out.push(item(
                    format!("Restart {}", action.label),
                    Glyph::Refresh,
                    Act::Restart(id),
                ));
                out.push(item(
                    format!("Stop {}", action.label),
                    Glyph::Stop,
                    Act::Kill(id),
                ));
            }
            (Some(session), Some(_)) => out.push(item(
                action.label.clone(),
                Glyph::Play,
                Act::Restart(session.id),
            )),
            _ => out.push(item(
                action.label.clone(),
                Glyph::Play,
                Act::RunAction(workspace, action.id),
            )),
        }
    }
    if !out.is_empty() {
        out.push(Entry::Rule);
    }
    out.push(item(
        if actions.is_empty() {
            "Set up actions…"
        } else {
            "Edit actions…"
        },
        Glyph::Edit,
        Act::Actions(project, workspace),
    ));
    out
}

/// `launchItems` over the host's `launchables`: a terminal, then each agent
/// and its profiles the person has not hidden. A missing agent stays listed
/// and disabled, so the menu also says what could run here.
fn launch_items(s: &AppState, workspace: WorkspaceId) -> Vec<Entry> {
    let mut out = vec![item(
        "New Terminal",
        Glyph::Terminal,
        Act::NewTerminal(workspace),
    )];
    let Some(store) = &s.store else {
        return out;
    };
    let visible =
        |key: &str| store.app_state_value(&crate::launch::visibility_key(key)) != Some("false");
    let mut agents = Vec::new();
    for info in &store.providers {
        let provider = &info.descriptor.id;
        let installed = matches!(
            info.detection.status,
            domain::DetectionStatus::Installed { .. }
        );
        let glyph = Glyph::Agent(Some(provider.to_string()));
        if visible(provider.as_str()) {
            agents.push(
                item(
                    info.descriptor.display_name.clone(),
                    glyph.clone(),
                    Act::NewAgent(workspace, provider.clone(), None),
                )
                .enabled(installed)
                .detail((!installed).then(|| "not installed".to_string())),
            );
        }
        for profile in store
            .agent_profiles
            .iter()
            .filter(|profile| profile.provider_id == *provider)
            .filter(|profile| visible(&format!("profile:{}", profile.id)))
        {
            agents.push(
                item(
                    profile.name.clone(),
                    glyph.clone(),
                    Act::NewAgent(workspace, provider.clone(), Some(profile.id)),
                )
                .enabled(installed || profile.executable.is_some()),
            );
        }
    }
    if !agents.is_empty() {
        out.push(Entry::Rule);
        out.extend(agents);
    }
    out
}

/// `workspaceMenu`.
fn checkout_items(s: &AppState, workspace_id: WorkspaceId) -> Vec<Entry> {
    let Some(store) = &s.store else {
        return Vec::new();
    };
    let Some(workspace) = store.workspaces.iter().find(|w| w.id == workspace_id) else {
        return Vec::new();
    };
    let project = store
        .projects
        .iter()
        .find(|project| project.id == workspace.project_id);
    let has_project = project.is_some();
    let mut out = launch_items(s, workspace_id);
    out.extend([
        Entry::Rule,
        Entry::Heading("Juva & Git"),
        item(
            "Commit with Juva…",
            Glyph::Agent(None),
            Act::CommitWithJuva(workspace_id),
        ),
        item(
            "Preview PR",
            Glyph::PullRequest,
            Act::Unported(Unported::PreviewPr),
        ),
        item(
            "Open PR with agent…",
            Glyph::Agent(None),
            Act::Unported(Unported::OpenPrWithAgent),
        ),
        Entry::Rule,
        Entry::Heading("Workspace"),
        item(
            "Shared files…",
            Glyph::FolderOpen,
            Act::SharedFiles(workspace.project_id, Some(workspace_id)),
        )
        .enabled(has_project),
        item(
            "New worktree…",
            Glyph::GitBranch,
            Act::NewWorktree(workspace.project_id),
        )
        .enabled(has_project),
        item(
            "Refresh status",
            Glyph::Refresh,
            Act::RefreshStatus(workspace_id),
        ),
        item(
            "Copy path",
            Glyph::Copy,
            Act::Copy(workspace.path.display().to_string()),
        ),
        item("Rename…", Glyph::Edit, Act::RenameCheckout(workspace_id)),
    ]);
    let order = &s.projects.rail.order;
    let up = crate::rail::moved_checkout(&store.workspaces, workspace_id, -1, order).is_some();
    let down = crate::rail::moved_checkout(&store.workspaces, workspace_id, 1, order).is_some();
    if up || down {
        out.push(
            item(
                "Move up",
                Glyph::ChevronUp,
                Act::MoveCheckout(workspace_id, -1),
            )
            .enabled(up),
        );
        out.push(
            item(
                "Move down",
                Glyph::ChevronDown,
                Act::MoveCheckout(workspace_id, 1),
            )
            .enabled(down),
        );
    }
    if workspace.kind == WorkspaceKind::GitWorktree {
        out.push(
            item(
                "Ignore worktrees in this folder",
                Glyph::Filter,
                Act::IgnoreFolder(workspace_id),
            )
            .enabled(has_project),
        );
        out.push(
            item(
                "Remove worktree",
                Glyph::Trash,
                Act::RemoveWorktree(workspace_id),
            )
            .danger(),
        );
    }
    if let Some(project) = project {
        let root = project.root_path.clone();
        out.push(Entry::Rule);
        out.push(Entry::Submenu {
            label: "Open in",
            glyph: Glyph::FolderOpen,
            entries: vec![
                item("Zed", Glyph::Zed, Act::OpenIn(Opener::Zed, root.clone())),
                item(
                    "Cursor",
                    Glyph::Cursor,
                    Act::OpenIn(Opener::Cursor, root.clone()),
                ),
                Entry::Rule,
                item(
                    "Finder",
                    Glyph::FolderOpen,
                    Act::OpenIn(Opener::Finder, root),
                ),
            ],
        });
    }
    out
}

/// `projectMenu`. A project whose one checkout is its own answers for that
/// checkout too, under a "Project" heading.
fn project_items(s: &AppState, project_id: ProjectId) -> Vec<Entry> {
    let Some(store) = &s.store else {
        return Vec::new();
    };
    let project = store.projects.iter().find(|p| p.id == project_id);
    let checkouts = crate::projects::checkouts_of(&store.workspaces, project_id);
    let sole = match checkouts.as_slice() {
        [only] if only.kind != WorkspaceKind::GitWorktree => Some(only.id),
        _ => None,
    };
    let main = checkouts
        .iter()
        .find(|w| w.kind == WorkspaceKind::Main)
        .or(checkouts.first());
    let mut items = Vec::new();
    if sole.is_none() {
        items.push(item(
            "New worktree…",
            Glyph::GitBranch,
            Act::NewWorktree(project_id),
        ));
        items.push(item(
            "Shared files…",
            Glyph::FolderOpen,
            Act::SharedFiles(project_id, None),
        ));
    }
    let choice = hue_choice(
        store.app_state_value(PROJECT_HUES_KEY),
        &project_id.to_string(),
    );
    let mut hues = vec![
        item("Theme default", Glyph::None, Act::Hue(project_id, None)).detail(Some(match choice {
            None => "Current".to_string(),
            Some(_) => HUE_NAMES[DEFAULT_HUE].to_string(),
        })),
        Entry::Rule,
    ];
    hues.extend(HUE_NAMES.iter().enumerate().map(|(slot, name)| {
        item(*name, Glyph::None, Act::Hue(project_id, Some(slot)))
            .detail((choice == Some(slot)).then(|| "Current".to_string()))
    }));
    items.extend([
        match main {
            Some(main) => item("Actions…", Glyph::Play, Act::Actions(project_id, main.id)),
            None => item("Actions…", Glyph::Play, Act::RefreshProject(project_id)).enabled(false),
        },
        item(
            "Ignored worktrees…",
            Glyph::Filter,
            Act::IgnoredWorktrees(project_id),
        ),
        item(
            "Refresh project",
            Glyph::Refresh,
            Act::RefreshProject(project_id),
        ),
        item("Set icon…", Glyph::Appearance, Act::SetIcon(project_id)),
        Entry::Submenu {
            label: "Color",
            glyph: Glyph::Appearance,
            entries: hues,
        },
    ]);
    let group = project.and_then(|p| p.project_group_id);
    if !store.project_groups.is_empty() || group.is_some() {
        items.push(Entry::Rule);
        items.extend(
            store
                .project_groups
                .iter()
                .filter(|g| Some(g.id) != group)
                .map(|g| {
                    item(
                        format!("Move to {}", g.name),
                        Glyph::FolderOpen,
                        Act::MoveToGroup(project_id, Some(g.id)),
                    )
                }),
        );
        if group.is_some() {
            items.push(item(
                "Move out of the group",
                Glyph::FolderOpen,
                Act::MoveToGroup(project_id, None),
            ));
        }
    }
    if sole.is_none() {
        items.push(Entry::Rule);
        items.push(
            item(
                "Copy path",
                Glyph::Copy,
                Act::Copy(project.map_or_else(String::new, |p| p.root_path.display().to_string())),
            )
            .enabled(project.is_some()),
        );
    }
    items.push(Entry::Rule);
    items.push(
        item(
            "Remove project…",
            Glyph::Trash,
            Act::RemoveProject(project_id),
        )
        .danger()
        .enabled(project.is_some()),
    );
    match sole {
        Some(only) => {
            let mut out = checkout_items(s, only);
            out.push(Entry::Rule);
            out.push(Entry::Heading("Project"));
            out.extend(items);
            out
        }
        None => items,
    }
}

/// `sessionMenuItems`, shared by the rail row and the tab.
fn session_items(s: &AppState, session_id: SessionId) -> Vec<Entry> {
    let Some(session) = s
        .store
        .as_ref()
        .and_then(|store| store.sessions.iter().find(|x| x.id == session_id))
    else {
        return Vec::new();
    };
    let running = session.state.is_active();
    vec![
        item("Rename…", Glyph::Terminal, Act::RenameSession(session_id)),
        item(
            "Continue in a New Session…",
            Glyph::MessagePlus,
            Act::Unported(Unported::ContinueInNewSession),
        ),
        item(
            "Spawn Child…",
            Glyph::SpawnChild,
            Act::Unported(Unported::SpawnChild),
        ),
        item(
            "Send Context…",
            Glyph::MessagePlus,
            Act::Unported(Unported::SendContext),
        ),
        item(
            "Session Changes",
            Glyph::Columns,
            Act::Unported(Unported::SessionChanges),
        ),
        item("Review", Glyph::ListChecks, Act::Unported(Unported::Review))
            .enabled(session.kind == SessionKind::Agent),
        Entry::Rule,
        if running {
            item("Kill", Glyph::Close, Act::Kill(session_id)).danger()
        } else {
            item("Restart", Glyph::Plus, Act::Restart(session_id))
        },
        item("Close", Glyph::Trash, Act::Close(session_id)).danger(),
    ]
}

/// `CenterStack`'s `tabMenuItems` over the strip's editors: each bulk close
/// is about the tab the menu opened on, not the active one.
fn editor_tab_items(s: &AppState, session_id: SessionId) -> Vec<Entry> {
    let editors: Vec<SessionId> = s
        .tab_rows()
        .into_iter()
        .filter(|row| row.kind == SessionKind::Editor)
        .map(|row| row.id)
        .collect();
    let at = editors.iter().position(|id| *id == session_id);
    let others: Vec<SessionId> = editors
        .iter()
        .copied()
        .filter(|id| *id != session_id)
        .collect();
    let right: Vec<SessionId> = at.map_or_else(Vec::new, |at| editors[at + 1..].to_vec());
    vec![
        item("Close", Glyph::Close, Act::CloseEditors(vec![session_id])),
        item(
            "Close Others",
            Glyph::Close,
            Act::CloseEditors(others.clone()),
        )
        .enabled(!others.is_empty()),
        item(
            "Close to the Right",
            Glyph::Close,
            Act::CloseEditors(right.clone()),
        )
        .enabled(!right.is_empty()),
        Entry::Rule,
        item("Close All", Glyph::Trash, Act::CloseEditors(editors)).danger(),
    ]
}

/// `FileTreePanel`'s `menuItems`.
fn file_items(s: &AppState, workspace: WorkspaceId, target: &FileTarget) -> Vec<Entry> {
    let root = s.store.as_ref().and_then(|store| {
        store
            .workspaces
            .iter()
            .find(|w| w.id == workspace)
            .map(|w| w.path.display().to_string())
    });
    let path = match target {
        FileTarget::Root => "",
        FileTarget::File(path) | FileTarget::Directory(path) => path.as_str(),
    };
    let absolute = match &root {
        Some(root) if path.is_empty() => root.clone(),
        Some(root) => format!("{}/{path}", root.trim_end_matches('/')),
        None => path.to_string(),
    };
    let parent = match target {
        FileTarget::Root => String::new(),
        FileTarget::File(path) => parent_of(path).to_string(),
        FileTarget::Directory(path) => path.clone(),
    };
    let create = [
        item(
            "New file…",
            Glyph::File,
            Act::NewPath {
                workspace,
                parent: parent.clone(),
                directory: false,
            },
        ),
        item(
            "New folder…",
            Glyph::Folder,
            Act::NewPath {
                workspace,
                parent,
                directory: true,
            },
        ),
    ];
    let copy = item(
        if *target == FileTarget::Root {
            "Copy workspace path"
        } else {
            "Copy path"
        },
        Glyph::Copy,
        Act::Copy(absolute),
    )
    .enabled(root.is_some());
    if *target == FileTarget::Root {
        let mut out = create.to_vec();
        out.extend([
            Entry::Rule,
            item("Refresh", Glyph::Refresh, Act::RefreshFiles),
            item("Collapse folders", Glyph::Folder, Act::CollapseFolders),
            copy,
        ]);
        return out;
    }
    let file = matches!(target, FileTarget::File(_));
    let mut out = Vec::new();
    if file {
        out.push(item("Open", Glyph::File, Act::OpenFile(path.to_string())));
        if crate::workbench::opens_rendered(path) {
            out.push(item(
                "Edit",
                Glyph::Edit,
                Act::EditFile(workspace, path.to_string()),
            ));
        }
        out.push(item(
            "Insert reference in terminal",
            Glyph::Terminal,
            Act::InsertReference(workspace, path.to_string()),
        ));
    }
    out.push(copy);
    out.push(item(
        "Copy relative path",
        Glyph::Copy,
        Act::Copy(path.to_string()),
    ));
    out.push(Entry::Rule);
    out.extend(create);
    if file {
        out.push(item(
            "Duplicate…",
            Glyph::Copy,
            Act::Duplicate(workspace, path.to_string()),
        ));
    }
    out.push(item(
        "Rename…",
        Glyph::Edit,
        Act::RenamePath(workspace, path.to_string()),
    ));
    out.push(item(
        "Move to…",
        Glyph::Folder,
        Act::MovePath(workspace, path.to_string()),
    ));
    out.push(
        item(
            "Delete",
            Glyph::Trash,
            Act::DeletePath {
                workspace,
                path: path.to_string(),
                file,
            },
        )
        .danger(),
    );
    out
}

#[derive(Clone, Copy, Debug)]
pub struct OpenMenu {
    pub point: PointerPress,
    /// The rendered entry whose submenu is open.
    pub sub: Option<usize>,
}

/// A name or path asked for by a menu item, with Tauri's `requestTextInput` copy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ask {
    RenameCheckout(WorkspaceId),
    SetIcon(ProjectId),
    NewPath {
        workspace: WorkspaceId,
        parent: String,
        directory: bool,
    },
    RenamePath(WorkspaceId, String),
    Duplicate(WorkspaceId, String),
    MovePath(WorkspaceId, String),
    NewGroup,
    RenameGroup(ProjectGroupId),
}

pub struct Prompt {
    pub ask: Ask,
    pub edit: TextEdit,
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Confirmed {
    DeletePath {
        workspace: WorkspaceId,
        path: String,
    },
    CloseEditors(Vec<SessionId>),
}

pub struct Confirm {
    pub title: String,
    pub description: &'static str,
    pub label: &'static str,
    pub then: Confirmed,
}

#[derive(Default)]
pub struct Menus {
    pub open: Option<(Target, OpenMenu)>,
    pub prompt: Option<Prompt>,
    pub confirm: Option<Confirm>,
    /// The project whose removal is being decided, and the policy picked.
    pub removal: Option<(ProjectId, RemoveProjectPolicy)>,
    /// The daemon's refusal of the last file tree write.
    pub file_error: Option<String>,
    pub button: Option<ButtonMenu>,
}

/// A secondary press on `target`'s row.
pub fn open(s: &mut AppState, target: Target, point: PointerPress) {
    s.menus.open = Some((target, OpenMenu { point, sub: None }));
}

/// The file tree's own secondary press: on empty space the root's menu, and
/// from the Menu key or Shift+F10 the menu of the row the cursor is on. Only
/// a key handler's `Tx` names a focused element, which is how the two differ.
pub fn files_secondary(s: &mut AppState, point: PointerPress, tx: &mut Tx) {
    let Some(workspace) = s.workbench.workspace else {
        return;
    };
    let tree = &s.workbench.files.tree;
    let row = tx
        .focused()
        .and(tree.cursor())
        .and_then(|id| tree.get(id))
        .filter(|node| !node.path.is_empty())
        .map(|node| file_target(&node.path, node.dir));
    open(
        s,
        Target::Files(workspace, row.unwrap_or(FileTarget::Root)),
        point,
    );
}

#[must_use]
pub fn file_target(path: &str, dir: bool) -> FileTarget {
    if dir {
        FileTarget::Directory(path.to_string())
    } else {
        FileTarget::File(path.to_string())
    }
}

/// A row's catcher for secondary presses, laid over the whole row so a press
/// anywhere on it names its node; primary presses still reach the row.
#[must_use]
pub fn tree_row_catcher(path: &str, dir: bool) -> El {
    let target = file_target(path, dir);
    el().absolute(0.0, 0.0)
        .w_full()
        .h_full()
        .a11y_hidden()
        .on_secondary_press(move |s: &mut AppState, point, _| {
            if let Some(workspace) = s.workbench.workspace {
                open(s, Target::Files(workspace, target.clone()), point);
            }
        })
}

/// The Menu key or Shift+F10, which the terminal must not take from the tree.
#[must_use]
pub fn tree_menu_key(ev: &KeyEvent, tx: &Tx) -> bool {
    let m = ev.mods;
    let plain = !m.cmd && !m.ctrl && !m.alt && !m.logo;
    let menu = match ev.key {
        rune::Key::ContextMenu => plain,
        rune::Key::Function(10) => plain && m.shift,
        _ => false,
    };
    menu && tx.focused() == Some(FILES_TREE)
}

/// The project-to-slot map as Tauri's `parseProjectHues` keeps it: anything
/// that is not a slot is dropped.
#[must_use]
pub fn parse_hues(raw: Option<&str>) -> BTreeMap<String, usize> {
    let Some(serde_json::Value::Object(map)) =
        raw.and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
    else {
        return BTreeMap::new();
    };
    map.into_iter()
        .filter_map(|(id, slot)| {
            let slot = usize::try_from(slot.as_u64()?).ok()?;
            (slot < HUE_NAMES.len()).then_some((id, slot))
        })
        .collect()
}

#[must_use]
pub fn hue_choice(raw: Option<&str>, project: &str) -> Option<usize> {
    parse_hues(raw).get(project).copied()
}

/// The stored map with `project` given `slot`, or back on the theme default.
#[must_use]
pub fn with_hue(raw: Option<&str>, project: &str, slot: Option<usize>) -> String {
    let mut map = parse_hues(raw);
    match slot {
        Some(slot) => {
            map.insert(project.to_string(), slot);
        }
        None => {
            map.remove(project);
        }
    }
    serde_json::to_string(&map).unwrap_or_else(|_| "{}".to_string())
}

#[must_use]
pub fn parent_of(path: &str) -> &str {
    path.rfind('/').map_or("", |at| &path[..at])
}

fn base_of(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

#[must_use]
pub fn under(parent: &str, name: &str) -> String {
    let parent = parent.trim_matches('/');
    if parent.is_empty() {
        name.to_string()
    } else {
        format!("{parent}/{name}")
    }
}

const COPY_LIMIT: u32 = 100;

/// Tauri's `duplicateFileName`: `notes.md` → `notes copy.md` → `notes copy 2.md`;
/// a copy's copy counts on rather than stacking. A leading dot is the name.
#[must_use]
pub fn duplicate_name(name: &str, taken: impl Fn(&str) -> bool) -> Option<String> {
    let (stem, extension) = match name.rfind('.') {
        Some(dot) if dot > 0 && !name.starts_with('.') => (&name[..dot], &name[dot..]),
        _ => (name, ""),
    };
    let mut base = stem;
    let mut n = 1;
    let numbered = stem
        .rsplit_once(" copy ")
        .filter(|(_, digits)| !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()));
    if let Some((head, digits)) = numbered {
        base = head;
        n = digits.parse::<u32>().ok()?.checked_add(1)?;
    } else if let Some(head) = stem.strip_suffix(" copy").filter(|head| !head.is_empty()) {
        base = head;
        n = 2;
    }
    (n..=COPY_LIMIT)
        .map(|n| {
            if n <= 1 {
                format!("{base} copy{extension}")
            } else {
                format!("{base} copy {n}{extension}")
            }
        })
        .find(|candidate| !taken(candidate))
}

/// Tauri's `shellReference`: the absolute path, single-quoted for a shell.
pub fn shell_reference(root: &str, path: &str) -> Result<String, &'static str> {
    const REFUSED: &str = "This path cannot be inserted as a terminal reference.";
    let control = |text: &str| text.chars().any(|c| c.is_control());
    if !root.starts_with('/')
        || root.len() > 4096
        || control(root)
        || control(path)
        || path.is_empty()
        || path.starts_with('/')
        || path.split('/').any(|part| part.is_empty() || part == "..")
    {
        return Err(REFUSED);
    }
    let full = format!("{}/{path}", root.trim_end_matches('/'));
    Ok(format!("'{}'", full.replace('\'', "'\\''")))
}

/// What removing a project would touch, counted from the replica.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Footprint {
    pub checkouts: usize,
    /// Worktrees Forge created, the only ones a policy may delete.
    pub managed_worktrees: usize,
    pub running_sessions: usize,
}

#[must_use]
pub fn footprint(store: &client::Store, project: ProjectId) -> Footprint {
    let owned: Vec<&domain::Workspace> = store
        .workspaces
        .iter()
        .filter(|w| w.project_id == project)
        .collect();
    Footprint {
        checkouts: owned.len(),
        managed_worktrees: owned.iter().filter(|w| w.managed_by_app).count(),
        running_sessions: store
            .sessions
            .iter()
            .filter(|session| {
                session.state.is_active() && owned.iter().any(|w| w.id == session.workspace_id)
            })
            .count(),
    }
}

/// The daemon refuses to keep everything while something runs.
#[must_use]
pub fn policy_refused(policy: RemoveProjectPolicy, footprint: Footprint) -> bool {
    policy == RemoveProjectPolicy::KeepEverything && footprint.running_sessions > 0
}

fn plural(count: usize, noun: &str) -> String {
    format!("{count} {noun}{}", if count == 1 { "" } else { "s" })
}

/// Tauri's `describeProjectRemoval`.
#[must_use]
pub fn describe_removal(policy: RemoveProjectPolicy, footprint: Footprint) -> Vec<String> {
    let mut lines = vec!["Forge Node stops tracking the project.".to_string()];
    let running = footprint.running_sessions;
    let killed = || {
        if running == 0 {
            "Nothing is running, so no session is killed.".to_string()
        } else {
            format!("{} killed.", plural(running, "running session"))
        }
    };
    match policy {
        RemoveProjectPolicy::KeepEverything => {
            lines.push("Every checkout stays on disk, exactly as it is.".into());
        }
        RemoveProjectPolicy::KillSessionsKeepWorktrees => {
            lines.push(killed());
            lines.push("Every checkout stays on disk.".into());
        }
        _ => {
            lines.push(killed());
            lines.push(match footprint.managed_worktrees {
                0 => "No worktree here was created by Forge Node, so none is deleted.".into(),
                1 => "1 worktree Forge Node created is deleted from disk.".into(),
                n => format!(
                    "{} Forge Node created are deleted from disk.",
                    plural(n, "worktree")
                ),
            });
        }
    }
    lines.push("No branch and no commit is deleted.".into());
    lines
}

const POLICIES: [(RemoveProjectPolicy, &str, &str); 3] = [
    (
        RemoveProjectPolicy::KeepEverything,
        "Keep everything",
        "Forget the project; nothing on disk changes",
    ),
    (
        RemoveProjectPolicy::KillSessionsKeepWorktrees,
        "Kill sessions",
        "Stop what is running, keep every checkout",
    ),
    (
        RemoveProjectPolicy::KillSessionsRemoveManagedWorktrees,
        "Kill sessions and delete Forge Node's worktrees",
        "Only the worktrees Forge Node created are removed",
    ),
];

/// The policy the dialog acts on: a refused pick falls to killing sessions.
#[must_use]
pub fn chosen_policy(policy: RemoveProjectPolicy, footprint: Footprint) -> RemoveProjectPolicy {
    if policy_refused(policy, footprint) {
        RemoveProjectPolicy::KillSessionsKeepWorktrees
    } else {
        policy
    }
}

fn send(
    s: &mut AppState,
    label: &'static str,
    request: impl FnOnce(&crate::connection::ConnectionHandle) -> Result<(), String>,
) -> bool {
    let result = match &s.connection {
        Some(connection) => request(connection),
        None => Err("client worker is still starting".to_string()),
    };
    match result {
        Ok(()) => true,
        Err(message) => {
            s.last_command = Some(CommandStatus {
                label,
                ok: false,
                message,
                took_us: 0,
            });
            false
        }
    }
}

fn ask(s: &mut AppState, ask: Ask, value: String) {
    let mut edit = TextEdit::new(value);
    edit.focused = true;
    s.menus.prompt = Some(Prompt {
        ask,
        edit,
        error: None,
    });
}

/// Does what an item says; the menu is closed first.
pub fn run(s: &mut AppState, act: Act, tx: &mut Tx) {
    s.menus.open = None;
    s.menus.button = None;
    match act {
        Act::NewTerminal(workspace) => s.shell_in(workspace),
        Act::NewAgent(workspace, provider, profile) => {
            if send(s, "CreateAgentSession", |c| {
                c.create_agent(workspace, provider, profile, None, false)
            }) {
                s.workbench.show(CenterMode::Session);
            }
        }
        Act::CommitWithJuva(workspace) => {
            s.focus_checkout(workspace);
            s.show_panel(Panel::Git);
            s.draft_commit();
        }
        Act::NewWorktree(project) => crate::branch_picker::open(s, project),
        Act::RefreshStatus(workspace) => {
            send(s, "RefreshWorkspaceStatus", |c| {
                c.refresh_workspace_status(workspace)
            });
        }
        Act::RefreshProject(project) => {
            send(s, "RefreshProject", |c| c.refresh_project(project));
        }
        Act::Copy(text) => tx.copy(text),
        Act::RenameCheckout(workspace) => {
            let label = s
                .store
                .as_ref()
                .and_then(|store| store.workspaces.iter().find(|w| w.id == workspace))
                .map(domain::Workspace::label)
                .unwrap_or_default();
            ask(s, Ask::RenameCheckout(workspace), label);
        }
        Act::IgnoreFolder(workspace) => ignore_folder(s, workspace),
        Act::RemoveWorktree(workspace) => {
            s.projects.refused = None;
            s.projects.removal = Some(crate::projects::Removal {
                workspace,
                reason: None,
            });
        }
        Act::SetIcon(project) => {
            let current = s
                .store
                .as_ref()
                .and_then(|store| store.projects.iter().find(|p| p.id == project))
                .and_then(|p| p.icon.clone())
                .unwrap_or_default();
            ask(s, Ask::SetIcon(project), current);
        }
        Act::Hue(project, slot) => {
            let raw = s
                .store
                .as_ref()
                .and_then(|store| store.app_state_value(PROJECT_HUES_KEY));
            let value = with_hue(raw, &project.to_string(), slot);
            send(s, "SetAppState", |c| {
                c.set_app_state(PROJECT_HUES_KEY, &value)
            });
        }
        Act::RemoveProject(project) => {
            s.menus.removal = Some((project, RemoveProjectPolicy::KeepEverything));
        }
        Act::OpenIn(app, path) => open_in(app, path),
        Act::RenameSession(session) => {
            if let Some(row) = s.rows.iter().find(|row| row.id == session) {
                let mut edit = TextEdit::new(row.title.clone());
                edit.focused = true;
                s.rename = Some((session, edit));
            }
        }
        Act::Kill(session) => s.kill_session(session),
        Act::Restart(session) => s.restart_session(session),
        Act::Close(session) => s.request_close(session),
        Act::CloseEditors(sessions) => close_editors(s, sessions),
        Act::OpenFile(path) => s.open_in_editor(&path),
        Act::EditFile(workspace, path) => {
            if send(s, "CreateEditorSession", |c| c.open_editor(workspace, path)) {
                s.workbench.show(CenterMode::Session);
            }
        }
        Act::InsertReference(workspace, path) => insert_reference(s, workspace, &path, tx),
        Act::NewPath {
            workspace,
            parent,
            directory,
        } => ask(
            s,
            Ask::NewPath {
                workspace,
                parent,
                directory,
            },
            String::new(),
        ),
        Act::Duplicate(workspace, path) => {
            let parent = parent_of(&path);
            let tree = &s.workbench.files.tree;
            let siblings: Vec<String> = tree
                .children((!parent.is_empty()).then_some(parent))
                .map(|id| base_of(id).to_string())
                .collect();
            match duplicate_name(base_of(&path), |name| siblings.iter().any(|s| s == name)) {
                Some(name) => ask(s, Ask::Duplicate(workspace, path), name),
                None => {
                    s.menus.file_error =
                        Some("Every copy name next to this file is already taken.".into());
                }
            }
        }
        Act::RenamePath(workspace, path) => {
            let name = base_of(&path).to_string();
            ask(s, Ask::RenamePath(workspace, path), name);
        }
        Act::MovePath(workspace, path) => {
            let parent = parent_of(&path).to_string();
            ask(s, Ask::MovePath(workspace, path), parent);
        }
        Act::DeletePath {
            workspace,
            path,
            file,
        } => {
            s.menus.confirm = Some(Confirm {
                title: format!("Delete {path}?"),
                description: if file {
                    "The file is removed from the checkout. Git is the only way back."
                } else {
                    "The folder and everything in it are removed from the checkout. Git is the only way back."
                },
                label: "Delete",
                then: Confirmed::DeletePath { workspace, path },
            });
        }
        Act::RefreshFiles => s.workbench.refresh(),
        Act::CollapseFolders => s.workbench.files.tree.collapse_all(),
        Act::Unported(_) => {}
        Act::AddProject(group) => add_project(group, tx),
        Act::NewGroup => ask(s, Ask::NewGroup, String::new()),
        Act::RenameGroup(group) => {
            let name = s
                .store
                .as_ref()
                .and_then(|store| store.project_groups.iter().find(|g| g.id == group))
                .map(|g| g.name.clone())
                .unwrap_or_default();
            ask(s, Ask::RenameGroup(group), name);
        }
        Act::RemoveGroup(group) => {
            crate::project_ops::send(s, crate::project_ops::Op::RemoveGroup(group));
        }
        Act::MoveToGroup(project, group) => {
            crate::project_ops::send(s, crate::project_ops::Op::MoveProject(project, group));
        }
        Act::MoveCheckout(workspace, by) => crate::projects::move_checkout(s, workspace, by),
        Act::SharedFiles(project, workspace) => {
            crate::shares::open_dialog(s, project, workspace);
        }
        Act::Actions(project, workspace) => crate::actions_editor::open(s, project, workspace),
        Act::IgnoredWorktrees(project) => s.projects.ignores = Some(project),
        Act::RunAction(workspace, action) => s.run_action(workspace, action),
        Act::FocusSession(session) => s.focus_session(session),
        Act::OpenUrl(url) => {
            if let Err(error) = tx.open_url(&url) {
                eprintln!("[open] {url}: {error:?}");
            }
        }
        Act::ReapplyRule(project, rule) => crate::shares::reapply_everywhere(s, project, rule),
        Act::MoveRule(project, rule, by) => crate::shares::move_rule(s, project, rule, by),
        Act::RemoveRule(project, rule) => {
            s.projects.share_form.removing = Some((project, rule, domain::ShareCleanup::Leave));
        }
    }
}

/// Picks a folder and registers it, in `group` when one is named.
fn add_project(group: Option<ProjectGroupId>, tx: &mut Tx) {
    tx.open_folder(
        rune::FileDialogOptions {
            title: "Add a project".into(),
            ..rune::FileDialogOptions::default()
        },
        move |s: &mut AppState, answer, _tx| {
            let Ok(Some(path)) = answer else {
                return;
            };
            match group {
                Some(group) => {
                    crate::project_ops::send(s, crate::project_ops::Op::AddProjectTo(path, group));
                }
                None => s.add_project(path),
            }
        },
    );
}

/// Replaces a project's ignore rules; the daemon rescans before it acks.
pub fn set_ignores(s: &mut AppState, project: ProjectId, rules: Vec<domain::WorktreeIgnore>) {
    send(s, "SetWorktreeIgnores", |c| {
        c.set_worktree_ignores(project, rules)
    });
}

/// Tauri's gesture names the folder that holds the worktree, not the
/// worktree: the next agent run gets a different directory there.
fn ignore_folder(s: &mut AppState, workspace: WorkspaceId) {
    let Some(store) = &s.store else {
        return;
    };
    let Some(found) = store.workspaces.iter().find(|w| w.id == workspace) else {
        return;
    };
    let project = found.project_id;
    let folder = found
        .path
        .parent()
        .map_or_else(|| PathBuf::from("/"), Path::to_path_buf);
    let mut rules: Vec<domain::WorktreeIgnore> = store
        .worktree_ignores
        .iter()
        .filter(|rule| rule.project_id == project && rule.path != folder)
        .cloned()
        .collect();
    rules.push(domain::WorktreeIgnore {
        project_id: project,
        path: folder,
        scope: domain::IgnoreScope::Subtree,
        created_at: domain::Timestamp::now(),
    });
    set_ignores(s, project, rules);
}

/// Tauri's `closeViews`: one confirmation when any editor might hold unsaved
/// work, and an unknown state counts as unsaved.
fn close_editors(s: &mut AppState, sessions: Vec<SessionId>) {
    if sessions.is_empty() {
        return;
    }
    let unsaved = s.store.as_ref().is_some_and(|store| {
        sessions.iter().any(|id| {
            store
                .sessions
                .iter()
                .find(|session| session.id == *id)
                .is_some_and(|session| session.editor.as_ref().is_none_or(|e| e.dirty))
        })
    });
    if unsaved {
        s.menus.confirm = Some(Confirm {
            title: "Close files with unsaved changes?".into(),
            description: "These files have unsaved changes. Cancel to save them before closing.",
            label: "Close without saving",
            then: Confirmed::CloseEditors(sessions),
        });
    } else {
        for session in sessions {
            s.close_now(session);
        }
    }
}

fn insert_reference(s: &mut AppState, workspace: WorkspaceId, path: &str, tx: &mut Tx) {
    let terminal_here = s
        .active_row()
        .is_some_and(|row| row.workspace_id == workspace && row.terminal_id.is_some());
    let root = s.store.as_ref().and_then(|store| {
        store
            .workspaces
            .iter()
            .find(|w| w.id == workspace)
            .map(|w| w.path.display().to_string())
    });
    if !terminal_here {
        s.menus.file_error = Some("Open a connected terminal to insert a reference.".into());
        return;
    }
    match shell_reference(&root.unwrap_or_default(), path) {
        Ok(text) => {
            s.workbench.show(CenterMode::Session);
            s.terminal_paste(&text, tx);
        }
        Err(refused) => s.menus.file_error = Some(refused.to_string()),
    }
}

/// Tauri's `open.rs`: a CLI on `PATH`, else the app bundle; Finder opens the
/// folder. Located and reaped on its own thread.
fn open_in(app: Opener, path: PathBuf) {
    if app == Opener::Finder {
        crate::storage::reveal(&path);
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("forge-ui-open-in".into())
        .spawn(move || {
            let (cli, bundle) = match app {
                Opener::Zed => ("zed", "Zed.app"),
                _ => ("cursor", "Cursor.app"),
            };
            let mut command = match locate_cli(cli) {
                Some(program) => {
                    let mut command = std::process::Command::new(program);
                    command.arg(&path);
                    command
                }
                None => {
                    let Some(bundle) = locate_bundle(bundle) else {
                        eprintln!("[open] {bundle} is not installed");
                        return;
                    };
                    let mut command = std::process::Command::new("/usr/bin/open");
                    command.arg("-a").arg(bundle).arg(&path);
                    command
                }
            };
            let spawned = command
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn();
            match spawned {
                Ok(mut child) => {
                    let _ = child.wait();
                }
                Err(error) => eprintln!("[open] {cli}: {error}"),
            }
        });
    if let Err(error) = spawned {
        eprintln!("[open] could not start: {error}");
    }
}

fn locate_cli(name: &str) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|candidate| {
            std::fs::metadata(candidate)
                .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        })
}

fn locate_bundle(bundle: &str) -> Option<PathBuf> {
    let mut dirs = vec![PathBuf::from("/Applications")];
    if let Some(home) = std::env::var_os("HOME") {
        dirs.push(PathBuf::from(home).join("Applications"));
    }
    dirs.into_iter()
        .map(|dir| dir.join(bundle))
        .find(|candidate| candidate.is_dir())
}

/// The answer to a prompt; a bad one keeps the prompt open with the reason.
pub fn submit_prompt(s: &mut AppState) {
    let Some(prompt) = s.menus.prompt.take() else {
        return;
    };
    let value = prompt.edit.text.trim().to_string();
    let named = |value: &str| -> Option<&'static str> {
        if value.is_empty() {
            Some("Enter a name.")
        } else if value.contains(['/', '\\']) {
            Some("Enter a name without separators.")
        } else {
            None
        }
    };
    let problem = match &prompt.ask {
        Ask::RenameCheckout(_) | Ask::SetIcon(_) | Ask::MovePath(..) => None,
        Ask::NewPath { .. } | Ask::NewGroup | Ask::RenameGroup(_) => {
            value.is_empty().then_some("Enter a name.")
        }
        Ask::RenamePath(..) | Ask::Duplicate(..) => named(&value),
    };
    if let Some(problem) = problem {
        s.menus.prompt = Some(Prompt {
            error: Some(problem.to_string()),
            ..prompt
        });
        return;
    }
    let optional = (!value.is_empty()).then(|| value.clone());
    match prompt.ask {
        Ask::RenameCheckout(workspace) => {
            send(s, "RenameWorkspace", |c| {
                c.rename_workspace(workspace, optional)
            });
        }
        Ask::SetIcon(project) => {
            send(s, "SetProjectIcon", |c| {
                c.set_project_icon(project, optional)
            });
        }
        Ask::NewPath {
            workspace,
            parent,
            directory,
        } => {
            let target = under(&parent, &value);
            if send(s, "CreatePath", |c| {
                c.create_path(workspace, target.clone(), directory)
            }) {
                s.menus.file_error = None;
                reread(s, &parent);
                // An empty rendering is no place to type: a new file opens as source.
                if !directory
                    && send(s, "CreateEditorSession", |c| {
                        c.open_editor(workspace, target)
                    })
                {
                    s.workbench.show(CenterMode::Session);
                }
            }
        }
        Ask::RenamePath(workspace, path) => {
            let parent = parent_of(&path).to_string();
            let target = under(&parent, &value);
            if send(s, "RenamePath", |c| c.rename_path(workspace, path, target)) {
                reread(s, &parent);
            }
        }
        Ask::Duplicate(workspace, source) => {
            let parent = parent_of(&source).to_string();
            let target = under(&parent, &value);
            if send(s, "CopyPath", |c| {
                c.copy_path(workspace, source, target.clone())
            }) {
                reread(s, &parent);
                if crate::workbench::opens_rendered(&target) {
                    s.workbench.preview(&target);
                } else if send(s, "CreateEditorSession", |c| {
                    c.open_editor(workspace, target)
                }) {
                    s.workbench.show(CenterMode::Session);
                }
            }
        }
        Ask::NewGroup => {
            crate::project_ops::send(s, crate::project_ops::Op::CreateGroup(value));
        }
        Ask::RenameGroup(group) => {
            crate::project_ops::send(s, crate::project_ops::Op::RenameGroup(group, value));
        }
        Ask::MovePath(workspace, path) => {
            let from = parent_of(&path).to_string();
            let folder = value.trim_matches('/').to_string();
            let target = under(&folder, base_of(&path));
            if send(s, "RenamePath", |c| c.rename_path(workspace, path, target)) {
                reread(s, &from);
                reread(s, &folder);
            }
        }
    }
}

/// Lists `dir` again after a write; the listing is sent after the write,
/// on the same queue, so it sees the change.
fn reread(s: &mut AppState, dir: &str) {
    if !dir.is_empty() {
        s.workbench.files.tree.set_expanded(dir, true);
    }
    s.workbench.reread(dir);
}

pub fn confirm(s: &mut AppState) {
    let Some(confirm) = s.menus.confirm.take() else {
        return;
    };
    match confirm.then {
        Confirmed::DeletePath { workspace, path } => {
            let parent = parent_of(&path).to_string();
            if send(s, "DeletePath", |c| c.delete_path(workspace, path)) {
                reread(s, &parent);
            }
        }
        Confirmed::CloseEditors(sessions) => {
            for session in sessions {
                s.close_now(session);
            }
        }
    }
}

pub fn remove_project(s: &mut AppState) {
    let Some((project, policy)) = s.menus.removal.take() else {
        return;
    };
    let footprint = s
        .store
        .as_ref()
        .map(|store| footprint(store, project))
        .unwrap_or_default();
    let policy = chosen_policy(policy, footprint);
    send(s, "RemoveProject", |c| c.remove_project(project, policy));
}

/// A refused file write is said in the Files panel, as Tauri's `operationError`.
pub fn command_result(s: &mut AppState, label: &str, ok: bool, message: &str) {
    if matches!(
        label,
        "CreatePath" | "RenamePath" | "CopyPath" | "DeletePath"
    ) {
        s.menus.file_error = (!ok).then(|| message.to_string());
    }
    if label == "FetchRemote"
        && !ok
        && let Some(picker) = s.projects.picker.as_mut()
    {
        picker.fetching = false;
        picker.error = Some(format!("Fetch failed: {message}"));
    }
}

fn kit_item(item: Item) -> MenuItem {
    let mut built = MenuItem::new(item.label)
        .danger(item.danger)
        .disabled(!item.enabled);
    if let Some(glyph) = icon(&item.glyph) {
        built = built.icon(glyph);
    } else {
        built = built.lead(el().size(14.0, 14.0));
    }
    if let Some(detail) = item.detail {
        built = built.badge(detail);
    }
    let act = item.act;
    built.on_select_tx(move |s: &mut AppState, tx: &mut Tx| run(s, act.clone(), tx))
}

fn kit_entries(entries: Vec<Entry>) -> Vec<MenuEntry> {
    entries
        .into_iter()
        .map(|entry| match entry {
            Entry::Item(item) => MenuEntry::item(kit_item(item)),
            Entry::Rule => MenuEntry::Separator,
            Entry::Heading(label) => MenuEntry::Label(label.to_string()),
            Entry::Submenu { label, entries, .. } => {
                MenuEntry::item(MenuItem::new(label).submenu(kit_entries(entries), false))
            }
        })
        .collect()
}

/// The open menu and every dialog its items ask for, as overlay layers.
pub fn layer(s: &AppState, k: &Kit, layer: &mut Vec<El>) {
    if let Some((target, open)) = &s.menus.open {
        let entries = rendered(items(s, target));
        if !entries.is_empty() {
            let mut menu = Menu::new("context")
                .open(true)
                .at(open.point.x, open.point.y)
                .min_width(200.0)
                .on_close(|s: &mut AppState| s.menus.open = None);
            for (index, entry) in entries.into_iter().enumerate() {
                menu = match entry {
                    Entry::Item(item) => menu.item(kit_item(item)),
                    Entry::Rule => menu.separator(),
                    Entry::Heading(label) => menu.group_label(label),
                    Entry::Submenu {
                        label,
                        glyph,
                        entries,
                    } => {
                        let mut trigger = MenuItem::new(label)
                            .submenu(kit_entries(entries), open.sub == Some(index))
                            .on_select(move |s: &mut AppState| {
                                if let Some((_, open)) = s.menus.open.as_mut() {
                                    open.sub = (open.sub != Some(index)).then_some(index);
                                }
                            });
                        if let Some(glyph) = icon(&glyph) {
                            trigger = trigger.icon(glyph);
                        }
                        menu.item(trigger)
                    }
                };
            }
            let _ = menu.build(k, layer);
        }
    }
    if let Some(menu) = &s.menus.button {
        let entries = rendered(button_items(s, menu));
        if !entries.is_empty() {
            let mut built = Menu::new(menu.key())
                .open(true)
                .min_width(220.0)
                .on_close(|s: &mut AppState| s.menus.button = None);
            for entry in entries {
                built = match entry {
                    Entry::Item(item) => built.item(kit_item(item)),
                    Entry::Rule => built.separator(),
                    Entry::Heading(label) => built.group_label(label),
                    Entry::Submenu { .. } => built,
                };
            }
            let _ = built.build(k, layer);
        }
    }
    prompt_dialog(s, k, layer);
    confirm_dialog(s, k, layer);
    removal_dialog(s, k, layer);
}

impl Ask {
    fn title(&self) -> String {
        match self {
            Ask::RenameCheckout(_) => "Rename checkout".into(),
            Ask::SetIcon(_) => "Set project icon".into(),
            Ask::NewPath {
                directory: false, ..
            } => "New file".into(),
            Ask::NewPath { .. } => "New folder".into(),
            Ask::RenamePath(_, path) => format!("Rename {path}"),
            Ask::Duplicate(_, path) => format!("Duplicate {path}"),
            Ask::MovePath(_, path) => format!("Move {path}"),
            Ask::NewGroup => "New group".into(),
            Ask::RenameGroup(_) => "Rename group".into(),
        }
    }

    fn label(&self) -> &'static str {
        match self {
            Ask::RenameCheckout(_) => "Checkout name",
            Ask::SetIcon(_) => "Icon (one or two characters)",
            Ask::MovePath(..) => "Destination folder relative to checkout (empty for root)",
            Ask::NewPath { .. } | Ask::RenamePath(..) | Ask::Duplicate(..) => "Name",
            Ask::NewGroup | Ask::RenameGroup(_) => "Group name",
        }
    }

    fn placeholder(&self) -> &'static str {
        match self {
            Ask::SetIcon(_) => "🦀",
            Ask::MovePath(..) => "e.g. src/components",
            _ => "",
        }
    }

    fn confirm(&self) -> &'static str {
        match self {
            Ask::SetIcon(_) => "Apply",
            Ask::MovePath(..) => "Move",
            Ask::NewPath { .. } | Ask::NewGroup => "Create",
            Ask::Duplicate(..) => "Duplicate",
            Ask::RenameCheckout(_) | Ask::RenamePath(..) | Ask::RenameGroup(_) => "Rename",
        }
    }
}

fn footer(cancel: El, confirm: El) -> El {
    row()
        .pt(12.0)
        .gap(8.0)
        .child(el().grow(1.0))
        .child(cancel)
        .child(confirm)
}

fn cancel_button(k: &Kit, key: &'static str, close: fn(&mut AppState)) -> El {
    Button::new("Cancel")
        .variant(ButtonVariant::Ghost)
        .on_press(close)
        .build(k)
        .key(key)
}

fn prompt_dialog(s: &AppState, k: &Kit, layer: &mut Vec<El>) {
    let Some(prompt) = &s.menus.prompt else {
        return;
    };
    let field = TextInput::new(&prompt.edit)
        .label(prompt.ask.label())
        .placeholder(prompt.ask.placeholder())
        .on_edit(|s: &mut AppState| match s.menus.prompt.as_mut() {
            Some(prompt) => &mut prompt.edit,
            None => &mut s.scratch_edit,
        })
        .on_key(|s: &mut AppState, ev, _| match ev.key {
            rune::Key::Enter => {
                submit_prompt(s);
                true
            }
            rune::Key::Escape => {
                s.menus.prompt = None;
                true
            }
            _ => false,
        })
        .build(k)
        .key("menu-prompt-input");
    let submit = Button::new(prompt.ask.confirm())
        .variant(ButtonVariant::Contrast)
        .on_press(submit_prompt)
        .build(k)
        .key("menu-prompt-confirm");
    let mut body = col().gap(6.0).child(field);
    if let Some(error) = &prompt.error {
        body = body.child(
            text(error.clone())
                .key("menu-prompt-error")
                .role("status")
                .font(k.ty.text_xsmall)
                .color(k.c.danger.fg),
        );
    }
    Dialog::new("menu-prompt", prompt.ask.title())
        .open(true)
        .fit()
        .width(420.0)
        .click_outside(true)
        .on_close(|s: &mut AppState| s.menus.prompt = None)
        .child(body)
        .child(footer(
            cancel_button(k, "menu-prompt-cancel", |s| s.menus.prompt = None),
            submit,
        ))
        .build(k, layer);
}

fn confirm_dialog(s: &AppState, k: &Kit, layer: &mut Vec<El>) {
    let Some(pending) = &s.menus.confirm else {
        return;
    };
    let go = Button::new(pending.label)
        .variant(ButtonVariant::Danger)
        .on_press(confirm)
        .build(k)
        .key("menu-confirm");
    Dialog::new("menu-confirm-dialog", pending.title.clone())
        .description(pending.description)
        .open(true)
        .fit()
        .width(420.0)
        .on_close(|s: &mut AppState| s.menus.confirm = None)
        .child(footer(
            cancel_button(k, "menu-confirm-cancel", |s| s.menus.confirm = None),
            go,
        ))
        .build(k, layer);
}

/// Tauri's `RemoveProjectDialog`: the policy is chosen before the request,
/// and the one the daemon would refuse is disabled with the reason.
fn removal_dialog(s: &AppState, k: &Kit, layer: &mut Vec<El>) {
    let (Some((project, policy)), Some(store)) = (s.menus.removal, &s.store) else {
        return;
    };
    let name = store
        .projects
        .iter()
        .find(|p| p.id == project)
        .map_or_else(|| "this project".to_string(), |p| p.name.clone());
    let footprint = footprint(store, project);
    let chosen = chosen_policy(policy, footprint);
    let mut group = RadioGroup::new().label("What to do with its sessions and worktrees");
    for (value, label, note) in POLICIES {
        let refused = policy_refused(value, footprint);
        let note = if refused {
            format!(
                "Not available while {} running",
                if footprint.running_sessions == 1 {
                    "a session is".to_string()
                } else {
                    format!("{} sessions are", footprint.running_sessions)
                }
            )
        } else {
            note.to_string()
        };
        group = group.item(
            RadioItem::new(label, chosen == value)
                .description(note)
                .disabled(refused)
                .on_select(move |s: &mut AppState| {
                    if let Some((_, policy)) = s.menus.removal.as_mut() {
                        *policy = value;
                    }
                }),
        );
    }
    let mut consequences = col().key("remove-project-consequences").gap(4.0).child(
        text("What happens")
            .font(k.ty.small_medium)
            .color(k.c.text_muted),
    );
    for line in describe_removal(chosen, footprint) {
        consequences = consequences.child(text(line).font(k.ty.small).color(k.c.text_base));
    }
    let go = Button::new("Remove project")
        .variant(ButtonVariant::Danger)
        .on_press(remove_project)
        .build(k)
        .key("remove-project-confirm");
    Dialog::new("remove-project", format!("Remove {name}?"))
        .description(if footprint.checkouts == 1 {
            "One checkout is registered under it.".to_string()
        } else {
            format!("{} checkouts are registered under it.", footprint.checkouts)
        })
        .open(true)
        .fit()
        .width(460.0)
        .on_close(|s: &mut AppState| s.menus.removal = None)
        .child(col().gap(12.0).child(group.build(k)).child(consequences))
        .child(footer(
            cancel_button(k, "remove-project-cancel", |s| s.menus.removal = None),
            go,
        ))
        .build(k, layer);
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::Arc;

    use rune::{Env, Key, KeyEvent, Mods, Runtime};

    use super::*;
    use crate::connection::{Command, ConnectionHandle, Metrics, fixture_store};
    use crate::terminal::SharedTerminal;

    fn id<T: std::str::FromStr>(tail: &str) -> T
    where
        T::Err: std::fmt::Debug,
    {
        format!("00000000-0000-7000-8000-{tail:0>12}")
            .parse()
            .expect("a fixture id")
    }

    fn store() -> client::Store {
        fixture_store(&Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/snapshot.json"))
            .expect("fixture snapshot decodes")
    }

    fn state(store: client::Store) -> AppState {
        AppState::for_bench(store, Arc::new(Metrics::default()), SharedTerminal::new())
    }

    /// An item's label, `(off)` when disabled and `!` when destructive; a
    /// rule is `—`, a heading `# …` and a submenu `… ▸`.
    fn labels(entries: &[Entry]) -> Vec<String> {
        entries
            .iter()
            .map(|entry| match entry {
                Entry::Item(item) => format!(
                    "{}{}{}",
                    item.label,
                    if item.enabled { "" } else { " (off)" },
                    if item.danger { "!" } else { "" }
                ),
                Entry::Rule => "—".into(),
                Entry::Heading(label) => format!("# {label}"),
                Entry::Submenu { label, .. } => format!("{label} ▸"),
            })
            .collect()
    }

    fn act_of(entries: &[Entry], label: &str) -> Act {
        entries
            .iter()
            .find_map(|entry| match entry {
                Entry::Item(item) if item.label == label => Some(item.act.clone()),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no item {label}"))
    }

    const PROJECT: &str = "1";
    const MAIN: &str = "3";
    const RUNE_SHELL: &str = "31";

    #[test]
    fn a_project_with_several_checkouts_lists_tauris_items_in_order() {
        let s = state(store());
        let all = items(&s, &Target::Project(id(PROJECT)));
        assert_eq!(
            labels(&all),
            [
                "New worktree…",
                "Shared files…",
                "Actions…",
                "Ignored worktrees…",
                "Refresh project",
                "Set icon…",
                "Color ▸",
                "—",
                "Move out of the group",
                "—",
                "Copy path",
                "—",
                "Remove project…!",
            ],
            "its own group is not a place to move it to"
        );
        assert_eq!(
            labels(&rendered(all.clone())),
            labels(&all),
            "every project item is ported"
        );
        assert_eq!(
            act_of(&all, "Copy path"),
            Act::Copy("/home/dev/forge".into())
        );
        assert_eq!(
            act_of(&all, "Move out of the group"),
            Act::MoveToGroup(id(PROJECT), None)
        );
        assert_eq!(
            act_of(&all, "Actions…"),
            Act::Actions(id(PROJECT), id(MAIN)),
            "actions are read at the main checkout"
        );
        assert_eq!(
            act_of(&all, "Shared files…"),
            Act::SharedFiles(id(PROJECT), None)
        );
    }

    #[test]
    fn a_project_with_one_own_checkout_answers_for_it_under_a_heading() {
        let mut store = store();
        let shell: WorkspaceId = id(RUNE_SHELL);
        store.workspaces.retain(|w| w.id != shell);
        store.project_groups.clear();
        store.projects[0].project_group_id = None;
        let s = state(store);
        assert_eq!(
            labels(&rendered(items(&s, &Target::Project(id(PROJECT))))),
            [
                "New Terminal",
                "—",
                "# Juva & Git",
                "Commit with Juva…",
                "—",
                "# Workspace",
                "Shared files…",
                "New worktree…",
                "Refresh status",
                "Copy path",
                "Rename…",
                "—",
                "Open in ▸",
                "—",
                "# Project",
                "Actions…",
                "Ignored worktrees…",
                "Refresh project",
                "Set icon…",
                "Color ▸",
                "—",
                "Remove project…!",
            ]
        );
    }

    #[test]
    fn a_worktree_lists_agents_and_its_own_removal() {
        let mut store = store();
        let shell: WorkspaceId = id(RUNE_SHELL);
        store
            .workspaces
            .iter_mut()
            .find(|w| w.id == shell)
            .expect("the second checkout")
            .kind = WorkspaceKind::GitWorktree;
        store.providers = vec![
            crate::test_support::provider("claude", true, true, true),
            crate::test_support::provider("opencode", false, true, false),
        ];
        let s = state(store);
        let all = items(&s, &Target::Checkout(shell));
        assert_eq!(
            labels(&all),
            [
                "New Terminal",
                "—",
                "claude",
                "opencode (off)",
                "—",
                "# Juva & Git",
                "Commit with Juva…",
                "Preview PR",
                "Open PR with agent…",
                "—",
                "# Workspace",
                "Shared files…",
                "New worktree…",
                "Refresh status",
                "Copy path",
                "Rename…",
                "Move up",
                "Move down (off)",
                "Ignore worktrees in this folder",
                "Remove worktree!",
                "—",
                "Open in ▸",
            ],
            "a worktree sorts after the main checkout, so it can only move up"
        );
        assert_eq!(
            act_of(&all, "Copy path"),
            Act::Copy("/home/dev/forge-node-rune-shell".into())
        );
        let Some(Entry::Submenu { entries, .. }) = all.last() else {
            panic!("Open in is last");
        };
        assert_eq!(labels(entries), ["Zed", "Cursor", "—", "Finder"]);
        assert_eq!(
            act_of(entries, "Zed"),
            Act::OpenIn(Opener::Zed, "/home/dev/forge".into()),
            "Tauri opens the project's root, not the checkout"
        );
    }

    #[test]
    fn a_session_offers_kill_while_it_runs_and_restart_after() {
        let s = state(store());
        let running = items(&s, &Target::Session(id("1000")));
        assert_eq!(
            labels(&running),
            [
                "Rename…",
                "Continue in a New Session…",
                "Spawn Child…",
                "Send Context…",
                "Session Changes",
                "Review (off)",
                "—",
                "Kill!",
                "Close!",
            ],
            "Review is for agents"
        );
        assert_eq!(
            labels(&rendered(running)),
            ["Rename…", "—", "Kill!", "Close!"]
        );
        assert_eq!(
            labels(&rendered(items(&s, &Target::Tab(id("1003"))))),
            ["Rename…", "—", "Restart", "Close!"],
            "a shell's tab has the session's menu"
        );
    }

    #[test]
    fn an_editor_tab_closes_others_and_to_its_right_among_editors() {
        let mut store = store();
        let editors: Vec<SessionId> = ["1002", "1005", "1006"].map(id).to_vec();
        for session in &mut store.sessions {
            if editors.contains(&session.id) {
                session.kind = SessionKind::Editor;
            }
        }
        let mut s = state(store);
        s.select(id("1000"));
        let middle = items(&s, &Target::Tab(editors[1]));
        assert_eq!(
            labels(&middle),
            [
                "Close",
                "Close Others",
                "Close to the Right",
                "—",
                "Close All!"
            ]
        );
        assert_eq!(
            act_of(&middle, "Close Others"),
            Act::CloseEditors(vec![editors[0], editors[2]])
        );
        assert_eq!(
            act_of(&middle, "Close to the Right"),
            Act::CloseEditors(vec![editors[2]])
        );
        assert_eq!(
            labels(&items(&s, &Target::Tab(editors[2])))[2],
            "Close to the Right (off)"
        );
    }

    #[test]
    fn the_file_tree_menus_follow_root_file_and_folder() {
        let s = state(store());
        let workspace: WorkspaceId = id(MAIN);
        let menu = |target| labels(&items(&s, &Target::Files(workspace, target)));
        assert_eq!(
            menu(FileTarget::Root),
            [
                "New file…",
                "New folder…",
                "—",
                "Refresh",
                "Collapse folders",
                "Copy workspace path"
            ]
        );
        assert_eq!(
            menu(FileTarget::File("docs/README.md".into())),
            [
                "Open",
                "Edit",
                "Insert reference in terminal",
                "Copy path",
                "Copy relative path",
                "—",
                "New file…",
                "New folder…",
                "Duplicate…",
                "Rename…",
                "Move to…",
                "Delete!",
            ],
            "a rendered file can also be edited as source"
        );
        assert_eq!(
            menu(FileTarget::Directory("src".into())),
            [
                "Copy path",
                "Copy relative path",
                "—",
                "New file…",
                "New folder…",
                "Rename…",
                "Move to…",
                "Delete!",
            ]
        );
        let file = items(
            &s,
            &Target::Files(workspace, FileTarget::File("src/a.rs".into())),
        );
        assert_eq!(
            act_of(&file, "Copy path"),
            Act::Copy("/home/dev/forge/src/a.rs".into())
        );
        assert_eq!(
            act_of(&file, "New file…"),
            Act::NewPath {
                workspace,
                parent: "src".into(),
                directory: false
            },
            "a file's siblings are created beside it"
        );
    }

    #[test]
    fn rendering_drops_what_unported_items_leave_empty() {
        let unported = || item("x", Glyph::None, Act::Unported(Unported::Review));
        let kept = || item("y", Glyph::None, Act::CollapseFolders);
        let entries = vec![
            Entry::Rule,
            Entry::Heading("Empty"),
            unported(),
            Entry::Rule,
            Entry::Rule,
            Entry::Heading("Kept"),
            kept(),
            Entry::Submenu {
                label: "Gone",
                glyph: Glyph::None,
                entries: vec![unported()],
            },
            Entry::Rule,
            unported(),
        ];
        assert_eq!(labels(&rendered(entries)), ["# Kept", "y"]);
    }

    #[test]
    fn project_hues_keep_tauris_map_and_its_default() {
        let raw = Some(r#"{"a": 2, "b": 12, "c": "x", "d": 8}"#);
        assert_eq!(
            parse_hues(raw),
            BTreeMap::from([("a".to_string(), 2), ("d".to_string(), 8)]),
            "anything that is not a slot is dropped"
        );
        assert_eq!(parse_hues(Some("not json")), BTreeMap::new());
        assert_eq!(with_hue(raw, "e", Some(0)), r#"{"a":2,"d":8,"e":0}"#);
        assert_eq!(with_hue(raw, "a", None), r#"{"d":8}"#);

        let mut store = store();
        store.app_state.push((
            PROJECT_HUES_KEY.to_string(),
            format!(r#"{{"{}": 3}}"#, id::<ProjectId>(PROJECT)),
        ));
        let s = state(store);
        let all = items(&s, &Target::Project(id(PROJECT)));
        let Some(Entry::Submenu { entries, .. }) = all
            .iter()
            .find(|entry| matches!(entry, Entry::Submenu { label: "Color", .. }))
        else {
            panic!("a Color submenu");
        };
        let details: Vec<Option<&str>> = entries
            .iter()
            .filter_map(|entry| match entry {
                Entry::Item(item) => Some(item.detail.as_deref()),
                _ => None,
            })
            .collect();
        assert_eq!(details.len(), 10);
        assert_eq!(details[0], Some("Violet"), "the default names its hue");
        assert_eq!(
            details[4],
            Some("Current"),
            "slot 3 is Orchid, the fifth row"
        );
        assert_eq!(details.iter().filter(|d| d.is_some()).count(), 2);
    }

    #[test]
    fn a_copy_is_named_as_tauri_names_it() {
        let none = |_: &str| false;
        assert_eq!(
            duplicate_name("notes.md", none).as_deref(),
            Some("notes copy.md")
        );
        assert_eq!(
            duplicate_name("notes.md", |n| n == "notes copy.md").as_deref(),
            Some("notes copy 2.md")
        );
        assert_eq!(
            duplicate_name("notes copy.md", none).as_deref(),
            Some("notes copy 2.md")
        );
        assert_eq!(
            duplicate_name("notes copy 7.md", none).as_deref(),
            Some("notes copy 8.md")
        );
        assert_eq!(
            duplicate_name(".env.staging", none).as_deref(),
            Some(".env.staging copy"),
            "a leading dot is the name"
        );
        assert_eq!(duplicate_name("Makefile", |_| true), None);
    }

    #[test]
    fn a_terminal_reference_is_one_quoted_absolute_path() {
        assert_eq!(
            shell_reference("/home/dev/forge/", "src/it's.rs").as_deref(),
            Ok(r"'/home/dev/forge/src/it'\''s.rs'")
        );
        assert!(shell_reference("relative", "a").is_err());
        assert!(shell_reference("/r", "../etc/passwd").is_err());
        assert!(shell_reference("/r", "a\nb").is_err());
    }

    #[test]
    fn removing_a_project_never_offers_what_the_daemon_refuses() {
        let s = state(store());
        let print = footprint(s.store.as_ref().expect("a store"), id(PROJECT));
        assert_eq!(print.checkouts, 2);
        assert!(print.running_sessions > 0);
        assert!(policy_refused(RemoveProjectPolicy::KeepEverything, print));
        assert_eq!(
            chosen_policy(RemoveProjectPolicy::KeepEverything, print),
            RemoveProjectPolicy::KillSessionsKeepWorktrees
        );
        let idle = Footprint {
            checkouts: 1,
            managed_worktrees: 2,
            running_sessions: 0,
        };
        assert_eq!(
            describe_removal(
                RemoveProjectPolicy::KillSessionsRemoveManagedWorktrees,
                idle
            ),
            [
                "Forge Node stops tracking the project.",
                "Nothing is running, so no session is killed.",
                "2 worktrees Forge Node created are deleted from disk.",
                "No branch and no commit is deleted.",
            ]
        );

        let (handle, rx) = ConnectionHandle::test_handle(Arc::new(Metrics::default()));
        let mut s = s;
        s.connection = Some(handle);
        run(&mut s, Act::RemoveProject(id(PROJECT)), &mut Tx::default());
        assert!(
            rx.try_recv().is_err(),
            "the menu item asks, it does not remove"
        );
        remove_project(&mut s);
        assert!(matches!(
            rx.try_recv(),
            Ok(Command::RemoveProject {
                policy: RemoveProjectPolicy::KillSessionsKeepWorktrees,
                ..
            })
        ));
    }

    fn runtime(s: AppState) -> Runtime<AppState> {
        let mut rt = Runtime::new(
            s,
            crate::ui::view,
            (1180.0, 760.0),
            Env {
                system_dark: true,
                scale: 1.0,
            },
        );
        settle(&mut rt);
        rt
    }

    fn settle(rt: &mut Runtime<AppState>) {
        rt.frame(0.0);
        for _ in 0..30 {
            rt.frame(1.0 / 60.0);
        }
    }

    fn menu_items(rt: &Runtime<AppState>) -> Vec<String> {
        rt.elements()
            .into_iter()
            .filter(|e| e.role == "menuitem")
            .filter_map(|e| e.label)
            .collect()
    }

    fn center_of(rt: &Runtime<AppState>, pick: impl Fn(&rune::ElementInfo) -> bool) -> (f32, f32) {
        let found = rt
            .elements()
            .into_iter()
            .find(|e| pick(e))
            .expect("the element is drawn");
        (
            found.rect[0] + found.rect[2] / 2.0,
            found.rect[1] + found.rect[3] / 2.0,
        )
    }

    fn secondary_click(rt: &mut Runtime<AppState>, (x, y): (f32, f32)) {
        rt.secondary_down(x, y);
        rt.secondary_up(x, y);
        settle(rt);
    }

    fn click(rt: &mut Runtime<AppState>, (x, y): (f32, f32)) {
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        settle(rt);
    }

    #[test]
    fn a_right_click_on_a_project_opens_its_menu_and_an_item_sends_its_request() {
        let (handle, rx) = ConnectionHandle::test_handle(Arc::new(Metrics::default()));
        let mut s = state(store());
        s.connection = Some(handle);
        let mut rt = runtime(s);
        let row = format!("project-row-{}", id::<ProjectId>(PROJECT));
        let at = rt.find(&row).expect("the project row");
        secondary_click(&mut rt, at);
        assert_eq!(
            menu_items(&rt),
            [
                "New worktree…",
                "Shared files…",
                "Actions…",
                "Ignored worktrees…",
                "Refresh project",
                "Set icon…",
                "Color",
                "Move out of the group",
                "Copy path",
                "Remove project…"
            ]
        );
        let color = center_of(&rt, |e| e.label.as_deref() == Some("Color"));
        click(&mut rt, color);
        assert!(
            menu_items(&rt).iter().any(|label| label == "Violet"),
            "Color opens its hues"
        );
        let refresh = center_of(&rt, |e| {
            e.role == "menuitem" && e.label.as_deref() == Some("Refresh project")
        });
        click(&mut rt, refresh);
        assert!(menu_items(&rt).is_empty(), "choosing closes the menu");
        assert!(
            matches!(rx.try_recv(), Ok(Command::RefreshProject(project)) if project == id(PROJECT))
        );
    }

    #[test]
    fn the_file_tree_opens_a_rows_menu_the_roots_and_the_cursor_rows_by_key() {
        let mut s = state(store());
        s.show_panel(Panel::Files);
        let scope = s.workbench.scope().expect("a scope");
        let entry = |path: &str, kind| domain::FileEntry {
            path: path.into(),
            kind,
            ignored: false,
            symlink: None,
        };
        s.workbench.apply_directory(
            scope,
            "",
            Ok(domain::DirectoryListing {
                path: String::new(),
                entries: vec![
                    entry("src", domain::FileKind::Directory),
                    entry("README.md", domain::FileKind::File),
                ],
                truncated: false,
            }),
        );
        let mut rt = runtime(s);
        let row = rt.find("README.md").expect("the file's row");
        secondary_click(&mut rt, row);
        assert_eq!(menu_items(&rt)[0], "Open", "a file's own menu");
        rt.key(KeyEvent {
            key: Key::Escape,
            mods: Mods::default(),
        });
        settle(&mut rt);
        assert!(menu_items(&rt).is_empty());

        let tree = rt
            .elements()
            .into_iter()
            .find(|e| e.key.as_deref() == Some(FILES_TREE))
            .expect("the tree");
        secondary_click(
            &mut rt,
            (tree.rect[0] + 40.0, tree.rect[1] + tree.rect[3] - 20.0),
        );
        assert_eq!(menu_items(&rt)[0], "New file…", "empty space is the root's");
        rt.key(KeyEvent {
            key: Key::Escape,
            mods: Mods::default(),
        });
        settle(&mut rt);

        let folder = rt.find("src").expect("the folder's row");
        click(&mut rt, folder);
        rt.key(KeyEvent {
            key: Key::Function(10),
            mods: Mods {
                shift: true,
                ..Mods::default()
            },
        });
        settle(&mut rt);
        assert_eq!(
            menu_items(&rt)[0],
            "Copy path",
            "Shift+F10 opens the cursor row's menu, a folder's here"
        );
    }
}
