//! Shared files: a project's share rules, what the
//! scan proposes, and their state in each checkout. Shown as Settings ›
//! Shared files and as a dialog from a project or checkout menu. Every edit
//! of the set is one `SetProjectShares`; only removal has its own request.

use std::collections::HashMap;

use domain::{
    ProjectId, ShareCandidate, ShareClass, ShareCleanup, ShareRule, ShareRuleId, ShareState,
    ShareStatusEntry, ShareStrategy, ShareVerb, WorkspaceId,
};
use rune::{El, FileDialogOptions, TextEdit, col, el, row, text};
use rune_kit::{
    Button, ButtonVariant, ControlSize, Dialog, Kit, RadioGroup, RadioItem, SegmentedControl,
    Switch, TextInput,
};

use crate::app::AppState;
use crate::project_ops::{Op, request, send};

pub const RUN_TIMEOUT_SECS: u64 = 600;

pub const STRATEGIES: [(&str, &str); 4] = [
    (
        "Link",
        "One file, every worktree — an edit is visible everywhere. Some editors replace a link \
         when they save; Forge Node notices and says so.",
    ),
    (
        "Copy",
        "An independent copy per worktree. Nothing a worktree does can reach the others.",
    ),
    (
        "Clone",
        "A copy-on-write clone: instant and free for a big directory, and independent \
         afterwards. Falls back to a full copy where the filesystem cannot.",
    ),
    (
        "Run a command",
        "Produce it instead of sharing it — the honest answer for dependencies, because two \
         branches can disagree on a lockfile.",
    ),
];

#[derive(Default)]
pub struct Form {
    /// The dialog's project, and the checkout it was opened from.
    pub target: Option<(ProjectId, Option<WorkspaceId>)>,
    pub draft: TextEdit,
    pub path_error: Option<String>,
    pub removing: Option<(ProjectId, ShareRuleId, ShareCleanup)>,
    /// A run rule's command while it is typed, by rule.
    pub commands: HashMap<ShareRuleId, TextEdit>,
    /// The project the last scan was for, so a page scans once per project.
    pub scanned: Option<ProjectId>,
}

impl Form {
    #[must_use]
    pub fn focused(&self) -> bool {
        self.draft.focused || self.commands.values().any(|edit| edit.focused)
    }
}

#[must_use]
pub fn strategy_index(strategy: &ShareStrategy) -> usize {
    match strategy {
        ShareStrategy::Link => 0,
        ShareStrategy::Copy => 1,
        ShareStrategy::Clone => 2,
        _ => 3,
    }
}

#[must_use]
pub fn strategy_at(index: usize, previous: &ShareStrategy) -> ShareStrategy {
    match index {
        0 => ShareStrategy::Link,
        1 => ShareStrategy::Copy,
        2 => ShareStrategy::Clone,
        _ => match previous {
            ShareStrategy::Run { .. } => previous.clone(),
            _ => ShareStrategy::Run {
                command: String::new(),
                timeout_secs: RUN_TIMEOUT_SECS,
            },
        },
    }
}

fn strategy_label(strategy: &ShareStrategy) -> String {
    match strategy {
        ShareStrategy::Run { command, .. } => format!("run: {command}"),
        other => STRATEGIES[strategy_index(other)].0.to_lowercase(),
    }
}

fn class_label(class: ShareClass) -> &'static str {
    match class {
        ShareClass::Secret => "secret",
        ShareClass::Dependencies => "dependencies",
        ShareClass::BuildOutput => "build output",
        ShareClass::Cache => "cache",
        ShareClass::EditorState => "editor state",
        _ => "other",
    }
}

/// Bytes as a person reads them.
#[must_use]
pub fn size(bytes: Option<u64>) -> String {
    let Some(bytes) = bytes else {
        return "—".to_string();
    };
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let units = ["KB", "MB", "GB", "TB"];
    let mut value = bytes as f64 / 1024.0;
    let mut unit = 0;
    while value >= 1024.0 && unit < units.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if value < 10.0 {
        format!("{value:.1} {}", units[unit])
    } else {
        format!("{} {}", value.round(), units[unit])
    }
}

