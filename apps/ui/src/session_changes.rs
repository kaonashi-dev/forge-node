//! One session's changes since its own baseline: a read the person asks for,
//! never a subscription.

use domain::{BaseOrigin, SessionChanges, SessionId, Timestamp, WorkspaceId};
use rune::{El, ScrollSpec, col, el, row, text};
use rune_kit::{Button, ButtonVariant, ControlSize, Kit};

use crate::app::AppState;
use crate::workbench::CenterMode;

#[derive(Default)]
pub struct SessionChangesView {
    pub session: Option<SessionId>,
    pub changes: Option<SessionChanges>,
    pub loading: bool,
    pub error: Option<String>,
    pub read_at: Option<Timestamp>,
}

/// What a header says about a base the reader did not choose.
#[must_use]
pub fn base_note(origin: BaseOrigin, base: Option<&str>) -> Option<&'static str> {
    match origin {
        BaseOrigin::Recorded => None,
        BaseOrigin::Unreachable => {
            Some("The recorded baseline no longer resolves — showing changes against HEAD.")
        }
        _ if base.is_none() => Some("No baseline was recorded — showing uncommitted changes."),
        _ => None,
    }
}

fn commits_heading(count: u32) -> String {
    format!("{count} commit{}", if count == 1 { "" } else { "s" })
}

fn workspace_of(s: &AppState, session: SessionId) -> Option<WorkspaceId> {
    s.store
        .as_ref()?
        .sessions
        .iter()
        .find(|candidate| candidate.id == session)
        .map(|candidate| candidate.workspace_id)
}

pub fn open(s: &mut AppState, session: SessionId) {
    let same = s.session_changes.session == Some(session);
    s.session_changes.session = Some(session);
    if !same {
        s.session_changes.changes = None;
        s.session_changes.error = None;
        s.session_changes.read_at = None;
    }
    if let Some(workspace) = workspace_of(s, session) {
        s.chosen_workspace = Some(workspace);
    }
    s.show_mode(CenterMode::SessionChanges);
    reload(s);
}

pub fn toggle(s: &mut AppState, session: SessionId) {
    if s.workbench.mode == CenterMode::SessionChanges && s.session_changes.session == Some(session)
    {
        s.workbench.show(CenterMode::Session);
        return;
    }
    open(s, session);
}

pub fn reload(s: &mut AppState) {
    let Some(session) = s.session_changes.session else {
        return;
    };
    s.session_changes.loading = true;
    s.session_changes.error = None;
    let result = match &s.connection {
        Some(connection) => connection.session_changes(s.generation, session),
        None => Err("The client worker is still starting.".to_string()),
    };
    if let Err(error) = result {
        s.session_changes.loading = false;
        s.session_changes.error = Some(error);
    }
}

pub fn apply(
    s: &mut AppState,
    session: SessionId,
    result: Result<domain::SessionChanges, String>,
) -> bool {
    if s.session_changes.session != Some(session) {
        return false;
    }
    s.session_changes.loading = false;
    match result {
        Ok(changes) => {
            s.session_changes.error = None;
            s.session_changes.changes = Some(changes);
            s.session_changes.read_at = Some(Timestamp::now());
        }
        Err(error) => s.session_changes.error = Some(error),
    }
    true
}

fn header(s: &AppState, k: &Kit) -> El {
    let view = &s.session_changes;
    let summary = view.changes.as_ref().map(|changes| &changes.summary);
    let (additions, deletions) = summary.map_or((0, 0), domain::ChangeSummary::totals);
    let branch = summary
        .and_then(|summary| summary.branch.clone())
        .unwrap_or_else(|| "detached".to_string());
    let base = summary.and_then(|summary| summary.base.clone());
    let mut head = row()
        .key("session-changes-head")
        .w_full()
        .h(38.0)
        .px(12.0)
        .gap(10.0)
        .items_center()
        .child(
            text("Session changes")
                .font(k.ty.small_medium)
                .color(k.c.text_base),
        )
        .child(
            text(branch)
                .font(k.ty.mono)
                .color(k.c.text_muted)
                .truncate()
                .min_w(0.0),
        );
    if let Some(base) = base {
        head = head.child(
            text(format!("base {base}"))
                .font(k.ty.text_xsmall)
                .color(k.c.text_faint),
        );
    }
    head = head
        .child(el().grow(1.0))
        .child(
            text(format!("+{additions}"))
                .font(k.ty.text_xsmall)
                .color(crate::theme::roles(k).git.added),
        )
        .child(
            text(format!("−{deletions}"))
                .font(k.ty.text_xsmall)
                .color(crate::theme::roles(k).git.deleted),
        )
        .child(
            Button::new("Refresh")
                .variant(ButtonVariant::GhostMuted)
                .control_size(ControlSize::Small)
                .disabled(view.loading)
                .on_press(|s: &mut AppState| reload(s))
                .build(k)
                .key("session-changes-refresh"),
        );
    head
}

