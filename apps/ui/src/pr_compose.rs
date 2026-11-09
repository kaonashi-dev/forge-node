//! Opening a pull request with an agent: the task, the agent, the prompt it is
//! handed, and the draft it writes back.
//!
//! A draft is runtime-only, like every other remote answer: it is broadcast
//! once as `PullRequestDraftReady` and this side is its only copy until the
//! person submits or discards it.

use client::Store;
use domain::{
    AgentProfileId, AgentProviderId, PullRequestDraft, SessionId, TaskMode, WorkspaceDiff,
    WorkspaceId,
};
use rune::{El, ScrollSpec, TextEdit, col, row, text};
use rune_kit::{Button, ButtonVariant, ControlSize, Kit, TextArea, TextInput};

use crate::app::AppState;

/// Tauri's cap on the diff embedded in a draft prompt.
pub const MAX_PROMPT_PATCH: usize = 24 * 1024;

pub struct ReceivedDraft {
    pub workspace: WorkspaceId,
    pub session: SessionId,
    pub draft: PullRequestDraft,
}

/// Everything `CommitAndOpenPullRequest` needs, already trimmed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Submission {
    pub workspace: WorkspaceId,
    pub title: String,
    pub branch: Option<String>,
    pub message: String,
    pub body: String,
}

pub struct Compose {
    pub workspace: Option<WorkspaceId>,
    /// `provider:<id>`-less launch key: the provider id, or `profile:<uuid>`.
    pub agent_key: Option<String>,
    pub task_id: String,
    pub draft: Option<ReceivedDraft>,
    pub title: TextEdit,
    pub body: TextEdit,
    pub branch: TextEdit,
    pub commit: TextEdit,
    pub launching: bool,
    pub launching_session: Option<SessionId>,
    pub opening: bool,
    pub error: Option<String>,
    pub opened: Option<String>,
    pub prompt_open: bool,
}

impl Default for Compose {
    fn default() -> Self {
        Self {
            workspace: None,
            agent_key: None,
            task_id: domain::pr_task::builtin_tasks()
                .into_iter()
                .next()
                .map(|task| task.id)
                .unwrap_or_default(),
            draft: None,
            title: TextEdit::new(""),
            body: TextEdit::new(""),
            branch: TextEdit::new(""),
            commit: TextEdit::new(""),
            launching: false,
            launching_session: None,
            opening: false,
            error: None,
            opened: None,
            prompt_open: false,
        }
    }
}

impl Compose {
    pub fn follow(&mut self, workspace: Option<WorkspaceId>) {
        if self.workspace == workspace {
            return;
        }
        self.workspace = workspace;
        self.launching = false;
        self.opening = false;
        self.error = None;
        self.opened = None;
    }

    pub fn receive(&mut self, received: ReceivedDraft) {
        self.workspace = Some(received.workspace);
        self.title = TextEdit::new(received.draft.title.clone());
        self.body = TextEdit::new(received.draft.body.clone());
        self.branch = TextEdit::new(received.draft.branch.clone());
        self.commit = TextEdit::new(received.draft.commit_message.clone());
        self.launching = false;
        self.launching_session = None;
        self.opening = false;
        self.error = None;
        self.opened = None;
        self.draft = Some(received);
    }

    pub fn clear_draft(&mut self) {
        self.draft = None;
        self.opening = false;
        self.opened = None;
        self.title = TextEdit::new("");
        self.body = TextEdit::new("");
        self.branch = TextEdit::new("");
        self.commit = TextEdit::new("");
    }

    /// The draft only belongs to the checkout on screen; another checkout's
    /// draft stays with its workspace.
    #[must_use]
    pub fn visible_draft(&self) -> Option<&ReceivedDraft> {
        self.draft
            .as_ref()
            .filter(|draft| Some(draft.workspace) == self.workspace)
    }

    #[must_use]
    pub fn can_open(&self) -> bool {
        !self.opening
            && self.visible_draft().is_some()
            && !self.title.text.trim().is_empty()
            && !self.commit.text.trim().is_empty()
    }