#[must_use]
pub fn state_label(state: &ShareState) -> String {
    match state {
        ShareState::Applied => "applied".into(),
        ShareState::Missing => "missing".into(),
        ShareState::Severed => "link replaced by a real file".into(),
        ShareState::Diverged => "changed since it was written".into(),
        ShareState::Skipped { reason } => reason.clone(),
        ShareState::Failed { message } => message.clone(),
        _ => "unknown".into(),
    }
}

fn bad(state: &ShareState) -> bool {
    matches!(state, ShareState::Severed | ShareState::Failed { .. })
}

/// How a rule stands across the project's checkouts: how many have it
/// applied, and the worst state of the rest.
#[must_use]
pub fn summary(
    rule: ShareRuleId,
    checkouts: &[WorkspaceId],
    status: &HashMap<WorkspaceId, Vec<ShareStatusEntry>>,
) -> (usize, usize, Option<ShareState>) {
    let mut applied = 0;
    let mut worst: Option<ShareState> = None;
    for checkout in checkouts {
        let Some(state) = status
            .get(checkout)
            .and_then(|entries| entries.iter().find(|e| e.rule_id == rule))
            .map(|e| &e.state)
        else {
            continue;
        };
        if *state == ShareState::Applied {
            applied += 1;
        } else if worst.is_none() || bad(state) {
            worst = Some(state.clone());
        }
    }
    (applied, checkouts.len(), worst)
}

/// A path typed or picked for a new rule, checked before it is added.
pub fn clean_path(raw: &str, rules: &[ShareRule]) -> Result<String, &'static str> {
    let clean = raw.trim().trim_start_matches('/').trim_end_matches('/');
    if clean.is_empty() {
        return Err("Type a path first.");
    }
    if clean.starts_with("..") || clean.contains("/../") {
        return Err("The path must stay inside the repository.");
    }
    if rules.iter().any(|rule| rule.path == clean) {
        return Err("That path already has a rule.");
    }
    Ok(clean.to_string())
}

/// The rule set with every position matching its place in the list.
#[must_use]
pub fn renumbered(mut rules: Vec<ShareRule>) -> Vec<ShareRule> {
    for (position, rule) in rules.iter_mut().enumerate() {
        rule.position = u32::try_from(position).unwrap_or(u32::MAX);
    }
    rules
}

/// `rules` with `id` moved `by` places, or `None` at an end.
#[must_use]
pub fn moved(rules: &[ShareRule], id: ShareRuleId, by: isize) -> Option<Vec<ShareRule>> {
    let from = rules.iter().position(|rule| rule.id == id)?;
    let to = from.checked_add_signed(by).filter(|to| *to < rules.len())?;
    let mut next = rules.to_vec();
    let rule = next.remove(from);
    next.insert(to, rule);
    Some(renumbered(next))
}

#[must_use]
pub fn rules_of(rules: &[ShareRule], project: ProjectId) -> Vec<ShareRule> {
    let mut out: Vec<ShareRule> = rules
        .iter()
        .filter(|rule| rule.project_id == project)
        .cloned()
        .collect();
    out.sort_by_key(|rule| rule.position);
    out
}

fn project_rules(s: &AppState, project: ProjectId) -> Vec<ShareRule> {
    s.store
        .as_ref()
        .map(|store| rules_of(&store.worktree_shares, project))
        .unwrap_or_default()
}

fn checkouts(s: &AppState, project: ProjectId) -> Vec<WorkspaceId> {
    s.store
        .as_ref()
        .map(|store| {
            crate::projects::checkouts_of(&store.workspaces, project)
                .iter()
                .map(|w| w.id)
                .collect()
        })
        .unwrap_or_default()
}

fn save(s: &mut AppState, project: ProjectId, rules: Vec<ShareRule>) {
    send(s, Op::SetShares(project, renumbered(rules)));
}

/// Scans the project once per page and reads each checkout's state.
pub fn scan(s: &mut AppState, project: ProjectId) {
    if s.projects.share_form.scanned == Some(project) {
        return;
    }
    s.projects.share_form.scanned = Some(project);
    request(s, Op::DetectShares(project));
    crate::project_ops::refresh_status(s, project);
}

