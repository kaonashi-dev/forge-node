//! Project actions: validation of a saved set, detection from a workspace's
//! files, and the readiness probes of a running one. `core.rs` decides when
//! and under which lock; everything here is pure or runs off the core lock.

pub mod detect;
pub mod ports;
pub(crate) mod ready;
pub mod repo;
pub(crate) mod run;

use std::collections::{HashMap, HashSet};

use domain::{
    ActionReady, ProjectAction, ProjectActionId, ProjectId, ACTION_PORT_SPAN,
    MAX_ACTION_COMMAND_BYTES, MAX_ACTION_DEPENDENCIES, MAX_ACTION_LABEL_BYTES, MAX_PROJECT_ACTIONS,
    MAX_READY_PATTERN_BYTES, MAX_READY_TIMEOUT_SECS,
};

use crate::shares::apply as share_apply;

/// Longest dependency chain, counting the action itself.
pub const MAX_DEPENDENCY_DEPTH: usize = 4;

/// Why a set was refused, phrased for the dialog.
#[derive(Debug, PartialEq, Eq)]
pub struct InvalidActions(pub String);

/// Check a whole set and return it normalized: owned by `project_id`, labels
/// and commands trimmed, `cwd` in share-path form.
///
/// # Errors
/// [`InvalidActions`] naming the first action that breaks a limit, a duplicate
/// label, a dependency outside the set, or a cycle.
pub fn validate_set(
    project_id: ProjectId,
    actions: Vec<ProjectAction>,
) -> Result<Vec<ProjectAction>, InvalidActions> {
    if actions.len() > MAX_PROJECT_ACTIONS {
        return Err(InvalidActions(format!(
            "at most {MAX_PROJECT_ACTIONS} actions per project"
        )));
    }
    let mut labels = HashSet::new();
    let mut normalized = Vec::with_capacity(actions.len());
    for mut action in actions {
        action.project_id = project_id;
        action.label = action.label.trim().to_owned();
        action.command = action.command.trim().to_owned();
        let label = action.label.clone();
        let fail = |reason: &str| InvalidActions(format!("{label:?}: {reason}"));
        if action.label.is_empty() {
            return Err(InvalidActions("an action needs a label".to_owned()));
        }
        if action.label.len() > MAX_ACTION_LABEL_BYTES {
            return Err(fail(&format!(
                "the label is over {MAX_ACTION_LABEL_BYTES} bytes"
            )));
        }
        if !labels.insert(action.label.to_lowercase()) {
            return Err(fail("another action has this label"));
        }
        if action.command.is_empty() {
            return Err(fail("the command is empty"));
        }
        if action.command.len() > MAX_ACTION_COMMAND_BYTES {
            return Err(fail(&format!(
                "the command is over {MAX_ACTION_COMMAND_BYTES} bytes"
            )));
        }
        action.cwd = match action.cwd.as_deref().map(str::trim) {
            None | Some("" | ".") => None,
            Some(cwd) => Some(share_apply::validate(cwd).map_err(|e| fail(&e.0))?),
        };
        validate_ready(&action).map_err(|reason| fail(&reason))?;
        if let Some(port) = action.port {
            if port.base == 0 {
                return Err(fail("port 0 is not a port"));
            }
            if port.per_worktree && port.base.checked_add(ACTION_PORT_SPAN).is_none() {
                return Err(fail("a per-worktree port needs room above its base"));
            }
        }
        if action.depends_on.len() > MAX_ACTION_DEPENDENCIES {
            return Err(fail(&format!(
                "at most {MAX_ACTION_DEPENDENCIES} dependencies"
            )));
        }
        if action.depends_on.contains(&action.id) {
            return Err(fail("an action cannot depend on itself"));
        }
        normalized.push(action);
    }
    validate_graph(&normalized)?;
    Ok(normalized)
}

fn validate_ready(action: &ProjectAction) -> Result<(), String> {
    if let Some(timeout) = action.ready.timeout_secs() {
        if !(1..=MAX_READY_TIMEOUT_SECS).contains(&timeout) {
            return Err(format!(
                "the readiness timeout must be 1 to {MAX_READY_TIMEOUT_SECS} seconds"
            ));
        }
    }
    match &action.ready {
        ActionReady::Port { .. } if action.port.is_none() => {
            Err("a port probe needs a port".to_owned())
        }
        ActionReady::Output { pattern, .. } if pattern.trim().is_empty() => {
            Err("the ready pattern is empty".to_owned())
        }
        ActionReady::Output { pattern, .. } if pattern.len() > MAX_READY_PATTERN_BYTES => Err(
            format!("the ready pattern is over {MAX_READY_PATTERN_BYTES} bytes"),
        ),
        _ => Ok(()),
    }
}