    /// What the daemon's `CommitAndOpenPullRequest` needs, or `None` while a
    /// required field is empty or a request is in flight.
    pub fn submission(&self) -> Option<Submission> {
        if !self.can_open() {
            return None;
        }
        let branch = self.branch.text.trim();
        Some(Submission {
            workspace: self.workspace?,
            title: self.title.text.trim().to_string(),
            branch: (!branch.is_empty()).then(|| branch.to_string()),
            message: self.commit.text.trim().to_string(),
            body: self.body.text.trim().to_string(),
        })
    }

    pub fn pr_opened(
        &mut self,
        workspace: WorkspaceId,
        url: Option<String>,
        error: Option<String>,
    ) {
        if self.workspace != Some(workspace) {
            return;
        }
        self.opening = false;
        self.opened = None;
        if let Some(error) = error {
            self.error = Some(error);
            return;
        }
        if let Some(url) = url {
            self.opened = Some(url);
            self.draft = None;
            self.title = TextEdit::new("");
            self.body = TextEdit::new("");
            self.branch = TextEdit::new("");
            self.commit = TextEdit::new("");
        }
    }
}

/// One row of the agent picker: an installed, prompt-capable provider or one
/// of its launch profiles.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choice {
    pub key: String,
    pub label: String,
    pub provider: AgentProviderId,
    pub profile: Option<AgentProfileId>,
}

#[must_use]
pub fn choices(store: &Store) -> Vec<Choice> {
    let visible =
        |key: &str| store.app_state_value(&crate::launch::visibility_key(key)) != Some("false");
    let mut out = Vec::new();
    for info in &store.providers {
        let provider = &info.descriptor.id;
        if !matches!(
            info.detection.status,
            domain::DetectionStatus::Installed { .. }
        ) || !info.descriptor.capabilities.supports_initial_prompt
            || !visible(provider.as_str())
        {
            continue;
        }
        out.push(Choice {
            key: provider.to_string(),
            label: info.descriptor.display_name.clone(),
            provider: provider.clone(),
            profile: None,
        });
        for profile in store
            .agent_profiles
            .iter()
            .filter(|profile| profile.provider_id == *provider)
            .filter(|profile| visible(&format!("profile:{}", profile.id)))
        {
            out.push(Choice {
                key: format!("profile:{}", profile.id),
                label: profile.name.clone(),
                provider: provider.clone(),
                profile: Some(profile.id),
            });
        }
    }
    out
}

/// The agent a launch would pick: an explicit choice while it is still
/// offered, else the configured default, else the only prompt-capable agent.
pub fn resolve(
    store: &Store,
    picked: Option<&str>,
) -> Result<(AgentProviderId, Option<AgentProfileId>), String> {
    let installed: Vec<AgentProviderId> = store
        .providers
        .iter()
        .filter(|info| {
            matches!(
                info.detection.status,
                domain::DetectionStatus::Installed { .. }
            )
        })
        .map(|info| info.descriptor.id.clone())
        .collect();
    let profiles: Vec<(AgentProfileId, AgentProviderId)> = store
        .agent_profiles
        .iter()
        .map(|profile| (profile.id, profile.provider_id.clone()))
        .collect();
    let can = crate::launch::Launchables {
        visible: |key: &str| {
            store.app_state_value(&crate::launch::visibility_key(key)) != Some("false")
        },
        installed: &installed,
        profiles: &profiles,
        takes_prompt: |provider: &AgentProviderId| {
            store.providers.iter().any(|info| {
                info.descriptor.id == *provider
                    && info.descriptor.capabilities.supports_initial_prompt
            })
        },
    };
    crate::launch::resolver(
        &crate::launch::parse(store.app_state_value(crate::launch::DEFAULT_AGENT_KEY)),
        picked,
        &can,
    )
}

/// The key that names `provider`/`profile` in [`resolve`].
#[must_use]
pub fn key_of(provider: &AgentProviderId, profile: Option<AgentProfileId>) -> String {
    profile.map_or_else(
        || provider.to_string(),
        |profile| format!("profile:{profile}"),
    )
}