/// The project a settings page is about: the window's checkout's.
#[must_use]
pub fn current_project(s: &AppState) -> Option<ProjectId> {
    let workspace = s.workbench.workspace?;
    s.store
        .as_ref()?
        .workspaces
        .iter()
        .find(|w| w.id == workspace)
        .map(|w| w.project_id)
}

pub fn open_dialog(s: &mut AppState, project: ProjectId, workspace: Option<WorkspaceId>) {
    s.projects.share_form.target = Some((project, workspace));
    s.projects.share_form.scanned = None;
    s.projects.share_form.draft = TextEdit::new("");
    s.projects.share_form.path_error = None;
    scan(s, project);
}

pub fn add(s: &mut AppState, project: ProjectId, path: &str, strategy: ShareStrategy) {
    let mut rules = project_rules(s, project);
    match clean_path(path, &rules) {
        Err(problem) => s.projects.share_form.path_error = Some(problem.to_string()),
        Ok(clean) => {
            s.projects.share_form.path_error = None;
            s.projects.share_form.draft = TextEdit::new("");
            rules.push(ShareRule {
                id: ShareRuleId::new(),
                project_id: project,
                path: clean,
                strategy,
                enabled: true,
                position: 0,
                created_at: domain::Timestamp::now(),
            });
            save(s, project, rules);
        }
    }
}

/// A new strategy for a rule. Linking a real file has to move it into the
/// store, the one step that changes the primary checkout, so it is its own
/// request.
pub fn retarget(s: &mut AppState, project: ProjectId, id: ShareRuleId, index: usize) {
    let mut rules = project_rules(s, project);
    let mut link = None;
    for rule in &mut rules {
        if rule.id == id {
            let next = strategy_at(index, &rule.strategy);
            if next == rule.strategy {
                return;
            }
            if next == ShareStrategy::Link {
                link = Some(rule.path.clone());
            }
            rule.strategy = next;
        }
    }
    save(s, project, rules);
    if let Some(path) = link {
        send(s, Op::AdoptIntoStore(project, path));
    }
}

pub fn set_enabled(s: &mut AppState, project: ProjectId, id: ShareRuleId, enabled: bool) {
    let mut rules = project_rules(s, project);
    for rule in rules.iter_mut().filter(|rule| rule.id == id) {
        rule.enabled = enabled;
    }
    save(s, project, rules);
}

pub fn set_command(s: &mut AppState, project: ProjectId, id: ShareRuleId) {
    let Some(command) = s
        .projects
        .share_form
        .commands
        .get(&id)
        .map(|edit| edit.text.trim().to_string())
    else {
        return;
    };
    let mut rules = project_rules(s, project);
    for rule in rules.iter_mut().filter(|rule| rule.id == id) {
        let timeout_secs = match &rule.strategy {
            ShareStrategy::Run { timeout_secs, .. } => *timeout_secs,
            _ => RUN_TIMEOUT_SECS,
        };
        rule.strategy = ShareStrategy::Run {
            command: command.clone(),
            timeout_secs,
        };
    }
    s.projects.share_form.commands.remove(&id);
    save(s, project, rules);
}

pub fn move_rule(s: &mut AppState, project: ProjectId, id: ShareRuleId, by: isize) {
    if let Some(next) = moved(&project_rules(s, project), id, by) {
        send(s, Op::SetShares(project, next));
    }
}

pub fn reapply_everywhere(s: &mut AppState, project: ProjectId, id: ShareRuleId) {
    for checkout in checkouts(s, project) {
        request(s, Op::ApplyShares(checkout, Some(vec![id])));
    }
}

pub fn apply_all(s: &mut AppState, project: ProjectId) {
    for checkout in checkouts(s, project) {
        request(s, Op::ApplyShares(checkout, None));
    }
}

pub fn confirm_removal(s: &mut AppState) {
    if let Some((project, rule, cleanup)) = s.projects.share_form.removing.take() {
        send(s, Op::RemoveShare(project, rule, cleanup));
    }
}

