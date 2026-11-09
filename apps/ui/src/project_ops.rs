//! The project, share and action requests the rail and its dialogs send, run
//! on the client worker like every other request. Reads come back as
//! `Reply`s; mutations ack and the change arrives as a daemon event.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use domain::{
    ProjectAction, ProjectGroupId, ProjectId, ShareAction, ShareCandidate, ShareCleanup, ShareRule,
    ShareRuleId, ShareStatusEntry, ShareTrigger, WorkspaceId,
};

use crate::app::{AppState, CommandStatus};

#[derive(Clone, Debug)]
pub enum Op {
    CreateGroup(String),
    RenameGroup(ProjectGroupId, String),
    RemoveGroup(ProjectGroupId),
    MoveProject(ProjectId, Option<ProjectGroupId>),
    AddProjectTo(PathBuf, ProjectGroupId),
    ListBranches(ProjectId),
    CreateWorktree {
        project: ProjectId,
        branch: String,
        base: Option<String>,
        name: Option<String>,
    },
    RemoveWorktree {
        workspace: WorkspaceId,
        force: bool,
    },
    DetectShares(ProjectId),
    SetShares(ProjectId, Vec<ShareRule>),
    RemoveShare(ProjectId, ShareRuleId, ShareCleanup),
    PreviewShares(WorkspaceId),
    ShareStatus(WorkspaceId),
    ApplyShares(WorkspaceId, Option<Vec<ShareRuleId>>),
    AdoptIntoStore(ProjectId, String),
    SuggestActions(WorkspaceId),
    SetActions(ProjectId, Vec<ProjectAction>),
}

impl Op {
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Op::CreateGroup(_) => "CreateProjectGroup",
            Op::RenameGroup(..) => "RenameProjectGroup",
            Op::RemoveGroup(_) => "RemoveProjectGroup",
            Op::MoveProject(..) => "MoveProject",
            Op::AddProjectTo(..) => "AddProjectToGroup",
            Op::ListBranches(_) => "ListBranches",
            Op::CreateWorktree { .. } => "CreateWorktree",
            Op::RemoveWorktree { .. } => "RemoveWorktree",
            Op::DetectShares(_) => "DetectShareCandidates",
            Op::SetShares(..) => "SetProjectShares",
            Op::RemoveShare(..) => "RemoveShareRule",
            Op::PreviewShares(_) => "PreviewShares",
            Op::ShareStatus(_) => "GetShareStatus",
            Op::ApplyShares(..) => "ApplyShares",
            Op::AdoptIntoStore(..) => "AdoptIntoShareStore",
            Op::SuggestActions(_) => "SuggestActions",
            Op::SetActions(..) => "SetProjectActions",
        }
    }
}

#[derive(Debug)]
pub enum Answer {
    Done,
    Branches(client::Branches),
    Candidates(Vec<ShareCandidate>, bool),
    Plan(Vec<ShareAction>),
    Status(Vec<ShareStatusEntry>),
    Suggestions(client::ActionSuggestions),
}

#[derive(Debug)]
pub struct Reply {
    pub op: Op,
    pub result: Result<Answer, String>,
}