pub fn diff_summary(diff: &WorkspaceDiff) -> String {
    let (additions, deletions) = diff.totals();
    format!(
        "{} {} · +{additions} −{deletions}",
        diff.files.len(),
        if diff.files.len() == 1 {
            "file"
        } else {
            "files"
        }
    )
}

/// The prompt handed to a draft agent: the task's wording, the change summary
/// and each file's patch within a fixed budget. Nothing here reads the
/// checkout; it is the diff the Git panel already has.
#[must_use]
pub fn compose_prompt(task: &domain::PrTask, diff: &WorkspaceDiff, extra: &str) -> String {
    let mut prompt = task.body.clone();
    prompt.push_str("\n\n## The change\n\n");
    if let Some(branch) = &diff.branch {
        prompt.push_str(&format!("Branch `{branch}`, "));
    }
    prompt.push_str(&diff_summary(diff));
    prompt.push_str(".\n");
    for file in &diff.files {
        prompt.push_str(&format!(
            "\n- `{}` ({}, +{} −{})",
            file.path,
            status_word(file.status),
            file.additions,
            file.deletions
        ));
    }
    prompt.push('\n');
    if task.mode == TaskMode::Draft {
        let mut patch = String::new();
        let mut truncated = diff.truncated;
        for file in &diff.files {
            if patch.len() + file.patch.len() > MAX_PROMPT_PATCH {
                truncated = true;
                break;
            }
            patch.push_str(&file.patch);
        }
        if !patch.is_empty() {
            prompt.push_str("\n## The diff\n\n```diff\n");
            prompt.push_str(&patch);
            prompt.push_str("\n```\n");
        }
        if truncated {
            prompt.push_str(
                "\nThe diff above is incomplete: some files were left out for length. Describe \
                 what you can see and say that it is partial.\n",
            );
        }
    }
    let extra = extra.trim();
    if !extra.is_empty() {
        prompt.push_str("\n## Also\n\n");
        prompt.push_str(extra);
        prompt.push('\n');
    }
    prompt
}

fn status_word(status: domain::DiffStatus) -> &'static str {
    match status {
        domain::DiffStatus::Added => "added",
        domain::DiffStatus::Modified => "modified",
        domain::DiffStatus::Deleted => "deleted",
        domain::DiffStatus::Conflicted => "unresolved conflict",
        _ => "changed",
    }
}

impl AppState {
    /// Opens (or returns to) the compose view for the window's checkout.
    pub fn open_pr_compose(&mut self, workspace: WorkspaceId) {
        self.chosen_workspace = Some(workspace);
        self.sync_workbench_workspace();
        self.compose.follow(self.workbench.workspace);
        self.show_mode(crate::workbench::CenterMode::PrCompose);
    }

    /// Launches the selected agent with the task's prompt. It never commits:
    /// the agent hands the draft back through `pr_draft`.
    pub fn launch_pr_agent(&mut self) {
        if self.compose.launching {
            return;
        }
        let Some(workspace) = self.workbench.workspace else {
            return;
        };
        let Some(store) = &self.store else {
            return;
        };
        let agent = match resolve(store, self.compose.agent_key.as_deref()) {
            Ok(agent) => agent,
            Err(reason) => {
                self.compose.error = Some(reason);
                return;
            }
        };
        let Some(diff) = self.workbench.changes.diff() else {
            self.compose.error = Some("Nothing uncommitted here.".into());
            return;
        };
        if diff.files.is_empty() {
            self.compose.error = Some("Nothing uncommitted here.".into());
            return;
        }
        let task = domain::pr_task::task_or_default(Some(&self.compose.task_id));
        let prompt = compose_prompt(&task, &diff, "");
        let Some(connection) = &self.connection else {
            return;
        };
        match connection.create_agent(workspace, agent.0, agent.1, Some(prompt), false) {
            Ok(()) => {
                self.compose.launching = true;
                self.compose.error = None;
            }
            Err(error) => self.compose.error = Some(error),
        }
    }

