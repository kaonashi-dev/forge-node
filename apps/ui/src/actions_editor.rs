//! A project's actions: what is saved, what the checkout's files suggest,
//! and what `.forge/actions.toml` offers. Nothing reaches the daemon until
//! Save sends the whole set; a repository's file is only ever imported as an
//! approved copy.

use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};

use domain::{
    ActionKind, ActionPort, ActionReady, ActionSource, ActionSuggestion, ProjectAction,
    ProjectActionId, ProjectId, RepoActionsFile, WorkspaceId,
};
use rune::{El, TextEdit, col, el, row, text};
use rune_kit::{
    Button, ButtonVariant, Checkbox, ControlSize, Dialog, Kit, SegmentedControl, Switch, TextInput,
};

use crate::app::AppState;
use crate::project_ops::{Answer, Op, send};

pub const DEFAULT_READY_TIMEOUT_SECS: u64 = 60;
const DEFAULT_PORT: u16 = 3000;

pub const KINDS: [(ActionKind, &str); 5] = [
    (ActionKind::Dev, "Dev"),
    (ActionKind::Test, "Test"),
    (ActionKind::Lint, "Lint"),
    (ActionKind::Build, "Build"),
    (ActionKind::Custom, "Custom"),
];

const READY: [&str; 3] = ["Running is enough", "Its port answers", "Its output says…"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    Saved,
    Suggested,
    Repo,
}

/// One row of the dialog: the action as it would be saved, and the fields
/// being typed into it.
#[derive(Clone)]
pub struct DraftRow {
    /// The row's identity in the dialog; `action.id` is shared by a saved
    /// repository action and the update the file offers for it.
    pub key: u64,
    pub action: ProjectAction,
    pub origin: Origin,
    pub include: bool,
    pub detected_from: Option<String>,
    /// A suggestion's dependencies by label, resolved when the set is built.
    pub depends_on_labels: Vec<String>,
    /// A saved repository action whose file changed since it was imported.
    pub drift: bool,
    pub label: TextEdit,
    pub command: TextEdit,
    pub cwd: TextEdit,
    pub pattern: TextEdit,
    pub port: TextEdit,
}

impl DraftRow {
    fn new(
        action: ProjectAction,
        origin: Origin,
        include: bool,
        detected_from: Option<String>,
    ) -> Self {
        let pattern = match &action.ready {
            ActionReady::Output { pattern, .. } => pattern.clone(),
            _ => String::new(),
        };
        static NEXT_KEY: AtomicU64 = AtomicU64::new(0);
        DraftRow {
            key: NEXT_KEY.fetch_add(1, Ordering::Relaxed),
            label: TextEdit::new(action.label.clone()),
            command: TextEdit::new(action.command.clone()),
            cwd: TextEdit::new(action.cwd.clone().unwrap_or_default()),
            pattern: TextEdit::new(pattern),
            port: TextEdit::new(action.port.map(|p| p.base.to_string()).unwrap_or_default()),
            action,
            origin,
            include,
            detected_from,
            depends_on_labels: Vec::new(),
            drift: false,
        }
    }

    /// Writes what was typed into the action.
    pub fn sync(&mut self) {
        self.action.label = self.label.text.clone();
        self.action.command = self.command.text.clone();
        let cwd = self.cwd.text.trim();
        self.action.cwd = (!cwd.is_empty()).then(|| cwd.to_string());
        if let ActionReady::Output { pattern, .. } = &mut self.action.ready {
            pattern.clone_from(&self.pattern.text);
        }
        let per_worktree = self.action.port.is_none_or(|p| p.per_worktree);
        self.action.port = self
            .port
            .text
            .trim()
            .parse::<u16>()
            .ok()
            .filter(|base| *base > 0)
            .map(|base| ActionPort { base, per_worktree });
    }

    fn focused(&self) -> bool {
        [
            &self.label,
            &self.command,
            &self.cwd,
            &self.pattern,
            &self.port,
        ]
        .iter()
        .any(|edit| edit.focused)
    }
}

fn from_suggestion(
    suggestion: &ActionSuggestion,
    project: ProjectId,
    id: ProjectActionId,
) -> ProjectAction {
    ProjectAction {
        id,
        project_id: project,
        label: suggestion.label.clone(),
        command: suggestion.command.clone(),
        kind: suggestion.kind,
        cwd: suggestion.cwd.clone(),
        single_instance: suggestion.single_instance,
        pinned: suggestion.pinned,
        ready: suggestion.ready.clone(),
        port: suggestion.port,
        depends_on: Vec::new(),
        source: suggestion.source.clone(),
        position: 0,
        created_at: domain::Timestamp::now(),
    }
}