fn validate_graph(actions: &[ProjectAction]) -> Result<(), InvalidActions> {
    let by_id: HashMap<ProjectActionId, &ProjectAction> =
        actions.iter().map(|action| (action.id, action)).collect();
    for action in actions {
        for dep in &action.depends_on {
            if !by_id.contains_key(dep) {
                return Err(InvalidActions(format!(
                    "{:?} depends on an action that is not in the set",
                    action.label
                )));
            }
        }
    }
    let mut depth: HashMap<ProjectActionId, usize> = HashMap::new();
    for action in actions {
        let mut visiting = HashSet::new();
        depth_of(action.id, &by_id, &mut depth, &mut visiting)?;
    }
    Ok(())
}

fn depth_of(
    id: ProjectActionId,
    by_id: &HashMap<ProjectActionId, &ProjectAction>,
    memo: &mut HashMap<ProjectActionId, usize>,
    visiting: &mut HashSet<ProjectActionId>,
) -> Result<usize, InvalidActions> {
    if let Some(&known) = memo.get(&id) {
        return Ok(known);
    }
    let action = by_id[&id];
    if !visiting.insert(id) {
        return Err(InvalidActions(format!(
            "{:?} is part of a dependency cycle",
            action.label
        )));
    }
    let mut deepest = 0;
    for dep in &action.depends_on {
        deepest = deepest.max(depth_of(*dep, by_id, memo, visiting)?);
    }
    visiting.remove(&id);
    let depth = deepest + 1;
    if depth > MAX_DEPENDENCY_DEPTH {
        return Err(InvalidActions(format!(
            "{:?} sits more than {MAX_DEPENDENCY_DEPTH} dependencies deep",
            action.label
        )));
    }
    memo.insert(id, depth);
    Ok(depth)
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::{ActionKind, ActionPort, ActionSource, Timestamp};

    fn action(label: &str) -> ProjectAction {
        ProjectAction {
            id: ProjectActionId::new(),
            project_id: ProjectId::new(),
            label: label.to_owned(),
            command: format!("make {label}"),
            kind: ActionKind::Custom,
            cwd: None,
            single_instance: true,
            pinned: false,
            ready: ActionReady::None,
            port: None,
            depends_on: Vec::new(),
            source: ActionSource::Manual,
            position: 0,
            created_at: Timestamp::now(),
        }
    }

    fn refused(actions: Vec<ProjectAction>) -> String {
        validate_set(ProjectId::new(), actions).unwrap_err().0
    }

    #[test]
    fn a_set_is_normalized_onto_its_project() {
        let project = ProjectId::new();
        let mut dev = action(" dev ");
        dev.cwd = Some("./web/".into());
        let set = validate_set(project, vec![dev]).unwrap();
        assert_eq!(set[0].project_id, project);
        assert_eq!(set[0].label, "dev");
        assert_eq!(set[0].cwd.as_deref(), Some("web"));
    }

    #[test]
    fn labels_are_unique_ignoring_case() {
        assert!(refused(vec![action("Dev"), action("dev")]).contains("another action"));
    }

    #[test]
    fn a_cwd_may_not_leave_the_checkout() {
        let mut escaping = action("dev");
        escaping.cwd = Some("../other".into());
        assert!(refused(vec![escaping]).contains(".."));
    }

    #[test]
    fn too_many_actions_are_refused() {
        let many = (0..=MAX_PROJECT_ACTIONS)
            .map(|n| action(&format!("a{n}")))
            .collect();
        assert!(refused(many).contains("at most"));
    }

    #[test]
    fn a_port_probe_needs_a_port() {
        let mut dev = action("dev");
        dev.ready = ActionReady::Port { timeout_secs: 30 };
        assert!(refused(vec![dev.clone()]).contains("needs a port"));
        dev.port = Some(ActionPort {
            base: 5173,
            per_worktree: true,
        });
        assert!(validate_set(ProjectId::new(), vec![dev]).is_ok());
    }

    #[test]
    fn dependencies_must_be_in_the_set_and_acyclic() {
        let mut a = action("a");
        let mut b = action("b");
        a.depends_on = vec![ProjectActionId::new()];
        assert!(refused(vec![a.clone(), b.clone()]).contains("not in the set"));
        a.depends_on = vec![b.id];
        b.depends_on = vec![a.id];
        assert!(refused(vec![a, b]).contains("cycle"));
    }

    #[test]
    fn a_chain_deeper_than_the_limit_is_refused() {
        let mut chain: Vec<ProjectAction> = (0..=MAX_DEPENDENCY_DEPTH)
            .map(|n| action(&format!("step{n}")))
            .collect();
        for n in 1..chain.len() {
            chain[n].depends_on = vec![chain[n - 1].id];
        }
        assert!(refused(chain.clone()).contains("deep"));
        chain.pop();
        assert!(validate_set(ProjectId::new(), chain).is_ok());
    }
}
