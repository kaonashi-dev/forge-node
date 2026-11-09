//! The window's chrome as Tauri draws it: the card strip (session tabs on
//! the left, actions on the right, the window's drag band), the sidebar's
//! view switcher, and its footer. Tauri's `CardStrip`, `SessionTabs` and
//! `Sidebar` are the reference.

use rune::{El, Icon, col, el, icon, row, text};
use rune_kit::Kit;

use crate::app::{AppState, LinkStatus, SessionRow, SessionStatus};
use crate::glyphs;
use crate::workbench::{CenterMode, Panel};

/// A tab never grows past this, as `.session-tab-wrap { max-width: 280px }`.
const TAB_MAX_W: f32 = 240.0;

/// The sidebar's views, in Tauri's order: panel, name, element key, glyph.
pub const VIEWS: [(Panel, &str, &str, &Icon); 6] = [
    (
        Panel::Projects,
        "Projects",
        "view-projects",
        &glyphs::PROJECTS,
    ),
    (Panel::Files, "Files", "view-files", &glyphs::FILES),
    (Panel::History, "History", "view-history", &glyphs::HISTORY),
    (
        Panel::PullRequests,
        "Pull Requests",
        "view-pr",
        &glyphs::PULL_REQUEST,
    ),
    (Panel::Git, "Git", "view-git", &glyphs::GIT),
    (
        Panel::Processes,
        "Processes",
        "view-processes",
        &glyphs::ACTIVITY,
    ),
];

fn icon_button(
    k: &Kit,
    key: &str,
    label: &str,
    glyph: &'static Icon,
    press: impl Fn(&mut AppState) + 'static,
) -> El {
    row()
        .key(key.to_string())
        .role("button")
        .label(label.to_string())
        .size(28.0, 28.0)
        .radius(6.0)
        .center()
        .hover(k.c.hover)
        .on_press(press)
        .child(icon(glyph, 16.0).color(k.c.icon_muted))
}

fn tab(s: &AppState, session: &SessionRow, k: &Kit) -> El {
    let active = s.active == Some(session.id) && s.workbench.mode == CenterMode::Session;
    let id = session.id;
    let roles = crate::theme::roles(k);
    let tint = match session.status {
        Some(SessionStatus::Attention) => roles.needs_you.text,
        Some(SessionStatus::Busy) => k.c.icon_accent,
        None if active => k.c.icon_base,
        None => k.c.icon_muted,
    };
    let waiting = session.status == Some(SessionStatus::Attention);
    let tab_h = s.chrome_metrics().tab;
    let mut chip = row()
        .key(format!("t-{}", session.id_text))
        .role("tab")
        .label(if waiting {
            format!("{}, waiting on you", session.title)
        } else {
            session.title.clone()
        })
        .checked(active)
        .h(tab_h)
        .max_w(TAB_MAX_W)
        .pl(10.0)
        .pr(if active { 4.0 } else { 10.0 })
        .gap(7.0)
        .radius(7.0)
        .center_y()
        .on_press(move |s: &mut AppState| s.select(id))
        .on_double_press(move |s: &mut AppState| {
            s.select(id);
            s.request_rename();
        })
        .on_layout(move |s: &mut AppState, rect| s.tabs.note_rect(id, rect))
        .on_drag(move |s: &mut AppState, drag, _| match drag.phase {
            rune::Phase::Down => s.begin_tab_drag(id),
            rune::Phase::Move => s.drag_tab_to(drag.window_point()[0]),
            _ if drag.cancelled() => s.tabs.cancel_drag(),
            _ => s.drop_tab(),
        })
        .child(icon(glyphs::for_row(session, active, !k.dark), 14.0).color(tint))
        .child(
            text(session.title.clone())
                .font(if active {
                    k.ty.small_medium
                } else {
                    k.ty.small
                })
                .color(if active {
                    k.c.text_strong
                } else {
                    k.c.text_muted
                })
                .truncate()
                .min_w(0.0)
                .shrink(),
        );
    if active {
        chip = chip.fill(roles.tab).child(
            row()
                .key(format!("close-{}", session.id_text))
                .role("button")
                .label(format!("Close {}", session.title))
                .size(20.0, 20.0)
                .radius(5.0)
                .center()
                .hover(k.c.hover)
                .on_press(move |s: &mut AppState| s.request_close(id))
                .child(icon(&glyphs::CLOSE, 12.0).color(k.c.icon_faint)),
        );
    } else {
        chip = chip.hover(k.c.hover);
    }
    // The strip's bottom edge carries the active tab's line, as `.tab-indicator::after`.
    col()
        .key(format!("tab-{}", session.id_text))
        .h_full()
        .justify_center()
        .on_secondary_press(move |s: &mut AppState, point, _| {
            crate::menus::open(s, crate::menus::Target::Tab(id), point);
        })
        .child(chip)
        .child(
            el().pin(Some(10.0), None, Some(10.0), Some(0.0))
                .h(2.0)
                .radius(1.0)
                .fill(if active {
                    roles.hue(None)
                } else {
                    rune::Color::TRANSPARENT
                }),
        )
}

