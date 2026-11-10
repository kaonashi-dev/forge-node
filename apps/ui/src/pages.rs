//! A two-finger horizontal swipe over the sidebar turns its views like pages,
//! as Safari goes back and forward: the page follows the fingers, and letting
//! go past `COMMIT` shows the neighbouring view.

use rune::{GesturePhase, Tx, WheelEvent};

use crate::app::AppState;
use crate::workbench::Panel;

/// How far the fingers travel before a release turns the page.
const COMMIT: f32 = 72.0;
/// The farthest the page follows the fingers.
const MAX_DRAG: f32 = 120.0;
/// The page's resistance where there is no view to turn to.
const EDGE_RESISTANCE: f32 = 0.25;
/// A gesture is a swipe only while it runs this much more across than down,
/// so a vertical scroll that drifts never turns a page.
const AXIS_RATIO: f32 = 1.5;

/// The view a swipe of `dx` leads to: fingers moving left (`dx < 0`) bring in
/// the next view, moving right the previous one.
#[must_use]
pub fn neighbour(panel: Panel, dx: f32) -> Option<Panel> {
    let views = &crate::chrome::VIEWS;
    let at = views.iter().position(|(view, ..)| *view == panel)?;
    let to = if dx < 0.0 {
        at.checked_add(1)?
    } else {
        at.checked_sub(1)?
    };
    views.get(to).map(|(view, ..)| *view)
}

/// Where the shown page sits while the fingers hold it.
#[must_use]
pub fn drag_offset(panel: Panel, dx: f32) -> f32 {
    let follow = if neighbour(panel, dx).is_some() {
        dx
    } else {
        dx * EDGE_RESISTANCE
    };
    follow.clamp(-MAX_DRAG, MAX_DRAG)
}

/// The sidebar's `on_wheel`: takes a precise, mostly horizontal gesture and
/// leaves every other delta to scroll as usual. Each panel's scroll view
/// carries it too, because a wheel delta never travels past the nearest one.
pub fn wheel(s: &mut AppState, ev: WheelEvent, tx: &mut Tx) -> bool {
    if !ev.precise {
        return false;
    }
    let starting = ev.phase == GesturePhase::Began && !ev.momentum;
    match s.page_swipe {
        Some(_) if ev.momentum => true,
        Some(dx) if !starting => {
            let dx = dx + ev.dx;
            if ev.phase == GesturePhase::Ended {
                s.page_swipe = None;
                if dx.abs() >= COMMIT
                    && let Some(to) = neighbour(s.workbench.panel, dx)
                {
                    s.show_panel(to);
                }
            } else {
                s.page_swipe = Some(dx);
                tx.instant();
            }
            true
        }
        _ => {
            if ev.momentum || ev.dx == 0.0 || ev.dx.abs() <= ev.dy.abs() * AXIS_RATIO {
                return false;
            }
            s.page_swipe = Some(ev.dx);
            tx.instant();
            true
        }
    }
}

pub const SIDEBAR_SWIPE_KEY: &str = "ui.sidebar_swipe";

/// Which swipe over the centre shows or hides the sidebar.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SidebarSwipe {
    #[default]
    Swipe,
    /// Only with Option held, so a plain sideways scroll stays the terminal's.
    OptionSwipe,
    Off,
}

impl SidebarSwipe {
    pub const ALL: [SidebarSwipe; 3] = [Self::Swipe, Self::OptionSwipe, Self::Off];

    /// An unknown or missing value is the default, so the gesture is on.
    #[must_use]
    pub fn read(value: Option<&str>) -> Self {
        match value {
            Some("option") => Self::OptionSwipe,
            Some("off") => Self::Off,
            _ => Self::Swipe,
        }
    }

    #[must_use]
    pub fn value(self) -> &'static str {
        match self {
            Self::Swipe => "swipe",
            Self::OptionSwipe => "option",
            Self::Off => "off",
        }
    }

    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Swipe => "Swipe",
            Self::OptionSwipe => "Option-swipe",
            Self::Off => "Off",
        }
    }
}