fn browse(s: &mut AppState, tx: &mut rune::Tx, project: ProjectId) {
    let root = s
        .store
        .as_ref()
        .and_then(|store| store.projects.iter().find(|p| p.id == project))
        .map(|p| p.root_path.clone());
    tx.open_file(
        FileDialogOptions {
            title: "Share a file between worktrees".into(),
            directory: root.clone(),
            ..FileDialogOptions::default()
        },
        move |s: &mut AppState, answer, _tx| {
            let Ok(Some(picked)) = answer else {
                return;
            };
            let form = &mut s.projects.share_form;
            match root
                .as_ref()
                .and_then(|root| picked.strip_prefix(root).ok())
            {
                Some(relative) if !relative.as_os_str().is_empty() => {
                    form.draft = TextEdit::new(relative.display().to_string());
                    form.path_error = None;
                }
                _ => {
                    form.path_error = Some(
                        "Pick a file inside the repository — a path outside it cannot be shared."
                            .into(),
                    );
                }
            }
        },
    );
}

fn small(
    k: &Kit,
    label: impl Into<String>,
    key: String,
    press: impl Fn(&mut AppState) + 'static,
) -> El {
    Button::new(label.into())
        .variant(ButtonVariant::Ghost)
        .control_size(ControlSize::Small)
        .on_press(press)
        .build(k)
        .key(key)
}

fn caption(k: &Kit, words: impl Into<String>) -> El {
    text(words.into())
        .font(k.ty.text_xsmall)
        .color(k.c.text_faint)
        .wrap()
}

fn verb(verb: ShareVerb) -> &'static str {
    match verb {
        ShareVerb::Copy => "copy",
        ShareVerb::Clone => "clone",
        ShareVerb::Link => "link",
        ShareVerb::Run => "run",
        ShareVerb::Skip => "skip",
        ShareVerb::Backup => "back up",
        ShareVerb::Remove => "remove",
        _ => "other",
    }
}