/// The line under the gap a dragged tab would drop into, in window points so
/// it does not have to know where the strip starts.
#[must_use]
pub fn drop_marker(s: &AppState, k: &Kit) -> Option<El> {
    let drag = s.tabs.drag.as_ref()?;
    let rows = s.tab_rows();
    let ids: Vec<domain::SessionId> = rows.iter().map(|row| row.id).collect();
    let (x, y, h) = if let Some(id) = ids.get(drag.gap) {
        let (_, rect) = s.tabs.rects.iter().find(|(candidate, _)| candidate == id)?;
        (rect[0] - 1.0, rect[1], rect[3])
    } else {
        let id = ids.last()?;
        let (_, rect) = s.tabs.rects.iter().find(|(candidate, _)| candidate == id)?;
        (rect[0] + rect[2] - 1.0, rect[1], rect[3])
    };
    Some(
        el().key("tab-drop-marker")
            .absolute(x, y)
            .size(2.0, h)
            .radius(1.0)
            .a11y_hidden()
            .fill(k.c.border_focus),
    )
}

/// The strip's leading part: the checkout's tabs, or Settings' crumbs.
pub fn strip(s: &AppState, k: &Kit) -> El {
    if s.workbench.mode == CenterMode::Settings {
        return row()
            .key("card-crumbs")
            .role("navigation")
            .label("Location")
            .gap(8.0)
            .px(8.0)
            .center_y()
            .child(text("Settings").font(k.ty.small).color(k.c.text_muted))
            .child(
                text("/")
                    .font(k.ty.small)
                    .color(k.c.text_faint)
                    .a11y_hidden(),
            )
            .child(
                text(s.settings.section.label())
                    .role("heading")
                    .font(k.ty.small_medium)
                    .color(k.c.text_base),
            );
    }
    let view_name = match s.workbench.mode {
        CenterMode::Compare => Some("Preview PR"),
        CenterMode::PrDetail => Some("Pull request"),
        CenterMode::PrCompose => Some("Open a pull request"),
        _ => None,
    };
    if let Some(name) = view_name {
        return row()
            .key("center-crumbs")
            .role("navigation")
            .label("Location")
            .gap(8.0)
            .px(8.0)
            .center_y()
            .child(
                text(name)
                    .role("heading")
                    .font(k.ty.small_medium)
                    .color(k.c.text_base),
            )
            .child(el().grow(1.0))
            .child(icon_button(
                k,
                "close-center",
                "Close this view (⌘W)",
                &glyphs::CLOSE,
                |s: &mut AppState| s.close_center(),
            ));
    }
    let tabs = s.tab_rows();
    let roles = crate::theme::roles(k);
    let mut strip = row()
        .key("session-tabs")
        .role("tablist")
        .label("Sessions")
        .h_full()
        .gap(2.0)
        .px(4.0)
        .center_y()
        .min_w(0.0)
        .shrink()
        .fill(roles.strip)
        .scroll_x()
        .child(
            el().pin(Some(0.0), None, Some(0.0), Some(0.0))
                .h(1.0)
                .fill(roles.strip_rule)
                .a11y_hidden(),
        );
    if tabs.is_empty() {
        return strip.child(
            text("No sessions")
                .key("tab-empty")
                .font(k.ty.small)
                .color(k.c.text_faint),
        );
    }
    // A tab cut at the strip's end fades out rather than stopping hard.
    strip = strip.clip_fade(0.0, 0.0, 24.0, 0.0);
    for session in tabs {
        strip = strip.child(tab(s, session, k));
    }
    strip
}

/// The strip's trailing actions: a new terminal and the palette.
pub fn actions(k: &Kit) -> [El; 2] {
    [
        icon_button(
            k,
            "new-session",
            "New terminal (⌘T)",
            &glyphs::PLUS,
            AppState::request_create,
        ),
        icon_button(
            k,
            "jump-to",
            "Jump to… (⌘K)",
            &glyphs::SEARCH,
            |s: &mut AppState| s.open_palette(crate::palette::Scope::Everything),
        ),
    ]
}

