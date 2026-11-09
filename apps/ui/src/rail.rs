//! The rail as data, as this client, the equivalent and
//! the equivalent: group → project → checkout → session, the folds and
//! checkout order kept in `app_state`, the flat row list the keyboard walks,
//! and the sessions waiting on the person.

use std::collections::{BTreeMap, BTreeSet};

use domain::{ProjectGroupId, ProjectId, Session, SessionId, Workspace, WorkspaceId};

/// Shared with the original client: a JSON array of folded row ids.
pub const FOLDS_KEY: &str = "ui.sidebar.collapsed";
/// Shared with the original client: `{ project id: [checkout id, …] }`.
pub const ORDER_KEY: &str = "ui.sidebar.workspace_order";

#[derive(Default)]
pub struct Rail {
    pub folds: BTreeSet<String>,
    pub order: BTreeMap<String, Vec<String>>,
    /// The folds were read from the first snapshot, or a fold made before it
    /// was, which the stored set must not overwrite.
    pub seeded: bool,
    /// The row the keyboard acts on.
    pub cursor: Option<String>,
}

/// The stored set; anything unparseable reads as nothing folded.
#[must_use]
pub fn parse_folds(raw: Option<&str>) -> BTreeSet<String> {
    match raw.and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok()) {
        Some(serde_json::Value::Array(items)) => items
            .into_iter()
            .filter_map(|item| item.as_str().map(str::to_string))
            .collect(),
        _ => BTreeSet::new(),
    }
}

/// Sorted, so the same folds always write the same string.
#[must_use]
pub fn folds_payload(folds: &BTreeSet<String>) -> String {
    serde_json::to_string(&folds.iter().collect::<Vec<_>>()).unwrap_or_else(|_| "[]".into())
}

#[must_use]
pub fn parse_order(raw: Option<&str>) -> BTreeMap<String, Vec<String>> {
    let Some(serde_json::Value::Object(map)) =
        raw.and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
    else {
        return BTreeMap::new();
    };
    map.into_iter()
        .filter_map(|(project, ids)| {
            let serde_json::Value::Array(ids) = ids else {
                return None;
            };
            Some((
                project,
                ids.into_iter()
                    .filter_map(|id| id.as_str().map(str::to_string))
                    .collect(),
            ))
        })
        .collect()
}

#[must_use]
pub fn order_payload(order: &BTreeMap<String, Vec<String>>) -> String {
    serde_json::to_string(order).unwrap_or_else(|_| "{}".into())
}

/// A project's checkouts: the stored order first, then anything new in the
/// default one (its own checkout, then worktrees by name).
#[must_use]
pub fn ordered<'a>(
    workspaces: &'a [Workspace],
    project: ProjectId,
    order: &BTreeMap<String, Vec<String>>,
) -> Vec<&'a Workspace> {
    let mut fallback = crate::projects::checkouts_of(workspaces, project);
    let Some(stored) = order.get(&project.to_string()).filter(|o| !o.is_empty()) else {
        return fallback;
    };
    let mut out = Vec::with_capacity(fallback.len());
    for id in stored {
        if let Some(at) = fallback.iter().position(|w| w.id.to_string() == *id) {
            out.push(fallback.remove(at));
        }
    }
    out.extend(fallback);
    out
}