/// The page for `project`; `workspace` adds the per-checkout group a menu
/// opened it for.
pub fn page(
    s: &AppState,
    k: &Kit,
    project: Option<ProjectId>,
    workspace: Option<WorkspaceId>,
) -> El {
    let Some(project) = project else {
        return col()
            .key("shared-files")
            .child(caption(k, "Open a checkout to reach its shared files."));
    };
    let reads = &s.projects.share_form;
    let shares = &s.projects.shares;
    let name = s
        .store
        .as_ref()
        .and_then(|store| store.projects.iter().find(|p| p.id == project))
        .map_or_else(|| "this project".to_string(), |p| p.name.clone());
    let rules = project_rules(s, project);
    let checkouts = checkouts(s, project);
    let mut page = col().key("shared-files").w_full().gap(20.0);

    if let Some(target) = workspace {
        let applying = shares.applying.contains(&target);
        let mut here = crate::settings::group(
            k,
            "shares-here",
            "This worktree",
            Some(
                "Apply project rules here. Copies can be edited independently; linked files share \
                 edits across checkouts.",
            ),
        )
        .child(
            Button::new("Apply missing files here")
                .variant(ButtonVariant::Outline)
                .control_size(ControlSize::Small)
                .disabled(applying)
                .on_press(move |s: &mut AppState| request(s, Op::ApplyShares(target, None)))
                .build(k)
                .key("shares-apply-here"),
        );
        if rules.is_empty() {
            here = here.child(caption(k, "No project rules yet."));
        }
        for rule in &rules {
            let state = shares
                .status
                .get(&target)
                .and_then(|entries| entries.iter().find(|e| e.rule_id == rule.id))
                .map_or_else(|| "Not checked".to_string(), |e| state_label(&e.state));
            let id = rule.id;
            here = here.child(crate::settings::setting_row(
                k,
                &rule.path,
                Some(&state),
                Button::new("Re-apply here")
                    .variant(ButtonVariant::Outline)
                    .control_size(ControlSize::Small)
                    .disabled(!rule.enabled || applying)
                    .on_press(move |s: &mut AppState| {
                        request(s, Op::ApplyShares(target, Some(vec![id])));
                    })
                    .build(k)
                    .key(format!("shares-reapply-{id}")),
            ));
        }
        page = page.child(here.child(caption(
            k,
            "Re-applying backs up and replaces existing files. Editing or removing project rules \
             below affects all worktrees.",
        )));
    }

    if let Some(error) = &shares.error {
        page = page.child(
            text(error.clone())
                .key("shares-error")
                .role("alert")
                .font(k.ty.small)
                .color(k.c.warning.fg)
                .wrap(),
        );
    }

    let description = if rules.is_empty() {
        format!(
            "{name} has no rules of its own, so the global [worktrees] list in config.toml is \
             what runs."
        )
    } else {
        format!("They apply to every checkout of {name}, the main one included.")
    };
    let mut group = crate::settings::group(
        k,
        "shares-rules",
        &format!("Project rules · {name}"),
        Some(&description),
    )
    .child(
        Button::new("Apply to all checkouts")
            .variant(ButtonVariant::Outline)
            .control_size(ControlSize::Small)
            .on_press(move |s: &mut AppState| apply_all(s, project))
            .build(k)
            .key("shares-apply-all"),
    );
    if rules.is_empty() {
        group = group.child(caption(k, "Nothing shared yet. Add a path below."));
    }
    for rule in &rules {
        group = group.child(rule_row(s, k, project, rule, &checkouts));
    }
    let draft = TextInput::new(&reads.draft)
        .placeholder(".env.local")
        .label("Path to share")
        .on_edit(|s: &mut AppState| &mut s.projects.share_form.draft)
        .on_key(move |s: &mut AppState, ev, _| {
            if ev.key == rune::Key::Enter {
                let path = s.projects.share_form.draft.text.clone();
                add(s, project, &path, ShareStrategy::Copy);
                true
            } else {
                false
            }
        })
        .build(k)
        .key("shares-draft")
        .grow(1.0)
        .min_w(0.0);
    let mut adder = col().gap(4.0).child(
        row()
            .gap(6.0)
            .items_end()
            .child(draft)
            .child(
                Button::new("Browse…")
                    .variant(ButtonVariant::Outline)
                    .control_size(ControlSize::Small)
                    .on_press_tx(move |s: &mut AppState, tx| browse(s, tx, project))
                    .build(k)
                    .key("shares-browse"),
            )
            .child(
                Button::new("Add")
                    .variant(ButtonVariant::Contrast)
                    .control_size(ControlSize::Small)
                    .on_press(move |s: &mut AppState| {
                        let path = s.projects.share_form.draft.text.clone();
                        add(s, project, &path, ShareStrategy::Copy);
                    })
                    .build(k)
                    .key("shares-add"),
            ),
    );
    if let Some(problem) = &reads.path_error {
        adder = adder.child(
            text(problem.clone())
                .key("shares-path-error")
                .role("status")
                .font(k.ty.text_xsmall)
                .color(k.c.danger.fg),
        );
    }
    page = page.child(
        group
            .child(crate::settings::setting_row(
                k,
                "Add a path",
                Some("Anything in the repository, whether or not the scan proposed it."),
                el(),
            ))
            .child(adder),
    );

    page = page.child(candidates(s, k, project));

    let mut facts =
        crate::settings::group(k, "shares-strategies", "What each strategy means", None);
    for (label, help) in STRATEGIES {
        facts = facts.child(
            col()
                .gap(2.0)
                .child(text(label).font(k.ty.small_medium).color(k.c.text_base))
                .child(caption(k, help)),
        );
    }
    let preview_at = workspace.or(s.workbench.workspace);
    if let Some(at) = preview_at {
        facts = facts.child(
            Button::new("Preview this checkout")
                .variant(ButtonVariant::Outline)
                .control_size(ControlSize::Small)
                .on_press(move |s: &mut AppState| {
                    send(s, Op::PreviewShares(at));
                })
                .build(k)
                .key("shares-preview"),
        );
        if let Some(actions) = shares.actions.get(&at) {
            let mut plan = col().key("shares-plan").gap(2.0);
            if actions.is_empty() {
                plan = plan.child(caption(k, "Nothing to do here."));
            }
            for action in actions {
                let mut line = format!("{} {}", verb(action.verb), action.path);
                if action.bytes.is_some() {
                    line.push_str(&format!(" · {}", size(action.bytes)));
                }
                if action.fallback {
                    line.push_str(" · full copy");
                }
                if let Some(note) = &action.note {
                    line.push_str(&format!(" · {note}"));
                }
                plan = plan.child(text(line).font(k.ty.mono).color(k.c.text_muted).truncate());
            }
            facts = facts.child(plan);
        }
    }
    page.child(facts)
}

