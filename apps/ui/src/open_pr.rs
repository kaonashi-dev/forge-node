//! "Open PR": the change at a glance, the agent that will write it up, and one
//! Confirm. The agent then runs in the background (`OpenPullRequestWithAgent`)
//! and the outcome arrives as `PullRequestOpened`, shown as a toast wherever
//! the person is by then.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use client::Store;
use domain::{AgentProfileId, AgentProviderId, SessionId, WorkspaceDiff, WorkspaceId};
use rune::{El, ScrollSpec, col, row, text};
use rune_kit::{Button, ButtonVariant, Checkbox, ControlSize, Kit};

use crate::app::AppState;

/// Files listed in the overview; the rest are counted.
const LISTED_FILES: usize = 200;

#[derive(Default)]
pub struct OpenPr {
    /// The picker's choice: a provider id, or `profile:<uuid>`.
    pub agent_key: Option<String>,
    /// Open it as a draft pull request.
    pub as_draft: bool,
    /// Checkouts whose agent is drafting, from Confirm to `PullRequestOpened`.
    pub pending: HashMap<WorkspaceId, Pending>,
    /// The daemon's refusal of the last Confirm, for the checkout it named.
    pub refused: Option<(WorkspaceId, String)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pending {
    /// What the progress row calls the agent.
    pub agent: String,
    pub since: Instant,
}

/// How long a Confirm may go without its agent session appearing before a
/// fresh snapshot counts the run as over: launching probes the agent first.
pub const LAUNCH_GRACE: Duration = Duration::from_secs(20);

/// What a `PullRequestOpened` means for the person.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Opened(String),
    Failed {
        error: String,
        session: Option<SessionId>,
    },
}

impl OpenPr {
    pub fn started(&mut self, workspace: WorkspaceId, agent: String) {
        self.pending.insert(
            workspace,
            Pending {
                agent,
                since: Instant::now(),
            },
        );
        self.refused = None;
    }

    /// The daemon refused the request itself: nothing is running there.
    pub fn refused(&mut self, workspace: WorkspaceId, message: &str) {
        self.pending.remove(&workspace);
        self.refused = Some((workspace, message.to_string()));
    }

    /// `PullRequestOpened` for `workspace`. `None` when this window did not
    /// ask, so a pull request `forgectl` opened raises no toast here.
    pub fn finished(
        &mut self,
        workspace: WorkspaceId,
        url: Option<String>,
        error: Option<String>,
        session: Option<SessionId>,
    ) -> Option<Outcome> {
        self.pending.remove(&workspace)?;
        Some(match (url, error) {
            (Some(url), _) => Outcome::Opened(url),
            (None, error) => Outcome::Failed {
                error: error.unwrap_or_else(|| "the daemon reported no pull request".into()),
                session,
            },
        })
    }
}