    /// Commits everything, pushes a branch when the checkout needs one, and
    /// opens the pull request; the outcome arrives as `PullRequestOpened`.
    pub fn submit_compose(&mut self) {
        let Some(submission) = self.compose.submission() else {
            return;
        };
        let Some(connection) = &self.connection else {
            return;
        };
        if connection
            .commit_and_open_pull_request(
                submission.workspace,
                submission.title,
                submission.body,
                submission.message,
                submission.branch,
            )
            .is_ok()
        {
            self.compose.opening = true;
            self.compose.error = None;
            self.compose.opened = None;
        }
    }
}

const CONTROL_W: f32 = 560.0;

pub fn view(s: &AppState, k: &Kit) -> El {
    let compose = &s.compose;
    let diff = s.workbench.changes.diff();
    let workspace = s.workbench.workspace.zip(s.store.as_ref());
    let branch = diff
        .as_ref()
        .and_then(|diff| diff.branch.clone())
        .unwrap_or_else(|| "detached".to_string());
    let detail = diff
        .as_ref()
        .map_or_else(|| "reading the working tree…".to_string(), diff_summary);
    let reload = Button::new("Reload")
        .variant(ButtonVariant::GhostMuted)
        .control_size(ControlSize::Small)
        .on_press(|s: &mut AppState| s.workbench.refresh())
        .build(k)
        .key("compose-reload");
    let close = Button::new("Close")
        .variant(ButtonVariant::GhostMuted)
        .control_size(ControlSize::Small)
        .on_press(|s: &mut AppState| s.close_center())
        .build(k)
        .key("compose-close");
    let mut head = row()
        .key("compose-head")
        .w_full()
        .h(36.0)
        .px(12.0)
        .gap(10.0)
        .items_center()
        .child(text(branch).font(k.ty.small_medium).color(k.c.text_base))
        .child(
            text(detail)
                .font(k.ty.text_xsmall)
                .color(k.c.text_faint)
                .grow(1.0),
        )
        .child(
            Button::new(if compose.prompt_open {
                "Hide prompt"
            } else {
                "Show prompt"
            })
            .variant(ButtonVariant::GhostMuted)
            .control_size(ControlSize::Small)
            .on_press(|s: &mut AppState| s.compose.prompt_open = !s.compose.prompt_open)
            .build(k)
            .key("compose-prompt-toggle"),
        )
        .child(reload)
        .child(close);
    if compose.workspace.is_some() && compose.workspace != s.workbench.workspace {
        head = head.child(
            text("This draft belongs to another checkout.".to_string())
                .font(k.ty.text_xsmall)
                .color(k.c.warning.fg),
        );
    }

    let tasks = domain::pr_task::builtin_tasks();
    let mut task_list = col().key("compose-tasks").w_full().gap(4.0);
    for task in &tasks {
        let chosen = compose.task_id == task.id;
        let id = task.id.clone();
        let mode = match task.mode {
            TaskMode::Draft => "writes the PR",
            TaskMode::Implement => "changes the code",
            _ => "changes the checkout",
        };
        task_list = task_list.child(
            row()
                .key(format!("compose-task-{}", task.id))
                .role("radio")
                .label(task.name.clone())
                .checked(chosen)
                .w_full()
                .gap(8.0)
                .px(10.0)
                .py(6.0)
                .radius(6.0)
                .items_center()
                .fill(if chosen {
                    k.c.surface_hover
                } else {
                    rune::Color::TRANSPARENT
                })
                .hover(k.c.hover)
                .on_press(move |s: &mut AppState| s.compose.task_id = id.clone())
                .child(
                    text(task.name.clone())
                        .font(k.ty.small_medium)
                        .color(k.c.text_base)
                        .grow(1.0),
                )
                .child(text(mode).font(k.ty.text_xsmall).color(k.c.text_faint))
                .child(
                    text(task.detail.clone())
                        .font(k.ty.text_xsmall)
                        .color(k.c.text_muted)
                        .truncate()
                        .min_w(0.0)
                        .max_w(320.0),
                ),
        );
    }

    let mut picker = row().key("compose-agents").w_full().gap(4.0).flex_wrap();
    let offered = workspace
        .map(|(_, store)| choices(store))
        .unwrap_or_default();
    let selected = workspace
        .and_then(|(_, store)| resolve(store, compose.agent_key.as_deref()).ok())
        .map(|(provider, profile)| key_of(&provider, profile));
    for choice in &offered {
        let key = choice.key.clone();
        let chosen = selected.as_deref() == Some(choice.key.as_str());
        picker = picker.child(
            Button::new(choice.label.clone())
                .variant(if chosen {
                    ButtonVariant::Outline
                } else {
                    ButtonVariant::GhostMuted
                })
                .control_size(ControlSize::Small)
                .on_press(move |s: &mut AppState| s.compose.agent_key = Some(key.clone()))
                .build(k)
                .key(format!("compose-agent-{}", choice.key)),
        );
    }
    if offered.is_empty() {
        picker = picker.child(
            text("No installed agent takes a prompt.".to_string())
                .font(k.ty.text_xsmall)
                .color(k.c.warning.fg),
        );
    }

    let task = domain::pr_task::task_or_default(Some(&compose.task_id));
    let empty = diff.as_ref().is_none_or(WorkspaceDiff::is_empty);
    let hint = if let Some(error) = &compose.error {
        error.clone()
    } else if compose.launching {
        "An agent is already running for this checkout.".to_string()
    } else if diff.is_none() {
        "Reading the working tree…".to_string()
    } else if empty {
        "Nothing uncommitted here.".to_string()
    } else if offered.is_empty() {
        "No installed agent can be started with a prompt.".to_string()
    } else if task.mode == TaskMode::Draft {
        "You edit what the agent writes; Forge Node then commits, pushes and opens the PR."
            .to_string()
    } else {
        "The agent works in this checkout and opens the PR itself.".to_string()
    };
    let launch = Button::new(if compose.launching {
        "Launching…".to_string()
    } else {
        task.mode.action().to_string()
    })
    .variant(ButtonVariant::Contrast)
    .disabled(compose.launching || empty || offered.is_empty())
    .on_press(|s: &mut AppState| s.launch_pr_agent())
    .build(k)
    .key("compose-launch");

    let mut body = col()
        .key("pr-compose")
        .w_full()
        .h_full()
        .min_h(0.0)
        .child(head)
        .child(
            col()
                .key("compose-scroll")
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .padding(16.0)
                .gap(14.0)
                .scroll(ScrollSpec {
                    stick_bottom: false,
                })
                .child(
                    text("How should this pull request be prepared?")
                        .font(k.ty.small_medium)
                        .color(k.c.text_base),
                )
                .child(task_list)
                .child(text("Agent").font(k.ty.text_xsmall).color(k.c.text_faint))
                .child(picker)
                .child(
                    row().gap(12.0).items_center().child(launch).child(
                        text(hint)
                            .key("compose-note")
                            .role("status")
                            .font(k.ty.text_xsmall)
                            .color(if compose.error.is_some() {
                                k.c.warning.fg
                            } else {
                                k.c.text_faint
                            })
                            .grow(1.0),
                    ),
                )
                .children(
                    (if compose.prompt_open {
                        diff.as_ref()
                    } else {
                        None
                    })
                    .map(|diff| {
                        text(compose_prompt(&task, diff, ""))
                            .key("compose-prompt")
                            .font(k.ty.mono)
                            .color(k.c.text_muted)
                    }),
                )
                .children(compose.visible_draft().is_some().then(|| draft_form(s, k))),
        );
    if let Some(url) = &compose.opened {
        body = body.child(
            text(format!("Opened {url}"))
                .key("compose-opened")
                .role("status")
                .font(k.ty.text_xsmall)
                .color(k.c.success.fg)
                .padding(12.0),
        );
    }
    if let Some(error) = &compose.error {
        body = body.child(
            text(error.clone())
                .key("compose-error")
                .role("status")
                .font(k.ty.text_xsmall)
                .color(k.c.warning.fg),
        );
    }
    body
}

