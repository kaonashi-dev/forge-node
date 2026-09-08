//! `KeyboardEvent` → PTY bytes.
//!
//! The mapping itself is not reimplemented here: `terminal-input` is the one
//! place a key becomes bytes, and `client` re-exports it so a GUI never depends
//! on `protocol` or on a VT engine (§17, ADR-011). This module only translates
//! the WebView's vocabulary — DOM `KeyboardEvent.key` names — into that crate's [`Key`].
//!
//! Nothing here decides *whether* a key belongs to the terminal. The WebView
//! keeps the shell's own chords (⌘T, ⌘W, the palette) and sends what is left,
//! which is also why a `Meta`-modified press never reaches this module.

use client::{encode_key, encode_mouse, paste, Key, Modifiers, MouseButton, MouseEventKind};
use domain::TermModes;
use serde::Deserialize;

/// A key press as the WebView reports it.
#[derive(Clone, Debug, Deserialize)]
pub struct KeyPress {
    /// `KeyboardEvent.key`.
    pub key: String,
    #[serde(default)]
    pub ctrl: bool,
    #[serde(default)]
    pub alt: bool,
    #[serde(default)]
    pub shift: bool,
}

/// The bytes a key press writes to the PTY, or `None` when it produces nothing.
///
/// Returns `None` — rather than an empty vector — for the keys that are not
/// input at all: the modifier keys themselves, and the placeholder names a
/// browser reports mid-composition. An IME commit arrives as text instead, so
/// it never reaches this path.
#[must_use]
pub fn encode(press: &KeyPress, modes: &TermModes) -> Option<Vec<u8>> {
    let mods = Modifiers {
        ctrl: press.ctrl,
        alt: press.alt,
        shift: press.shift,
    };
    let key = match press.key.as_str() {
        "Enter" => Key::Enter,
        "Escape" => Key::Escape,
        "Backspace" => Key::Backspace,
        "Delete" => Key::Delete,
        "Tab" if mods.shift => Key::BackTab,
        "Tab" => Key::Tab,
        "ArrowUp" => Key::Up,
        "ArrowDown" => Key::Down,
        "ArrowLeft" => Key::Left,
        "ArrowRight" => Key::Right,
        "Home" => Key::Home,
        "End" => Key::End,
        "PageUp" => Key::PageUp,
        "PageDown" => Key::PageDown,
        "Insert" => Key::Insert,
        // A composition in progress, a key the browser could not name, and the
        // modifier keys themselves: all of them fire `keydown` and none of them
        // is input.
        "Dead" | "Unidentified" | "Process" | "Shift" | "Control" | "Alt" | "Meta" | "CapsLock"
        | "NumLock" | "ScrollLock" | "ContextMenu" | "OS" => return None,
        name if is_function_key(name) => Key::F(name[1..].parse().ok()?),
        name => {
            let mut chars = name.chars();
            let first = chars.next()?;
            // A multi-character name that is not one of the above is a key this
            // mapping does not know ("AudioVolumeUp", "F13"), not a grapheme.
            if chars.next().is_some() {
                return None;
            }
            Key::Char(first)
        }
    };
    Some(encode_key(key, mods, modes))
}

/// The bytes a mouse event writes to the PTY, or `None` when the active mode
/// does not report it.
///
/// The mode check is the encoder's, not this module's: `Normal` (1000) reports
/// presses and releases but not motion, and a caller that filtered by hand
/// would have to keep that table in a second place.
///
/// An unknown button or kind name is `None` rather than a guess: a wheel event
/// arriving as a left-click is worse for the program under it than nothing.
#[must_use]
pub fn encode_mouse_event(
    button: &str,
    kind: &str,
    col: u16,
    row: u16,
    mods: Modifiers,
    modes: &TermModes,
) -> Option<Vec<u8>> {
    let button = match button {
        "left" => MouseButton::Left,
        "middle" => MouseButton::Middle,
        "right" => MouseButton::Right,
        "wheel_up" => MouseButton::WheelUp,
        "wheel_down" => MouseButton::WheelDown,
        _ => return None,
    };
    let kind = match kind {
        "press" => MouseEventKind::Press,
        "release" => MouseEventKind::Release,
        "motion" => MouseEventKind::Motion,
        _ => return None,
    };
    encode_mouse(button, kind, col, row, mods, modes)
}

/// The bytes pasted text writes to the PTY, bracketed when the mode is active.
#[must_use]
pub fn encode_paste(text: &str, modes: &TermModes) -> Vec<u8> {
    paste(text, modes)
}

/// Text committed by an input method, which is typed rather than pasted: it
/// goes to the PTY verbatim, never wrapped in paste markers.
#[must_use]
pub fn encode_text(text: &str) -> Vec<u8> {
    text.as_bytes().to_vec()
}