impl OpenPr {
    /// After a snapshot: runs whose agent is gone ended while this window was
    /// not listening, so their outcome will never arrive. Returns how many.
    pub fn reconcile(&mut self, store: &Store, now: Instant) -> usize {
        let before = self.pending.len();
        self.pending.retain(|workspace, pending| {
            now.saturating_duration_since(pending.since) < LAUNCH_GRACE
                || drafting_session(store, *workspace).is_some()
        });
        before - self.pending.len()
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

#[must_use]
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

/// The background agent drafting for `workspace`, if it is still alive.
#[must_use]
pub fn drafting_session(store: &Store, workspace: WorkspaceId) -> Option<&domain::Session> {
    store.sessions.iter().find(|session| {
        session.background && session.workspace_id == workspace && session.state.is_active()
    })
}

impl AppState {
    /// Shows the overview for `workspace`.
    pub fn open_pr_view(&mut self, workspace: WorkspaceId) {
        self.chosen_workspace = Some(workspace);
        self.sync_workbench_workspace();
        self.show_mode(crate::workbench::CenterMode::OpenPr);
    }

    /// Starts the background agent and returns to what was on screen before.
    pub fn confirm_open_pr(&mut self) {
        let Some(workspace) = self.workbench.workspace else {
            return;
        };
        if self.open_pr.pending.contains_key(&workspace) {
            return;
        }
        let Some(store) = &self.store else {
            return;
        };
        let (provider, profile) = match resolve(store, self.open_pr.agent_key.as_deref()) {
            Ok(agent) => agent,
            Err(reason) => {
                self.open_pr.refused = Some((workspace, reason));
                return;
            }
        };
        let label = choices(store)
            .into_iter()
            .find(|choice| choice.key == key_of(&provider, profile))
            .map_or_else(|| provider.to_string(), |choice| choice.label);
        let Some(connection) = &self.connection else {
            self.worker_not_ready("OpenPullRequestWithAgent");
            return;
        };
        match connection.open_pull_request_with_agent(
            workspace,
            provider,
            profile,
            self.open_pr.as_draft,
        ) {
            Ok(()) => {
                self.open_pr.started(workspace, label);
                self.close_center();
            }
            Err(error) => self.open_pr.refused = Some((workspace, error)),
        }
    }

    /// Stops the drafting agent; the daemon then reports the run as failed.
    /// With no agent left to stop, the run is only forgotten here.
    pub fn cancel_open_pr(&mut self, workspace: WorkspaceId) {
        let session = self
            .store
            .as_ref()
            .and_then(|store| drafting_session(store, workspace))
            .map(|session| session.id);
        match (session, &self.connection) {
            (Some(session), Some(connection)) => {
                let _ = connection.kill_session(session);
            }
            _ => {
                self.open_pr.pending.remove(&workspace);
            }
        }
    }

    pub fn reconcile_open_pr(&mut self, now: Instant) {
        let Some(store) = &self.store else {
            return;
        };
        if self.open_pr.reconcile(store, now) > 0 {
            self.toasts.neutral(
                "A pull request run ended while disconnected",
                Some("Check the Pull Requests view for its outcome.".into()),
            );
        }
    }

    pub fn open_pr_finished(
        &mut self,
        workspace: WorkspaceId,
        url: Option<String>,
        error: Option<String>,
        session: Option<SessionId>,
    ) {
        match self.open_pr.finished(workspace, url, error, session) {
            Some(Outcome::Opened(url)) => self.toasts.success("Pull request opened", Some(url)),
            Some(Outcome::Failed { error, .. }) => {
                self.toasts.danger("The pull request was not opened", error);
            }
            None => {}
        }
    }
}

pub fn view(s: &AppState, k: &Kit) -> El {
    let workspace = s.workbench.workspace;
    let diff = s.workbench.changes.diff();
    let store = s.store.as_ref();
    let status = workspace.and_then(|id| {
        store?
            .workspaces
            .iter()
            .find(|row| row.id == id)
            .map(|row| row.status.clone())
    });
    let branch = diff
        .as_ref()
        .and_then(|diff| diff.branch.clone())
        .unwrap_or_else(|| "detached".to_string());
    let pending = workspace.and_then(|id| s.open_pr.pending.get(&id));

    let head = row()
        .key("open-pr-head")
        .w_full()
        .h(36.0)
        .px(12.0)
        .gap(10.0)
        .items_center()
        .child(
            text("Open a pull request")
                .font(k.ty.small_medium)
                .color(k.c.text_base),
        )
        .child(
            text(branch)
                .font(k.ty.text_xsmall)
                .color(k.c.text_faint)
                .grow(1.0)
                .truncate(),
        )
        .child(
            Button::new("Close")
                .variant(ButtonVariant::GhostMuted)
                .control_size(ControlSize::Small)
                .on_press(|s: &mut AppState| s.close_center())
                .build(k)
                .key("open-pr-close"),
        );

    let mut changes = col()
        .key("open-pr-changes")
        .w_full()
        .gap(crate::ds::SPACE_2);
    let ahead = status.as_ref().and_then(|status| status.ahead).unwrap_or(0);
    match &diff {
        None if s.workbench.changes.error.is_some() => {
            changes = changes.child(note(
                k,
                s.workbench.changes.error.clone().unwrap_or_default(),
                true,
            ));
        }
        None => changes = changes.child(note(k, "Reading the working tree…".into(), false)),
        Some(diff) if diff.files.is_empty() => {
            changes = changes.child(note(
                k,
                "Nothing uncommitted: the pull request is the commits on this branch.".into(),
                false,
            ));
        }
        Some(diff) => {
            changes = changes.child(
                text(format!("Uncommitted · {}", diff_summary(diff)))
                    .font(k.ty.small_medium)
                    .color(k.c.text_base),
            );
            for file in diff.files.iter().take(LISTED_FILES) {
                changes = changes.child(
                    row()
                        .w_full()
                        .gap(crate::ds::SPACE_4)
                        .items_center()
                        .child(
                            text(file.status.letter())
                                .font(k.ty.mono)
                                .color(k.c.text_faint)
                                .w(14.0),
                        )
                        .child(
                            text(file.path.clone())
                                .font(k.ty.mono)
                                .color(k.c.text_muted)
                                .grow(1.0)
                                .truncate(),
                        )
                        .child(
                            text(format!("+{} −{}", file.additions, file.deletions))
                                .font(k.ty.text_xsmall)
                                .color(k.c.text_faint),
                        ),
                );
            }
            if diff.files.len() > LISTED_FILES {
                changes = changes.child(note(
                    k,
                    format!("and {} more", diff.files.len() - LISTED_FILES),
                    false,
                ));
            }
        }
    }
    if ahead > 0 {
        changes = changes.child(note(
            k,
            format!(
                "{ahead} commit{} not yet pushed go in too.",
                if ahead == 1 { "" } else { "s" }
            ),
            false,
        ));
    }

    let offered = store.map(choices).unwrap_or_default();
    let selected = store
        .and_then(|store| resolve(store, s.open_pr.agent_key.as_deref()).ok())
        .map(|(provider, profile)| key_of(&provider, profile));
    let mut picker = row()
        .key("open-pr-agents")
        .w_full()
        .gap(crate::ds::SPACE_2)
        .flex_wrap();
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
                .on_press(move |s: &mut AppState| s.open_pr.agent_key = Some(key.clone()))
                .build(k)
                .key(format!("open-pr-agent-{}", choice.key)),
        );
    }
    if offered.is_empty() {
        picker = picker.child(note(k, "No installed agent takes a prompt.".into(), true));
    }

