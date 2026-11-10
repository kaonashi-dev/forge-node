//! The macOS menu bar. Every item with a chord synthesizes
//! that chord and runs the app's own key path, so a menu pick and the keyboard
//! can never drift into two implementations. Edit and Window come from Rune's
//! standard menus, which AppKit already owns.

use std::sync::OnceLock;

use rune::{App, AppHandle, Key, KeyEvent, MenuItem, Mods, Shortcut, Tx, col, row, text};
use rune_kit::{Button, ButtonVariant, Dialog, Kit};

use crate::app::AppState;
use crate::workbench::Panel;

static APP: OnceLock<AppHandle<AppState>> = OnceLock::new();

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MenuSignature {
    has_session: bool,
    has_workspace: bool,
    has_project: bool,
    session_shown: bool,
    split: bool,
    sidebar_open: bool,
    panel: u8,
}

impl MenuSignature {
    #[must_use]
    pub fn of(s: &AppState) -> Self {
        Self {
            has_session: s.active.is_some(),
            has_workspace: s.workbench.workspace.is_some(),
            has_project: s
                .workbench
                .workspace
                .and_then(|workspace| {
                    s.store
                        .as_ref()?
                        .workspaces
                        .iter()
                        .find(|w| w.id == workspace)
                })
                .is_some(),
            session_shown: s.session_shown(),
            split: s.pane_count() == 2,
            sidebar_open: s.sidebar.is_open(),
            panel: match s.workbench.panel {
                Panel::Projects => 0,
                Panel::Files => 1,
                Panel::History => 2,
                Panel::PullRequests => 3,
                Panel::Git => 4,
                Panel::Processes => 5,
            },
        }
    }
}

pub fn install_handle(handle: AppHandle<AppState>) {
    let _ = APP.set(handle);
}

pub fn install(app: App<AppState>) -> App<AppState> {
    let mut app = app.standard_menus();
    for item in items(&MenuSignature::default()) {
        app = app.menu_item(item);
    }
    app
}

/// Replaces the menu's enabled and checked state when something it reads
/// changed. Cheap enough to ask each poll: the signature is seven small
/// values and the menu is rebuilt only on a change.
pub fn refresh(s: &mut AppState) {
    let signature = MenuSignature::of(s);
    if s.native_menu == Some(signature) {
        return;
    }
    let Some(handle) = APP.get() else {
        return;
    };
    if handle.set_menu(items(&signature)).is_ok() {
        s.native_menu = Some(signature);
    }
}

fn command(key: char, shift: bool) -> KeyEvent {
    KeyEvent {
        key: Key::Command(key),
        mods: Mods {
            cmd: true,
            shift,
            ..Mods::default()
        },
    }
}

fn dispatch(s: &mut AppState, tx: &mut Tx, event: &KeyEvent) {
    crate::app_key(s, event, tx);
}

fn shortcut(text: &str) -> Shortcut {
    text.parse().expect("the menu's own shortcut is valid")
}

