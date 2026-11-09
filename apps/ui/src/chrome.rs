//! The window's chrome as Tauri draws it: the card strip (session tabs on
//! the left, actions on the right, the window's drag band), the sidebar's
//! view switcher, and its footer. Tauri's `CardStrip`, `SessionTabs` and
//! `Sidebar` are the reference.

use rune::{El, Icon, col, el, icon, row, text};
use rune_kit::Kit;

use crate::app::{AppState, LinkStatus, SessionRow, SessionStatus};
use crate::glyphs;
use crate::workbench::{CenterMode, Panel};

/// Tauri's `tabH`: the chip inside the strip's band.
const TAB_H: f32 = 30.0;
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
    let tint = match session.status {
        Some(SessionStatus::Attention) => k.c.warning.icon,
        Some(SessionStatus::Busy) => k.c.icon_accent,
        None if active => k.c.icon_base,
        None => k.c.icon_muted,
    };
    let waiting = session.status == Some(SessionStatus::Attention);
    let mut chip = row()
        .key(format!("t-{}", session.id_text))
        .role("tab")
        .label(if waiting {
            format!("{}, waiting on you", session.title)
        } else {
            session.title.clone()
        })
        .checked(active)
        .h(TAB_H)
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
        chip = chip.fill(k.c.surface_hover).child(
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
        .child(chip)
        .child(
            el().pin(Some(10.0), None, Some(10.0), Some(0.0))
                .h(2.0)
                .radius(1.0)
                .fill(if active {
                    k.c.text_accent
                } else {
                    rune::Color::TRANSPARENT
                }),
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
                text("Appearance · Storage · Usage")
                    .role("heading")
                    .font(k.ty.small_medium)
                    .color(k.c.text_base),
            );
    }
    let tabs = s.tab_rows();
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
        .scroll_x();
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
            |s: &mut AppState| s.palette.show(false),
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
                .h(32.0)
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
            let next = if s.workbench.mode == CenterMode::Settings {
                CenterMode::Session
            } else {
                CenterMode::Settings
            };
            s.show_mode(next);
        },
    ))
}