/// `F1`–`F12`, and only those: `terminal-input` has no encoding above F12, and
/// `F13` must not silently become `F1`.
fn is_function_key(name: &str) -> bool {
    matches!(
        name,
        "F1" | "F2" | "F3" | "F4" | "F5" | "F6" | "F7" | "F8" | "F9" | "F10" | "F11" | "F12"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(key: &str) -> KeyPress {
        KeyPress {
            key: key.to_string(),
            ctrl: false,
            alt: false,
            shift: false,
        }
    }

    fn modes() -> TermModes {
        TermModes::default()
    }

    #[test]
    fn a_printable_key_is_its_own_bytes() {
        assert_eq!(encode(&press("a"), &modes()), Some(b"a".to_vec()));
        assert_eq!(encode(&press("ñ"), &modes()), Some("ñ".as_bytes().to_vec()));
    }

    #[test]
    fn ctrl_folds_a_letter_into_its_control_byte() {
        let mut key = press("c");
        key.ctrl = true;
        assert_eq!(encode(&key, &modes()), Some(vec![0x03]));
    }

    #[test]
    fn the_dom_arrow_names_reach_the_cursor_family() {
        assert_eq!(
            encode(&press("ArrowUp"), &modes()),
            Some(vec![0x1b, b'[', b'A'])
        );
        let app = TermModes {
            app_cursor_keys: true,
            ..TermModes::default()
        };
        assert_eq!(
            encode(&press("ArrowUp"), &app),
            Some(vec![0x1b, b'O', b'A'])
        );
    }

    #[test]
    fn shift_tab_is_back_tab() {
        let mut key = press("Tab");
        key.shift = true;
        assert_eq!(encode(&key, &modes()), Some(vec![0x1b, b'[', b'Z']));
    }

    #[test]
    fn a_bare_modifier_press_is_not_input() {
        for name in ["Shift", "Control", "Alt", "Meta", "CapsLock", "Dead"] {
            assert_eq!(
                encode(&press(name), &modes()),
                None,
                "{name} produced bytes"
            );
        }
    }

    /// A name this mapping does not know must produce nothing rather than its
    /// first letter: `F13` reaching the PTY as `f` is worse than silence.
    #[test]
    fn an_unknown_multi_character_key_produces_nothing() {
        for name in ["F13", "AudioVolumeUp", "BrowserBack"] {
            assert_eq!(
                encode(&press(name), &modes()),
                None,
                "{name} produced bytes"
            );
        }
    }

    #[test]
    fn function_keys_stop_at_twelve() {
        assert!(encode(&press("F1"), &modes()).is_some_and(|bytes| !bytes.is_empty()));
        assert!(encode(&press("F12"), &modes()).is_some_and(|bytes| !bytes.is_empty()));
        assert_eq!(encode(&press("F13"), &modes()), None);
    }

    #[test]
    fn a_paste_is_bracketed_only_when_the_mode_asks() {
        assert_eq!(encode_paste("ls", &modes()), b"ls".to_vec());
        let bracketed = TermModes {
            bracketed_paste: true,
            ..TermModes::default()
        };
        assert_eq!(
            encode_paste("ls", &bracketed),
            b"\x1b[200~ls\x1b[201~".to_vec()
        );
    }

    /// Committed IME text is typing, not a paste: wrapping it in the markers
    /// would make a shell in bracketed mode treat a typed word as pasted input.
    #[test]
    fn committed_text_is_never_bracketed() {
        assert_eq!(encode_text("漢字"), "漢字".as_bytes().to_vec());
    }

    fn sgr_modes() -> TermModes {
        TermModes {
            mouse_mode: domain::MouseMode::ButtonEvent,
            mouse_sgr: true,
            ..TermModes::default()
        }
    }

    #[test]
    fn a_click_reaches_a_program_that_asked_for_the_mouse() {
        let bytes = encode_mouse_event("left", "press", 4, 2, Modifiers::default(), &sgr_modes());
        assert_eq!(bytes, Some(b"\x1b[<0;5;3M".to_vec()));
    }

    /// Nothing is reported while the program never asked: the pane keeps the
    /// mouse for selection, and bytes sent anyway would land in the shell.
    #[test]
    fn nothing_is_reported_with_the_mouse_off() {
        assert_eq!(
            encode_mouse_event("left", "press", 0, 0, Modifiers::default(), &modes()),
            None
        );
    }

    /// A name this shim does not know produces nothing rather than a guess: a
    /// wheel event arriving as a left click is worse than no event at all.
    #[test]
    fn an_unknown_button_or_kind_produces_nothing() {
        let modes = sgr_modes();
        assert_eq!(
            encode_mouse_event("back", "press", 0, 0, Modifiers::default(), &modes),
            None
        );
        assert_eq!(
            encode_mouse_event("left", "hover", 0, 0, Modifiers::default(), &modes),
            None
        );
    }
}