/// Saved actions first, then suggestions not saved yet, then what the
/// repository file offers. Nothing new is ticked: ticking is the consent.
/// A repository action saved from an older file is offered as an update.
pub fn build_draft(
    project: ProjectId,
    saved: &[ProjectAction],
    suggestions: &[ActionSuggestion],
    repo: Option<&RepoActionsFile>,
    mut mint: impl FnMut() -> ProjectActionId,
) -> Vec<DraftRow> {
    let drifted: HashSet<ProjectActionId> = repo
        .map(|file| file.drift.iter().copied().collect())
        .unwrap_or_default();
    let mut ordered: Vec<&ProjectAction> = saved.iter().collect();
    ordered.sort_by_key(|action| action.position);
    let mut rows: Vec<DraftRow> = ordered
        .into_iter()
        .map(|action| {
            let mut row = DraftRow::new(action.clone(), Origin::Saved, true, None);
            row.drift = drifted.contains(&action.id);
            row
        })
        .collect();
    for suggestion in suggestions.iter().filter(|s| s.saved_as.is_none()) {
        let mut row = DraftRow::new(
            from_suggestion(suggestion, project, mint()),
            Origin::Suggested,
            false,
            Some(suggestion.detected_from.clone()),
        );
        row.depends_on_labels.clone_from(&suggestion.depends_on);
        rows.push(row);
    }
    for suggestion in repo.map(|file| file.actions.as_slice()).unwrap_or_default() {
        if suggestion
            .saved_as
            .is_some_and(|replaces| !drifted.contains(&replaces))
        {
            continue;
        }
        let id = suggestion.saved_as.unwrap_or_else(&mut mint);
        let mut row = DraftRow::new(
            from_suggestion(suggestion, project, id),
            Origin::Repo,
            false,
            Some(suggestion.detected_from.clone()),
        );
        row.depends_on_labels.clone_from(&suggestion.depends_on);
        rows.push(row);
    }
    rows
}

/// The set Save sends: included rows in order, a repository update in place
/// of the saved row it shares an id with, and dependencies kept only among
/// what is saved — a suggestion's by label, an edited row's by id.
#[must_use]
pub fn to_action_set(rows: &[DraftRow]) -> Vec<ProjectAction> {
    let included: Vec<&DraftRow> = rows.iter().filter(|row| row.include).collect();
    let updates: HashSet<ProjectActionId> = included
        .iter()
        .filter(|row| row.origin == Origin::Repo)
        .map(|row| row.action.id)
        .collect();
    let kept: Vec<&DraftRow> = included
        .into_iter()
        .filter(|row| !(row.origin == Origin::Saved && updates.contains(&row.action.id)))
        .collect();
    let by_label = |label: &str| {
        kept.iter()
            .find(|row| row.action.label.to_lowercase() == label.trim().to_lowercase())
            .map(|row| row.action.id)
    };
    let kept_ids: HashSet<ProjectActionId> = kept.iter().map(|row| row.action.id).collect();
    kept.iter()
        .enumerate()
        .map(|(position, row)| {
            let depends_on = if row.depends_on_labels.is_empty() {
                row.action
                    .depends_on
                    .iter()
                    .copied()
                    .filter(|id| kept_ids.contains(id) && *id != row.action.id)
                    .collect()
            } else {
                row.depends_on_labels
                    .iter()
                    .filter_map(|label| by_label(label))
                    .filter(|id| *id != row.action.id)
                    .collect()
            };
            ProjectAction {
                depends_on,
                position: u32::try_from(position).unwrap_or(u32::MAX),
                ..row.action.clone()
            }
        })
        .collect()
}

/// Labels that collide ignoring case, which the daemon would refuse.
#[must_use]
pub fn duplicate_labels(rows: &[DraftRow]) -> Vec<String> {
    let mut seen: Vec<(String, usize)> = Vec::new();
    for row in rows.iter().filter(|row| row.include) {
        let label = row.action.label.trim().to_lowercase();
        match seen.iter_mut().find(|(l, _)| *l == label) {
            Some((_, count)) => *count += 1,
            None => seen.push((label, 1)),
        }
    }
    seen.into_iter()
        .filter(|(_, count)| *count > 1)
        .map(|(label, _)| label)
        .collect()
}

#[must_use]
pub fn matches_query(row: &DraftRow, query: &str) -> bool {
    let needle = query.trim().to_lowercase();
    needle.is_empty()
        || [
            row.action.label.as_str(),
            row.action.command.as_str(),
            row.detected_from.as_deref().unwrap_or_default(),
        ]
        .iter()
        .any(|field| field.to_lowercase().contains(&needle))
}

/// The row `by` steps from `index` among those of the same origin that
/// `shown` keeps; a move swaps the two.
#[must_use]
pub fn neighbour_index(
    rows: &[DraftRow],
    index: usize,
    by: isize,
    shown: impl Fn(&DraftRow) -> bool,
) -> Option<usize> {
    let origin = rows.get(index)?.origin;
    let mut at = index;
    loop {
        at = at.checked_add_signed(by)?;
        let row = rows.get(at)?;
        if row.origin == origin && shown(row) {
            return Some(at);
        }
    }
}

