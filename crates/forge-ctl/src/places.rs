//! Projects, checkouts and project actions: where a session runs, resolved one way for every command.

use super::*;
use client::Store;
use domain::orchestration::PrefixError;
use domain::{Project, ProjectAction, ProjectId, Workspace, WorkspaceKind};

#[derive(Subcommand)]
pub(crate) enum ProjectCmd {
    /// Known projects: name, root path, id and checkout count.
    List,
}

#[derive(Subcommand)]
pub(crate) enum WorkspaceCmd {
    /// Checkouts, optionally of one project.
    List {
        #[arg(long)]
        project: Option<String>,
    },
    /// Remove a managed worktree. The branch stays.
    Remove {
        /// Workspace UUID or unique prefix.
        workspace: String,
        /// Remove even with a dirty tree or running sessions.
        #[arg(long)]
        force: bool,
    },
}

#[derive(Subcommand)]
pub(crate) enum ActionCmd {
    /// Approved actions of a project.
    List {
        #[arg(long)]
        project: String,
    },
    /// Run an approved action as a task session.
    Run {
        /// Action label (case-insensitive), UUID or unique prefix.
        action: String,
        #[arg(long)]
        project: String,
        /// Workspace UUID or unique prefix. Defaults to the main checkout.
        #[arg(long)]
        workspace: Option<String>,
    },
}

pub(crate) fn load(app: &App, client: &Client) -> Result<Store, u8> {
    client.load_store().map_err(|e| app.client_error(e, false))
}

/// Exact id, case-insensitive name, root path, or a path that canonicalizes to the root.
pub(crate) fn matching_projects<'a>(store: &'a Store, raw: &str) -> Vec<&'a Project> {
    let canonical = std::fs::canonicalize(raw).ok();
    store
        .projects
        .iter()
        .filter(|p| {
            p.id.to_string() == raw
                || p.name.eq_ignore_ascii_case(raw)
                || p.root_path == Path::new(raw)
                || canonical.as_ref() == Some(&p.root_path)
        })
        .collect()
}

pub(crate) fn project<'a>(app: &App, store: &'a Store, raw: &str) -> Result<&'a Project, u8> {
    match matching_projects(store, raw).as_slice() {
        [project] => Ok(project),
        [] => Err(app.fail(
            4,
            "not_found",
            "project not found",
            json!({ "projects": store.projects.iter().map(|p| &p.name).collect::<Vec<_>>() }),
            next(&["forgectl", "project", "list"]),
        )),
        many => Err(app.fail(
            5,
            "ambiguous",
            "project is ambiguous; use its id or path",
            json!({ "projects": many }),
            next(&["forgectl", "project", "list"]),
        )),
    }
}

/// `raw` names a checkout by UUID or prefix; `None` is the project's main checkout.
pub(crate) fn workspace<'a>(
    app: &App,
    store: &'a Store,
    project: Option<ProjectId>,
    raw: Option<&str>,
) -> Result<&'a Workspace, u8> {
    let in_project: Vec<&Workspace> = store
        .workspaces
        .iter()
        .filter(|w| project.is_none_or(|id| w.project_id == id))
        .collect();
    let found = match raw {
        None => in_project
            .iter()
            .find(|w| w.kind == WorkspaceKind::Main)
            .copied()
            .ok_or(PrefixError::None),
        Some(raw) => {
            let ids: Vec<WorkspaceId> = in_project.iter().map(|w| w.id).collect();
            resolve_prefix(ids.iter(), raw).and_then(|id| {
                in_project
                    .iter()
                    .find(|w| w.id == *id)
                    .copied()
                    .ok_or(PrefixError::None)
            })
        }
    };
    found.map_err(|error| {
        let (code, name) = if error == PrefixError::Ambiguous {
            (5, "ambiguous")
        } else {
            (4, "not_found")
        };
        app.fail(
            code,
            name,
            &format!("workspace: {error}"),
            json!({}),
            next(&["forgectl", "workspace", "list"]),
        )
    })
}

fn project_name(store: &Store, id: ProjectId) -> Option<&str> {
    store
        .projects
        .iter()
        .find(|p| p.id == id)
        .map(|p| p.name.as_str())
}