/// `workspace` moved `by` places among its project's checkouts, as the new
/// stored order, or `None` at an end.
#[must_use]
pub fn moved_checkout(
    workspaces: &[Workspace],
    workspace: WorkspaceId,
    by: isize,
    order: &BTreeMap<String, Vec<String>>,
) -> Option<(ProjectId, Vec<String>)> {
    let project = workspaces.iter().find(|w| w.id == workspace)?.project_id;
    let mut ids: Vec<String> = ordered(workspaces, project, order)
        .iter()
        .map(|w| w.id.to_string())
        .collect();
    let from = ids.iter().position(|id| *id == workspace.to_string())?;
    let to = from.checked_add_signed(by).filter(|to| *to < ids.len())?;
    let id = ids.remove(from);
    ids.insert(to, id);
    Some((project, ids))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowKind {
    Group,
    Project,
    Workspace,
    Session,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RailRow {
    pub id: String,
    pub kind: RowKind,
    pub parent: Option<String>,
    pub expandable: bool,
    pub expanded: bool,
    /// What the row acts on, and its fold key.
    pub target: String,
}

/// The projects of each group in the rail's order: named groups first, then
/// the projects in none, which have no header.
#[must_use]
pub fn grouped(store: &client::Store) -> Vec<(Option<ProjectGroupId>, Vec<ProjectId>)> {
    let mut out: Vec<(Option<ProjectGroupId>, Vec<ProjectId>)> = store
        .project_groups
        .iter()
        .map(|group| (Some(group.id), Vec::new()))
        .collect();
    let mut loose = Vec::new();
    for project in &store.projects {
        match out
            .iter_mut()
            .find(|(group, _)| group.is_some() && *group == project.project_group_id)
        {
            Some((_, projects)) => projects.push(project.id),
            None => loose.push(project.id),
        }
    }
    if !loose.is_empty() {
        out.push((None, loose));
    }
    out
}

/// The rail as one flat list in painted order; a folded row's children are
/// absent, so no arrow key reaches what cannot be seen.
#[must_use]
pub fn rows(
    store: &client::Store,
    rail: &Rail,
    sessions: &[(WorkspaceId, SessionId)],
) -> Vec<RailRow> {
    let open = |id: &str| !rail.folds.contains(id);
    let mut out = Vec::new();
    for (group, projects) in grouped(store) {
        let header = group.map(|group| {
            let target = group.to_string();
            out.push(RailRow {
                id: format!("group:{target}"),
                kind: RowKind::Group,
                parent: None,
                expandable: true,
                expanded: open(&target),
                target: target.clone(),
            });
            target
        });
        if header.as_deref().is_some_and(|target| !open(target)) {
            continue;
        }
        for project in projects {
            let target = project.to_string();
            let id = format!("project:{target}");
            out.push(RailRow {
                id: id.clone(),
                kind: RowKind::Project,
                parent: header.as_ref().map(|g| format!("group:{g}")),
                expandable: true,
                expanded: open(&target),
                target: target.clone(),
            });
            if !open(&target) {
                continue;
            }
            for workspace in ordered(&store.workspaces, project, &rail.order) {
                let target = workspace.id.to_string();
                let workspace_row = format!("workspace:{target}");
                out.push(RailRow {
                    id: workspace_row.clone(),
                    kind: RowKind::Workspace,
                    parent: Some(id.clone()),
                    expandable: true,
                    expanded: open(&target),
                    target: target.clone(),
                });
                if !open(&target) {
                    continue;
                }
                for (_, session) in sessions.iter().filter(|(w, _)| *w == workspace.id) {
                    out.push(RailRow {
                        id: format!("session:{session}"),
                        kind: RowKind::Session,
                        parent: Some(workspace_row.clone()),
                        expandable: false,
                        expanded: false,
                        target: session.to_string(),
                    });
                }
            }
        }
    }
    out
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    Fold(String),
    Select(String),
}

/// `←`: fold an open row, else go to its parent.
#[must_use]
pub fn collapse_target(row: Option<&RailRow>) -> Option<Step> {
    let row = row?;
    if row.expandable && row.expanded {
        return Some(Step::Fold(row.target.clone()));
    }
    row.parent.clone().map(Step::Select)
}

/// `→`: unfold a folded row, else go to its first child.
#[must_use]
pub fn expand_target(rows: &[RailRow], index: usize) -> Option<Step> {
    let row = rows.get(index).filter(|row| row.expandable)?;
    if !row.expanded {
        return Some(Step::Fold(row.target.clone()));
    }
    rows.get(index + 1)
        .filter(|child| child.parent.as_deref() == Some(row.id.as_str()))
        .map(|child| Step::Select(child.id.clone()))
}

/// Agent sessions waiting on the person, the longest-waiting first. A
/// shell's bell is a completion beep and never asks.
#[must_use]
pub fn needs_you(sessions: &[Session]) -> Vec<&Session> {
    let mut out: Vec<&Session> = sessions
        .iter()
        .filter(|session| {
            !session.state.is_terminal()
                && session.agent_provider_id.is_some()
                && session.activity.state == domain::ActivityState::Waiting
        })
        .collect();
    out.sort_by_key(|session| session.last_activity_at);
    out
}

/// the original client's `shortDuration` of how long a session has waited.
#[must_use]
pub fn waited(session: &Session, now: &domain::Timestamp) -> String {
    let secs = (now.0 - session.last_activity_at.0).whole_seconds().max(0);
    match secs {
        0..60 => "now".to_string(),
        60..3600 => format!("{}m", secs / 60),
        3600..86_400 => format!("{}h", secs / 3600),
        _ => format!("{}d", secs / 86_400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn store() -> client::Store {
        crate::connection::fixture_store(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/snapshot.json"),
        )
        .expect("fixture snapshot decodes")
    }

    #[test]
    fn folds_round_trip_sorted_and_junk_reads_as_open() {
        let folds = parse_folds(Some(r#"["b", 3, "a"]"#));
        assert_eq!(folds_payload(&folds), r#"["a","b"]"#);
        assert!(parse_folds(Some("{")).is_empty());
        assert!(parse_folds(None).is_empty());
        let order = parse_order(Some(r#"{"p": ["x", 1, "y"], "q": "z"}"#));
        assert_eq!(
            order.get("p"),
            Some(&vec!["x".to_string(), "y".to_string()])
        );
        assert!(!order.contains_key("q"));
    }

    #[test]
    fn a_stored_order_leads_and_new_checkouts_follow_in_the_default() {
        let store = store();
        let project = store.projects[0].id;
        let default: Vec<WorkspaceId> = crate::projects::checkouts_of(&store.workspaces, project)
            .iter()
            .map(|w| w.id)
            .collect();
        assert!(default.len() >= 2, "the fixture has two checkouts");
        let (_, moved) =
            moved_checkout(&store.workspaces, default[1], -1, &BTreeMap::new()).expect("moves up");
        assert_eq!(moved[0], default[1].to_string());
        let order = BTreeMap::from([(project.to_string(), vec![default[1].to_string()])]);
        let now: Vec<WorkspaceId> = ordered(&store.workspaces, project, &order)
            .iter()
            .map(|w| w.id)
            .collect();
        assert_eq!(now, vec![default[1], default[0]]);
        assert!(moved_checkout(&store.workspaces, default[1], -1, &order).is_none());
    }

    #[test]
    fn a_folded_row_hides_its_children_from_the_keyboard() {
        let store = store();
        let project = store.projects[0].id;
        let workspace = crate::projects::checkouts_of(&store.workspaces, project)[0].id;
        let session = SessionId::new();
        let sessions = [(workspace, session)];
        let mut rail = Rail::default();
        let all = rows(&store, &rail, &sessions);
        let kinds: Vec<RowKind> = all.iter().map(|row| row.kind).collect();
        assert_eq!(kinds[0], RowKind::Group, "the fixture's project is grouped");
        assert!(all.iter().any(|row| row.id == format!("session:{session}")));
        let at = all
            .iter()
            .position(|row| row.id == format!("workspace:{workspace}"))
            .expect("the checkout row");
        assert_eq!(
            expand_target(&all, at),
            Some(Step::Select(format!("session:{session}")))
        );
        assert_eq!(
            collapse_target(all.get(at + 1)),
            Some(Step::Select(format!("workspace:{workspace}"))),
            "a session goes to its checkout"
        );
        assert_eq!(
            collapse_target(all.get(at)),
            Some(Step::Fold(workspace.to_string()))
        );
        rail.folds.insert(workspace.to_string());
        let folded = rows(&store, &rail, &sessions);
        assert!(!folded.iter().any(|row| row.kind == RowKind::Session));
        assert_eq!(
            expand_target(&folded, at),
            Some(Step::Fold(workspace.to_string()))
        );
        rail.folds.insert(all[0].target.clone());
        assert_eq!(
            rows(&store, &rail, &sessions).len(),
            1,
            "a folded group is one row"
        );
    }

    #[test]
    fn only_a_waiting_agent_needs_you_and_the_oldest_leads() {
        let store = store();
        let mut sessions = store.sessions.clone();
        for session in &mut sessions {
            session.activity.state = domain::ActivityState::Idle;
        }
        let agents: Vec<usize> = sessions
            .iter()
            .enumerate()
            .filter(|(_, s)| s.agent_provider_id.is_some() && !s.state.is_terminal())
            .map(|(at, _)| at)
            .collect();
        assert!(!agents.is_empty(), "the fixture has a live agent");
        let at = agents[0];
        sessions[at].activity.state = domain::ActivityState::Waiting;
        let shell = agents[1];
        sessions[shell].agent_provider_id = None;
        sessions[shell].activity.state = domain::ActivityState::Waiting;
        let waiting = needs_you(&sessions);
        assert_eq!(waiting.len(), 1, "a shell never asks");
        assert_eq!(waiting[0].id, sessions[at].id);
        let mut late = sessions[at].clone();
        late.last_activity_at = domain::Timestamp::now();
        assert_eq!(waited(&late, &domain::Timestamp::now()), "now");
    }
}
