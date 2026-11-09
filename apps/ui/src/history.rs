//! The sidebar's History: agent runs recorded on disk (the snapshot's
//! `external_agents`), as Tauri's `HistoryPanel` lists them. Resume is the
//! ordinary agent launch told which conversation to re-enter, under the
//! account that recorded it.

use domain::{ExternalAgentSession, ProjectId, WorkspaceId};
use rune::{El, ScrollSpec, TextEdit, col, icon, row, text};
use rune_kit::{Button, ButtonVariant, ControlSize, Kit, SegmentedControl};

use crate::app::AppState;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Scope {
    #[default]
    Checkout,
    Project,
    Everywhere,
}

const SCOPES: [(Scope, &str); 3] = [
    (Scope::Checkout, "Checkout"),
    (Scope::Project, "Project"),
    (Scope::Everywhere, "All"),
];

#[derive(Default)]
pub struct History {
    pub scope: Scope,
    pub query: TextEdit,
    /// Store positions, newest first, with each run's lowered search text;
    /// built when the store changes so a keystroke only filters.
    pub index: Vec<(usize, String)>,
}

/// The runs newest first, each with what a filter matches against: title,
/// branch and provider, lowered.
#[must_use]
pub fn index(sessions: &[ExternalAgentSession]) -> Vec<(usize, String)> {
    let mut index: Vec<(usize, String)> = sessions
        .iter()
        .enumerate()
        .map(|(at, session)| {
            let haystack = format!(
                "{} {} {}",
                session.title,
                session.branch.as_deref().unwrap_or_default(),
                session.provider
            )
            .to_lowercase();
            (at, haystack)
        })
        .collect();
    index.sort_by(|a, b| {
        sessions[b.0]
            .last_activity
            .cmp(&sessions[a.0].last_activity)
    });
    index
}

/// The store positions shown for a scope and query, as `HistoryPanel`
/// filters: every whitespace-separated term must appear.
#[must_use]
pub fn shown(
    sessions: &[ExternalAgentSession],
    index: &[(usize, String)],
    scope: Scope,
    workspace: Option<WorkspaceId>,
    project: Option<ProjectId>,
    query: &str,
) -> Vec<usize> {
    let terms: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    index
        .iter()
        .filter(|(at, haystack)| {
            let session = &sessions[*at];
            let in_scope = match scope {
                Scope::Checkout => workspace.is_some() && session.workspace_id == workspace,
                Scope::Project => project == Some(session.project_id),
                Scope::Everywhere => true,
            };
            in_scope && terms.iter().all(|term| haystack.contains(term.as_str()))
        })
        .map(|(at, _)| *at)
        .collect()
}

impl AppState {
    /// Why a run cannot be resumed, or `None` when it can: its account must
    /// still exist and its provider be installed and offered.
    #[must_use]
    pub fn resume_blocked(&self, session: &ExternalAgentSession) -> Option<String> {
        let store = self.store.as_ref()?;
        if let Some(profile) = session.profile_id
            && !store.agent_profiles.iter().any(|item| item.id == profile)
        {
            return Some("The profile that recorded this run no longer exists".into());
        }
        let installed = self
            .installed_agents()
            .iter()
            .any(|(id, _)| id.as_str() == session.provider);
        (!installed).then(|| format!("{} is not installed", session.provider))
    }

    pub fn resume_run(&mut self, at: usize) {
        let Some(store) = &self.store else {
            return;
        };
        let Some(session) = store.external_agents.get(at) else {
            return;
        };
        if self.resume_blocked(session).is_some() {
            return;
        }
        // A run recorded outside any known checkout resumes in its project's
        // first one, so the provider can find its transcript by project.
        let workspace = session.workspace_id.or_else(|| {
            crate::projects::checkouts_of(&store.workspaces, session.project_id)
                .first()
                .map(|workspace| workspace.id)
        });
        let (Some(workspace), Some(connection)) = (workspace, &self.connection) else {
            return;
        };
        let provider = domain::AgentProviderId::new(session.provider.as_str());
        if connection
            .resume_agent(
                workspace,
                provider,
                session.profile_id,
                session.session_id.clone(),
            )
            .is_ok()
        {
            self.chosen_workspace = Some(workspace);
            self.show_mode(crate::workbench::CenterMode::Session);
        }
    }
}