    let refused = s
        .open_pr
        .refused
        .as_ref()
        .filter(|(id, _)| Some(*id) == workspace)
        .map(|(_, message)| message.clone());
    let blocked = diff.is_none() || offered.is_empty() || pending.is_some();
    let confirm = Button::new(if pending.is_some() {
        "Drafting…"
    } else {
        "Draft & open PR"
    })
    .variant(ButtonVariant::Contrast)
    .disabled(blocked)
    .on_press(|s: &mut AppState| s.confirm_open_pr())
    .build(k)
    .key("open-pr-confirm");

    let explain = if diff.as_ref().is_some_and(|diff| !diff.files.is_empty()) {
        "The agent writes the title, description and commit message in the background. Forge \
         then commits everything uncommitted (on a new branch when this is the default \
         branch), pushes, and opens the pull request."
    } else {
        "The agent writes the title and description in the background. Forge then pushes this \
         branch and opens the pull request."
    };

    col()
        .key("open-pr")
        .w_full()
        .h_full()
        .min_h(0.0)
        .child(head)
        .child(
            col()
                .key("open-pr-scroll")
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .padding(16.0)
                .gap(14.0)
                .scroll(ScrollSpec {
                    stick_bottom: false,
                })
                .child(changes)
                .child(text("Agent").font(k.ty.text_xsmall).color(k.c.text_faint))
                .child(picker)
                .child(
                    Checkbox::new(s.open_pr.as_draft)
                        .label("Open as a draft pull request")
                        .on_toggle(|s: &mut AppState| s.open_pr.as_draft = !s.open_pr.as_draft)
                        .build(k)
                        .key("open-pr-as-draft"),
                )
                .child(note(k, explain.into(), false))
                .child(
                    row()
                        .gap(crate::ds::SPACE_4)
                        .items_center()
                        .child(
                            text("Its instructions are [pull_requests] in config.toml.")
                                .font(k.ty.text_xsmall)
                                .color(k.c.text_faint),
                        )
                        .child(
                            Button::new("View prompt")
                                .variant(ButtonVariant::GhostMuted)
                                .control_size(ControlSize::Small)
                                .on_press(|s: &mut AppState| {
                                    s.open_settings(Some(crate::settings::Section::Config));
                                })
                                .build(k)
                                .key("open-pr-view-prompt"),
                        ),
                )
                .child(
                    row()
                        .gap(crate::ds::SPACE_5)
                        .items_center()
                        .child(confirm)
                        .children(refused.map(|message| {
                            text(message)
                                .key("open-pr-refused")
                                .role("status")
                                .font(k.ty.text_xsmall)
                                .color(k.c.warning.fg)
                                .grow(1.0)
                        })),
                ),
        )
}