pub struct Editor {
    pub project: ProjectId,
    pub workspace: WorkspaceId,
    pub name: String,
    /// `None` until the suggestions land; drafted once, so a later broadcast
    /// never wipes what is being typed.
    pub rows: Option<Vec<DraftRow>>,
    pub repo: Option<RepoActionsFile>,
    pub query: TextEdit,
    pub advanced: HashSet<u64>,
    pub saving: bool,
    pub error: Option<String>,
}

impl Editor {
    #[must_use]
    pub fn focused(&self) -> bool {
        self.query.focused || self.rows.iter().flatten().any(DraftRow::focused)
    }

    fn synced(&self) -> Vec<DraftRow> {
        let mut rows = self.rows.clone().unwrap_or_default();
        for row in &mut rows {
            row.sync();
        }
        rows
    }
}

pub fn open(s: &mut AppState, project: ProjectId, workspace: WorkspaceId) {
    let name = s
        .store
        .as_ref()
        .and_then(|store| store.projects.iter().find(|p| p.id == project))
        .map_or_else(String::new, |p| p.name.clone());
    s.projects.actions = Some(Editor {
        project,
        workspace,
        name,
        rows: None,
        repo: None,
        query: TextEdit::new(""),
        advanced: HashSet::new(),
        saving: false,
        error: None,
    });
    send(s, Op::SuggestActions(workspace));
}

pub fn apply_suggestions(s: &mut AppState, workspace: WorkspaceId, result: Result<Answer, String>) {
    let saved: Vec<ProjectAction> = s
        .store
        .as_ref()
        .map(|store| store.project_actions.clone())
        .unwrap_or_default();
    let Some(editor) = s
        .projects
        .actions
        .as_mut()
        .filter(|e| e.workspace == workspace && e.rows.is_none())
    else {
        return;
    };
    match result {
        Ok(Answer::Suggestions(found)) => {
            let saved: Vec<ProjectAction> = saved
                .into_iter()
                .filter(|a| a.project_id == editor.project)
                .collect();
            editor.rows = Some(build_draft(
                editor.project,
                &saved,
                &found.suggestions,
                found.repo_file.as_ref(),
                ProjectActionId::new,
            ));
            editor.repo = found.repo_file;
        }
        Ok(_) => {}
        Err(error) => {
            editor.error = Some(error);
            editor.rows = Some(build_draft(
                editor.project,
                &saved
                    .into_iter()
                    .filter(|a| a.project_id == editor.project)
                    .collect::<Vec<_>>(),
                &[],
                None,
                ProjectActionId::new,
            ));
        }
    }
}

pub fn saved(s: &mut AppState, project: ProjectId, error: Option<String>) {
    let Some(editor) = s.projects.actions.as_mut().filter(|e| e.project == project) else {
        return;
    };
    editor.saving = false;
    match error {
        Some(error) => editor.error = Some(error),
        None => s.projects.actions = None,
    }
}

fn save(s: &mut AppState) {
    let Some(editor) = s.projects.actions.as_mut() else {
        return;
    };
    let rows = editor.synced();
    let set = to_action_set(&rows);
    editor.saving = true;
    editor.error = None;
    let project = editor.project;
    if !send(s, Op::SetActions(project, set))
        && let Some(editor) = s.projects.actions.as_mut()
    {
        editor.saving = false;
    }
}

fn add_row(s: &mut AppState) {
    let Some(editor) = s.projects.actions.as_mut() else {
        return;
    };
    editor.query = TextEdit::new("");
    let rows = editor.rows.get_or_insert_with(Vec::new);
    let mut row = DraftRow::new(
        ProjectAction {
            id: ProjectActionId::new(),
            project_id: editor.project,
            label: String::new(),
            command: String::new(),
            kind: ActionKind::Custom,
            cwd: None,
            single_instance: true,
            pinned: false,
            ready: ActionReady::None,
            port: None,
            depends_on: Vec::new(),
            source: ActionSource::Manual,
            position: u32::try_from(rows.len()).unwrap_or(u32::MAX),
            created_at: domain::Timestamp::now(),
        },
        Origin::Saved,
        true,
        None,
    );
    row.label.focused = true;
    rows.push(row);
}

fn with_row(s: &mut AppState, key: u64, f: impl FnOnce(&mut DraftRow)) {
    if let Some(row) = s
        .projects
        .actions
        .as_mut()
        .and_then(|e| e.rows.as_mut())
        .and_then(|rows| rows.iter_mut().find(|row| row.key == key))
    {
        f(row);
    }
}

