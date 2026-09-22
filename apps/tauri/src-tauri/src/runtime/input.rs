//! Translate WebView keys and cursor gestures into `client`'s shared input encoder.
//! Cursor navigation reads the passive grid; the daemon remains the only VT engine.

use client::{
    encode_key, encode_mouse, paste, CellGrid, Key, Modifiers, MouseButton, MouseEventKind,
};
use domain::{CellFlags, MouseMode, TermModes};
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

pub fn encode_terminal(press: &KeyPress, modes: &TermModes) -> Option<Vec<u8>> {
    if press.alt && !press.ctrl && !press.shift && shell_navigation(modes) {
        let key = match press.key.as_str() {
            "ArrowLeft" => Some(Key::Char('b')),
            "ArrowRight" => Some(Key::Char('f')),
            _ => None,
        };
        if let Some(key) = key {
            return Some(encode_key(key, Modifiers::alt(), modes));
        }
    }
    encode(press, modes)
}

fn shell_navigation(modes: &TermModes) -> bool {
    !modes.alt_screen && modes.mouse_mode == MouseMode::Off
}

const MAX_CURSOR_STEPS: usize = 4096;

/// Only traverse soft-wrapped rows: up/down would recall shell history.
pub fn encode_cursor_move(grid: &CellGrid, row: u16, col: u16) -> Option<Vec<u8>> {
    if !shell_navigation(&grid.modes) || !grid.cursor.visible {
        return None;
    }
    let width = usize::from(grid.size.cols);
    let cursor = (usize::from(grid.cursor.line), usize::from(grid.cursor.col));
    let mut target = (usize::from(row), usize::from(col));
    if width == 0 || target.1 >= width || cursor.1 >= width {
        return None;
    }
    let target_row = grid.visible.get(target.0)?;
    if target_row
        .cells
        .get(target.1)?
        .flags
        .contains(CellFlags::WIDE_SPACER)
    {
        target.1 = target.1.checked_sub(1)?;
    }
    let (start, end, key) = if target < cursor {
        (target, cursor, Key::Left)
    } else {
        (cursor, target, Key::Right)
    };
    let distance = (end.0 - start.0)
        .checked_mul(width)?
        .checked_add(end.1)?
        .checked_sub(start.1)?;
    if distance == 0 || distance > MAX_CURSOR_STEPS {
        return None;
    }
    let mut steps = 0;
    for line in start.0..=end.0 {
        let row = grid.visible.get(line)?;
        if line < end.0 && !row.wrapped {
            return None;
        }
        let from = if line == start.0 { start.1 } else { 0 };
        let to = if line == end.0 { end.1 } else { width };
        steps += row
            .cells
            .get(from..to)?
            .iter()
            .filter(|cell| !cell.flags.contains(CellFlags::WIDE_SPACER))
            .count();
    }
    let arrow = encode_key(key, Modifiers::none(), &grid.modes);
    Some(arrow.repeat(steps))
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

/// Most bytes one paste into the *editor* may carry.
///
/// Mirrors `editor_core::limits::MAX_DOCUMENT_BYTES`. Not shared as a constant
/// because this host does not depend on the editor's crate, and a mismatch is
/// safe in both directions: the editor refuses an over-budget transaction
/// anyway, and this only bounds what it has to hold while deciding.
pub const MAX_EDITOR_PASTE_BYTES: usize = 2 * 1024 * 1024;

/// A paste for the editor, clamped before it is encoded.
///
/// crossterm 0.29 accumulates a whole bracketed paste into a `String` before it
/// delivers `Event::Paste`, so the editor's document budget refuses an oversize
/// paste only once the bytes are already resident. Clamping the *source* is the
/// half of that debt this side owns: a paste that arrives over this route
/// cannot make the peak larger than the budget. A terminal paste is deliberately
/// not clamped — that text is going to a shell, and truncating what somebody
/// pasted into one is a worse failure than the allocation.
#[must_use]
pub fn encode_editor_paste(text: &str, modes: &TermModes) -> (Vec<u8>, bool) {
    if text.len() <= MAX_EDITOR_PASTE_BYTES {
        return (paste(text, modes), false);
    }
    let mut cut = MAX_EDITOR_PASTE_BYTES;
    while cut > 0 && !text.is_char_boundary(cut) {
        cut -= 1;
    }
    (paste(&text[..cut], modes), true)
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
    use domain::{Cell, Cursor, PtySize, Row, TerminalSnapshot};

    fn cursor_grid(cols: u16, rows: u16, line: u16, col: u16) -> CellGrid {
        CellGrid::from_snapshot(&TerminalSnapshot {
            seq: 1,
            size: PtySize {
                cols,
                rows,
                pixel_width: 0,
                pixel_height: 0,
            },
            visible: (0..rows).map(|_| Row::blank(cols)).collect(),
            scrollback_tail: Vec::new(),
            scrollback_len: 0,
            scrollback_generation: 0,
            cursor: Cursor {
                line,
                col,
                visible: true,
                ..Cursor::default()
            },
            modes: TermModes::default(),
            title: None,
        })
    }

    #[test]
    fn terminal_alt_arrows_use_shell_word_bindings() {
        for (name, expected) in [("ArrowLeft", b"\x1bb"), ("ArrowRight", b"\x1bf")] {
            let mut key = press(name);
            key.alt = true;
            assert_eq!(encode_terminal(&key, &modes()), Some(expected.to_vec()));
            let app = TermModes {
                app_cursor_keys: true,
                ..modes()
            };
            assert_eq!(encode_terminal(&key, &app), Some(expected.to_vec()));
            for modes in [
                TermModes {
                    alt_screen: true,
                    ..modes()
                },
                TermModes {
                    mouse_mode: MouseMode::Normal,
                    ..modes()
                },
            ] {
                assert_eq!(encode_terminal(&key, &modes), encode(&key, &modes));
            }
            key.shift = true;
            assert_eq!(encode_terminal(&key, &modes()), encode(&key, &modes()));
            key.shift = false;
            key.ctrl = true;
            assert_eq!(encode_terminal(&key, &modes()), encode(&key, &modes()));
        }
    }

    #[test]
    fn clicks_batch_horizontal_moves_and_honor_application_cursor_mode() {
        let mut grid = cursor_grid(40, 3, 1, 24);
        assert_eq!(encode_cursor_move(&grid, 1, 10), Some(b"\x1b[D".repeat(14)));
        assert_eq!(encode_cursor_move(&grid, 1, 30), Some(b"\x1b[C".repeat(6)));
        assert_eq!(encode_cursor_move(&grid, 1, 24), None);
        grid.modes.app_cursor_keys = true;
        assert_eq!(encode_cursor_move(&grid, 1, 10), Some(b"\x1bOD".repeat(14)));
    }

    #[test]
    fn clicks_cross_soft_wraps_but_never_hard_line_breaks() {
        let mut grid = cursor_grid(10, 3, 2, 3);
        assert_eq!(encode_cursor_move(&grid, 0, 8), None);
        grid.visible[0].wrapped = true;
        grid.visible[1].wrapped = true;
        assert_eq!(encode_cursor_move(&grid, 0, 8), Some(b"\x1b[D".repeat(15)));
        grid.cursor.line = 0;
        grid.cursor.col = 8;
        assert_eq!(encode_cursor_move(&grid, 2, 3), Some(b"\x1b[C".repeat(15)));
        grid.visible[1].wrapped = false;
        assert_eq!(encode_cursor_move(&grid, 2, 3), None);
    }

    #[test]
    fn wide_and_combining_glyphs_are_single_cursor_steps() {
        let mut grid = cursor_grid(6, 1, 0, 4);
        grid.visible[0].cells = vec![
            Cell {
                text: "a".into(),
                ..Cell::default()
            },
            Cell {
                text: "界".into(),
                flags: CellFlags::WIDE_CHAR,
                ..Cell::default()
            },
            Cell {
                flags: CellFlags::WIDE_SPACER,
                ..Cell::default()
            },
            Cell {
                text: "e\u{301}".into(),
                ..Cell::default()
            },
            Cell::default(),
            Cell::default(),
        ]
        .into();
        assert_eq!(encode_cursor_move(&grid, 0, 0), Some(b"\x1b[D".repeat(3)));
        assert_eq!(encode_cursor_move(&grid, 0, 2), Some(b"\x1b[D".repeat(2)));
        grid.cursor.col = 0;
        assert_eq!(encode_cursor_move(&grid, 0, 4), Some(b"\x1b[C".repeat(3)));
        assert_eq!(encode_cursor_move(&grid, 0, 2), Some(b"\x1b[C".to_vec()));
    }

    #[test]
    fn clicks_ignore_full_screen_mouse_reporting_hidden_cursors_and_invalid_targets() {
        let mut grid = cursor_grid(40, 3, 1, 24);
        for modes in [
            TermModes {
                alt_screen: true,
                ..modes()
            },
            TermModes {
                mouse_mode: MouseMode::Normal,
                ..modes()
            },
            TermModes {
                mouse_mode: MouseMode::ButtonEvent,
                ..modes()
            },
            TermModes {
                mouse_mode: MouseMode::AnyEvent,
                ..modes()
            },
        ] {
            grid.modes = modes;
            assert_eq!(encode_cursor_move(&grid, 1, 10), None);
        }
        grid.modes = modes();
        assert_eq!(encode_cursor_move(&grid, 1, 40), None);
        assert_eq!(encode_cursor_move(&grid, 3, 10), None);
        assert_eq!(encode_cursor_move(&grid, u16::MAX, u16::MAX), None);
        grid.cursor.visible = false;
        assert_eq!(encode_cursor_move(&grid, 1, 10), None);
    }

    #[test]
    fn cursor_move_budget_is_checked_before_building_input() {
        let mut grid = cursor_grid(256, 18, 0, 0);
        for row in &mut grid.visible {
            row.wrapped = true;
        }
        assert_eq!(
            encode_cursor_move(&grid, 16, 0),
            Some(b"\x1b[C".repeat(MAX_CURSOR_STEPS))
        );
        assert_eq!(encode_cursor_move(&grid, 16, 1), None);
    }

    /// The editor's document budget refuses an oversize paste only once the
    /// bytes are resident; clamping the source is the half this side owns.
    #[test]
    fn an_oversize_editor_paste_is_cut_before_it_is_encoded() {
        let modes = TermModes::default();
        let small = "x".repeat(16);
        let (bytes, cut) = encode_editor_paste(&small, &modes);
        assert!(!cut);
        assert_eq!(bytes, encode_paste(&small, &modes));

        let huge = "x".repeat(MAX_EDITOR_PASTE_BYTES + 1024);
        let (bytes, cut) = encode_editor_paste(&huge, &modes);
        assert!(cut);
        assert!(
            bytes.len() <= MAX_EDITOR_PASTE_BYTES + 16,
            "plus the markers"
        );
    }

    /// Half a multi-byte character is not text, so the cut walks back to a
    /// boundary rather than splitting one.
    #[test]
    fn the_cut_lands_on_a_character_boundary() {
        let modes = TermModes::default();
        // One byte over the budget, ending inside a three-byte character.
        let mut text = "a".repeat(MAX_EDITOR_PASTE_BYTES - 1);
        text.push('\u{4e2d}');
        let (bytes, cut) = encode_editor_paste(&text, &modes);
        assert!(cut);
        assert!(std::str::from_utf8(&bytes).is_ok());
    }

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
    fn shift_enter_is_esc_cr() {
        let mut key = press("Enter");
        key.shift = true;
        assert_eq!(encode(&key, &modes()), Some(vec![0x1b, b'\r']));
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