pub fn view(s: &AppState, k: &Kit) -> El {
    let view = &s.session_changes;
    let workspace = view.session.and_then(|session| workspace_of(s, session));
    let mut body = col()
        .key("session-changes")
        .w_full()
        .gap(crate::ds::SPACE_4);
    if view.session.is_none() {
        body = body.child(
            text("Open a session and choose Session Changes.")
                .font(k.ty.small)
                .color(k.c.text_faint),
        );
    } else if let Some(error) = &view.error {
        body = body.child(
            text(format!("Could not read this session's changes: {error}"))
                .key("session-changes-error")
                .role("alert")
                .font(k.ty.small)
                .color(k.c.danger.fg),
        );
    }
    if let Some(changes) = &view.changes {
        if let Some(note) = base_note(changes.origin, changes.summary.base.as_deref()) {
            body = body.child(
                text(note)
                    .key("session-changes-base-note")
                    .font(k.ty.text_xsmall)
                    .color(k.c.text_faint),
            );
        }
        if changes.sharing_sessions > 0 {
            body = body.child(
                text(format!(
                    "{} other session{} writing this checkout, so some of this may not be theirs.",
                    changes.sharing_sessions,
                    if changes.sharing_sessions == 1 {
                        " is"
                    } else {
                        "s are"
                    }
                ))
                .key("session-changes-sharing")
                .font(k.ty.text_xsmall)
                .color(k.c.warning.fg),
            );
        }
        if changes.summary.commit_count > 0 {
            let mut commits = col()
                .key("session-changes-commits")
                .w_full()
                .gap(crate::ds::SPACE_1)
                .child(
                    text(commits_heading(changes.summary.commit_count))
                        .font(k.ty.small_medium)
                        .color(k.c.text_muted),
                );
            for commit in &changes.summary.commits {
                commits = commits.child(
                    row()
                        .w_full()
                        .gap(crate::ds::SPACE_4)
                        .child(
                            text(commit.short_id.clone())
                                .font(k.ty.mono)
                                .color(k.c.text_faint),
                        )
                        .child(
                            text(commit.subject.clone())
                                .font(k.ty.small)
                                .color(k.c.text_base)
                                .truncate()
                                .min_w(0.0),
                        ),
                );
            }
            let hidden = changes.summary.commit_count as usize - changes.summary.commits.len();
            if hidden > 0 {
                commits = commits.child(
                    text(format!("…and {hidden} more."))
                        .font(k.ty.text_xsmall)
                        .color(k.c.text_faint),
                );
            }
            body = body.child(commits);
        }
        if changes.summary.files.is_empty() {
            body = body.child(
                text("Nothing changed since this session started.")
                    .key("session-changes-empty")
                    .font(k.ty.small)
                    .color(k.c.text_faint),
            );
        } else {
            let mut files = col().key("session-changes-files").w_full().gap(1.0);
            for file in &changes.summary.files {
                let path = file.path.clone();
                let label = format!(
                    "{}, {}, {} added, {} removed",
                    file.path,
                    crate::workbench::view::status_word(file.status),
                    file.additions,
                    file.deletions
                );
                let mut counts = row().gap(crate::ds::SPACE_3);
                if file.binary {
                    counts = counts.child(text("bin").font(k.ty.text_xsmall).color(k.c.text_faint));
                } else {
                    counts = counts
                        .child(
                            text(format!("+{}", file.additions))
                                .font(k.ty.text_xsmall)
                                .color(crate::theme::roles(k).git.added),
                        )
                        .child(
                            text(format!("−{}", file.deletions))
                                .font(k.ty.text_xsmall)
                                .color(crate::theme::roles(k).git.deleted),
                        );
                }
                files = files.child(
                    row()
                        .key(format!("session-change-{}", file.path))
                        .role("button")
                        .label(label)
                        .w_full()
                        .h(26.0)
                        .px(8.0)
                        .gap(crate::ds::SPACE_4)
                        .radius(k.radius.sm)
                        .items_center()
                        .hover(k.c.hover)
                        .on_press_tx(move |s: &mut AppState, tx| {
                            if let Some(workspace) = workspace {
                                s.chosen_workspace = Some(workspace);
                                s.sync_workbench_workspace();
                            }
                            s.reveal_in_files(path.clone(), tx);
                        })
                        .child(
                            text(crate::workbench::view::status_letter(file.status))
                                .font(k.ty.mono)
                                .color(if file.status == domain::DiffStatus::Conflicted {
                                    crate::theme::roles(k).git.conflict
                                } else {
                                    k.c.text_muted
                                })
                                .w(12.0),
                        )
                        .child(
                            text(file.path.clone())
                                .font(k.ty.small)
                                .color(k.c.text_base)
                                .truncate()
                                .grow(1.0)
                                .min_w(0.0),
                        )
                        .child(counts),
                );
            }
            body = body.child(files);
        }
        if changes.summary.truncated {
            body = body.child(
                text("Some entries were over budget and are not listed.")
                    .key("session-changes-truncated")
                    .font(k.ty.text_xsmall)
                    .color(k.c.text_faint),
            );
        }
    } else if view.loading {
        body = body.child(
            text("Reading this session's changes…")
                .font(k.ty.small)
                .color(k.c.text_faint),
        );
    }
    if let Some(read_at) = view.read_at {
        let at = read_at.as_offset();
        body = body.child(
            text(format!("as of {:02}:{:02}", at.hour(), at.minute()))
                .key("session-changes-read-at")
                .font(k.ty.text_xsmall)
                .color(k.c.text_faint),
        );
    }
    col()
        .key("session-changes-view")
        .w_full()
        .h_full()
        .min_h(0.0)
        .child(header(s, k))
        .child(
            col()
                .key("session-changes-scroll")
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .px(12.0)
                .py(6.0)
                .scroll(ScrollSpec {
                    stick_bottom: false,
                })
                .child(body),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_view_names_the_sharing_caveat_and_the_file_rows() {
        use crate::connection::{Metrics, fixture_store};
        use crate::terminal::SharedTerminal;
        use rune::{Env, Runtime};
        use std::sync::Arc;

        let store = fixture_store(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/snapshot.json"),
        )
        .expect("fixture snapshot decodes");
        let session = store.sessions[0].id;
        let mut state =
            AppState::for_bench(store, Arc::new(Metrics::default()), SharedTerminal::new());
        open(&mut state, session);
        let changes = SessionChanges {
            session_id: session,
            origin: BaseOrigin::Missing,
            summary: domain::ChangeSummary {
                branch: Some("topic".into()),
                base: Some("abc1234".into()),
                files: vec![domain::ChangeSummaryFile {
                    path: "src/lib.rs".into(),
                    status: domain::DiffStatus::Modified,
                    additions: 3,
                    deletions: 1,
                    binary: false,
                }],
                commits: vec![domain::CommitLine {
                    short_id: "abc1234".into(),
                    subject: "Change it".into(),
                }],
                commit_count: 1,
                truncated: false,
            },
            sharing_sessions: 2,
        };
        assert!(apply(&mut state, session, Ok(changes)));
        let mut rt = Runtime::new(
            state,
            crate::ui::view,
            (1180.0, 760.0),
            Env {
                system_dark: true,
                scale: 1.0,
            },
        );
        rt.frame(0.0);
        rt.frame(1.0 / 60.0);
        let keys: Vec<String> = rt.elements().into_iter().filter_map(|e| e.key).collect();
        assert!(
            keys.iter().any(|key| key == "session-change-src/lib.rs"),
            "{keys:?}"
        );
        assert!(keys.iter().any(|key| key == "session-changes-sharing"));
        assert!(keys.iter().any(|key| key == "session-changes-commits"));
    }

    #[test]
    fn a_base_note_names_only_what_the_reader_did_not_choose() {
        assert_eq!(base_note(BaseOrigin::Recorded, Some("abc1234")), None);
        assert_eq!(
            base_note(BaseOrigin::Unreachable, Some("abc1234")),
            Some("The recorded baseline no longer resolves — showing changes against HEAD.")
        );
        assert_eq!(
            base_note(BaseOrigin::Missing, None),
            Some("No baseline was recorded — showing uncommitted changes.")
        );
        assert_eq!(commits_heading(1), "1 commit");
        assert_eq!(commits_heading(3), "3 commits");
    }
}