fn draft_form(s: &AppState, k: &Kit) -> El {
    let compose = &s.compose;
    col()
        .key("compose-draft")
        .w_full()
        .gap(8.0)
        .padding(12.0)
        .radius(8.0)
        .fill(k.c.surface_weak)
        .child(
            text("Draft ready".to_string())
                .font(k.ty.small_medium)
                .color(k.c.text_base),
        )
        .child(
            TextInput::new(&compose.title)
                .label("Title")
                .on_edit(|s: &mut AppState| &mut s.compose.title)
                .build(k)
                .key("compose-title")
                .w(CONTROL_W),
        )
        .child(
            TextArea::new(&compose.body)
                .label("Description")
                .on_edit(|s: &mut AppState| &mut s.compose.body)
                .build(k)
                .key("compose-body")
                .w(CONTROL_W)
                .h(180.0),
        )
        .child(
            TextInput::new(&compose.commit)
                .label("Commit message")
                .on_edit(|s: &mut AppState| &mut s.compose.commit)
                .build(k)
                .key("compose-commit")
                .w(CONTROL_W),
        )
        .child(
            TextInput::new(&compose.branch)
                .label("Branch")
                .on_edit(|s: &mut AppState| &mut s.compose.branch)
                .build(k)
                .key("compose-branch")
                .w(CONTROL_W),
        )
        .child(
            text(
                "Everything uncommitted goes into one commit. The branch is created only when \
                 this checkout is on its default branch.",
            )
            .font(k.ty.text_xsmall)
            .color(k.c.text_faint),
        )
        .child(
            row()
                .gap(8.0)
                .items_center()
                .child(
                    Button::new(if compose.opening {
                        "Opening…"
                    } else {
                        "Commit, push & open PR"
                    })
                    .variant(ButtonVariant::Contrast)
                    .disabled(!compose.can_open())
                    .on_press(|s: &mut AppState| s.submit_compose())
                    .build(k)
                    .key("compose-submit"),
                )
                .child(
                    Button::new("Discard draft")
                        .variant(ButtonVariant::GhostMuted)
                        .control_size(ControlSize::Small)
                        .on_press(|s: &mut AppState| s.compose.clear_draft())
                        .build(k)
                        .key("compose-discard"),
                )
                .children(compose.visible_draft().map(|draft| {
                    let session = draft.session;
                    Button::new("Show agent session")
                        .variant(ButtonVariant::GhostMuted)
                        .control_size(ControlSize::Small)
                        .on_press(move |s: &mut AppState| {
                            s.select(session);
                            s.show_mode(crate::workbench::CenterMode::Session);
                        })
                        .build(k)
                        .key("compose-show-agent")
                })),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::{DiffFile, DiffStatus};

    fn file(path: &str, patch: &str, additions: u32) -> DiffFile {
        DiffFile {
            path: path.into(),
            status: DiffStatus::Modified,
            additions,
            deletions: 1,
            patch: patch.into(),
            binary: false,
            truncated: false,
        }
    }

    fn diff() -> WorkspaceDiff {
        WorkspaceDiff {
            workspace_id: WorkspaceId::new(),
            branch: Some("feature/pr".into()),
            files: vec![
                file("src/a.rs", "@@ -1 +1 @@\n-old\n+new\n", 1),
                file("src/b.rs", "@@ -2 +2 @@\n-old\n+new\n", 1),
            ],
            truncated: false,
        }
    }

    #[test]
    fn a_prompt_names_the_change_and_carries_the_patch() {
        let task = domain::pr_task::task_or_default(Some("describe"));
        let prompt = compose_prompt(&task, &diff(), "");
        assert!(prompt.contains("## The change"));
        assert!(prompt.contains("Branch `feature/pr`, 2 files · +2 −2."));
        assert!(prompt.contains("`src/a.rs` (modified, +1 −1)"));
        assert!(prompt.contains("## The diff"));
        assert!(prompt.contains("+new"));
        assert!(!prompt.contains("## Also"));
    }

    #[test]
    fn an_implement_task_gets_no_patch_block() {
        let task = domain::pr_task::task_or_default(Some("finish"));
        let prompt = compose_prompt(&task, &diff(), "");
        assert!(!prompt.contains("## The diff"));
        assert!(prompt.contains("## The change"));
    }

    #[test]
    fn a_prompt_over_budget_says_so_instead_of_cutting_a_file() {
        let task = domain::pr_task::task_or_default(Some("describe"));
        let big = "x".repeat(MAX_PROMPT_PATCH + 1);
        let mut huge = diff();
        huge.files = vec![file("big.txt", &big, 1)];
        huge.truncated = false;
        let prompt = compose_prompt(&task, &huge, "");
        assert!(prompt.contains("The diff above is incomplete"));
        assert!(
            !prompt.contains(&big),
            "the oversized patch is left out whole"
        );
    }

    #[test]
    fn extra_words_go_under_their_own_heading() {
        let task = domain::pr_task::task_or_default(Some("describe"));
        let prompt = compose_prompt(&task, &diff(), "  Mention the cache. ");
        assert!(prompt.contains("## Also\n\nMention the cache."));
    }

    #[test]
    fn a_draft_needs_a_title_and_a_commit_message() {
        let mut compose = Compose::default();
        assert!(!compose.can_open(), "no draft yet");
        compose.receive(ReceivedDraft {
            workspace: WorkspaceId::new(),
            session: SessionId::new(),
            draft: PullRequestDraft {
                title: "Title".into(),
                body: "Body".into(),
                branch: " feature/x ".into(),
                commit_message: "Commit".into(),
            },
        });
        assert!(compose.can_open());
        let submission = compose.submission().expect("a submission");
        assert_eq!(submission.title, "Title");
        assert_eq!(submission.branch.as_deref(), Some("feature/x"));
        assert_eq!(submission.message, "Commit");
        assert_eq!(submission.body, "Body");
        assert_eq!(
            submission.workspace,
            compose.workspace.expect("a workspace")
        );
        compose.title = TextEdit::new("  ");
        assert!(!compose.can_open());
        assert!(compose.submission().is_none());
    }

    #[test]
    fn a_successful_open_consumes_the_draft_and_a_failure_keeps_it() {
        let workspace = WorkspaceId::new();
        let mut compose = Compose::default();
        compose.receive(ReceivedDraft {
            workspace,
            session: SessionId::new(),
            draft: PullRequestDraft {
                title: "Title".into(),
                body: String::new(),
                branch: String::new(),
                commit_message: "Commit".into(),
            },
        });
        compose.opening = true;
        compose.pr_opened(workspace, None, Some("refused".into()));
        assert!(compose.draft.is_some(), "a refusal keeps the text");
        assert_eq!(compose.error.as_deref(), Some("refused"));
        compose.pr_opened(workspace, Some("https://x/1".into()), None);
        assert!(compose.draft.is_none());
    }

    #[test]
    fn another_checkouts_draft_is_not_shown() {
        let mut compose = Compose::default();
        let workspace = WorkspaceId::new();
        compose.receive(ReceivedDraft {
            workspace,
            session: SessionId::new(),
            draft: PullRequestDraft {
                title: "Title".into(),
                body: String::new(),
                branch: String::new(),
                commit_message: "Commit".into(),
            },
        });
        compose.follow(Some(WorkspaceId::new()));
        assert!(compose.visible_draft().is_none());
        compose.follow(Some(workspace));
        assert!(compose.visible_draft().is_some());
    }

    #[test]
    fn a_summary_counts_files_and_lines() {
        assert_eq!(diff_summary(&diff()), "2 files · +2 −2");
        let one = WorkspaceDiff {
            files: vec![file("a.rs", "", 0)],
            ..WorkspaceDiff::default()
        };
        assert_eq!(diff_summary(&one), "1 file · +0 −1");
    }
}
