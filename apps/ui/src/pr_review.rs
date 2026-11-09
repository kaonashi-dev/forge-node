//! Pull requests, and reviewing one with an agent. A review is a read twice
//! guarded: the launch is read-only, which applies the provider's own review
//! flags, and the recipe from `domain::pr_review` forbids editing, committing
//! and posting. The choice of checkout and agent follows Tauri's `prReview.ts`.

use client::ProviderInfo;
use domain::{AgentProviderId, ProjectId, PullRequest, Workspace, WorkspaceId, WorkspaceKind};
use rune::{El, ScrollSpec, TextEdit, col, row, text};
use rune_kit::{Button, ButtonVariant, ControlSize, Kit};

use crate::app::AppState;

/// The review being set up: which pull request, recipe and agent.
pub struct ReviewDialog {
    pub repository: String,
    pub number: u32,
    pub recipe: String,
    pub custom: TextEdit,
    pub agent: Option<AgentProviderId>,
    pub error: Option<String>,
}

/// Where a review runs: the focused checkout when it belongs to the pull
/// request's project, else that project's main checkout, else any of its own.
#[must_use]
pub fn review_workspace(
    workspaces: &[Workspace],
    project: Option<ProjectId>,
    focused: Option<WorkspaceId>,
) -> Option<WorkspaceId> {
    let project = project?;
    let owned: Vec<&Workspace> = workspaces
        .iter()
        .filter(|workspace| workspace.project_id == project)
        .collect();
    owned
        .iter()
        .find(|workspace| Some(workspace.id) == focused)
        .or_else(|| {
            owned
                .iter()
                .find(|workspace| workspace.kind == WorkspaceKind::Main)
        })
        .or_else(|| owned.first())
        .map(|workspace| workspace.id)
}

/// Installed agents that take a first prompt and declare a review mode; one
/// without such a mode is never offered, so nothing that could write reviews.
#[must_use]
pub fn review_agents(providers: &[ProviderInfo]) -> Vec<(AgentProviderId, String)> {
    providers
        .iter()
        .filter(|info| {
            matches!(
                info.detection.status,
                domain::DetectionStatus::Installed { .. }
            ) && info.descriptor.capabilities.supports_initial_prompt
                && info.descriptor.review.is_some()
        })
        .map(|info| {
            (
                info.descriptor.id.clone(),
                info.descriptor.display_name.clone(),
            )
        })
        .collect()
}

impl ReviewDialog {
    #[must_use]
    pub fn new(pr: &PullRequest, agents: &[(AgentProviderId, String)]) -> Self {
        Self {
            repository: pr.repository.clone(),
            number: pr.number,
            recipe: domain::pr_review::recipe_or_default(None).id,
            custom: TextEdit::new(""),
            agent: agents.first().map(|(id, _)| id.clone()),
            error: None,
        }
    }

    /// The prompt to launch with, or `None` while a Custom recipe is empty.
    #[must_use]
    pub fn prompt(&self, pr: &PullRequest) -> Option<String> {
        let recipe = domain::pr_review::recipe_or_default(Some(&self.recipe));
        let custom = recipe.id == domain::pr_review::CUSTOM_RECIPE;
        if custom && self.custom.text.trim().is_empty() {
            return None;
        }
        let extra = if custom {
            self.custom.text.as_str()
        } else {
            ""
        };
        Some(domain::pr_review::compose_review_prompt(&recipe, pr, extra))
    }
}

fn pr_line(pr: &PullRequest) -> String {
    format!(
        "{} · {} · +{} −{}",
        pr.author, pr.head_ref, pr.additions, pr.deletions
    )
}

pub fn view(s: &AppState, k: &Kit) -> El {
    let refresh = Button::new("Refresh")
        .variant(ButtonVariant::GhostMuted)
        .control_size(ControlSize::Small)
        .on_press(|s: &mut AppState| s.refresh_pull_requests())
        .build(k)
        .key("prs-refresh");
    let mut list = col().key("pr-list").w_full().gap(6.0);
    let state = s.store.as_ref().map(|store| &store.pull_requests);
    if let Some(error) = state.and_then(|state| state.error.as_ref()) {
        list = list.child(text(error.clone()).font(k.ty.small).color(k.c.warning.fg));
    }
    let pulls = state.map_or(&[][..], |state| state.pull_requests.as_slice());
    if pulls.is_empty() {
        list = list.child(
            text(if state.is_some_and(|state| state.refreshed_at.is_some()) {
                "No open pull requests."
            } else {
                "Not read yet. Refresh to ask the forge."
            })
            .font(k.ty.small)
            .color(k.c.text_faint),
        );
    }
    for pr in pulls {
        let (repository, number) = (pr.repository.clone(), pr.number);
        list = list.child(
            row()
                .key(format!("pr-{}-{}", pr.repository, pr.number))
                .role("group")
                .label(crate::projects::pr_label(pr))
                .w_full()
                .gap(10.0)
                .px(10.0)
                .py(6.0)
                .radius(6.0)
                .fill(k.c.surface_hover)
                .items_center()
                .child(
                    col()
                        .grow(1.0)
                        .min_w(0.0)
                        .child(
                            text(crate::projects::pr_label(pr))
                                .font(k.ty.small)
                                .color(k.c.text_base)
                                .truncate(),
                        )
                        .child(
                            text(pr_line(pr))
                                .font(k.ty.text_xsmall)
                                .color(k.c.text_faint)
                                .truncate(),
                        ),
                )
                .child(
                    Button::new("Review…")
                        .variant(ButtonVariant::Outline)
                        .control_size(ControlSize::Small)
                        .on_press(move |s: &mut AppState| s.open_review(&repository, number))
                        .build(k)
                        .key(format!("review-{}-{}", pr.repository, pr.number)),
                ),
        );
    }
    col()
        .key("pull-requests")
        .w_full()
        .h_full()
        .min_h(0.0)
        .child(
            row()
                .key("prs-bar")
                .w_full()
                .h(30.0)
                .items_center()
                .child(
                    text("Pull Requests")
                        .font(k.ty.small_medium)
                        .color(k.c.text_base)
                        .grow(1.0),
                )
                .child(refresh),
        )
        .child(
            col()
                .key("prs-scroll")
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .scroll(ScrollSpec {
                    stick_bottom: false,
                })
                .child(list),
        )
}