fn rule_row(
    s: &AppState,
    k: &Kit,
    project: ProjectId,
    rule: &ShareRule,
    checkouts: &[WorkspaceId],
) -> El {
    let id = rule.id;
    let labels: Vec<&str> = ["Link", "Copy", "Clone", "Run"].to_vec();
    let strategies = SegmentedControl::new(&labels, Some(strategy_index(&rule.strategy)))
        .label(format!("Strategy for {}", rule.path))
        .fit()
        .on_pick(move |s: &mut AppState, index, _| retarget(s, project, id, index))
        .build(k)
        .key(format!("share-strategy-{id}"));
    let (applied, total, worst) = summary(id, checkouts, &s.projects.shares.status);
    let badge = worst
        .as_ref()
        .map_or_else(|| format!("applied in {applied}/{total}"), state_label);
    let enabled = rule.enabled;
    let menu = crate::menus::ButtonMenu::ShareRule(project, id);
    let line = row()
        .w_full()
        .gap(8.0)
        .items_center()
        .child(
            text(rule.path.clone())
                .font(k.ty.mono)
                .color(if enabled {
                    k.c.text_base
                } else {
                    k.c.text_faint
                })
                .truncate()
                .min_w(0.0)
                .grow(1.0),
        )
        .child(strategies)
        .child(text(badge).font(k.ty.text_xsmall).color(match &worst {
            Some(state) if bad(state) => k.c.danger.fg,
            Some(_) => k.c.warning.fg,
            None => k.c.success.fg,
        }))
        .child(
            Switch::new(enabled)
                .label(format!("Share {}", rule.path))
                .hide_label()
                .control_size(ControlSize::Small)
                .on_toggle(move |s: &mut AppState| set_enabled(s, project, id, !enabled))
                .build(k)
                .key(format!("share-enabled-{id}")),
        )
        .child(
            Button::new("⋯")
                .variant(ButtonVariant::GhostMuted)
                .control_size(ControlSize::Small)
                .on_press(move |s: &mut AppState| s.menus.button = Some(menu.clone()))
                .build(k)
                .key(crate::menus::ButtonMenu::ShareRule(project, id).trigger()),
        );
    let mut block = col()
        .key(format!("share-rule-{id}"))
        .role("group")
        .label(rule.path.clone())
        .w_full()
        .gap(4.0)
        .child(line);
    if let ShareStrategy::Run { command, .. } = &rule.strategy {
        let edit = s
            .projects
            .share_form
            .commands
            .get(&id)
            .cloned()
            .unwrap_or_else(|| TextEdit::new(command.clone()));
        let field = TextInput::new(&edit)
            .placeholder("pnpm install")
            .label(format!("Command for {} · Enter saves", rule.path))
            .on_edit(move |s: &mut AppState| {
                let current = s
                    .store
                    .as_ref()
                    .and_then(|store| store.worktree_shares.iter().find(|r| r.id == id))
                    .and_then(|r| match &r.strategy {
                        ShareStrategy::Run { command, .. } => Some(command.clone()),
                        _ => None,
                    })
                    .unwrap_or_default();
                s.projects
                    .share_form
                    .commands
                    .entry(id)
                    .or_insert_with(|| TextEdit::new(current))
            })
            .on_key(move |s: &mut AppState, ev, _| {
                if ev.key == rune::Key::Enter {
                    set_command(s, project, id);
                    true
                } else {
                    false
                }
            })
            .build(k)
            .key(format!("share-command-{id}"));
        block = block.child(field);
    }
    block
}

