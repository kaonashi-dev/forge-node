//! Settings › Keyboard: one row per action, its context, the chord it answers
//! to now, and a capture cell that rebinds it. The table reads the same keymap
//! the dispatcher does, so a chord shown here is the chord that fires.

use rune::{El, KeyEvent, col, row, text};
use rune_kit::{Button, ButtonVariant, ControlSize, Kit};

use crate::app::AppState;
use crate::keymap::{self, Action};

#[derive(Default)]
pub struct Keyboard {
    /// The action whose chord cell is listening for a keystroke.
    pub capturing: Option<Action>,
    pub error: Option<String>,
}

impl AppState {
    pub fn begin_key_capture(&mut self, action: Action) {
        self.settings.keyboard.capturing =
            (self.settings.keyboard.capturing != Some(action)).then_some(action);
        self.settings.keyboard.error = None;
    }

    /// A key inside a listening chord cell: Escape cancels, a complete chord
    /// becomes the binding, and a bare letter is refused with the reason.
    pub fn capture_key(&mut self, action: Action, event: &KeyEvent) -> bool {
        if event.key == rune::Key::Escape {
            self.settings.keyboard.capturing = None;
            self.settings.keyboard.error = None;
            return true;
        }
        match keymap::capture_chord(event) {
            Ok(chord) => {
                self.settings.keyboard.capturing = None;
                self.settings.keyboard.error = None;
                self.rebind_key(action, Some(chord));
            }
            Err(message) => self.settings.keyboard.error = Some(message.to_string()),
        }
        true
    }
}

pub fn section(s: &AppState, k: &Kit) -> El {
    let mut page = col()
        .key("keyboard-keymap")
        .w_full()
        .gap(crate::ds::SPACE_6);

    let conflicts = s.keymap.conflicts();
    if !conflicts.is_empty() {
        let mut list = col().key("keymap-conflicts").gap(crate::ds::SPACE_3);
        for conflict in conflicts {
            list = list.child(
                row()
                    .key(format!("keymap-conflict-{}", conflict.label()))
                    .gap(crate::ds::SPACE_4)
                    .child(
                        text(conflict.label())
                            .font(k.ty.mono)
                            .color(k.c.warning.fg)
                            .w(110.0),
                    )
                    .child(
                        text(conflict.context)
                            .font(k.ty.small)
                            .color(k.c.text_faint)
                            .w(90.0),
                    )
                    .child(
                        text(conflict.action_labels())
                            .font(k.ty.small)
                            .color(k.c.text_muted)
                            .grow(1.0)
                            .truncate(),
                    ),
            );
        }
        page = page.child(
            super::group(
                k,
                "keymap-conflict-group",
                "Conflicts",
                Some("Two actions answer to the same chord in one context. The first one fires; the other is unreachable until one of them moves."),
            )
            .child(list),
        );
    }

    page = page.child(
        super::group(
            k,
            "keymap-table-group",
            "Shortcuts",
            Some("Click a chord and press the next one. Escape cancels; Reset puts the shipped chord back."),
        )
        .child(table(s, k)),
    );

    if let Some(error) = &s.settings.keyboard.error {
        page = page.child(
            text(error.clone())
                .key("keymap-error")
                .role("status")
                .font(k.ty.small)
                .color(k.c.danger.fg),
        );
    }

    page
}