/// A two-finger horizontal swipe over the centre shows or hides the sidebar,
/// as Rex does: fingers moving right bring it in, moving left put it away. It
/// acts once the fingers have travelled `COMMIT`, so the rest of the gesture
/// does nothing, and a vertical scroll never starts one.
pub fn sidebar_swipe(s: &mut AppState, ev: &WheelEvent) -> bool {
    if !ev.precise {
        return false;
    }
    let starting = ev.phase == GesturePhase::Began && !ev.momentum;
    match s.sidebar_swipe {
        Some(_) if ev.momentum => true,
        Some(travel) if !starting => {
            if ev.phase == GesturePhase::Ended {
                s.sidebar_swipe = None;
                return true;
            }
            let Some(dx) = travel else {
                return true;
            };
            let dx = dx + ev.dx;
            if dx.abs() >= COMMIT {
                let open = dx > 0.0;
                if s.sidebar.is_open() != open {
                    s.sidebar.set_open(open);
                }
                s.sidebar_swipe = Some(None);
            } else {
                s.sidebar_swipe = Some(Some(dx));
            }
            true
        }
        _ => {
            let wanted = match SidebarSwipe::read(s.pref(SIDEBAR_SWIPE_KEY)) {
                SidebarSwipe::Swipe => true,
                SidebarSwipe::OptionSwipe => ev.mods.alt,
                SidebarSwipe::Off => false,
            };
            if !wanted || ev.momentum || ev.dx == 0.0 || ev.dx.abs() <= ev.dy.abs() * AXIS_RATIO {
                return false;
            }
            s.sidebar_swipe = Some(Some(ev.dx));
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingers_moving_left_bring_in_the_next_view_and_the_ends_resist() {
        assert_eq!(neighbour(Panel::Projects, -10.0), Some(Panel::Files));
        assert_eq!(neighbour(Panel::Files, 10.0), Some(Panel::Projects));
        assert_eq!(neighbour(Panel::Projects, 10.0), None);
        assert_eq!(neighbour(Panel::Processes, -10.0), None);
        assert_eq!(drag_offset(Panel::Files, -40.0), -40.0);
        assert_eq!(
            drag_offset(Panel::Projects, 40.0),
            10.0,
            "nothing before the first view"
        );
        assert_eq!(drag_offset(Panel::Files, -500.0), -MAX_DRAG);
    }

    #[test]
    fn the_swipe_is_on_unless_set_otherwise() {
        assert_eq!(SidebarSwipe::read(None), SidebarSwipe::Swipe);
        assert_eq!(SidebarSwipe::read(Some("bogus")), SidebarSwipe::Swipe);
        for choice in SidebarSwipe::ALL {
            assert_eq!(SidebarSwipe::read(Some(choice.value())), choice);
        }
    }

    #[test]
    fn a_sideways_swipe_over_the_centre_hides_then_shows_the_sidebar_once_per_gesture() {
        use std::sync::Arc;
        let store = crate::connection::fixture_store(&crate::bench::fixture_path())
            .expect("fixture snapshot decodes");
        let mut s = AppState::for_bench(
            store,
            Arc::new(crate::connection::Metrics::default()),
            crate::terminal::SharedTerminal::new(),
        );
        let swipe = |s: &mut AppState, dx: f32| {
            let ev = |dx: f32, phase| WheelEvent::new(dx, 0.0).phase(phase).precise(true);
            sidebar_swipe(s, &ev(dx, GesturePhase::Began));
            for _ in 0..6 {
                sidebar_swipe(s, &ev(dx, GesturePhase::Moved));
            }
            sidebar_swipe(s, &ev(0.0, GesturePhase::Ended));
        };
        s.sidebar.set_open(true);
        swipe(&mut s, -30.0);
        assert!(!s.sidebar.is_open(), "fingers left put it away");
        swipe(&mut s, 30.0);
        assert!(s.sidebar.is_open(), "fingers right bring it back");
        assert!(
            !sidebar_swipe(
                &mut s,
                &WheelEvent::new(1.0, 20.0)
                    .phase(GesturePhase::Began)
                    .precise(true)
            ),
            "a scroll is left to scroll"
        );
    }
}