fn candidates(s: &AppState, k: &Kit, project: ProjectId) -> El {
    let shares = &s.projects.shares;
    let found: Vec<&ShareCandidate> = shares
        .candidates
        .get(&project)
        .map(|(list, _)| list.iter().filter(|c| !c.already_ruled).collect())
        .unwrap_or_default();
    let mut group = crate::settings::group(
        k,
        "shares-candidates",
        "Detected, not shared yet",
        Some("What this repository ignores, with what Forge Node would do about it."),
    )
    .child(small(
        k,
        "Scan again",
        "shares-scan".into(),
        move |s: &mut AppState| {
            s.projects.share_form.scanned = None;
            scan(s, project);
        },
    ));
    if found.is_empty() {
        group = group.child(caption(
            k,
            if shares.scanning.contains(&project) {
                "Scanning…"
            } else {
                "Nothing left to propose."
            },
        ));
    }
    for (index, candidate) in found.into_iter().enumerate() {
        let path = candidate.path.clone();
        let suggested = candidate.suggested.clone();
        group = group.child(
            row()
                .key(format!("share-candidate-{index}"))
                .w_full()
                .gap(8.0)
                .items_center()
                .child(
                    text(candidate.path.clone())
                        .font(k.ty.mono)
                        .color(k.c.text_base)
                        .truncate()
                        .min_w(0.0)
                        .grow(1.0),
                )
                .child(caption(k, class_label(candidate.class)))
                .child(caption(k, size(candidate.size_bytes)))
                .child(
                    Button::new(format!("Share as {}", strategy_label(&candidate.suggested)))
                        .variant(ButtonVariant::Outline)
                        .control_size(ControlSize::Small)
                        .on_press(move |s: &mut AppState| {
                            add(s, project, &path, suggested.clone());
                        })
                        .build(k)
                        .key(format!("share-candidate-add-{index}")),
                ),
        );
    }
    if shares
        .candidates
        .get(&project)
        .is_some_and(|(_, truncated)| *truncated)
    {
        group = group.child(caption(
            k,
            "The scan stopped at its budget, so this list is a floor, not the whole tree.",
        ));
    }
    group
}

/// The dialog a project or checkout menu opens, and the removal it may ask.
pub fn dialogs(s: &AppState, k: &Kit, layer: &mut Vec<El>) {
    if let Some((project, workspace)) = s.projects.share_form.target {
        let name = workspace
            .and_then(|id| {
                s.store
                    .as_ref()?
                    .workspaces
                    .iter()
                    .find(|w| w.id == id)
                    .map(domain::Workspace::label)
            })
            .or_else(|| {
                s.store
                    .as_ref()?
                    .projects
                    .iter()
                    .find(|p| p.id == project)
                    .map(|p| p.name.clone())
            })
            .unwrap_or_default();
        Dialog::new("shared-files-dialog", format!("Shared files · {name}"))
            .open(true)
            .width(720.0)
            .on_close(|s: &mut AppState| s.projects.share_form.target = None)
            .child(
                col()
                    .key("shared-files-scroll")
                    .w_full()
                    .max_h(560.0)
                    .scroll(rune::ScrollSpec {
                        stick_bottom: false,
                    })
                    .child(page(s, k, Some(project), workspace)),
            )
            .child(
                row().pt(12.0).child(el().grow(1.0)).child(
                    Button::new("Done")
                        .variant(ButtonVariant::Outline)
                        .on_press(|s: &mut AppState| s.projects.share_form.target = None)
                        .build(k)
                        .key("shared-files-done"),
                ),
            )
            .build(k, layer);
    }
    removal_dialog(s, k, layer);
}

