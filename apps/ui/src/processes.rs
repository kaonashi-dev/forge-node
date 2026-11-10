//! The sidebar's Processes: every project action run still in the store,
//! grouped by project with live runs first. Grouped when the store changes.

use domain::{ActionReadiness, Project, Session, SessionId, SessionState, Workspace};
use rune::{El, ScrollSpec, col, el, icon, row, text};
use rune_kit::Kit;

use crate::app::AppState;

/// Where a run is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Phase {
    Waiting,
    Probing,
    Running,
    Ready(Option<String>),
    TimedOut,
    PortInUse(u16),
    Exited(Option<i32>),
    Failed(String),
}

impl Phase {
    #[must_use]
    pub fn live(&self) -> bool {
        !matches!(self, Phase::Exited(_) | Phase::Failed(_))
    }
}

#[must_use]
pub fn phase_of(session: &Session) -> Phase {
    match &session.state {
        SessionState::Exited { code, .. } => return Phase::Exited(*code),
        SessionState::Failed { reason } => return Phase::Failed(reason.clone()),
        state if !state.is_active() => return Phase::Exited(None),
        _ => {}
    }
    match session.action.as_ref().map(|run| &run.readiness) {
        Some(ActionReadiness::Waiting { .. }) => Phase::Waiting,
        Some(ActionReadiness::Probing) => Phase::Probing,
        Some(ActionReadiness::Ready { url }) => Phase::Ready(url.clone()),
        Some(ActionReadiness::TimedOut) => Phase::TimedOut,
        Some(ActionReadiness::PortInUse { port }) => Phase::PortInUse(*port),
        _ => Phase::Running,
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProcessRow {
    pub session: SessionId,
    pub label: String,
    pub workspace: String,
    pub phase: Phase,
    pub port: Option<u16>,
    /// When the run's process started (or the session, before it did).
    pub since: domain::Timestamp,
    created: domain::Timestamp,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProcessGroup {
    pub project: String,
    pub rows: Vec<ProcessRow>,
}

/// Every action run grouped by project: live runs first, then the latest
/// exits; a group with a live run before one without, then by name.
#[must_use]
pub fn group(
    sessions: &[Session],
    workspaces: &[Workspace],
    projects: &[Project],
) -> Vec<ProcessGroup> {
    let mut groups: Vec<(domain::ProjectId, ProcessGroup)> = Vec::new();
    for session in sessions {
        let Some(run) = session.action.as_ref() else {
            continue;
        };
        let Some(workspace) = workspaces.iter().find(|w| w.id == session.workspace_id) else {
            continue;
        };
        let project = workspace.project_id;
        let at = match groups.iter().position(|(id, _)| *id == project) {
            Some(at) => at,
            None => {
                let name = projects
                    .iter()
                    .find(|item| item.id == project)
                    .map_or_else(|| workspace.label(), |item| item.name.clone());
                groups.push((
                    project,
                    ProcessGroup {
                        project: name,
                        rows: Vec::new(),
                    },
                ));
                groups.len() - 1
            }
        };
        groups[at].1.rows.push(ProcessRow {
            session: session.id,
            label: run.label.clone(),
            workspace: workspace.label(),
            phase: phase_of(session),
            port: run.port,
            since: run.started_at.unwrap_or(session.created_at),
            created: session.created_at,
        });
    }
    let mut ordered: Vec<ProcessGroup> = groups.into_iter().map(|(_, group)| group).collect();
    for group in &mut ordered {
        group.rows.sort_by(|a, b| {
            b.phase
                .live()
                .cmp(&a.phase.live())
                .then(b.created.cmp(&a.created))
        });
    }
    let live = |group: &ProcessGroup| group.rows.iter().any(|row| row.phase.live());
    ordered.sort_by(|a, b| {
        live(b)
            .cmp(&live(a))
            .then_with(|| a.project.cmp(&b.project))
    });
    ordered
}

/// Where a ready run serves, for opening in the browser: the URL it printed,
/// else its port on localhost; none before it is up or once it stopped.
#[must_use]
pub fn browser_url(row: &ProcessRow) -> Option<String> {
    if let Phase::Ready(Some(url)) = &row.phase {
        return Some(url.clone());
    }
    let up = matches!(
        row.phase,
        Phase::Ready(_) | Phase::Running | Phase::TimedOut
    );
    row.port
        .filter(|_| up)
        .map(|port| format!("http://localhost:{port}"))
}

fn elapsed(since: &domain::Timestamp, now: &domain::Timestamp) -> String {
    let seconds = (now.0 - since.0).whole_seconds().max(0);
    if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 3600 {
        format!("{}m", seconds / 60)
    } else {
        format!("{}h", seconds / 3600)
    }
}

/// What the right side of a row says, as `ProcessesPanel`'s `status`.
#[must_use]
pub fn status(row: &ProcessRow, now: &domain::Timestamp) -> String {
    match &row.phase {
        Phase::Ready(_) => row
            .port
            .map_or_else(|| "ready".into(), |port| format!(":{port}")),
        Phase::Waiting => "waiting".into(),
        Phase::PortInUse(port) => format!(":{port} taken"),
        Phase::TimedOut => "not ready".into(),
        Phase::Exited(None) => "stopped".into(),
        Phase::Exited(Some(code)) => format!("exit {code}"),
        Phase::Failed(_) => "failed".into(),
        Phase::Probing | Phase::Running => elapsed(&row.since, now),
    }
}

fn small_icon(
    k: &Kit,
    key: String,
    label: String,
    glyph: &'static rune::Icon,
    press: impl Fn(&mut AppState, &mut rune::Tx) + 'static,
) -> El {
    row()
        .key(key)
        .role("button")
        .label(label)
        .size(22.0, 22.0)
        .radius(k.radius.sm)
        .center()
        .hover(k.c.hover)
        .on_press_tx(press)
        .child(icon(glyph, 12.0).color(k.c.icon_muted))
}

pub fn view(s: &AppState, k: &Kit) -> El {
    let now = domain::Timestamp::now();
    let mut list = col().key("process-list").w_full().gap(crate::ds::SPACE_5);
    if s.processes.is_empty() {
        list = list.child(
            text("Nothing running. Run an action from a checkout's row or the command palette.")
                .font(k.ty.small)
                .color(k.c.text_faint)
                .wrap(),
        );
    }
    for group in &s.processes {
        let mut section = col()
            .role("group")
            .label(group.project.clone())
            .gap(crate::ds::SPACE_1)
            .child(
                text(group.project.clone())
                    .font(k.ty.small_medium)
                    .color(k.c.text_muted),
            );
        for process in &group.rows {
            let id = process.session;
            let dot = match process.phase {
                Phase::Failed(_) => k.c.danger.icon,
                Phase::TimedOut | Phase::PortInUse(_) => k.c.warning.icon,
                Phase::Probing | Phase::Running | Phase::Ready(_) => k.c.success.icon,
                Phase::Waiting => k.c.icon_muted,
                Phase::Exited(_) => k.c.icon_faint,
            };
            let mut side = row().gap(crate::ds::SPACE_1).items_center().child(
                text(status(process, &now))
                    .font(k.ty.text_xsmall)
                    .color(k.c.text_faint),
            );
            if let Some(url) = browser_url(process) {
                side = side.child(small_icon(
                    k,
                    format!("process-open-{id}"),
                    format!("Open {} in the browser", process.label),
                    &crate::glyphs::GLOBE,
                    move |_s: &mut AppState, tx| {
                        let _ = tx.open_url(&url);
                    },
                ));
            }
            side = side.child(small_icon(
                k,
                format!("process-restart-{id}"),
                if process.phase.live() {
                    format!("Restart {}", process.label)
                } else {
                    format!("Run {} again", process.label)
                },
                &crate::glyphs::REFRESH,
                move |s: &mut AppState, _tx| s.restart_session(id),
            ));
            if process.phase.live() {
                side = side.child(small_icon(
                    k,
                    format!("process-stop-{id}"),
                    format!("Stop {}", process.label),
                    &crate::glyphs::STOP,
                    move |s: &mut AppState, _tx| s.kill_session(id),
                ));
            }
            let reason = match &process.phase {
                Phase::Failed(reason) => reason.clone(),
                _ => String::new(),
            };
            section = section.child(
                row()
                    .key(format!("process-{id}"))
                    .w_full()
                    .h(28.0)
                    .px(6.0)
                    .gap(crate::ds::SPACE_3)
                    .radius(k.radius.sm)
                    .items_center()
                    .hover(k.c.hover)
                    .child(
                        icon(
                            if process.phase.live() {
                                &crate::glyphs::ACTIVITY
                            } else {
                                &crate::glyphs::STOP
                            },
                            12.0,
                        )
                        .color(dot),
                    )
                    .child(
                        row()
                            .role("button")
                            .label(format!("Show {} in {}", process.label, process.workspace))
                            .description(reason)
                            .grow(1.0)
                            .min_w(0.0)
                            .gap(crate::ds::SPACE_3)
                            .items_center()
                            .on_press(move |s: &mut AppState| s.focus_session(id))
                            .child(
                                text(process.label.clone())
                                    .font(k.ty.small)
                                    .color(k.c.text_base)
                                    .truncate(),
                            )
                            .child(
                                text(process.workspace.clone())
                                    .font(k.ty.text_xsmall)
                                    .color(k.c.text_faint)
                                    .truncate()
                                    .min_w(0.0),
                            ),
                    )
                    .child(side),
            );
        }
        list = list.child(section);
    }
    col()
        .key("processes")
        .w_full()
        .h_full()
        .min_h(0.0)
        .gap(crate::ds::SPACE_4)
        .child(
            text("Processes")
                .font(k.ty.small_medium)
                .color(k.c.text_base),
        )
        .child(
            col()
                .key("processes-scroll")
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .scroll(ScrollSpec {
                    stick_bottom: false,
                })
                .on_wheel(crate::pages::wheel)
                .child(list.child(el().h(1.0))),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ready_run_names_its_port_and_a_stopped_one_its_exit() {
        let row = |phase, port| ProcessRow {
            session: SessionId::new(),
            label: "dev".into(),
            workspace: "main".into(),
            phase,
            port,
            since: domain::Timestamp::now(),
            created: domain::Timestamp::now(),
        };
        let now = domain::Timestamp::now();
        assert_eq!(status(&row(Phase::Ready(None), Some(5174)), &now), ":5174");
        assert_eq!(
            browser_url(&row(
                Phase::Ready(Some("http://127.0.0.1:3000/".into())),
                None
            ))
            .as_deref(),
            Some("http://127.0.0.1:3000/")
        );
        assert_eq!(status(&row(Phase::Ready(None), None), &now), "ready");
        assert_eq!(
            status(&row(Phase::Exited(Some(1)), Some(5174)), &now),
            "exit 1"
        );
        assert_eq!(
            browser_url(&row(Phase::Exited(None), Some(5174))),
            None,
            "stopped serves nothing"
        );
        assert_eq!(status(&row(Phase::Exited(None), None), &now), "stopped");
        assert_eq!(
            status(&row(Phase::PortInUse(3000), None), &now),
            ":3000 taken"
        );
        assert_eq!(status(&row(Phase::Running, None), &now), "0s");
    }
}