fn move_row(s: &mut AppState, key: u64, by: isize) {
    let Some(editor) = s.projects.actions.as_mut() else {
        return;
    };
    let query = editor.query.text.clone();
    let Some(rows) = editor.rows.as_mut() else {
        return;
    };
    let Some(index) = rows.iter().position(|row| row.key == key) else {
        return;
    };
    if let Some(to) = neighbour_index(rows, index, by, |row| matches_query(row, &query)) {
        rows.swap(index, to);
    }
}

fn set_ready(row: &mut DraftRow, choice: usize) {
    let timeout_secs = row
        .action
        .ready
        .timeout_secs()
        .unwrap_or(DEFAULT_READY_TIMEOUT_SECS);
    row.action.ready = match choice {
        0 => ActionReady::None,
        1 => {
            if row.action.port.is_none() {
                row.port = TextEdit::new(DEFAULT_PORT.to_string());
                row.action.port = Some(ActionPort {
                    base: DEFAULT_PORT,
                    per_worktree: true,
                });
            }
            ActionReady::Port { timeout_secs }
        }
        _ => {
            if row.pattern.text.is_empty() {
                row.pattern = TextEdit::new("localhost:");
            }
            ActionReady::Output {
                pattern: row.pattern.text.clone(),
                timeout_secs,
            }
        }
    };
}

fn field<F>(
    k: &Kit,
    edit: &TextEdit,
    key: u64,
    name: &str,
    label: &str,
    placeholder: &str,
    pick: F,
) -> El
where
    F: Fn(&mut DraftRow) -> &mut TextEdit + 'static,
{
    TextInput::new(edit)
        .label(label.to_string())
        .placeholder(placeholder.to_string())
        .on_edit(move |s: &mut AppState| {
            match s
                .projects
                .actions
                .as_mut()
                .and_then(|e| e.rows.as_mut())
                .and_then(|rows| rows.iter_mut().find(|row| row.key == key))
            {
                Some(row) => pick(row),
                None => &mut s.scratch_edit,
            }
        })
        .build(k)
        .key(format!("action-{name}-{key}"))
}

fn action_row(
    k: &Kit,
    editor: &Editor,
    rows: &[DraftRow],
    index: usize,
    include_label: &str,
    movable: bool,
) -> El {
    let row_data = &rows[index];
    let key = row_data.key;
    let query = editor.query.text.clone();
    let shown = |row: &DraftRow| matches_query(row, &query);
    let kind = KINDS
        .iter()
        .position(|(kind, _)| *kind == row_data.action.kind)
        .unwrap_or(4);
    let kinds: Vec<&str> = KINDS.iter().map(|(_, label)| *label).collect();
    let include = row_data.include;
    let pinned = row_data.action.pinned;
    let mut main = row()
        .w_full()
        .gap(6.0)
        .items_end()
        .child(
            Checkbox::new(include)
                .label(format!(
                    "{include_label} {}",
                    if row_data.label.text.is_empty() {
                        "this action"
                    } else {
                        row_data.label.text.as_str()
                    }
                ))
                .hide_label()
                .on_toggle(move |s: &mut AppState| with_row(s, key, |row| row.include = !include))
                .build(k)
                .key(format!("action-include-{key}")),
        )
        .child(
            field(k, &row_data.label, key, "label", "Label", "label", |row| {
                &mut row.label
            })
            .w(140.0),
        )
        .child(
            field(
                k,
                &row_data.command,
                key,
                "command",
                "Command",
                "command",
                |row| &mut row.command,
            )
            .grow(1.0)
            .min_w(0.0),
        )
        .child(
            Switch::new(pinned)
                .label("Pinned")
                .control_size(ControlSize::Small)
                .on_toggle(move |s: &mut AppState| {
                    with_row(s, key, |row| row.action.pinned = !pinned);
                })
                .build(k)
                .key(format!("action-pinned-{key}")),
        );
    if movable {
        let up = neighbour_index(rows, index, -1, shown).is_some();
        let down = neighbour_index(rows, index, 1, shown).is_some();
        main = main
            .child(
                Button::new("↑")
                    .variant(ButtonVariant::GhostMuted)
                    .control_size(ControlSize::Small)
                    .disabled(!up)
                    .on_press(move |s: &mut AppState| move_row(s, key, -1))
                    .build(k)
                    .key(format!("action-up-{key}"))
                    .label("Move up"),
            )
            .child(
                Button::new("↓")
                    .variant(ButtonVariant::GhostMuted)
                    .control_size(ControlSize::Small)
                    .disabled(!down)
                    .on_press(move |s: &mut AppState| move_row(s, key, 1))
                    .build(k)
                    .key(format!("action-down-{key}"))
                    .label("Move down"),
            );
    }
    if row_data.origin == Origin::Saved {
        main = main.child(
            Button::new("Delete")
                .variant(ButtonVariant::GhostMuted)
                .control_size(ControlSize::Small)
                .on_press(move |s: &mut AppState| {
                    if let Some(rows) = s.projects.actions.as_mut().and_then(|e| e.rows.as_mut()) {
                        rows.retain(|row| row.key != key);
                    }
                })
                .build(k)
                .key(format!("action-delete-{key}")),
        );
    }
    let mut block = col()
        .key(format!("action-row-{key}"))
        .role("group")
        .label(row_data.label.text.clone())
        .w_full()
        .gap(6.0)
        .py(6.0)
        .child(main)
        .child(
            SegmentedControl::new(&kinds, Some(kind))
                .label("Kind")
                .fit()
                .on_pick(move |s: &mut AppState, at, _| {
                    with_row(s, key, |row| row.action.kind = KINDS[at.min(4)].0);
                })
                .build(k)
                .key(format!("action-kind-{key}")),
        );
    let mut meta = Vec::new();
    if let Some(from) = &row_data.detected_from {
        meta.push(from.clone());
    }
    if row_data.drift {
        meta.push("the repository file has changed since this import".into());
    }
    if !meta.is_empty() {
        block = block.child(text(meta.join(" · ")).font(k.ty.text_xsmall).color(
            if row_data.drift {
                k.c.warning.fg
            } else {
                k.c.text_faint
            },
        ));
    }
    let open = editor.advanced.contains(&key);
    block = block.child(
        Button::new(if open { "Advanced ▾" } else { "Advanced ▸" })
            .variant(ButtonVariant::GhostMuted)
            .control_size(ControlSize::Small)
            .on_press(move |s: &mut AppState| {
                if let Some(editor) = s.projects.actions.as_mut()
                    && !editor.advanced.remove(&key)
                {
                    editor.advanced.insert(key);
                }
            })
            .build(k)
            .key(format!("action-advanced-{key}")),
    );
    if open {
        block = block.child(advanced(k, rows, index));
    }
    block
}