pub(crate) fn workspace_json(store: &Store, workspace: &Workspace) -> Value {
    let mut value = serde_json::to_value(workspace).unwrap_or(Value::Null);
    if let Some(object) = value.as_object_mut() {
        object.insert(
            "project".into(),
            json!(project_name(store, workspace.project_id)),
        );
        object.insert("prefix".into(), json!(&workspace.id.to_string()[..8]));
    }
    value
}

fn action<'a>(
    app: &App,
    actions: &[&'a ProjectAction],
    raw: &str,
) -> Result<&'a ProjectAction, u8> {
    let by_label: Vec<_> = actions
        .iter()
        .filter(|a| a.label.eq_ignore_ascii_case(raw))
        .copied()
        .collect();
    let found = match by_label.as_slice() {
        [one] => Ok(*one),
        [] => {
            let ids: Vec<_> = actions.iter().map(|a| a.id).collect();
            resolve_prefix(ids.iter(), raw).and_then(|id| {
                actions
                    .iter()
                    .find(|a| a.id == *id)
                    .copied()
                    .ok_or(PrefixError::None)
            })
        }
        _ => Err(PrefixError::Ambiguous),
    };
    found.map_err(|error| {
        let code = if error == PrefixError::Ambiguous { 5 } else { 4 };
        app.fail(
            code,
            if code == 5 { "ambiguous" } else { "not_found" },
            &format!("action: {error}"),
            json!({ "actions": actions.iter().map(|a| json!({ "id": a.id, "label": &a.label })).collect::<Vec<_>>() }),
            next(&["forgectl", "action", "list", "--help"]),
        )
    })
}

pub(crate) fn project_cmd(app: &App, client: &Client, cmd: ProjectCmd) -> Result<(), u8> {
    match cmd {
        ProjectCmd::List => {
            let store = load(app, client)?;
            let projects: Vec<Value> = store
                .projects
                .iter()
                .map(|p| {
                    json!({ "id": p.id, "name": &p.name, "root_path": &p.root_path,
                        "git": p.git_root.is_some(),
                        "workspaces": store.workspaces.iter().filter(|w| w.project_id == p.id).count() })
                })
                .collect();
            app.print_ok(json!({ "projects": projects }));
            Ok(())
        }
    }
}

pub(crate) fn workspace_cmd(app: &App, client: &Client, cmd: WorkspaceCmd) -> Result<(), u8> {
    match cmd {
        WorkspaceCmd::List { project: raw } => {
            let store = load(app, client)?;
            let project = raw
                .as_deref()
                .map(|raw| project(app, &store, raw).map(|p| p.id))
                .transpose()?;
            let workspaces: Vec<Value> = store
                .workspaces
                .iter()
                .filter(|w| project.is_none_or(|id| w.project_id == id))
                .map(|w| workspace_json(&store, w))
                .collect();
            app.print_ok(json!({ "workspaces": workspaces }));
            Ok(())
        }
        WorkspaceCmd::Remove {
            workspace: raw,
            force,
        } => {
            let store = load(app, client)?;
            let workspace_id = workspace(app, &store, None, Some(&raw))?.id;
            let response = app.call(
                client,
                Request::RemoveWorktree {
                    workspace_id,
                    force,
                },
                true,
            )?;
            app.print_ok(flatten(&response));
            Ok(())
        }
    }
}

pub(crate) fn action_cmd(app: &App, client: &Client, cmd: ActionCmd) -> Result<(), u8> {
    let store = load(app, client)?;
    match cmd {
        ActionCmd::List { project: raw } => {
            let project_id = project(app, &store, &raw)?.id;
            let actions: Vec<_> = store
                .project_actions
                .iter()
                .filter(|a| a.project_id == project_id)
                .collect();
            app.print_ok(json!({ "actions": actions }));
            Ok(())
        }
        ActionCmd::Run {
            action: raw,
            project: raw_project,
            workspace: raw_workspace,
        } => {
            let project_id = project(app, &store, &raw_project)?.id;
            let actions: Vec<&ProjectAction> = store
                .project_actions
                .iter()
                .filter(|a| a.project_id == project_id)
                .collect();
            let action_id = action(app, &actions, &raw)?.id;
            let workspace_id =
                workspace(app, &store, Some(project_id), raw_workspace.as_deref())?.id;
            let response = app.call(
                client,
                Request::RunAction {
                    workspace_id,
                    action_id,
                },
                true,
            )?;
            app.print_ok(flatten(&response));
            Ok(())
        }
    }
}