fn note(k: &Kit, words: String, warn: bool) -> El {
    text(words)
        .font(k.ty.text_xsmall)
        .color(if warn { k.c.warning.fg } else { k.c.text_faint })
}

/// The Git panel's line while an agent drafts for this checkout.
pub fn progress(s: &AppState, k: &Kit, workspace: WorkspaceId) -> Option<El> {
    let pending = s.open_pr.pending.get(&workspace)?;
    let asking = s
        .store
        .as_ref()
        .and_then(|store| drafting_session(store, workspace))
        .filter(|session| session.activity.state == domain::ActivityState::Waiting)
        .map(|session| session.id);
    let words = if asking.is_some() {
        format!("{} needs your approval to go on", pending.agent)
    } else {
        format!("{} is drafting the pull request…", pending.agent)
    };
    Some(
        row()
            .key("open-pr-progress")
            .role("status")
            .w_full()
            .gap(crate::ds::SPACE_4)
            .items_center()
            .child(
                text(words)
                    .font(k.ty.text_xsmall)
                    .color(if asking.is_some() {
                        k.c.warning.fg
                    } else {
                        k.c.text_muted
                    })
                    .grow(1.0)
                    .truncate(),
            )
            .children(asking.map(|session| {
                Button::new("Show")
                    .variant(ButtonVariant::Outline)
                    .control_size(ControlSize::Small)
                    .on_press(move |s: &mut AppState| {
                        s.select(session);
                        s.show_mode(crate::workbench::CenterMode::Session);
                    })
                    .build(k)
                    .key("open-pr-show-agent")
            }))
            .child(
                Button::new("Cancel")
                    .variant(ButtonVariant::GhostMuted)
                    .control_size(ControlSize::Small)
                    .on_press(move |s: &mut AppState| s.cancel_open_pr(workspace))
                    .build(k)
                    .key("open-pr-cancel"),
            ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_run_this_window_started_raises_an_outcome() {
        let mut open = OpenPr::default();
        let workspace = WorkspaceId::new();
        assert_eq!(
            open.finished(workspace, Some("https://x/1".into()), None, None),
            None,
            "a pull request forgectl opened is not ours to announce"
        );
        open.started(workspace, "Claude".into());
        assert_eq!(
            open.finished(workspace, Some("https://x/1".into()), None, None),
            Some(Outcome::Opened("https://x/1".into()))
        );
        assert!(open.pending.is_empty());
        open.started(workspace, "Claude".into());
        let session = SessionId::new();
        assert_eq!(
            open.finished(workspace, None, Some("timed out".into()), Some(session)),
            Some(Outcome::Failed {
                error: "timed out".into(),
                session: Some(session)
            })
        );
    }

    #[test]
    fn a_snapshot_forgets_a_run_whose_agent_is_gone_after_the_launch_grace() {
        let store = crate::connection::fixture_store(&crate::bench::fixture_path()).unwrap();
        let mut open = OpenPr::default();
        let workspace = WorkspaceId::new();
        open.started(workspace, "Claude".into());
        let now = Instant::now();
        assert_eq!(open.reconcile(&store, now), 0, "still launching");
        assert_eq!(open.reconcile(&store, now + LAUNCH_GRACE), 1);
        assert!(open.pending.is_empty());
    }

    #[test]
    fn a_refusal_clears_the_pending_run_and_says_why() {
        let mut open = OpenPr::default();
        let workspace = WorkspaceId::new();
        let other = WorkspaceId::new();
        open.started(workspace, "Claude".into());
        open.started(other, "Claude".into());
        open.refused(workspace, "nothing to propose");
        assert_eq!(
            open.pending.keys().collect::<Vec<_>>(),
            [&other],
            "the other checkout's run is untouched"
        );
        assert_eq!(
            open.refused,
            Some((workspace, "nothing to propose".to_string()))
        );
    }
}
