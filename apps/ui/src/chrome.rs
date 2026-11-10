//! The window's chrome: the card strip (session tabs on the left, actions on
//! the right, the window's drag band), the sidebar's view switcher, and its
//! footer.

use rune::{El, Icon, el, row, text};
use rune_kit::Kit;

use crate::app::{AppState, LinkStatus};
use crate::ds::{self, IconButtonSize, Tab, TabShape, Tone};
use crate::glyphs;
use crate::workbench::{CenterMode, Panel};

/// A view the switcher is not showing, drawn as its glyph alone.
const VIEW_TAB_W: f32 = 34.0;

/// The sidebar's views, in order: panel, name, element key, glyph.
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
    shortcut: Option<&str>,
    glyph: &'static Icon,
    press: impl Fn(&mut AppState) + 'static,
) -> El {
    let mut button = ds::icon_button(glyph, label, IconButtonSize::Regular).on_press(press);
    if let Some(shortcut) = shortcut {
        button = button.shortcut(shortcut);
    }
    button.build(k).key(key.to_string())
}

fn crumbs(key: &str, strip_h: f32) -> El {
    row()
        .key(key.to_string())
        .role("navigation")
        .label("Location")
        .h(strip_h)
        .transition(rune::Transition::none())
        .gap(ds::SPACE_4)
        .px(ds::SPACE_4)
        .center_y()
}

/// The bar's leading part: Settings' or a centre view's crumbs, and nothing
/// while sessions are shown, since the sidebar already lists them.
pub fn strip(s: &AppState, k: &Kit) -> Option<El> {
    // The toolbar's leading slot has no height of its own, so crumbs set
    // theirs, or they would be cut while the previous ones were leaving.
    let strip_h = s.chrome_metrics().tab + ds::SPACE_4;
    if s.workbench.mode == CenterMode::Settings {
        return Some(
            crumbs("card-crumbs", strip_h)
                .child(text("Settings").font(k.ty.ui).color(k.c.text_muted))
                .child(text("/").font(k.ty.ui).color(k.c.text_faint).a11y_hidden())
                .child(
                    text(s.settings.section.label())
                        .role("heading")
                        .font(k.ty.ui_medium)
                        .color(k.c.text_base),
                ),
        );
    }
    let name = match s.workbench.mode {
        CenterMode::Compare => "Preview PR",
        CenterMode::PrDetail => "Pull request",
        CenterMode::PrCompose => "Open a pull request",
        _ => return None,
    };
    Some(
        crumbs("center-crumbs", strip_h)
            .child(
                text(name)
                    .role("heading")
                    .font(k.ty.ui_medium)
                    .color(k.c.text_base),
            )
            .child(icon_button(
                k,
                "close-center",
                "Close this view",
                Some("⌘W"),
                &glyphs::CLOSE,
                |s: &mut AppState| s.close_center(),
            )),
    )
}

/// The sidebar's view switcher: the chosen view by glyph and name, the others
/// by glyph alone so six fit across the sidebar.
pub fn views(s: &AppState, k: &Kit) -> El {
    let roles = crate::theme::roles(k);
    let mut bar = row()
        .key("sidebar-views")
        .role("tablist")
        .label("Sidebar views")
        .w_full()
        .gap(ds::SPACE_1)
        .padding(ds::SPACE_1)
        .radius(k.radius.lg)
        .fill(roles.surface);
    for (panel, label, key, glyph) in VIEWS {
        let shown = s.workbench.panel == panel;
        bar = bar.child(
            Tab::new(key, label, glyph)
                .shape(TabShape::IconUntilChosen { width: VIEW_TAB_W })
                .height(s.chrome_metrics().tab)
                .chosen(shown)
                .build(k, &roles)
                .on_press(move |s: &mut AppState| s.show_panel(panel)),
        );
    }
    bar
}

/// What the foot says about the daemon, and in which tone.
#[must_use]
pub fn link_line(status: LinkStatus) -> (&'static str, Tone) {
    match status {
        LinkStatus::Connected => ("Daemon connected", Tone::Success),
        LinkStatus::Connecting => ("Connecting to the daemon…", Tone::Neutral),
        LinkStatus::Reconnecting => ("Daemon disconnected", Tone::Danger),
    }
}