/// The sidebar's view switcher, as Tauri's `sidebar-views`.
pub fn views(s: &AppState, k: &Kit) -> El {
    let mut bar = row()
        .key("sidebar-views")
        .role("tablist")
        .label("Sidebar views")
        .w_full()
        .gap(2.0)
        .padding(3.0);
    for (panel, label, key, glyph) in VIEWS {
        let shown = s.workbench.panel == panel;
        bar = bar.child(
            row()
                .key(key)
                .role("tab")
                .label(label.to_string())
                .checked(shown)
                .grow(1.0)
                .h(s.chrome_metrics().tab)
                .radius(6.0)
                .center()
                .fill(if shown {
                    k.c.surface_hover
                } else {
                    rune::Color::TRANSPARENT
                })
                .hover(k.c.hover)
                .on_press(move |s: &mut AppState| s.show_panel(panel))
                // Six views share 300 pt, so the glyph stands alone; the name
                // is what assistive tech and the panel's heading say.
                .child(
                    icon(if shown { glyphs::chosen(glyph) } else { glyph }, 16.0).color(if shown {
                        k.c.icon_accent
                    } else {
                        k.c.icon_muted
                    }),
                ),
        );
    }
    bar
}

/// The sidebar's foot: the daemon's state only while it is not connected,
/// and Settings, as Tauri's `SidebarFooter`.
pub fn footer(s: &AppState, k: &Kit) -> El {
    let trouble = match s.status {
        LinkStatus::Connected => None,
        LinkStatus::Connecting => Some(("Connecting to the daemon…", k.c.icon_faint)),
        LinkStatus::Reconnecting => Some(("Daemon disconnected", k.c.danger.icon)),
    };
    let mut foot = row()
        .key("connection-status")
        .w_full()
        .h(32.0)
        .gap(7.0)
        .center_y();
    if let Some((label, dot)) = trouble {
        foot = foot.child(el().size(6.0, 6.0).radius(3.0).fill(dot)).child(
            text(label)
                .role("status")
                .font(k.ty.small)
                .color(k.c.text_muted)
                .truncate()
                .min_w(0.0),
        );
    }
    let settings_shown = s.workbench.mode == CenterMode::Settings;
    foot.child(el().grow(1.0)).child(icon_button(
        k,
        "open-settings",
        if settings_shown {
            "Back to work (⌘,)"
        } else {
            "Settings (⌘,)"
        },
        if settings_shown {
            &glyphs::ARROW_LEFT
        } else {
            &glyphs::SETTINGS
        },
        |s: &mut AppState| {
            if s.workbench.mode == CenterMode::Settings {
                s.close_settings();
            } else {
                s.open_settings(None);
            }
        },
    ))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use rune::{Env, Runtime};

    use super::*;
    use crate::connection::{Metrics, fixture_store};
    use crate::terminal::SharedTerminal;

    fn runtime() -> Runtime<AppState> {
        let store = fixture_store(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/snapshot.json"),
        )
        .expect("fixture");
        let mut state =
            AppState::for_bench(store, Arc::new(Metrics::default()), SharedTerminal::new());
        state.generation = 1;
        let mut rt = Runtime::new(
            state,
            crate::ui::view,
            (1180.0, 760.0),
            Env {
                system_dark: true,
                scale: 1.0,
            },
        );
        rt.poll = Some(crate::app::poll);
        rt.frame(0.0);
        rt.frame(0.1);
        rt
    }

    #[test]
    fn dragging_a_tab_moves_it_and_leaves_the_active_session_alone() {
        let mut rt = runtime();
        let before: Vec<crate::app::SessionRow> =
            rt.state.tab_rows().into_iter().cloned().collect();
        if before.len() < 2 {
            return;
        }
        let active = rt.state.active;
        let first = before[0].id_text.clone();
        let second = before[1].id_text.clone();
        let from = rt.find(&format!("t-{second}")).expect("the second tab");
        let to = rt.find(&format!("t-{first}")).expect("the first tab");
        rt.pointer_down(from.0, from.1);
        rt.frame(0.2);
        for step in 1..=6 {
            let x = from.0 + (to.0 - from.0) * step as f32 / 6.0;
            rt.pointer_move(x, from.1);
            rt.frame(0.2 + step as f64 / 60.0);
        }
        rt.pointer_up(to.0 - 4.0, to.1);
        rt.frame(0.4);
        let after: Vec<String> = rt
            .state
            .tab_rows()
            .into_iter()
            .map(|row| row.id_text.clone())
            .collect();
        assert_eq!(after[0], second, "the dragged tab took the first slot");
        assert_eq!(rt.state.active, active, "reordering never selects a tab");
    }
}