fn advanced(k: &Kit, rows: &[DraftRow], index: usize) -> El {
    let row_data = &rows[index];
    let id = row_data.action.id;
    let key = row_data.key;
    let single = row_data.action.single_instance;
    let ready = match row_data.action.ready {
        ActionReady::Port { .. } => 1,
        ActionReady::Output { .. } => 2,
        _ => 0,
    };
    let mut body = col()
        .key(format!("action-advanced-body-{key}"))
        .gap(8.0)
        .pl(24.0)
        .child(field(
            k,
            &row_data.cwd,
            key,
            "cwd",
            "Directory · relative to the checkout; empty is its root",
            "",
            |row| &mut row.cwd,
        ))
        .child(
            Switch::new(single)
                .label("One at a time")
                .description("A second click shows the running one instead of starting another.")
                .on_toggle(move |s: &mut AppState| {
                    with_row(s, key, |row| row.action.single_instance = !single);
                })
                .build(k)
                .key(format!("action-single-{key}")),
        )
        .child(
            SegmentedControl::new(&READY, Some(ready))
                .label("Ready when")
                .fit()
                .on_pick(move |s: &mut AppState, at, _| with_row(s, key, |row| set_ready(row, at)))
                .build(k)
                .key(format!("action-ready-{key}")),
        );
    if ready == 2 {
        body = body.child(field(
            k,
            &row_data.pattern,
            key,
            "pattern",
            "Output contains · plain text, any case, not a regular expression",
            "localhost:",
            |row| &mut row.pattern,
        ));
    }
    body = body.child(field(
        k,
        &row_data.port,
        key,
        "port",
        "Port · injected as PORT and substituted for {port} in the command",
        "",
        |row| &mut row.port,
    ));
    if let Some(port) = row_data.action.port {
        let per = port.per_worktree;
        body = body.child(
            Switch::new(per)
                .label("A port per worktree")
                .description(
                    "Take the next free port above this one, so each checkout can serve at once.",
                )
                .on_toggle(move |s: &mut AppState| {
                    with_row(s, key, |row| {
                        if let Some(port) = row.action.port.as_mut() {
                            port.per_worktree = !per;
                        }
                    });
                })
                .build(k)
                .key(format!("action-per-worktree-{key}")),
        );
    }
    let labels: Vec<String> = row_data
        .depends_on_labels
        .iter()
        .map(|l| l.to_lowercase())
        .collect();
    let others: Vec<&DraftRow> = rows
        .iter()
        .filter(|row| row.include && row.action.id != id && !row.label.text.trim().is_empty())
        .collect();
    if !others.is_empty() {
        let depends: Vec<ProjectActionId> = if labels.is_empty() {
            row_data.action.depends_on.clone()
        } else {
            others
                .iter()
                .filter(|o| labels.contains(&o.label.text.to_lowercase()))
                .map(|o| o.action.id)
                .collect()
        };
        let mut deps = col()
            .gap(4.0)
            .child(text("Starts after").font(k.ty.small).color(k.c.text_muted));
        for other in others {
            let other_id = other.action.id;
            let on = depends.contains(&other_id);
            let current = depends.clone();
            deps = deps.child(
                Checkbox::new(on)
                    .label(other.label.text.clone())
                    .on_toggle(move |s: &mut AppState| {
                        let mut next = current.clone();
                        if on {
                            next.retain(|dep| *dep != other_id);
                        } else {
                            next.push(other_id);
                        }
                        with_row(s, key, |row| {
                            row.action.depends_on = next;
                            row.depends_on_labels.clear();
                        });
                    })
                    .build(k)
                    .key(format!("action-dep-{key}-{other_id}")),
            );
        }
        body = body.child(deps);
    }
    body
}