#[must_use]
pub fn items(signature: &MenuSignature) -> Vec<MenuItem<AppState>> {
    let items = vec![
        MenuItem::new("About Forge Node", |s: &mut AppState, _| s.about = true).id("about"),
        MenuItem::new("Check for Updates…", |s: &mut AppState, _| {
            s.open_update_check();
        })
        .id("check_for_update"),
        MenuItem::separator(),
        MenuItem::new("Settings…", |s: &mut AppState, tx: &mut Tx| {
            dispatch(s, tx, &command(',', false));
        })
        .id("open_settings")
        .key_equivalent(shortcut("cmd+,")),
        MenuItem::separator(),
        MenuItem::new("New Terminal", |s: &mut AppState, tx: &mut Tx| {
            dispatch(s, tx, &command('t', false));
        })
        .id("new_terminal")
        .menu("File")
        .key_equivalent(shortcut("cmd+t")),
        MenuItem::new("New Agent", |s: &mut AppState, tx: &mut Tx| {
            dispatch(s, tx, &command('a', true));
        })
        .id("new_agent")
        .menu("File")
        .key_equivalent(shortcut("cmd+shift+a"))
        .enabled(signature.has_workspace),
        MenuItem::new("New Worktree", |s: &mut AppState, tx: &mut Tx| {
            dispatch(s, tx, &command('n', true));
        })
        .id("new_worktree")
        .menu("File")
        .key_equivalent(shortcut("cmd+shift+n"))
        .enabled(signature.has_project),
        MenuItem::separator().menu("File"),
        MenuItem::new("Close Session", |s: &mut AppState, tx: &mut Tx| {
            dispatch(s, tx, &command('w', false));
        })
        .id("close_session")
        .menu("File")
        .key_equivalent(shortcut("cmd+w"))
        .keys_first()
        .enabled(signature.has_session),
        MenuItem::new("Command Palette…", |s: &mut AppState, tx: &mut Tx| {
            dispatch(s, tx, &command('k', false));
        })
        .id("open_command_palette")
        .menu("View")
        .key_equivalent(shortcut("cmd+k")),
        MenuItem::new("Open File…", |s: &mut AppState, tx: &mut Tx| {
            dispatch(s, tx, &command('p', false));
        })
        .id("open_file_palette")
        .menu("View")
        .key_equivalent(shortcut("cmd+p")),
        MenuItem::new("Find in Files…", |s: &mut AppState, tx: &mut Tx| {
            dispatch(s, tx, &command('f', true));
        })
        .id("find_in_project")
        .menu("View")
        .key_equivalent(shortcut("cmd+shift+f")),
        MenuItem::separator().menu("View"),
        MenuItem::new("Toggle Sidebar", |s: &mut AppState, tx: &mut Tx| {
            dispatch(s, tx, &command('b', false));
        })
        .id("toggle_sidebar")
        .menu("View")
        .key_equivalent(shortcut("cmd+b"))
        .keys_first()
        .checked(signature.sidebar_open),
        MenuItem::new("Projects", |s: &mut AppState, tx: &mut Tx| {
            dispatch(s, tx, &command('1', false));
        })
        .id("toggle_projects")
        .menu("View")
        .key_equivalent(shortcut("cmd+1"))
        .checked(signature.sidebar_open && signature.panel == 0),
        MenuItem::new("Pull Requests", |s: &mut AppState, tx: &mut Tx| {
            dispatch(s, tx, &command('r', true));
        })
        .id("toggle_pull_requests")
        .menu("View")
        .key_equivalent(shortcut("cmd+shift+r"))
        .checked(signature.sidebar_open && signature.panel == 3),
        MenuItem::new("Files", |s: &mut AppState, tx: &mut Tx| {
            dispatch(s, tx, &command('2', false));
        })
        .id("toggle_files")
        .menu("View")
        .key_equivalent(shortcut("cmd+2"))
        .checked(signature.sidebar_open && signature.panel == 1),
        MenuItem::separator().menu("View"),
        MenuItem::new("Split Pane", |s: &mut AppState, tx: &mut Tx| {
            dispatch(s, tx, &command('d', false));
        })
        .id("split_pane")
        .menu("View")
        .key_equivalent(shortcut("cmd+d"))
        .enabled(signature.session_shown),
        MenuItem::new("Join Panes", |s: &mut AppState, _| s.join_panes())
            .id("join_panes")
            .menu("View")
            .enabled(signature.split),
        MenuItem::new("Maximize", |_s: &mut AppState, tx: &mut Tx| {
            tx.toggle_maximized()
        })
        .id("maximize")
        .menu("Window"),
        MenuItem::separator().menu("Window"),
        MenuItem::new("Next Session", |s: &mut AppState, tx: &mut Tx| {
            dispatch(
                s,
                tx,
                &KeyEvent {
                    key: Key::Tab,
                    mods: Mods {
                        ctrl: true,
                        ..Mods::default()
                    },
                },
            );
        })
        .id("next_session")
        .menu("Window"),
        MenuItem::new("Previous Session", |s: &mut AppState, tx: &mut Tx| {
            dispatch(
                s,
                tx,
                &KeyEvent {
                    key: Key::Tab,
                    mods: Mods {
                        ctrl: true,
                        shift: true,
                        ..Mods::default()
                    },
                },
            );
        })
        .id("previous_session")
        .menu("Window"),
        MenuItem::new("About Forge Node", |s: &mut AppState, _| s.about = true)
            .id("help_about")
            .menu("Help"),
    ];
    items
}

pub fn dialog(s: &AppState, k: &Kit, layer: &mut Vec<rune::El>) {
    if !s.about {
        return;
    }
    let line = |name: &str, value: String| {
        row()
            .w_full()
            .gap(12.0)
            .child(
                text(name.to_string())
                    .font(k.ty.small)
                    .color(k.c.text_faint)
                    .w(120.0),
            )
            .child(text(value).font(k.ty.small).color(k.c.text_base))
    };
    let close = Button::new("Close")
        .variant(ButtonVariant::Outline)
        .on_press(|s: &mut AppState| s.about = false)
        .build(k)
        .key("about-close");
    Dialog::new("about", "Forge Node")
        .description("A terminal workbench for coding agents.")
        .open(true)
        .fit()
        .width(460.0)
        .click_outside(true)
        .on_close(|s: &mut AppState| s.about = false)
        .child(
            col()
                .key("about-facts")
                .w_full()
                .gap(6.0)
                .child(line("Version", crate::updates::APP_VERSION.to_string()))
                .child(line("Protocol", client::PROTOCOL_VERSION.to_string()))
                .child(line(
                    "Editor control",
                    editor_control::CONTROL_VERSION.to_string(),
                ))
                .child(
                    text(
                        "The daemon owns every terminal. An update to this window reconnects to \
                         the one already running.",
                    )
                    .font(k.ty.text_xsmall)
                    .color(k.c.text_faint)
                    .wrap(),
                ),
        )
        .child(row().gap(8.0).child(rune::el().grow(1.0)).child(close))
        .build(k, layer);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_menu_builds_every_listed_item() {
        assert_eq!(
            items(&MenuSignature::default()).len(),
            26,
            "app, File, View, Window and Help items, with their separators"
        );
    }

    #[test]
    fn the_signature_follows_the_window() {
        let none = MenuSignature::default();
        assert!(!none.has_session && !none.split && !none.sidebar_open);
        let shown = MenuSignature {
            has_session: true,
            has_project: true,
            session_shown: true,
            split: true,
            sidebar_open: true,
            panel: 3,
            ..MenuSignature::default()
        };
        assert!(shown.has_session && shown.has_project && shown.split);
        assert_eq!(shown.panel, 3);
    }

    #[test]
    fn every_shortcut_parses_on_this_platform() {
        for chord in [
            "cmd+,",
            "cmd+t",
            "cmd+shift+a",
            "cmd+shift+n",
            "cmd+w",
            "cmd+k",
            "cmd+p",
            "cmd+shift+f",
            "cmd+b",
            "cmd+1",
            "cmd+shift+r",
            "cmd+2",
            "cmd+d",
        ] {
            assert!(chord.parse::<Shortcut>().is_ok(), "{chord}");
        }
    }
}