fn table(s: &AppState, k: &Kit) -> El {
    let header = row()
        .role("row")
        .w_full()
        .h(26.0)
        .px(8.0)
        .items_center()
        .child(
            text("Action")
                .role("columnheader")
                .font(k.ty.text_xsmall_medium)
                .color(k.c.text_faint)
                .grow(1.0),
        )
        .child(
            text("Context")
                .role("columnheader")
                .font(k.ty.text_xsmall_medium)
                .color(k.c.text_faint)
                .w(96.0),
        )
        .child(
            text("Chord")
                .role("columnheader")
                .font(k.ty.text_xsmall_medium)
                .color(k.c.text_faint)
                .w(190.0),
        )
        .child(
            text("Reset")
                .role("columnheader")
                .font(k.ty.text_xsmall_medium)
                .color(k.c.text_faint)
                .w(58.0),
        );
    let mut table = col()
        .key("keyboard-table")
        .role("table")
        .label("Keyboard shortcuts")
        .w_full()
        .gap(1.0)
        .child(header);
    for (index, binding) in keymap::binding_rows().into_iter().enumerate() {
        let action = binding.action;
        let listening = s.settings.keyboard.capturing == Some(action);
        let chord = s
            .keymap
            .chord_label(action)
            .unwrap_or_else(|| "Not bound".to_string());
        let overridden = s.keymap.is_overridden(action);
        let mut chord_button = Button::new(if listening {
            "Press a key…".to_string()
        } else {
            chord.clone()
        })
        .variant(ButtonVariant::Ghost)
        .control_size(ControlSize::Small)
        .on_press(move |s: &mut AppState| s.begin_key_capture(action))
        .build(k)
        .key(format!("chord-{index}"))
        .on_key(move |s: &mut AppState, ev, _| s.capture_key(action, ev));
        if listening {
            chord_button = chord_button.outline(1.0, 0.0, k.c.border_focus);
        }
        if overridden {
            chord_button = chord_button.help(format!("Shipped: {}", binding.display()));
        }
        let mut row = row()
            .key(format!("binding-{index}"))
            .role("row")
            .label(format!("{}: {}", binding.label, chord))
            .w_full()
            .h(30.0)
            .px(8.0)
            .radius(k.radius.sm)
            .items_center()
            .fill(if index % 2 == 0 {
                k.c.surface_hover
            } else {
                rune::Color::TRANSPARENT
            })
            .child(
                text(binding.label)
                    .role("cell")
                    .font(k.ty.small)
                    .color(k.c.text_base)
                    .grow(1.0),
            )
            .child(
                text(action.context())
                    .role("cell")
                    .font(k.ty.small)
                    .color(k.c.text_faint)
                    .w(96.0),
            )
            .child(
                row()
                    .role("cell")
                    .w(190.0)
                    .items_center()
                    .child(chord_button),
            );
        row = row.child(if overridden {
            Button::new("Reset")
                .variant(ButtonVariant::Ghost)
                .control_size(ControlSize::Small)
                .on_press(move |s: &mut AppState| s.reset_key(action))
                .build(k)
                .key(format!("reset-{index}"))
        } else {
            rune::row().w(58.0)
        });
        table = table.child(row);
    }
    table
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use rune::{Key, KeyEvent, Mods};

    use super::*;
    use crate::connection::{Metrics, fixture_store};
    use crate::settings::Section;
    use crate::terminal::SharedTerminal;

    fn state() -> AppState {
        let store = fixture_store(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/snapshot.json"),
        )
        .expect("fixture");
        AppState::for_bench(store, Arc::new(Metrics::default()), SharedTerminal::new())
    }

    fn command(ch: char) -> KeyEvent {
        KeyEvent {
            key: Key::Command(ch),
            mods: Mods {
                cmd: true,
                ..Mods::default()
            },
        }
    }

    #[test]
    fn a_captured_chord_is_stored_under_ui_keymap_and_reset_restores_the_ship() {
        let mut state = state();
        state.open_settings(Some(Section::Keyboard));
        state.begin_key_capture(Action::NewShell);
        assert!(state.capture_key(Action::NewShell, &command('j')));
        assert_eq!(
            state
                .keymap
                .chord_for(Action::NewShell)
                .map(|chord| chord.to_string()),
            Some("mod+j".to_string())
        );
        assert!(
            state
                .pref(crate::keymap::KEYMAP_KEY)
                .is_some_and(|stored| stored.contains("new_terminal")),
            "the override is written to the daemon's app_state"
        );
        assert!(state.settings.keyboard.capturing.is_none());

        state.reset_key(Action::NewShell);
        assert_eq!(
            state
                .keymap
                .chord_for(Action::NewShell)
                .map(|chord| chord.to_string()),
            Some("mod+t".to_string()),
            "Reset puts the shipped chord back"
        );
        assert!(!state.keymap.is_overridden(Action::NewShell));
    }

    #[test]
    fn escape_cancels_a_capture_without_writing_a_binding() {
        let mut state = state();
        state.begin_key_capture(Action::FilePalette);
        assert!(state.capture_key(
            Action::FilePalette,
            &KeyEvent {
                key: Key::Escape,
                mods: Mods::default(),
            }
        ));
        assert!(state.settings.keyboard.capturing.is_none());
        assert!(!state.keymap.is_overridden(Action::FilePalette));
    }

    #[test]
    fn a_conflicting_capture_says_which_action_it_shadows() {
        let mut state = state();
        state.rebind_key(Action::NewShell, keymap::parse_chord("mod+k"));
        assert!(
            state
                .toasts
                .visible()
                .iter()
                .any(|toast| toast.title.contains("already in use")),
            "the conflict is reported as a non-blocking toast"
        );
        assert_eq!(
            state
                .keymap
                .chord_for(Action::NewShell)
                .map(|chord| chord.to_string()),
            Some("mod+k".to_string()),
            "a conflict is reported, not refused"
        );
    }
}

#[cfg(test)]
mod runtime_tests {
    use std::sync::Arc;

    use rune::{Env, Key, KeyEvent, Mods, Runtime};

    use super::*;
    use crate::connection::{Metrics, fixture_store};
    use crate::settings::Section;
    use crate::terminal::SharedTerminal;

    #[test]
    fn clicking_a_chord_cell_and_typing_rebinds_the_running_table() {
        let store = fixture_store(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/snapshot.json"),
        )
        .expect("fixture");
        let mut state =
            AppState::for_bench(store, Arc::new(Metrics::default()), SharedTerminal::new());
        state.open_settings(Some(Section::Keyboard));
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
        rt.frame(0.1);
        let (x, y) = rt.find("chord-0").expect("the first chord cell");
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(0.2);
        rt.key(KeyEvent {
            key: Key::Command('j'),
            mods: Mods {
                cmd: true,
                ..Mods::default()
            },
        });
        rt.frame(0.3);
        assert_eq!(
            rt.state
                .keymap
                .chord_for(crate::keymap::Action::CommandPalette)
                .map(|chord| chord.to_string()),
            Some("mod+j".to_string()),
            "the focused cell took the captured chord"
        );
    }
}