fn removal_dialog(s: &AppState, k: &Kit, layer: &mut Vec<El>) {
    let Some((_, rule_id, cleanup)) = s.projects.share_form.removing else {
        return;
    };
    let Some(rule) = s
        .store
        .as_ref()
        .and_then(|store| store.worktree_shares.iter().find(|r| r.id == rule_id))
    else {
        return;
    };
    let link = rule.strategy == ShareStrategy::Link;
    let mut choices = RadioGroup::new().label("In every checkout");
    for (value, label, enabled) in [
        (ShareCleanup::Leave, "Leave the files as they are", true),
        (
            ShareCleanup::RemoveInjected,
            "Delete what Forge Node wrote (never a file you changed)",
            true,
        ),
        (
            ShareCleanup::Materialize,
            "Turn links into real files",
            link,
        ),
    ] {
        choices = choices.item(
            RadioItem::new(label, cleanup == value)
                .disabled(!enabled)
                .on_select(move |s: &mut AppState| {
                    if let Some((_, _, cleanup)) = s.projects.share_form.removing.as_mut() {
                        *cleanup = value;
                    }
                }),
        );
    }
    Dialog::new("share-remove", format!("Stop sharing {}?", rule.path))
        .description(
            "The rule goes either way. What happens to the files it already put in each \
             checkout is up to you.",
        )
        .open(true)
        .fit()
        .width(460.0)
        .on_close(|s: &mut AppState| s.projects.share_form.removing = None)
        .child(choices.build(k))
        .child(
            row()
                .pt(12.0)
                .gap(8.0)
                .child(el().grow(1.0))
                .child(
                    Button::new("Cancel")
                        .variant(ButtonVariant::Ghost)
                        .on_press(|s: &mut AppState| s.projects.share_form.removing = None)
                        .build(k)
                        .key("share-remove-cancel"),
                )
                .child(
                    Button::new("Remove the rule")
                        .variant(ButtonVariant::Danger)
                        .on_press(confirm_removal)
                        .build(k)
                        .key("share-remove-confirm"),
                ),
        )
        .build(k, layer);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(project: ProjectId, path: &str, position: u32) -> ShareRule {
        ShareRule {
            id: ShareRuleId::new(),
            project_id: project,
            path: path.into(),
            strategy: ShareStrategy::Copy,
            enabled: true,
            position,
            created_at: domain::Timestamp::now(),
        }
    }

    #[test]
    fn a_new_path_stays_inside_and_unique() {
        let project = ProjectId::new();
        let rules = vec![rule(project, ".env", 0)];
        assert_eq!(
            clean_path(" /config/local.toml/ ", &rules),
            Ok("config/local.toml".into())
        );
        assert_eq!(clean_path("  ", &rules), Err("Type a path first."));
        assert_eq!(
            clean_path("../secrets", &rules),
            Err("The path must stay inside the repository.")
        );
        assert_eq!(
            clean_path("a/../../b", &rules),
            Err("The path must stay inside the repository.")
        );
        assert_eq!(
            clean_path(".env", &rules),
            Err("That path already has a rule.")
        );
    }

    #[test]
    fn moving_a_rule_renumbers_and_stops_at_the_ends() {
        let project = ProjectId::new();
        let rules = vec![
            rule(project, "a", 0),
            rule(project, "b", 1),
            rule(project, "c", 2),
        ];
        let next = moved(&rules, rules[2].id, -1).expect("c moves up");
        let order: Vec<(&str, u32)> = next.iter().map(|r| (r.path.as_str(), r.position)).collect();
        assert_eq!(order, [("a", 0), ("c", 1), ("b", 2)]);
        assert!(moved(&rules, rules[0].id, -1).is_none());
        assert!(moved(&rules, rules[2].id, 1).is_none());
    }

    #[test]
    fn a_rule_summary_counts_applied_and_names_the_worst() {
        let project = ProjectId::new();
        let r = rule(project, ".env", 0);
        let (a, b, c) = (WorkspaceId::new(), WorkspaceId::new(), WorkspaceId::new());
        let entry = |state| {
            vec![ShareStatusEntry {
                rule_id: r.id,
                path: ".env".into(),
                state,
            }]
        };
        let status = HashMap::from([
            (a, entry(ShareState::Applied)),
            (b, entry(ShareState::Missing)),
            (c, entry(ShareState::Severed)),
        ]);
        let (applied, total, worst) = summary(r.id, &[a, b, c], &status);
        assert_eq!((applied, total), (1, 3));
        assert_eq!(worst, Some(ShareState::Severed), "a bad state wins");
        assert_eq!(
            state_label(&ShareState::Severed),
            "link replaced by a real file"
        );
    }

    #[test]
    fn a_run_strategy_keeps_its_command_and_a_new_one_starts_empty() {
        let kept = ShareStrategy::Run {
            command: "pnpm i".into(),
            timeout_secs: 30,
        };
        assert_eq!(strategy_at(3, &kept), kept);
        assert_eq!(
            strategy_at(3, &ShareStrategy::Copy),
            ShareStrategy::Run {
                command: String::new(),
                timeout_secs: RUN_TIMEOUT_SECS
            }
        );
        assert_eq!(strategy_at(0, &kept), ShareStrategy::Link);
        assert_eq!(size(Some(1536)), "1.5 KB");
        assert_eq!(size(Some(20 * 1024 * 1024)), "20 MB");
        assert_eq!(size(None), "—");
    }
}