/// The sidebar's foot: the daemon's state, and Settings.
pub fn footer(s: &AppState, k: &Kit) -> El {
    let roles = crate::theme::roles(k);
    let (label, tone) = link_line(s.status);
    let settings_shown = s.workbench.mode == CenterMode::Settings;
    row()
        .key("connection-status")
        .w_full()
        .h(ds::CONTROL_FIELD)
        .pl(ds::SPACE_2)
        .gap(ds::SPACE_4)
        .center_y()
        .child(ds::dot(k, &roles, tone))
        .child(
            text(label)
                .role("status")
                .label(label)
                .font(k.ty.small)
                .color(k.c.text_faint)
                .truncate()
                .min_w(0.0),
        )
        .child(el().grow(1.0))
        .child(icon_button(
            k,
            "open-settings",
            if settings_shown {
                "Back to work"
            } else {
                "Settings"
            },
            Some("⌘,"),
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
    fn sessions_take_the_bar_while_the_sidebar_is_open_and_give_it_back_closed() {
        let mut rt = runtime();
        rt.state.debug = false;
        rt.invalidate();
        for step in 0..40 {
            rt.frame(0.2 + f64::from(step) / 60.0);
        }
        let top = |rt: &Runtime<AppState>, key: &str| {
            rt.elements()
                .into_iter()
                .find(|e| e.key.as_deref() == Some(key))
                .map(|e| e.rect[1])
        };
        let card = top(&rt, "toolbar").expect("the band is still laid out");
        let center = top(&rt, "center").expect("the centre");
        assert!(
            center <= card + 1.0,
            "the centre starts where the band does"
        );
        rt.state.sidebar.set_open(false);
        rt.invalidate();
        for step in 0..40 {
            rt.frame(1.0 + f64::from(step) / 60.0);
        }
        let card = top(&rt, "toolbar").expect("the band");
        let center = top(&rt, "center").expect("the centre");
        assert!(
            center >= card + rune_kit::shell::TOOLBAR_HEIGHT - 1.0,
            "collapsed, the band keeps the lights and the toggle clear"
        );
    }

    #[test]
    fn the_foot_names_the_daemon_and_the_switcher_names_only_the_chosen_view() {
        let mut rt = runtime();
        rt.state.debug = false;
        rt.invalidate();
        rt.frame(0.2);
        let labels: Vec<String> = rt
            .elements()
            .into_iter()
            .filter(|e| e.role == "status")
            .filter_map(|e| e.label)
            .collect();
        assert!(
            labels.iter().any(|label| label == "Daemon connected"),
            "a healthy link is said too: {labels:?}"
        );
        assert_eq!(link_line(LinkStatus::Reconnecting).1, Tone::Danger);
        let shown = rt.find("view-projects").expect("the chosen view");
        let other = rt.find("view-files").expect("another view");
        assert!(shown.0 < other.0, "views keep their order");
    }

    #[test]
    fn switching_views_turns_the_page_from_the_side_the_view_sits_on() {
        use crate::workbench::Panel;
        let mut rt = runtime();
        let x = |rt: &Runtime<AppState>, key: &str| {
            rt.elements()
                .into_iter()
                .find(|e| e.key.as_deref() == Some(key))
                .map(|e| e.rect[0] + e.rect[2])
        };
        let rest = x(&rt, "sidebar-page-Projects").expect("the shown page");
        rt.state.show_panel(Panel::Git);
        assert!(!rt.state.workbench.panel_back);
        rt.invalidate();
        rt.frame(0.2);
        rt.frame(0.23);
        let entering = x(&rt, "sidebar-page-Git").expect("the next page mounts");
        assert!(
            entering > rest,
            "a later view comes in from the trailing edge"
        );
        for step in 1..=60 {
            rt.frame(0.23 + f64::from(step) / 60.0);
        }
        assert_eq!(
            x(&rt, "sidebar-page-Git"),
            Some(rest),
            "it settles in place"
        );
    }

    #[test]
    fn a_sideways_swipe_over_the_list_turns_the_page_and_a_vertical_one_scrolls() {
        use crate::workbench::Panel;
        use rune::{GesturePhase, WheelEvent};
        let mut rt = runtime();
        rt.state.show_panel(Panel::Projects);
        rt.invalidate();
        rt.frame(0.2);
        let (x, y) = rt.find("projects-scroll").expect("the projects list");
        let swipe = |rt: &mut Runtime<AppState>, dx: f32, dy: f32, t: f64| {
            let at = |ev: WheelEvent| ev.at(x, y).precise(true);
            rt.wheel_event(at(WheelEvent::new(dx, dy).phase(GesturePhase::Began)));
            rt.frame(t);
            for step in 1..=4 {
                rt.wheel_event(at(WheelEvent::new(dx, dy)));
                rt.frame(t + f64::from(step) / 120.0);
            }
            rt.wheel_event(at(WheelEvent::new(0.0, 0.0).phase(GesturePhase::Ended)));
            rt.frame(t + 0.1);
        };
        swipe(&mut rt, 2.0, 30.0, 0.3);
        assert_eq!(
            rt.state.workbench.panel,
            Panel::Projects,
            "a scroll is not a swipe"
        );
        swipe(&mut rt, -30.0, 2.0, 0.6);
        assert_eq!(
            rt.state.workbench.panel,
            Panel::Files,
            "fingers left bring the next view"
        );
        assert_eq!(rt.state.page_swipe, None);
    }
}