pub fn dialog(s: &AppState, k: &Kit, layer: &mut Vec<El>) {
    let Some(review) = &s.review else {
        return;
    };
    let agents = s
        .store
        .as_ref()
        .map(|store| review_agents(&store.providers))
        .unwrap_or_default();
    let mut recipes = col()
        .key("review-recipes")
        .gap(2.0)
        .role("radiogroup")
        .label("Recipe");
    for recipe in domain::pr_review::builtin_recipes() {
        let id = recipe.id.clone();
        recipes = recipes.child(
            Button::new(format!("{} — {}", recipe.name, recipe.detail))
                .variant(if review.recipe == recipe.id {
                    ButtonVariant::Outline
                } else {
                    ButtonVariant::Ghost
                })
                .control_size(ControlSize::Small)
                .on_press(move |s: &mut AppState| {
                    if let Some(review) = s.review.as_mut() {
                        review.recipe = id.clone();
                    }
                })
                .build(k)
                .key(format!("recipe-{}", recipe.id)),
        );
    }
    let mut picker = row()
        .key("review-agents")
        .gap(4.0)
        .role("radiogroup")
        .label("Agent");
    for (id, name) in &agents {
        let chosen = id.clone();
        picker = picker.child(
            Button::new(name.clone())
                .variant(if review.agent.as_ref() == Some(id) {
                    ButtonVariant::Outline
                } else {
                    ButtonVariant::Ghost
                })
                .control_size(ControlSize::Small)
                .on_press(move |s: &mut AppState| {
                    if let Some(review) = s.review.as_mut() {
                        review.agent = Some(chosen.clone());
                    }
                })
                .build(k)
                .key(format!("review-agent-{id}")),
        );
    }
    let mut body = col().gap(10.0).child(recipes);
    if review.recipe == domain::pr_review::CUSTOM_RECIPE {
        body = body.child(
            rune_kit::TextArea::new(&review.custom)
                .label("What to look for")
                .on_edit(|s: &mut AppState| match s.review.as_mut() {
                    Some(review) => &mut review.custom,
                    None => &mut s.scratch_edit,
                })
                .build(k)
                .key("review-custom")
                .h(90.0),
        );
    }
    let note = if agents.is_empty() {
        "No installed agent has a read-only review mode.".to_string()
    } else if let Some(error) = &review.error {
        error.clone()
    } else {
        "The agent runs read-only and is told not to edit, commit or post.".to_string()
    };
    body = body.child(picker).child(
        row()
            .gap(12.0)
            .items_center()
            .child(
                text(note)
                    .key("review-note")
                    .role("status")
                    .font(k.ty.text_xsmall)
                    .color(k.c.text_faint)
                    .grow(1.0),
            )
            .child(
                Button::new("Start review")
                    .variant(ButtonVariant::Contrast)
                    .on_press(|s: &mut AppState| s.start_review())
                    .build(k)
                    .key("review-start"),
            ),
    );
    rune_kit::Dialog::new(
        "review",
        format!("Review #{} in {}", review.number, review.repository),
    )
    .open(true)
    .width(620.0)
    .fit()
    .on_close(|s: &mut AppState| s.review = None)
    .child(body)
    .build(k, layer);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace(project: ProjectId, kind: WorkspaceKind) -> Workspace {
        Workspace {
            id: WorkspaceId::new(),
            project_id: project,
            kind,
            path: "/tmp/x".into(),
            branch: None,
            display_name: None,
            managed_by_app: false,
            created_at: domain::Timestamp::now(),
            status: domain::WorkspaceStatus::default(),
        }
    }

    #[test]
    fn a_review_runs_in_the_focused_checkout_or_the_main_one() {
        let project = ProjectId::new();
        let other = ProjectId::new();
        let main = workspace(project, WorkspaceKind::Main);
        let tree = workspace(project, WorkspaceKind::GitWorktree);
        let elsewhere = workspace(other, WorkspaceKind::Main);
        let all = vec![tree.clone(), main.clone(), elsewhere.clone()];
        assert_eq!(
            review_workspace(&all, Some(project), Some(tree.id)),
            Some(tree.id)
        );
        assert_eq!(
            review_workspace(&all, Some(project), Some(elsewhere.id)),
            Some(main.id),
            "a focused checkout of another project is not where its pull request is reviewed"
        );
        assert_eq!(review_workspace(&all, None, Some(main.id)), None);
    }
}