pub fn dialog(s: &AppState, k: &Kit, layer: &mut Vec<El>) {
    let Some(editor) = &s.projects.actions else {
        return;
    };
    let synced = editor.synced();
    let duplicates = duplicate_labels(&synced);
    let incomplete = synced.iter().any(|row| {
        row.include && (row.action.label.trim().is_empty() || row.action.command.trim().is_empty())
    });
    let mut body = col().key("actions-dialog-body").w_full().gap(6.0);
    if let Some(error) = &editor.error {
        body = body.child(
            text(error.clone())
                .key("actions-error")
                .role("alert")
                .font(k.ty.small)
                .color(k.c.danger.fg)
                .wrap(),
        );
    }
    if let Some(first) = duplicates.first() {
        body = body.child(
            text(format!(
                "Two actions are called “{first}”. Labels must differ, ignoring case."
            ))
            .role("alert")
            .font(k.ty.small)
            .color(k.c.danger.fg)
            .wrap(),
        );
    }
    match &editor.rows {
        None => {
            body = body.child(
                text("Reading the project’s manifests…")
                    .font(k.ty.small)
                    .color(k.c.text_muted),
            );
        }
        Some(rows) => {
            if !rows.is_empty() {
                body = body.child(
                    TextInput::new(&editor.query)
                        .placeholder("Filter by label, command or file")
                        .label("Filter actions")
                        .on_edit(|s: &mut AppState| match s.projects.actions.as_mut() {
                            Some(editor) => &mut editor.query,
                            None => &mut s.scratch_edit,
                        })
                        .build(k)
                        .key("actions-filter"),
                );
            }
            let query = editor.query.text.clone();
            let shown = |row: &DraftRow| matches_query(row, &query);
            let section = |mut body: El, origin: Origin, heading: &str| -> El {
                let indices: Vec<usize> = (0..rows.len())
                    .filter(|at| rows[*at].origin == origin && shown(&rows[*at]))
                    .collect();
                if indices.is_empty() {
                    return body;
                }
                body = body.child(
                    text(heading.to_string())
                        .role("heading")
                        .font(k.ty.small_medium)
                        .color(k.c.text_muted)
                        .pt(8.0),
                );
                for at in indices {
                    let include = match origin {
                        Origin::Saved => "Keep",
                        Origin::Suggested => "Add",
                        Origin::Repo
                            if rows.iter().any(|r| {
                                r.origin == Origin::Saved && r.action.id == rows[at].action.id
                            }) =>
                        {
                            "Update"
                        }
                        Origin::Repo => "Import",
                    };
                    body = body.child(action_row(
                        k,
                        editor,
                        rows,
                        at,
                        include,
                        origin == Origin::Saved,
                    ));
                }
                body
            };
            body = section(body, Origin::Saved, "Saved");
            body = section(body, Origin::Suggested, "Suggested");
            if let Some(file) = &editor.repo {
                body = body.child(
                    text("From .forge/actions.toml")
                        .role("heading")
                        .font(k.ty.small_medium)
                        .color(k.c.text_muted)
                        .pt(8.0),
                );
                match &file.error {
                    Some(error) => {
                        body =
                            body.child(text(error.clone()).font(k.ty.small).color(k.c.danger.fg));
                    }
                    None => {
                        body = body.child(
                            text(
                                "The repository offers these. Importing copies them into this \
                                 project: what runs is the copy you approve here, never the file \
                                 as it changes.",
                            )
                            .font(k.ty.text_xsmall)
                            .color(k.c.text_faint)
                            .wrap(),
                        );
                        if !file.drift.is_empty() {
                            body = body.child(
                                text(format!(
                                    "The file changed since {} imported. Saved copies keep \
                                     running as approved; tick an update below to take it.",
                                    if file.drift.len() == 1 {
                                        "an action was"
                                    } else {
                                        "actions were"
                                    }
                                ))
                                .font(k.ty.text_xsmall)
                                .color(k.c.warning.fg)
                                .wrap(),
                            );
                        }
                        body = section(body, Origin::Repo, "");
                    }
                }
            }
            if !rows.is_empty() && !rows.iter().any(shown) {
                body = body.child(
                    text(format!("No action matches “{}”.", query.trim()))
                        .font(k.ty.small)
                        .color(k.c.text_muted),
                );
            }
            if rows.is_empty() {
                body = body.child(
                    text(
                        "Nothing to suggest at this checkout’s root. Add an action for any \
                         command you run often.",
                    )
                    .font(k.ty.small)
                    .color(k.c.text_muted)
                    .wrap(),
                );
            }
        }
    }
    let sources: Vec<String> = {
        let mut seen: Vec<String> = Vec::new();
        for from in editor
            .rows
            .iter()
            .flatten()
            .filter_map(|row| row.detected_from.clone())
        {
            if !seen.contains(&from) {
                seen.push(from);
            }
        }
        seen
    };
    let mut dialog = Dialog::new("actions-dialog", format!("Actions · {}", editor.name))
        .open(true)
        .width(760.0)
        .on_close(|s: &mut AppState| s.projects.actions = None);
    if !sources.is_empty() {
        dialog = dialog.description(format!("Detected from {}", sources.join(" · ")));
    }
    dialog
        .child(
            col()
                .key("actions-dialog-scroll")
                .w_full()
                .max_h(520.0)
                .scroll(rune::ScrollSpec {
                    stick_bottom: false,
                })
                .child(body),
        )
        .child(
            row()
                .pt(12.0)
                .gap(8.0)
                .child(
                    Button::new("Add action")
                        .variant(ButtonVariant::Outline)
                        .on_press(add_row)
                        .build(k)
                        .key("actions-add"),
                )
                .child(el().grow(1.0))
                .child(
                    Button::new("Cancel")
                        .variant(ButtonVariant::Ghost)
                        .on_press(|s: &mut AppState| s.projects.actions = None)
                        .build(k)
                        .key("actions-cancel"),
                )
                .child(
                    Button::new(if editor.saving { "Saving…" } else { "Save" })
                        .variant(ButtonVariant::Contrast)
                        .disabled(
                            editor.rows.is_none()
                                || editor.saving
                                || !duplicates.is_empty()
                                || incomplete,
                        )
                        .on_press(save)
                        .build(k)
                        .key("actions-save"),
                ),
        )
        .build(k, layer);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn saved(project: ProjectId, label: &str, position: u32) -> ProjectAction {
        ProjectAction {
            id: ProjectActionId::new(),
            project_id: project,
            label: label.into(),
            command: format!("run {label}"),
            kind: ActionKind::Custom,
            cwd: None,
            single_instance: true,
            pinned: false,
            ready: ActionReady::None,
            port: None,
            depends_on: Vec::new(),
            source: ActionSource::Manual,
            position,
            created_at: domain::Timestamp::now(),
        }
    }

    fn suggestion(label: &str, saved_as: Option<ProjectActionId>) -> ActionSuggestion {
        ActionSuggestion {
            label: label.into(),
            command: format!("npm run {label}"),
            kind: ActionKind::Dev,
            pinned: true,
            cwd: None,
            single_instance: true,
            ready: ActionReady::None,
            port: None,
            depends_on: Vec::new(),
            source: ActionSource::Detected,
            detected_from: "package.json".into(),
            saved_as,
        }
    }

    #[test]
    fn the_draft_lists_saved_then_new_suggestions_then_the_repo_file() {
        let project = ProjectId::new();
        let lint = saved(project, "lint", 1);
        let dev = saved(project, "dev", 0);
        let drifted = saved(project, "serve", 2);
        let repo = RepoActionsFile {
            file_hash: "h".into(),
            actions: vec![
                suggestion("serve", Some(drifted.id)),
                suggestion("dev", Some(dev.id)),
                suggestion("seed", None),
            ],
            error: None,
            drift: vec![drifted.id],
        };
        let rows = build_draft(
            project,
            &[lint.clone(), dev.clone(), drifted.clone()],
            &[suggestion("dev", Some(dev.id)), suggestion("test", None)],
            Some(&repo),
            ProjectActionId::new,
        );
        let shown: Vec<(&str, Origin, bool)> = rows
            .iter()
            .map(|r| (r.action.label.as_str(), r.origin, r.include))
            .collect();
        assert_eq!(
            shown,
            [
                ("dev", Origin::Saved, true),
                ("lint", Origin::Saved, true),
                ("serve", Origin::Saved, true),
                ("test", Origin::Suggested, false),
                ("serve", Origin::Repo, false),
                ("seed", Origin::Repo, false),
            ],
            "a saved suggestion and an undrifted import are not offered again"
        );
        assert!(rows[2].drift);
        assert_eq!(
            rows[4].action.id, drifted.id,
            "an update keeps the saved id"
        );

        let mut rows = rows;
        rows[4].include = true;
        rows[3].include = true;
        let set = to_action_set(&rows);
        let labels: Vec<(&str, u32)> = set.iter().map(|a| (a.label.as_str(), a.position)).collect();
        assert_eq!(
            labels,
            [("dev", 0), ("lint", 1), ("test", 2), ("serve", 3)],
            "the update replaces its saved row"
        );
        assert!(matches!(set[3].source, ActionSource::Detected));
    }

    #[test]
    fn an_offered_update_is_edited_apart_from_the_saved_row_it_replaces() {
        let store = crate::connection::fixture_store(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/snapshot.json"),
        )
        .expect("fixture snapshot decodes");
        let mut s = AppState::for_bench(
            store,
            std::sync::Arc::new(crate::connection::Metrics::default()),
            crate::terminal::SharedTerminal::new(),
        );
        let project = ProjectId::new();
        let serve = saved(project, "serve", 0);
        let repo = RepoActionsFile {
            file_hash: "h".into(),
            actions: vec![suggestion("serve", Some(serve.id))],
            error: None,
            drift: vec![serve.id],
        };
        let rows = build_draft(project, &[serve], &[], Some(&repo), ProjectActionId::new);
        let (saved_key, update_key) = (rows[0].key, rows[1].key);
        assert_ne!(saved_key, update_key);
        s.projects.actions = Some(Editor {
            project,
            workspace: WorkspaceId::new(),
            name: String::new(),
            rows: Some(rows),
            repo: None,
            query: TextEdit::new(""),
            advanced: HashSet::new(),
            saving: false,
            error: None,
        });
        with_row(&mut s, update_key, |row| {
            row.include = true;
            row.command = TextEdit::new("npm run serve -- --open");
        });
        let rows = s.projects.actions.as_ref().and_then(|e| e.rows.as_ref());
        let rows = rows.expect("drafted rows");
        assert_eq!(rows[0].command.text, "run serve");
        assert!(rows[1].include);
        assert_eq!(rows[1].command.text, "npm run serve -- --open");
    }

    #[test]
    fn dependencies_resolve_by_label_and_drop_what_is_not_saved() {
        let project = ProjectId::new();
        let db = saved(project, "db", 0);
        let mut api = suggestion("api", None);
        api.depends_on = vec!["DB".into(), "cache".into()];
        let mut rows = build_draft(
            project,
            std::slice::from_ref(&db),
            &[api],
            None,
            ProjectActionId::new,
        );
        rows[1].include = true;
        let set = to_action_set(&rows);
        assert_eq!(set[1].depends_on, vec![db.id], "cache is not saved");
        rows[0].include = false;
        assert!(to_action_set(&rows)[0].depends_on.is_empty());
    }

    #[test]
    fn labels_collide_ignoring_case_and_moves_stay_in_a_section() {
        let project = ProjectId::new();
        let mut rows = build_draft(
            project,
            &[saved(project, "Dev", 0), saved(project, "lint", 1)],
            &[suggestion("dev", None)],
            None,
            ProjectActionId::new,
        );
        assert!(
            duplicate_labels(&rows).is_empty(),
            "an unticked row does not count"
        );
        rows[2].include = true;
        assert_eq!(duplicate_labels(&rows), ["dev"]);
        assert_eq!(neighbour_index(&rows, 0, 1, |_| true), Some(1));
        assert_eq!(
            neighbour_index(&rows, 1, 1, |_| true),
            None,
            "a suggestion is another section"
        );
        assert_eq!(
            neighbour_index(&rows, 1, -1, |r| r.action.label != "Dev"),
            None,
            "a hidden row is not a place to move to"
        );
        assert!(matches_query(&rows[2], "PACKAGE"));
        assert!(!matches_query(&rows[0], "xyz"));
    }

    #[test]
    fn typing_reaches_the_action_and_a_ready_choice_fills_its_defaults() {
        let project = ProjectId::new();
        let mut rows = build_draft(
            project,
            &[saved(project, "dev", 0)],
            &[],
            None,
            ProjectActionId::new,
        );
        let row = &mut rows[0];
        row.label = TextEdit::new("web");
        row.cwd = TextEdit::new("  ");
        set_ready(row, 1);
        assert_eq!(
            row.action.ready,
            ActionReady::Port {
                timeout_secs: DEFAULT_READY_TIMEOUT_SECS
            }
        );
        row.port = TextEdit::new("5173");
        row.sync();
        assert_eq!(row.action.label, "web");
        assert_eq!(row.action.cwd, None);
        assert_eq!(
            row.action.port,
            Some(ActionPort {
                base: 5173,
                per_worktree: true
            })
        );
        set_ready(row, 2);
        row.sync();
        assert_eq!(
            row.action.ready,
            ActionReady::Output {
                pattern: "localhost:".into(),
                timeout_secs: DEFAULT_READY_TIMEOUT_SECS
            }
        );
    }
}