pub fn view(s: &AppState, k: &Kit) -> El {
    let history = &s.history;
    let labels: Vec<&str> = SCOPES.iter().map(|(_, label)| *label).collect();
    let selected = SCOPES.iter().position(|(scope, _)| *scope == history.scope);
    let scopes = SegmentedControl::new(&labels, selected)
        .label("Scope")
        .fit()
        .on_pick(|s: &mut AppState, index: usize, _tx| {
            if let Some((scope, _)) = SCOPES.get(index) {
                s.history.scope = *scope;
            }
        })
        .build(k)
        .key("history-scope");
    let filter = rune_kit::TextInput::new(&history.query)
        .placeholder("Filter history…")
        .label("Filter history")
        .on_edit(|s: &mut AppState| &mut s.history.query)
        .width(None)
        .build(k)
        .key("history-filter")
        .w_full()
        .min_w(0.0);
    let mut list = col().key("history-list").w_full().gap(6.0);
    let rows = s.store.as_ref().map_or_else(Vec::new, |store| {
        let project = s.workbench.workspace.and_then(|workspace| {
            store
                .workspaces
                .iter()
                .find(|candidate| candidate.id == workspace)
                .map(|found| found.project_id)
        });
        shown(
            &store.external_agents,
            &history.index,
            history.scope,
            s.workbench.workspace,
            project,
            &history.query.text,
        )
    });
    if rows.is_empty() {
        list = list.child(
            text("No agent runs recorded here.")
                .font(k.ty.small)
                .color(k.c.text_faint),
        );
    }
    let profiles = s
        .store
        .as_ref()
        .map_or(&[][..], |store| store.agent_profiles.as_slice());
    for at in rows.into_iter().take(200) {
        let Some(session) = s
            .store
            .as_ref()
            .and_then(|store| store.external_agents.get(at))
        else {
            continue;
        };
        let mut meta: Vec<String> = Vec::new();
        if let Some(branch) = &session.branch {
            meta.push(branch.clone());
        }
        if let Some(profile) = session
            .profile_id
            .and_then(|id| profiles.iter().find(|item| item.id == id))
        {
            meta.push(profile.name.clone());
        }
        meta.push(format!("{} turns", session.message_count));
        if session.subagent_count > 0 {
            meta.push(format!("{} subagents", session.subagent_count));
        }
        let blocked = s.resume_blocked(session);
        let mut resume = Button::new("Resume")
            .variant(ButtonVariant::Outline)
            .control_size(ControlSize::Small)
            .disabled(blocked.is_some());
        if blocked.is_none() {
            resume = resume.on_press(move |s: &mut AppState| s.resume_run(at));
        }
        let mut card = col()
            .key(format!("history-{}", session.session_id))
            .role("group")
            .label(session.title.clone())
            .description(blocked.clone().unwrap_or_default())
            .w_full()
            .gap(3.0)
            .padding(8.0)
            .radius(6.0)
            .border(1.0, k.c.border_weak)
            .child(
                row()
                    .gap(6.0)
                    .items_center()
                    .child(icon(&crate::glyphs::AGENT, 13.0).color(k.c.icon_muted))
                    .child(
                        text(session.title.clone())
                            .font(k.ty.small_medium)
                            .color(k.c.text_base)
                            .truncate()
                            .min_w(0.0)
                            .grow(1.0),
                    )
                    .child(
                        text(session.provider.clone())
                            .font(k.ty.text_xsmall)
                            .color(k.c.text_faint),
                    ),
            )
            .child(
                text(meta.join(" · "))
                    .font(k.ty.text_xsmall)
                    .color(k.c.text_faint)
                    .truncate(),
            );
        if let Some(preview) = &session.preview {
            card = card.child(
                text(preview.clone())
                    .font(k.ty.text_xsmall)
                    .color(k.c.text_muted)
                    .truncate(),
            );
        }
        list = list.child(card.child(row().child(resume.build(k).key(format!("resume-{at}")))));
    }
    col()
        .key("history")
        .w_full()
        .h_full()
        .min_h(0.0)
        .gap(8.0)
        .child(text("History").font(k.ty.small_medium).color(k.c.text_base))
        .child(scopes)
        .child(filter)
        .child(
            col()
                .key("history-scroll")
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .scroll(ScrollSpec {
                    stick_bottom: false,
                })
                .child(list),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(
        title: &str,
        project: ProjectId,
        workspace: Option<WorkspaceId>,
        at: &str,
    ) -> ExternalAgentSession {
        serde_json::from_value(serde_json::json!({
            "session_id": title, "project_id": project, "workspace_id": workspace,
            "provider": "claude", "profile_id": null, "title": title, "branch": "topic",
            "preview": null, "model": null, "message_count": 4, "subagent_count": 0,
            "transcript_path": "/tmp/t.jsonl",
            "started_at": at, "last_activity": at
        }))
        .expect("a recorded run")
    }

    #[test]
    fn history_is_newest_first_scoped_and_filtered_by_every_term() {
        let (project, other) = (ProjectId::new(), ProjectId::new());
        let (here, there) = (WorkspaceId::new(), WorkspaceId::new());
        let sessions = vec![
            run(
                "Fix the parser",
                project,
                Some(here),
                "2026-10-01T00:00:00Z",
            ),
            run("Add tests", project, Some(there), "2026-10-03T00:00:00Z"),
            run("Elsewhere work", other, None, "2026-10-02T00:00:00Z"),
        ];
        let index = index(&sessions);
        let titles = |rows: Vec<usize>| -> Vec<&str> {
            rows.into_iter()
                .map(|at| sessions[at].title.as_str())
                .collect()
        };
        assert_eq!(
            titles(shown(
                &sessions,
                &index,
                Scope::Everywhere,
                Some(here),
                Some(project),
                ""
            )),
            vec!["Add tests", "Elsewhere work", "Fix the parser"],
            "newest first"
        );
        assert_eq!(
            titles(shown(
                &sessions,
                &index,
                Scope::Checkout,
                Some(here),
                Some(project),
                ""
            )),
            vec!["Fix the parser"]
        );
        assert_eq!(
            titles(shown(
                &sessions,
                &index,
                Scope::Project,
                Some(here),
                Some(project),
                ""
            )),
            vec!["Add tests", "Fix the parser"]
        );
        assert_eq!(
            titles(shown(
                &sessions,
                &index,
                Scope::Everywhere,
                None,
                None,
                "PARSER fix"
            )),
            vec!["Fix the parser"],
            "every term, any case"
        );
        assert!(
            shown(&sessions, &index, Scope::Checkout, None, None, "").is_empty(),
            "no checkout, nothing scoped to one"
        );
    }
}
