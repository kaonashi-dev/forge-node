//! Pull requests, and reviewing one with an agent. A review is a read twice
//! guarded: the launch is read-only, which applies the provider's own review
//! flags, and the recipe from `domain::pr_review` forbids editing, committing
//! and posting.

use client::ProviderInfo;
use domain::{AgentProviderId, ProjectId, PullRequest, Workspace, WorkspaceId, WorkspaceKind};
use rune::{El, ScrollSpec, TextEdit, col, row, text};
use rune_kit::{Button, ButtonVariant, ControlSize, Kit, TextInput};

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

fn tone_ink(k: &Kit, tone: crate::pr_filters::Tone) -> rune::Color {
    let roles = crate::theme::roles(k);
    match tone {
        crate::pr_filters::Tone::Neutral => k.c.text_muted,
        crate::pr_filters::Tone::Success => roles.git.added,
        tone => tone.ink(k, &roles).fg,
    }
}

pub fn view(s: &AppState, k: &Kit) -> El {
    use crate::pr_filters::{self, Scope};
    let refresh = crate::ds::icon_button(
        &crate::glyphs::REFRESH,
        "Refresh pull requests",
        crate::ds::IconButtonSize::Mini,
    )
    .on_press(|s: &mut AppState| s.refresh_pull_requests())
    .build(k)
    .key("prs-refresh");
    let compose = Button::new("New PR")
        .variant(ButtonVariant::Outline)
        .control_size(ControlSize::Small)
        .help("New pull request…")
        .on_press(|s: &mut AppState| {
            if let Some(workspace) = s.workbench.workspace {
                s.open_pr_view(workspace);
            }
        })
        .build(k)
        .key("pr-compose-open");
    let state = s.store.as_ref().map(|store| &store.pull_requests);
    let scope = Scope::parse(s.pref(pr_filters::PR_SCOPE_KEY));
    let project = s.workbench.workspace.and_then(|workspace| {
        s.store
            .as_ref()?
            .workspaces
            .iter()
            .find(|row| row.id == workspace)
            .map(|row| row.project_id)
    });
    let pulls = state.map_or(&[][..], |state| state.pull_requests.as_slice());
    let (mine, others) =
        s.workbench
            .pr_index
            .filter(pulls, scope, s.pr_query.text.as_str(), project);
    let selected = s.workbench.pr_detail.as_deref();
    let card = |pr: &PullRequest| {
        let key = pr_filters::key(pr);
        let chosen = selected == Some(key.as_str());
        let repository = crate::pr_detail::split_repository(&pr.repository)
            .1
            .to_string();
        let decision = pr_filters::decision_label(pr.review_decision.clone());
        let mut meta = row().gap(crate::ds::SPACE_3).items_center().child(
            text(repository)
                .font(k.ty.text_xsmall)
                .color(k.c.text_faint)
                .truncate()
                .min_w(0.0),
        );
        if pr.is_draft {
            meta = meta.child(
                text("Draft".to_string())
                    .font(k.ty.text_xsmall)
                    .color(k.c.text_muted),
            );
        } else if let Some((label, tone)) = decision {
            meta = meta.child(
                text(label.to_string())
                    .font(k.ty.text_xsmall)
                    .color(tone_ink(k, tone)),
            );
        }
        let open = key.clone();
        col()
            .key(format!("pr-row-{key}"))
            .role("button")
            .label(format!("Open pull request #{}: {}", pr.number, pr.title))
            .w_full()
            .gap(crate::ds::SPACE_2)
            .px(8.0)
            .py(6.0)
            .radius(k.radius.md)
            .fill(if chosen {
                k.c.surface_hover
            } else {
                rune::Color::TRANSPARENT
            })
            .hover(k.c.hover)
            .on_press(move |s: &mut AppState| s.open_pr_detail(open.clone()))
            .child(
                row()
                    .gap(crate::ds::SPACE_4)
                    .items_center()
                    .w_full()
                    .child(
                        text(format!("#{}", pr.number))
                            .font(k.ty.mono)
                            .color(k.c.text_faint),
                    )
                    .child(
                        text(pr.title.clone())
                            .font(k.ty.small)
                            .color(k.c.text_base)
                            .grow(1.0)
                            .min_w(0.0)
                            .truncate(),
                    ),
            )
            .child(meta)
    };
    let mut list = col().key("pr-list").w_full().gap(crate::ds::SPACE_2);
    let unread = state.is_none_or(|state| state.refreshed_at.is_none());
    if unread {
        list = list.child(
            text(if s.pr_refreshing {
                "Reading pull requests…"
            } else {
                "Not read yet. Refresh to ask the forge."
            })
            .key("prs-unread")
            .font(k.ty.small)
            .color(k.c.text_faint),
        );
    } else if mine.is_empty() && others.is_empty() {
        list = list.child(
            text(scope.empty_copy().to_string())
                .key("prs-empty")
                .font(k.ty.small)
                .color(k.c.text_faint),
        );
        let projects = s.store.as_ref().map(|store| &store.projects);
        for (index, source) in state
            .map(|state| state.sources.as_slice())
            .unwrap_or_default()
            .iter()
            .enumerate()
        {
            let Some(why) = pr_filters::source_note(&source.status) else {
                continue;
            };
            let name = projects
                .and_then(|projects| projects.iter().find(|p| p.id == source.project_id))
                .map_or_else(|| "A project".to_string(), |project| project.name.clone());
            list = list.child(
                text(format!("{name}: {why}"))
                    .key(format!("prs-source-{index}"))
                    .font(k.ty.text_xsmall)
                    .color(k.c.text_faint),
            );
        }
    }
    for pr in &mine {
        list = list.child(card(pr));
    }
    if !mine.is_empty() && !others.is_empty() {
        list = list.child(
            text("Others".to_string())
                .key("prs-others")
                .font(k.ty.text_xsmall)
                .color(k.c.text_faint)
                .pt(6.0),
        );
    }
    for pr in &others {
        list = list.child(card(pr));
    }
    let mut scopes = row()
        .key("pr-scopes")
        .role("radiogroup")
        .label("Which pull requests")
        .w_full()
        .gap(crate::ds::SPACE_2)
        .flex_wrap();
    for candidate in Scope::ALL {
        let chosen = candidate == scope;
        scopes = scopes.child(
            Button::new(candidate.label())
                .variant(if chosen {
                    ButtonVariant::Outline
                } else {
                    ButtonVariant::GhostMuted
                })
                .control_size(ControlSize::Small)
                .on_press(move |s: &mut AppState| {
                    s.write_pref(pr_filters::PR_SCOPE_KEY, candidate.value());
                })
                .build(k)
                .key(format!("pr-scope-{}", candidate.value())),
        );
    }
    let filter = TextInput::new(&s.pr_query)
        .placeholder("Filter pull requests…")
        .label("Filter pull requests")
        .leading_icon(k.glyph.search)
        .clear_button()
        .width(None)
        .on_edit(|s: &mut AppState| &mut s.pr_query)
        .build(k)
        .key("pr-filter");
    let mut panel = col()
        .key("pull-requests")
        .w_full()
        .h_full()
        .min_h(0.0)
        .gap(crate::ds::SPACE_4)
        .child(
            row()
                .key("prs-bar")
                .w_full()
                .h(30.0)
                .items_center()
                .gap(crate::ds::SPACE_3)
                .child(
                    text("Pull Requests")
                        .font(k.ty.small_medium)
                        .color(k.c.text_base)
                        .grow(1.0)
                        .truncate(),
                )
                .children(s.pr_refreshing.then(|| {
                    text("Refreshing…")
                        .key("prs-refreshing")
                        .role("status")
                        .font(k.ty.text_xsmall)
                        .color(k.c.text_faint)
                }))
                .child(compose)
                .child(refresh),
        )
        .child(filter)
        .child(scopes);
    if let Some(state) = state {
        for (index, failure) in state.failures.iter().enumerate() {
            panel = panel.child(
                text(pr_filters::failure_copy(failure))
                    .key(format!("pr-failure-{index}"))
                    .role("status")
                    .description(failure.message.clone())
                    .font(k.ty.text_xsmall)
                    .color(k.c.warning.fg),
            );
        }
        if state.failures.is_empty()
            && let Some(error) = &state.error
        {
            panel = panel.child(
                text(error.clone())
                    .key("prs-error")
                    .role("status")
                    .font(k.ty.text_xsmall)
                    .color(k.c.warning.fg),
            );
        }
    }
    panel.child(
        col()
            .key("prs-scroll")
            .w_full()
            .grow(1.0)
            .min_h(0.0)
            .scroll(ScrollSpec {
                stick_bottom: false,
            })
            .on_wheel(crate::pages::wheel)
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
        .gap(crate::ds::SPACE_1)
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
        .gap(crate::ds::SPACE_2)
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
    let note = if let Some(error) = &review.error {
        error.clone()
    } else if agents.is_empty() {
        "No installed agent has a read-only review mode.".to_string()
    } else {
        "The agent runs read-only and is told not to edit, commit or post.".to_string()
    };
    body = body.child(picker).child(
        row()
            .gap(crate::ds::SPACE_5)
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
                    .disabled(agents.is_empty())
                    .on_press(|s: &mut AppState| s.start_review())
                    .build(k)
                    .key("review-start"),
            ),
    );
    crate::modal::Modal::new(
        "review",
        format!("Review #{} in {}", review.number, review.repository),
    )
    .width(crate::modal::WIDTH_STANDARD)
    .on_close(|s: &mut AppState| s.review = None)
    .child(body)
    .build(s, k, layer);
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

    #[test]
    fn only_installed_prompt_taking_agents_with_a_review_mode_are_offered() {
        use crate::test_support::provider;
        let providers = vec![
            provider("claude", true, true, true),
            provider("opencode", true, true, false),
            provider("cursor", true, false, true),
            provider("gone", false, true, true),
        ];
        let offered: Vec<String> = review_agents(&providers)
            .into_iter()
            .map(|(id, _)| id.to_string())
            .collect();
        assert_eq!(offered, vec!["claude"]);
    }

    #[test]
    fn an_empty_custom_recipe_has_no_prompt_and_a_recipe_names_the_pull_request() {
        let pr: PullRequest = serde_json::from_value(serde_json::json!({
            "project_id": null, "repository": "o/r", "host": "github.com", "number": 12,
            "title": "Title", "body": "", "body_truncated": false, "url": "https://x/12",
            "author": "a", "base_ref": "main", "head_ref": "b", "is_draft": false,
            "review_decision": null, "labels": [], "assignees": [], "review_requests": [],
            "additions": 1, "deletions": 0, "changed_files": 1, "comment_count": 0,
            "created_at": "2026-11-09T00:00:00Z", "updated_at": "2026-11-09T00:00:00Z",
            "relations": { "assigned": false, "review_requested": false, "authored": false }
        }))
        .expect("a pull request");
        let agents = vec![(AgentProviderId::new("claude"), "Claude".to_string())];
        let mut dialog = ReviewDialog::new(&pr, &agents);
        assert_eq!(
            dialog.agent,
            Some(AgentProviderId::new("claude")),
            "the first agent"
        );
        let prompt = dialog.prompt(&pr).expect("a built-in recipe has a prompt");
        assert!(prompt.contains("12") && prompt.contains("o/r"), "{prompt}");
        dialog.recipe = domain::pr_review::CUSTOM_RECIPE.to_string();
        assert_eq!(
            dialog.prompt(&pr),
            None,
            "an empty custom recipe cannot start"
        );
        dialog.custom = TextEdit::new("Look at the error paths.");
        let custom = dialog.prompt(&pr).expect("a custom recipe with words");
        assert!(custom.contains("Look at the error paths."));
        assert_eq!(
            ReviewDialog::new(&pr, &[]).agent,
            None,
            "no agent, nothing chosen"
        );
    }
}