/// One request against the daemon, on the worker thread.
pub fn perform(client: &client::Client, op: Op) -> Reply {
    let done = |result: Result<(), client::ClientError>| result.map(|()| Answer::Done);
    let result = match &op {
        Op::CreateGroup(name) => done(client.create_project_group(name)),
        Op::RenameGroup(group, name) => done(client.rename_project_group(*group, name)),
        Op::RemoveGroup(group) => done(client.remove_project_group(*group)),
        Op::MoveProject(project, group) => done(client.move_project(*project, *group)),
        Op::AddProjectTo(path, group) => done(client.add_project_to_group(path, *group)),
        Op::ListBranches(project) => client.list_branches(*project).map(Answer::Branches),
        Op::CreateWorktree {
            project,
            branch,
            base,
            name,
        } => done(client.create_worktree(*project, branch, base.clone(), name.clone())),
        Op::RemoveWorktree { workspace, force } => done(client.remove_worktree(*workspace, *force)),
        Op::DetectShares(project) => client
            .detect_share_candidates(*project)
            .map(|(candidates, truncated)| Answer::Candidates(candidates, truncated)),
        Op::SetShares(project, rules) => done(client.set_project_shares(*project, rules.clone())),
        Op::RemoveShare(project, rule, cleanup) => client
            .remove_share_rule(*project, *rule, *cleanup)
            .map(Answer::Plan),
        Op::PreviewShares(workspace) => client.preview_shares(*workspace).map(Answer::Plan),
        Op::ShareStatus(workspace) => client.share_status(*workspace).map(Answer::Status),
        Op::ApplyShares(workspace, only) => done(client.apply_shares(*workspace, only.clone())),
        Op::AdoptIntoStore(project, path) => {
            done(client.adopt_into_share_store(*project, path.clone()))
        }
        Op::SuggestActions(workspace) => {
            client.suggest_actions(*workspace).map(Answer::Suggestions)
        }
        Op::SetActions(project, actions) => {
            done(client.set_project_actions(*project, actions.clone()))
        }
    };
    Reply {
        op,
        result: result.map_err(|error| error.to_string()),
    }
}

/// What the share pages read back; the rules themselves are the store's.
#[derive(Default)]
pub struct ShareReads {
    pub candidates: HashMap<ProjectId, (Vec<ShareCandidate>, bool)>,
    pub scanning: HashSet<ProjectId>,
    pub status: HashMap<WorkspaceId, Vec<ShareStatusEntry>>,
    /// The last preview or apply outcome, per checkout.
    pub actions: HashMap<WorkspaceId, Vec<ShareAction>>,
    pub trigger: HashMap<WorkspaceId, ShareTrigger>,
    pub applying: HashSet<WorkspaceId>,
    pub error: Option<String>,
}

/// Sends `op`; a worker that is not up yet is said where commands report.
pub fn send(s: &mut AppState, op: Op) -> bool {
    let label = op.label();
    let result = match &s.connection {
        Some(connection) => connection.project(op),
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

/// The share and branch reads begin here, so the state they wait on is set
/// before the answer can land.
pub fn request(s: &mut AppState, op: Op) {
    match &op {
        Op::DetectShares(project) => {
            s.projects.shares.scanning.insert(*project);
        }
        Op::ApplyShares(workspace, _) => {
            s.projects.shares.applying.insert(*workspace);
        }
        _ => {}
    }
    send(s, op);
}

/// A daemon event the share pages follow: an apply's outcome.
pub fn observe(s: &mut AppState, event: &protocol::DaemonEvent) {
    match event {
        protocol::DaemonEvent::WorkspaceCreated(workspace) => {
            crate::branch_picker::arrived(s, workspace);
        }
        protocol::DaemonEvent::RemoteRefsUpdated { project_id, .. } => {
            crate::branch_picker::fetched(s, *project_id);
        }
        _ => {}
    }
    if let protocol::DaemonEvent::SharesApplied {
        workspace_id,
        trigger,
        actions,
        error,
        ..
    } = event
    {
        let reads = &mut s.projects.shares;
        reads.applying.remove(workspace_id);
        reads.actions.insert(*workspace_id, actions.clone());
        reads.trigger.insert(*workspace_id, *trigger);
        if let Some(error) = error {
            reads.error = Some(error.clone());
        }
        let refresh = Op::ShareStatus(*workspace_id);
        send(s, refresh);
    }
}

pub fn apply(s: &mut AppState, reply: Reply) {
    let label = reply.op.label();
    let message = match &reply.result {
        Ok(_) => String::new(),
        Err(error) => error.clone(),
    };
    match (reply.op, reply.result) {
        (Op::ListBranches(project), result) => {
            if let Ok(Answer::Branches(branches)) = &result {
                s.workbench.apply_branches(project, Ok(branches.clone()));
            } else if let Err(error) = &result {
                s.workbench.apply_branches(project, Err(error.clone()));
            }
            crate::branch_picker::apply_branches(s, project, result);
            return;
        }
        (
            Op::CreateWorktree {
                project, branch, ..
            },
            Err(error),
        ) => {
            crate::branch_picker::failed(s, project, &branch, error);
        }
        (Op::RemoveWorktree { workspace, force }, Err(error)) => {
            if !force {
                s.projects.removal = Some(crate::projects::Removal {
                    workspace,
                    reason: Some(error),
                });
            } else {
                s.projects.refused = Some((workspace, crate::projects::removal_block(&error)));
            }
        }
        (Op::DetectShares(project), result) => {
            let reads = &mut s.projects.shares;
            reads.scanning.remove(&project);
            match result {
                Ok(Answer::Candidates(candidates, truncated)) => {
                    reads.error = None;
                    reads.candidates.insert(project, (candidates, truncated));
                }
                Err(error) => reads.error = Some(error),
                Ok(_) => {}
            }
            return;
        }
        (Op::ShareStatus(workspace), Ok(Answer::Status(entries))) => {
            s.projects.shares.status.insert(workspace, entries);
            return;
        }
        (Op::PreviewShares(workspace), Ok(Answer::Plan(actions))) => {
            s.projects.shares.actions.insert(workspace, actions);
            return;
        }
        (Op::ApplyShares(workspace, _), Err(error)) => {
            s.projects.shares.applying.remove(&workspace);
            s.projects.shares.error = Some(error);
        }
        (Op::SetShares(..) | Op::RemoveShare(..) | Op::AdoptIntoStore(..), Err(error))
        | (Op::PreviewShares(_) | Op::ShareStatus(_), Err(error)) => {
            s.projects.shares.error = Some(error);
        }
        (Op::RemoveShare(project, ..), Ok(_)) => {
            refresh_status(s, project);
        }
        (Op::SuggestActions(workspace), result) => {
            crate::actions_editor::apply_suggestions(s, workspace, result);
            return;
        }
        (Op::SetActions(project, _), result) => {
            crate::actions_editor::saved(s, project, result.err());
        }
        (Op::AddProjectTo(..), Ok(_)) => s.request_refresh(),
        _ => {}
    }
    s.last_command = Some(CommandStatus {
        label,
        ok: message.is_empty(),
        message: if message.is_empty() {
            "done".to_string()
        } else {
            message
        },
        took_us: 0,
    });
}

/// Asks for the per-rule state of every checkout of `project`.
pub fn refresh_status(s: &mut AppState, project: ProjectId) {
    let checkouts: Vec<WorkspaceId> = s
        .store
        .as_ref()
        .map(|store| {
            store
                .workspaces
                .iter()
                .filter(|w| w.project_id == project)
                .map(|w| w.id)
                .collect()
        })
        .unwrap_or_default();
    for workspace in checkouts {
        send(s, Op::ShareStatus(workspace));
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::Arc;

    use domain::{ProjectId, ShareStrategy, Workspace, WorkspaceKind};
    use rune::Tx;

    use super::*;
    use crate::connection::{Command, ConnectionHandle, Metrics, fixture_store};
    use crate::menus::{Act, ButtonMenu, Entry, Target};
    use crate::terminal::SharedTerminal;

    fn id<T: std::str::FromStr>(tail: &str) -> T
    where
        T::Err: std::fmt::Debug,
    {
        format!("00000000-0000-7000-8000-{tail:0>12}")
            .parse()
            .expect("a fixture id")
    }

    fn state() -> (AppState, flume::Receiver<Command>) {
        let store =
            fixture_store(&Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/snapshot.json"))
                .expect("fixture snapshot decodes");
        let metrics = Arc::new(Metrics::default());
        let mut s = AppState::for_bench(store, Arc::clone(&metrics), SharedTerminal::new());
        let (handle, rx) = ConnectionHandle::test_handle(metrics);
        s.connection = Some(handle);
        (s, rx)
    }

    fn sent(rx: &flume::Receiver<Command>) -> Vec<Op> {
        rx.try_iter()
            .filter_map(|command| match command {
                Command::Project(op) => Some(op),
                _ => None,
            })
            .collect()
    }

    fn labels(entries: &[Entry]) -> Vec<String> {
        entries
            .iter()
            .map(|entry| match entry {
                Entry::Item(item) => item.label.clone(),
                Entry::Rule => "—".into(),
                Entry::Heading(h) => format!("# {h}"),
                Entry::Submenu { label, .. } => format!("{label} ▸"),
            })
            .collect()
    }

    const PROJECT: &str = "1";
    const GROUP: &str = "2";
    const MAIN: &str = "3";

    #[test]
    fn the_group_and_rail_menus_create_rename_remove_and_move() {
        let (mut s, rx) = state();
        let group: ProjectGroupId = id(GROUP);
        let group_menu = crate::menus::items(&s, &Target::Group(group));
        assert_eq!(
            labels(&group_menu),
            ["Add project…", "—", "Rename group…", "Remove group"]
        );
        assert_eq!(
            labels(&crate::menus::button_items(&s, &ButtonMenu::RailAdd)),
            ["Add a project…", "New group…"]
        );
        crate::menus::run(&mut s, Act::NewGroup, &mut Tx::default());
        s.menus.prompt.as_mut().expect("a name is asked").edit = rune::TextEdit::new("Clients");
        crate::menus::submit_prompt(&mut s);
        crate::menus::run(&mut s, Act::RenameGroup(group), &mut Tx::default());
        assert_eq!(
            s.menus.prompt.as_ref().map(|p| p.edit.text.clone()),
            Some("Forge".into()),
            "a rename starts from the name"
        );
        crate::menus::submit_prompt(&mut s);
        crate::menus::run(&mut s, Act::RemoveGroup(group), &mut Tx::default());
        crate::menus::run(
            &mut s,
            Act::MoveToGroup(id(PROJECT), None),
            &mut Tx::default(),
        );
        let ops = sent(&rx);
        assert!(matches!(&ops[0], Op::CreateGroup(name) if name == "Clients"));
        assert!(matches!(&ops[1], Op::RenameGroup(g, name) if *g == group && name == "Forge"));
        assert!(matches!(&ops[2], Op::RemoveGroup(g) if *g == group));
        assert!(matches!(&ops[3], Op::MoveProject(p, None) if *p == id(PROJECT)));
    }

    #[test]
    fn removing_a_worktree_asks_then_forces_only_after_a_refusal() {
        let (mut s, rx) = state();
        let workspace: WorkspaceId = id("31");
        crate::menus::run(&mut s, Act::RemoveWorktree(workspace), &mut Tx::default());
        assert!(sent(&rx).is_empty(), "the item asks, it does not remove");
        crate::projects::confirm_removal(&mut s);
        let ops = sent(&rx);
        assert!(matches!(
            ops.as_slice(),
            [Op::RemoveWorktree { force: false, .. }]
        ));
        apply(
            &mut s,
            Reply {
                op: ops[0].clone(),
                result: Err("cannot remove: running_sessions=true, dirty=true".into()),
            },
        );
        let removal = s.projects.removal.clone().expect("the dialog asks again");
        assert_eq!(
            crate::projects::removal_block(removal.reason.as_deref().unwrap_or_default()),
            [
                "One or more sessions are still running in it.",
                "It has uncommitted changes."
            ]
        );
        crate::projects::confirm_removal(&mut s);
        assert!(matches!(
            sent(&rx).as_slice(),
            [Op::RemoveWorktree { force: true, .. }]
        ));
    }

    #[test]
    fn the_branch_picker_creates_from_a_remote_and_opens_a_terminal_when_it_lands() {
        let (mut s, rx) = state();
        let project: ProjectId = id(PROJECT);
        crate::branch_picker::open(&mut s, project);
        assert!(matches!(sent(&rx).as_slice(), [Op::ListBranches(p)] if *p == project));
        let branches = client::Branches {
            branches: vec![domain::BranchRef {
                name: "fix/b".into(),
                scope: domain::RefScope::Remote {
                    remote: "origin".into(),
                },
                upstream: None,
                committed_at: None,
                subject: None,
                checked_out_in: None,
            }],
            remotes: Vec::new(),
            default_branch: Some("main".into()),
        };
        apply(
            &mut s,
            Reply {
                op: Op::ListBranches(project),
                result: Ok(Answer::Branches(branches)),
            },
        );
        s.projects.picker.as_mut().expect("open").name = rune::TextEdit::new("fixb");
        crate::branch_picker::choose(&mut s);
        assert!(matches!(
            sent(&rx).as_slice(),
            [Op::CreateWorktree { base: Some(base), name: Some(name), .. }]
                if base == "origin/fix/b" && name == "fixb"
        ));
        let landed = Workspace {
            id: WorkspaceId::new(),
            project_id: project,
            kind: WorkspaceKind::GitWorktree,
            path: "/home/dev/fixb".into(),
            branch: Some("fix/b".into()),
            display_name: None,
            managed_by_app: true,
            created_at: domain::Timestamp::now(),
            status: domain::WorkspaceStatus::default(),
        };
        observe(
            &mut s,
            &protocol::DaemonEvent::WorkspaceCreated(landed.clone()),
        );
        assert!(
            s.projects.picker.is_none(),
            "the picker closes once it lands"
        );
        assert!(rx.try_iter().any(|command| matches!(
            command,
            Command::CreateShellSession { workspace_id } if workspace_id == landed.id
        )));
    }

    #[test]
    fn share_edits_send_the_whole_set_and_linking_adopts_the_file() {
        let (mut s, rx) = state();
        let project: ProjectId = id(PROJECT);
        crate::shares::open_dialog(&mut s, project, Some(id(MAIN)));
        let opened = sent(&rx);
        assert!(matches!(opened[0], Op::DetectShares(p) if p == project));
        assert_eq!(
            opened
                .iter()
                .filter(|op| matches!(op, Op::ShareStatus(_)))
                .count(),
            2,
            "each checkout's state is read"
        );
        crate::shares::add(&mut s, project, "/.env/", ShareStrategy::Copy);
        let Op::SetShares(_, rules) = &sent(&rx)[0] else {
            panic!("the set is sent");
        };
        assert_eq!(rules[0].path, ".env");
        if let Some(store) = s.store.as_mut() {
            store.worktree_shares = rules.clone();
        }
        crate::shares::add(&mut s, project, ".env", ShareStrategy::Copy);
        assert_eq!(
            s.projects.share_form.path_error.as_deref(),
            Some("That path already has a rule.")
        );
        crate::shares::retarget(&mut s, project, rules[0].id, 0);
        let ops = sent(&rx);
        assert!(
            matches!(&ops[0], Op::SetShares(_, next) if next[0].strategy == ShareStrategy::Link)
        );
        assert!(matches!(&ops[1], Op::AdoptIntoStore(_, path) if path == ".env"));
        crate::menus::run(
            &mut s,
            Act::RemoveRule(project, rules[0].id),
            &mut Tx::default(),
        );
        crate::shares::confirm_removal(&mut s);
        assert!(matches!(
            sent(&rx).as_slice(),
            [Op::RemoveShare(_, rule, domain::ShareCleanup::Leave)] if *rule == rules[0].id
        ));
        observe(
            &mut s,
            &protocol::DaemonEvent::SharesApplied {
                workspace_id: id(MAIN),
                trigger: domain::ShareTrigger::Requested,
                actions: Vec::new(),
                error: Some("setup failed".into()),
            },
        );
        assert_eq!(s.projects.shares.error.as_deref(), Some("setup failed"));
        assert!(matches!(sent(&rx).as_slice(), [Op::ShareStatus(w)] if *w == id(MAIN)));
    }

    #[test]
    fn the_run_menu_runs_an_idle_action_and_offers_the_editor() {
        let (mut s, _rx) = state();
        if let Some(store) = s.store.as_mut() {
            store.project_actions.clear();
        }
        let workspace: WorkspaceId = id(MAIN);
        assert_eq!(
            labels(&crate::menus::button_items(&s, &ButtonMenu::Run(workspace))),
            ["Set up actions…"]
        );
        let action = domain::ProjectAction {
            id: domain::ProjectActionId::new(),
            project_id: id(PROJECT),
            label: "dev".into(),
            command: "npm run dev".into(),
            kind: domain::ActionKind::Dev,
            cwd: None,
            single_instance: true,
            pinned: false,
            ready: domain::ActionReady::None,
            port: None,
            depends_on: Vec::new(),
            source: domain::ActionSource::Manual,
            position: 0,
            created_at: domain::Timestamp::now(),
        };
        if let Some(store) = s.store.as_mut() {
            store.project_actions.push(action.clone());
        }
        let idle = crate::menus::button_items(&s, &ButtonMenu::Run(workspace));
        assert_eq!(labels(&idle), ["dev", "—", "Edit actions…"]);
        let Entry::Item(first) = &idle[0] else {
            panic!("an item");
        };
        assert_eq!(first.act, Act::RunAction(workspace, action.id));
        if let Some(store) = s.store.as_mut() {
            let live = store
                .sessions
                .iter_mut()
                .find(|session| session.workspace_id == workspace && session.state.is_active())
                .expect("a live session");
            live.action = Some(Box::new(domain::ActionRun {
                action_id: action.id,
                label: "dev".into(),
                port: Some(5173),
                readiness: domain::ActionReadiness::Ready { url: None },
                started_at: None,
            }));
        }
        let live = crate::menus::button_items(&s, &ButtonMenu::Run(workspace));
        assert_eq!(
            labels(&live),
            [
                "dev",
                "Open http://localhost:5173 in the browser",
                "Restart dev",
                "Stop dev",
                "—",
                "Edit actions…"
            ]
        );
        assert_eq!(
            crate::menus::url_badge("http://localhost:5173/x").as_deref(),
            Some(":5173")
        );
        assert_eq!(
            crate::menus::url_badge("https://example.com/").as_deref(),
            Some("example.com")
        );
    }

    #[test]
    fn the_actions_editor_drafts_once_and_saves_the_whole_set() {
        let (mut s, rx) = state();
        if let Some(store) = s.store.as_mut() {
            store.project_actions.clear();
        }
        let project: ProjectId = id(PROJECT);
        let workspace: WorkspaceId = id(MAIN);
        crate::menus::run(&mut s, Act::Actions(project, workspace), &mut Tx::default());
        assert!(matches!(sent(&rx).as_slice(), [Op::SuggestActions(w)] if *w == workspace));
        let suggestion = domain::ActionSuggestion {
            label: "test".into(),
            command: "cargo test".into(),
            kind: domain::ActionKind::Test,
            pinned: false,
            cwd: None,
            single_instance: true,
            ready: domain::ActionReady::None,
            port: None,
            depends_on: Vec::new(),
            source: domain::ActionSource::Detected,
            detected_from: "Cargo.toml".into(),
            saved_as: None,
        };
        let found = || client::ActionSuggestions {
            suggestions: vec![suggestion.clone()],
            repo_file: None,
            truncated: false,
        };
        apply(
            &mut s,
            Reply {
                op: Op::SuggestActions(workspace),
                result: Ok(Answer::Suggestions(found())),
            },
        );
        let editor = s.projects.actions.as_mut().expect("open");
        let rows = editor.rows.as_mut().expect("drafted");
        assert_eq!(rows.len(), 1);
        rows[0].include = true;
        rows[0].label = rune::TextEdit::new("unit");
        apply(
            &mut s,
            Reply {
                op: Op::SuggestActions(workspace),
                result: Ok(Answer::Suggestions(found())),
            },
        );
        assert_eq!(
            s.projects
                .actions
                .as_ref()
                .and_then(|e| e.rows.as_ref())
                .map(|r| r[0].label.text.clone()),
            Some("unit".into()),
            "a later answer does not wipe what is typed"
        );
        let mut rt = rune::Runtime::new(
            s,
            crate::ui::view,
            (1180.0, 760.0),
            rune::Env {
                system_dark: true,
                scale: 1.0,
            },
        );
        rt.frame(0.0);
        rt.frame(0.5);
        let save = rt.find("actions-save").expect("the Save button");
        rt.pointer_down(save.0, save.1);
        rt.pointer_up(save.0, save.1);
        rt.frame(1.0);
        let ops = sent(&rx);
        let Some(Op::SetActions(p, set)) = ops.first() else {
            panic!("Save sends the set: {ops:?}");
        };
        assert_eq!(*p, project);
        assert_eq!(set.len(), 1);
        assert_eq!(set[0].label, "unit");
        apply(
            &mut rt.state,
            Reply {
                op: ops[0].clone(),
                result: Ok(Answer::Done),
            },
        );
        assert!(
            rt.state.projects.actions.is_none(),
            "a saved set closes the dialog"
        );
    }

    fn snapshot(client: &client::Client) -> client::Store {
        let response = client
            .request(protocol::Request::GetSnapshot)
            .expect("a snapshot");
        let mut store = client::Store::new();
        store.apply_snapshot(response);
        store
    }

    fn ok(client: &client::Client, op: Op) -> Answer {
        let label = op.label();
        perform(client, op)
            .result
            .unwrap_or_else(|error| panic!("{label}: {error}"))
    }

    fn wait_for(
        events: &flume::Receiver<protocol::DaemonEvent>,
        what: impl Fn(&protocol::DaemonEvent) -> bool,
    ) -> protocol::DaemonEvent {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        loop {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            match events.recv_timeout(left) {
                Ok(event) if what(&event) => return event,
                Ok(_) => {}
                Err(error) => panic!("no such event: {error}"),
            }
        }
    }

    /// Every request the rail sends, against a scratch daemon:
    /// `FORGE_UI_LIVE_SOCKET` names its socket and `FORGE_UI_LIVE_REPO` a
    /// throwaway repository with an `origin`, an ignored `.env` and a
    /// `.forge/actions.toml`. Never point it at a daily daemon.
    #[test]
    #[ignore = "needs an isolated daemon and a scratch repository"]
    fn every_project_request_round_trips_against_a_live_daemon() {
        let socket = std::env::var_os("FORGE_UI_LIVE_SOCKET").expect("FORGE_UI_LIVE_SOCKET");
        let repo = std::fs::canonicalize(
            std::env::var_os("FORGE_UI_LIVE_REPO").expect("FORGE_UI_LIVE_REPO"),
        )
        .expect("the repository exists");
        let client = client::Client::connect(Path::new(&socket), crate::connection::CLIENT_VERSION)
            .expect("the scratch daemon answers");
        let events = client.events();
        client.add_project(&repo).expect("AddProject");
        let store = snapshot(&client);
        let project = store
            .projects
            .iter()
            .find(|p| p.root_path == repo)
            .expect("the project")
            .id;
        let main = crate::projects::checkouts_of(&store.workspaces, project)[0].id;

        ok(&client, Op::CreateGroup("Scratch".into()));
        let group = snapshot(&client)
            .project_groups
            .iter()
            .find(|g| g.name == "Scratch")
            .expect("the group")
            .id;
        ok(&client, Op::RenameGroup(group, "Scratch 2".into()));
        ok(&client, Op::MoveProject(project, Some(group)));
        let moved = snapshot(&client);
        assert_eq!(
            moved
                .projects
                .iter()
                .find(|p| p.id == project)
                .and_then(|p| p.project_group_id),
            Some(group)
        );
        assert_eq!(moved.project_groups[0].name, "Scratch 2");

        let Answer::Branches(branches) = ok(&client, Op::ListBranches(project)) else {
            panic!("ListBranches answers with branches");
        };
        let rows = crate::branch_picker::entries("", &branches, &moved.workspaces);
        let remote = rows
            .iter()
            .find(|row| row.label == "feature/remote-only")
            .expect("the remote-only branch is offered");
        assert_eq!(remote.group, "Remote branches");
        let choice = remote
            .choice
            .clone()
            .expect("a remote branch can be checked out");
        assert!(rows.iter().any(|row| row.label == "main" && !row.enabled));
        ok(
            &client,
            Op::CreateWorktree {
                project,
                branch: choice.branch.clone(),
                base: choice.base.clone(),
                name: Some("remote-only".into()),
            },
        );
        let protocol::DaemonEvent::WorkspaceCreated(worktree) = wait_for(
            &events,
            |event| matches!(event, protocol::DaemonEvent::WorkspaceCreated(w) if w.kind == WorkspaceKind::GitWorktree),
        ) else {
            unreachable!()
        };
        assert_eq!(worktree.branch.as_deref(), Some("feature/remote-only"));
        assert!(worktree.path.ends_with("remote-only"));
        wait_for(
            &events,
            |event| matches!(event, protocol::DaemonEvent::SharesApplied { workspace_id, .. } if *workspace_id == worktree.id),
        );

        let Answer::Candidates(candidates, _) = ok(&client, Op::DetectShares(project)) else {
            panic!("DetectShareCandidates answers with candidates");
        };
        assert!(
            candidates.iter().any(|c| c.path == ".env"),
            "{candidates:?}"
        );
        let rule = domain::ShareRule {
            id: ShareRuleId::new(),
            project_id: project,
            path: ".env".into(),
            strategy: ShareStrategy::Copy,
            enabled: true,
            position: 0,
            created_at: domain::Timestamp::now(),
        };
        ok(&client, Op::SetShares(project, vec![rule.clone()]));
        let Answer::Plan(plan) = ok(&client, Op::PreviewShares(worktree.id)) else {
            panic!("PreviewShares answers with a plan");
        };
        assert!(plan.iter().any(|a| a.path == ".env"), "{plan:?}");
        ok(&client, Op::ApplyShares(worktree.id, None));
        wait_for(
            &events,
            |event| matches!(event, protocol::DaemonEvent::SharesApplied { workspace_id, .. } if *workspace_id == worktree.id),
        );
        let Answer::Status(status) = ok(&client, Op::ShareStatus(worktree.id)) else {
            panic!("GetShareStatus answers with entries");
        };
        assert_eq!(status[0].state, domain::ShareState::Applied);
        assert!(worktree.path.join(".env").is_file());
        ok(
            &client,
            Op::RemoveShare(project, rule.id, ShareCleanup::RemoveInjected),
        );
        assert!(
            !worktree.path.join(".env").exists(),
            "the injected copy is removed"
        );

        let Answer::Suggestions(found) = ok(&client, Op::SuggestActions(main)) else {
            panic!("SuggestActions answers with suggestions");
        };
        let repo_file = found
            .repo_file
            .clone()
            .expect("the repository offers actions");
        assert_eq!(repo_file.actions[0].label, "serve");
        let mut rows = crate::actions_editor::build_draft(
            project,
            &[],
            &found.suggestions,
            Some(&repo_file),
            domain::ProjectActionId::new,
        );
        assert!(rows.iter().any(|row| row.action.label == "test"));
        for row in &mut rows {
            row.include = true;
        }
        let set = crate::actions_editor::to_action_set(&rows);
        ok(&client, Op::SetActions(project, set.clone()));
        let saved = snapshot(&client);
        let labels: Vec<&str> = saved
            .project_actions
            .iter()
            .filter(|a| a.project_id == project)
            .map(|a| a.label.as_str())
            .collect();
        assert_eq!(
            labels,
            set.iter().map(|a| a.label.as_str()).collect::<Vec<_>>()
        );
        assert!(matches!(
            saved
                .project_actions
                .iter()
                .find(|a| a.label == "serve")
                .map(|a| &a.source),
            Some(domain::ActionSource::Repo { .. })
        ));

        std::fs::write(worktree.path.join("dirty.txt"), "x").expect("a dirty worktree");
        let refused = perform(
            &client,
            Op::RemoveWorktree {
                workspace: worktree.id,
                force: false,
            },
        );
        let reason = refused.result.expect_err("a dirty worktree is refused");
        assert!(
            crate::projects::removal_block(&reason)
                .iter()
                .any(|line| line == "It has uncommitted changes."),
            "{reason}"
        );
        ok(
            &client,
            Op::RemoveWorktree {
                workspace: worktree.id,
                force: true,
            },
        );
        assert!(!worktree.path.exists());

        ok(&client, Op::MoveProject(project, None));
        ok(&client, Op::RemoveGroup(group));
        assert!(snapshot(&client).project_groups.is_empty());
    }
}
