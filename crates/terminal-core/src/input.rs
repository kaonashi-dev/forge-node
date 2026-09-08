// Pure keyboard/mouse to PTY byte mapping (§11.6).
//
// This source is also compiled by the `terminal-input` leaf crate so the GUI
// can reuse the exact mapping without depending on PTYs or a terminal engine.

use domain::{MouseMode, TermModes};

/// A logical key press, provider-agnostic (no Kitty protocol).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    /// A character-producing key (already case-folded by the caller).
    Char(char),
    Enter,
    Escape,
    Backspace,
    /// The Delete (forward-delete) key.
    Delete,
    Tab,
    /// Back-tab (Shift+Tab as a distinct key).
    BackTab,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Insert,
    /// Function key F1–F12.
    F(u8),
}

/// Active modifier keys.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
}

impl Modifiers {
    /// No modifiers held.
    #[must_use]
    pub const fn none() -> Self {
        Self {
            ctrl: false,
            alt: false,
            shift: false,
        }
    }

    /// Only Ctrl held.
    #[must_use]
    pub const fn ctrl() -> Self {
        Self {
            ctrl: true,
            alt: false,
            shift: false,
        }
    }

    /// Only Alt/Meta held.
    #[must_use]
    pub const fn alt() -> Self {
        Self {
            ctrl: false,
            alt: true,
            shift: false,
        }
    }

    /// Only Shift held.
    #[must_use]
    pub const fn shift() -> Self {
        Self {
            ctrl: false,
            alt: false,
            shift: true,
        }
    }

    fn any(self) -> bool {
        self.ctrl || self.alt || self.shift
    }

    /// xterm modifier parameter: `1 + shift + 2*alt + 4*ctrl` (only meaningful
    /// when > 1).
    fn xterm_code(self) -> u8 {
        1 + u8::from(self.shift) + (u8::from(self.alt) << 1) + (u8::from(self.ctrl) << 2)
    }
}

const ESC: u8 = 0x1b;

/// Prefix `bytes` with ESC when Alt/Meta is held (Meta = ESC prefix, §11.6).
fn with_alt(mods: Modifiers, mut bytes: Vec<u8>) -> Vec<u8> {
    if mods.alt {
        let mut out = Vec::with_capacity(bytes.len() + 1);
        out.push(ESC);
        out.append(&mut bytes);
        out
    } else {
        bytes
    }
}

fn char_bytes(c: char) -> Vec<u8> {
    let mut buf = [0u8; 4];
    c.encode_utf8(&mut buf).as_bytes().to_vec()
}

/// The control byte for `Ctrl`+`c`, if one exists (0x00–0x1f, plus 0x7f for `?`).
fn ctrl_byte(c: char) -> Option<u8> {
    match c {
        ' ' | '@' => Some(0x00),
        'a'..='z' => Some(c as u8 - b'a' + 1),
        'A'..='Z' => Some(c as u8 - b'A' + 1),
        '[' => Some(0x1b),
        '\\' => Some(0x1c),
        ']' => Some(0x1d),
        '^' => Some(0x1e),
        '_' => Some(0x1f),
        '?' => Some(0x7f),
        _ => None,
    }
}

/// A `CSI <n> ~` sequence, adding the `;<mod>` parameter when modifiers are held.
fn tilde(n: u8, mods: Modifiers) -> Vec<u8> {
    let mut v = vec![ESC, b'['];
    if mods.any() {
        v.extend_from_slice(format!("{n};{}~", mods.xterm_code()).as_bytes());
    } else {
        v.extend_from_slice(format!("{n}~").as_bytes());
    }
    v
}

/// Encode a cursor key (arrows and Home/End), honoring app-cursor-keys mode and
/// modifiers.
///
/// These six keys share one encoding family in `xterm`, which is what the
/// terminfo entry for the `TERM=xterm-256color` we advertise to children (§13.3)
/// describes: `kcuu1=\EOA`/`khome=\EOH`/`kend=\EOF` in application-cursor mode,
/// the `CSI` form otherwise, and `CSI 1 ; <mod> <letter>` whenever a modifier is
/// held.
fn cursor_key(letter: u8, mods: Modifiers, modes: &TermModes) -> Vec<u8> {
    if mods.any() {
        // Modified cursor keys always use the CSI form with a modifier parameter.
        let mut v = vec![ESC, b'['];
        v.extend_from_slice(format!("1;{}", mods.xterm_code()).as_bytes());
        v.push(letter);
        v
    } else if modes.app_cursor_keys {
        vec![ESC, b'O', letter] // SS3
    } else {
        vec![ESC, b'[', letter] // CSI
    }
}

fn function_key(n: u8, mods: Modifiers) -> Vec<u8> {
    match n {
        1..=4 => {
            let letter = b'P' + (n - 1); // P, Q, R, S
            if mods.any() {
                let mut v = vec![ESC, b'['];
                v.extend_from_slice(format!("1;{}", mods.xterm_code()).as_bytes());
                v.push(letter);
                v
            } else {
                vec![ESC, b'O', letter] // SS3
            }
        }
        5 => tilde(15, mods),
        6 => tilde(17, mods),
        7 => tilde(18, mods),
        8 => tilde(19, mods),
        9 => tilde(20, mods),
        10 => tilde(21, mods),
        11 => tilde(23, mods),
        12 => tilde(24, mods),
        _ => Vec::new(),
    }
}

/// Map a key press to the bytes to write to the PTY (§11.6).
#[must_use]
pub fn encode_key(key: Key, mods: Modifiers, modes: &TermModes) -> Vec<u8> {
    match key {
        Key::Char(c) => {
            if mods.ctrl {
                match ctrl_byte(c) {
                    Some(b) => with_alt(mods, vec![b]),
                    None => with_alt(mods, char_bytes(c)),
                }
            } else {
                with_alt(mods, char_bytes(c))
            }
        }
        Key::Enter => with_alt(mods, vec![b'\r']),
        Key::Escape => with_alt(mods, vec![ESC]),
        Key::Backspace => {
            let b = if mods.ctrl { 0x08 } else { 0x7f };
            with_alt(mods, vec![b])
        }
        Key::Tab => {
            if mods.shift {
                vec![ESC, b'[', b'Z'] // CSI Z (back-tab)
            } else {
                with_alt(mods, vec![b'\t'])
            }
        }
        Key::BackTab => vec![ESC, b'[', b'Z'],
        Key::Up => cursor_key(b'A', mods, modes),
        Key::Down => cursor_key(b'B', mods, modes),
        Key::Right => cursor_key(b'C', mods, modes),
        Key::Left => cursor_key(b'D', mods, modes),
        // Home/End belong to the cursor-key family, not to the `CSI <n> ~`
        // family: `xterm-256color` declares `khome=\EOH`/`kend=\EOF`, so an
        // ncurses program compares incoming bytes against those, and the VT220
        // `CSI 1~`/`CSI 4~` forms this used to emit never matched.
        Key::Home => cursor_key(b'H', mods, modes),
        Key::End => cursor_key(b'F', mods, modes),
        Key::Insert => tilde(2, mods),
        Key::Delete => tilde(3, mods),
        Key::PageUp => tilde(5, mods),
        Key::PageDown => tilde(6, mods),
        Key::F(n) => function_key(n, mods),
    }
}

/// Encode pasted text: wrapped in bracketed-paste markers when the mode is
/// active, otherwise sent verbatim (§11.6). In bracketed mode any embedded end
/// marker is stripped so pasted content can't spoof the terminator.
#[must_use]
pub fn paste(text: &str, modes: &TermModes) -> Vec<u8> {
    if modes.bracketed_paste {
        let mut v = Vec::with_capacity(text.len() + 12);
        v.extend_from_slice(b"\x1b[200~");
        v.extend_from_slice(text.replace("\x1b[201~", "").as_bytes());
        v.extend_from_slice(b"\x1b[201~");
        v
    } else {
        text.as_bytes().to_vec()
    }
}

/// A mouse button (wheel events reported as buttons 64/65).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Middle,
    Right,
    WheelUp,
    WheelDown,
}

/// The kind of mouse event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseEventKind {
    Press,
    Release,
    /// Motion (drag when a button is held, or free motion in any-event mode).
    Motion,
}

/// Encode a mouse event for the active reporting mode (§11.6).
///
/// `col`/`row` are **0-based** cell coordinates; the protocol's 1-based values
/// are produced here. Emits the SGR (1006) form when [`TermModes::mouse_sgr`] is
/// set, otherwise the normal (1000/1002) byte-offset form (values clamped to
/// 255).
///
/// Returns `None` when the event must not be reported at all in the active mode:
/// - [`MouseMode::Off`]: nothing is reported.
/// - [`MouseMode::Normal`] (1000): button press/release only. A program that
///   only enabled 1000 does not expect motion reports, and sending them makes it
///   misread the stream — 1002 is the mode that asks for drag motion.
///
/// In [`MouseMode::ButtonEvent`] (1002) the caller is responsible for only
/// emitting motion while a button is held; [`MouseMode::AnyEvent`] (1003) takes
/// motion unconditionally.
#[must_use]
pub fn encode_mouse(
    button: MouseButton,
    kind: MouseEventKind,
    col: u16,
    row: u16,
    mods: Modifiers,
    modes: &TermModes,
) -> Option<Vec<u8>> {
    match modes.mouse_mode {
        MouseMode::Off => return None,
        MouseMode::Normal if matches!(kind, MouseEventKind::Motion) => return None,
        _ => {}
    }

    let mut cb: u32 = match button {
        MouseButton::Left => 0,
        MouseButton::Middle => 1,
        MouseButton::Right => 2,
        MouseButton::WheelUp => 64,
        MouseButton::WheelDown => 65,
    };
    if matches!(kind, MouseEventKind::Motion) {
        cb += 32;
    }
    if mods.shift {
        cb += 4;
    }
    if mods.alt {
        cb += 8;
    }
    if mods.ctrl {
        cb += 16;
    }

    if modes.mouse_sgr {
        let final_byte = if matches!(kind, MouseEventKind::Release) {
            'm'
        } else {
            'M'
        };
        Some(
            format!(
                "\x1b[<{};{};{}{}",
                cb,
                u32::from(col) + 1,
                u32::from(row) + 1,
                final_byte
            )
            .into_bytes(),
        )
    } else {
        // Normal encoding: release is button 3 (keeping motion/modifier bits);
        // all values are offset by 32 and clamped into a byte.
        let cb_norm = if matches!(kind, MouseEventKind::Release) {
            3 + (cb & !0b11)
        } else {
            cb
        };
        let bx = (32 + cb_norm).min(255) as u8;
        let cx = (32 + u32::from(col) + 1).min(255) as u8;
        let cy = (32 + u32::from(row) + 1).min(255) as u8;
        Some(vec![ESC, b'[', b'M', bx, cx, cy])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn modes() -> TermModes {
        TermModes::default()
    }

    #[test]
    fn arrows_normal_vs_app_cursor_keys() {
        let mut m = modes();
        assert_eq!(encode_key(Key::Up, Modifiers::none(), &m), b"\x1b[A");
        assert_eq!(encode_key(Key::Left, Modifiers::none(), &m), b"\x1b[D");

        m.app_cursor_keys = true;
        assert_eq!(encode_key(Key::Up, Modifiers::none(), &m), b"\x1bOA");
        assert_eq!(encode_key(Key::Right, Modifiers::none(), &m), b"\x1bOC");
    }

    #[test]
    fn modified_arrow_uses_csi_even_in_app_mode() {
        let mut m = modes();
        m.app_cursor_keys = true;
        // Ctrl+Up => CSI 1;5 A
        assert_eq!(encode_key(Key::Up, Modifiers::ctrl(), &m), b"\x1b[1;5A");
    }

    #[test]
    fn ctrl_c_is_etx() {
        assert_eq!(
            encode_key(Key::Char('c'), Modifiers::ctrl(), &modes()),
            vec![0x03]
        );
        // Case-insensitive.
        assert_eq!(
            encode_key(Key::Char('C'), Modifiers::ctrl(), &modes()),
            vec![0x03]
        );
    }

    #[test]
    fn ctrl_bracket_is_escape() {
        assert_eq!(
            encode_key(Key::Char('['), Modifiers::ctrl(), &modes()),
            vec![0x1b]
        );
    }

    #[test]
    fn alt_x_is_esc_prefixed() {
        assert_eq!(
            encode_key(Key::Char('x'), Modifiers::alt(), &modes()),
            vec![0x1b, b'x']
        );
    }

    #[test]
    fn plain_printable() {
        assert_eq!(
            encode_key(Key::Char('a'), Modifiers::none(), &modes()),
            b"a"
        );
        // UTF-8 multibyte.
        assert_eq!(
            encode_key(Key::Char('é'), Modifiers::none(), &modes()),
            "é".as_bytes()
        );
    }

    #[test]
    fn enter_backspace_tab() {
        assert_eq!(
            encode_key(Key::Enter, Modifiers::none(), &modes()),
            vec![b'\r']
        );
        assert_eq!(
            encode_key(Key::Backspace, Modifiers::none(), &modes()),
            vec![0x7f]
        );
        assert_eq!(
            encode_key(Key::Tab, Modifiers::none(), &modes()),
            vec![b'\t']
        );
        assert_eq!(
            encode_key(Key::Tab, Modifiers::shift(), &modes()),
            b"\x1b[Z"
        );
        assert_eq!(
            encode_key(Key::BackTab, Modifiers::none(), &modes()),
            b"\x1b[Z"
        );
    }

    #[test]
    fn function_keys() {
        assert_eq!(
            encode_key(Key::F(1), Modifiers::none(), &modes()),
            b"\x1bOP"
        );
        assert_eq!(
            encode_key(Key::F(4), Modifiers::none(), &modes()),
            b"\x1bOS"
        );
        assert_eq!(
            encode_key(Key::F(5), Modifiers::none(), &modes()),
            b"\x1b[15~"
        );
        assert_eq!(
            encode_key(Key::F(12), Modifiers::none(), &modes()),
            b"\x1b[24~"
        );
    }

    #[test]
    fn tilde_navigation_keys() {
        assert_eq!(
            encode_key(Key::Insert, Modifiers::none(), &modes()),
            b"\x1b[2~"
        );
        assert_eq!(
            encode_key(Key::Delete, Modifiers::none(), &modes()),
            b"\x1b[3~"
        );
        assert_eq!(
            encode_key(Key::PageUp, Modifiers::none(), &modes()),
            b"\x1b[5~"
        );
        assert_eq!(
            encode_key(Key::PageDown, Modifiers::none(), &modes()),
            b"\x1b[6~"
        );
        // Modified: the xterm modifier parameter is spliced in.
        assert_eq!(
            encode_key(Key::Delete, Modifiers::ctrl(), &modes()),
            b"\x1b[3;5~"
        );
    }

    #[test]
    fn home_end_follow_the_cursor_key_family() {
        // `xterm-256color` declares khome=\EOH / kend=\EOF, so Home/End must use
        // the cursor-key encoding, not the VT220 `CSI 1~` / `CSI 4~` forms.
        let mut m = modes();
        assert_eq!(encode_key(Key::Home, Modifiers::none(), &m), b"\x1b[H");
        assert_eq!(encode_key(Key::End, Modifiers::none(), &m), b"\x1b[F");

        m.app_cursor_keys = true;
        assert_eq!(encode_key(Key::Home, Modifiers::none(), &m), b"\x1bOH");
        assert_eq!(encode_key(Key::End, Modifiers::none(), &m), b"\x1bOF");

        // Modified Home/End use CSI with the modifier parameter, in both modes.
        assert_eq!(encode_key(Key::Home, Modifiers::ctrl(), &m), b"\x1b[1;5H");
        m.app_cursor_keys = false;
        assert_eq!(encode_key(Key::End, Modifiers::shift(), &m), b"\x1b[1;2F");
    }

    #[test]
    fn ctrl_covers_the_whole_letter_range() {
        // Ctrl+a..Ctrl+z map onto 0x01..=0x1a (§11.6 "Ctrl+letra").
        for (i, c) in ('a'..='z').enumerate() {
            let expected = u8::try_from(i + 1).unwrap();
            assert_eq!(
                encode_key(Key::Char(c), Modifiers::ctrl(), &modes()),
                vec![expected],
                "ctrl+{c}"
            );
        }
        // The non-letter control slots.
        for (c, byte) in [
            (' ', 0x00u8),
            ('@', 0x00),
            ('\\', 0x1c),
            (']', 0x1d),
            ('^', 0x1e),
            ('_', 0x1f),
            ('?', 0x7f),
        ] {
            assert_eq!(
                encode_key(Key::Char(c), Modifiers::ctrl(), &modes()),
                vec![byte],
                "ctrl+{c}"
            );
        }
    }

    #[test]
    fn alt_meta_prefixes_every_esc_capable_key() {
        // Alt/Meta is an ESC prefix (§11.6), including on top of Ctrl.
        assert_eq!(
            encode_key(
                Key::Char('c'),
                Modifiers {
                    ctrl: true,
                    alt: true,
                    shift: false
                },
                &modes()
            ),
            vec![ESC, 0x03]
        );
        assert_eq!(
            encode_key(Key::Enter, Modifiers::alt(), &modes()),
            vec![ESC, b'\r']
        );
        assert_eq!(
            encode_key(Key::Backspace, Modifiers::alt(), &modes()),
            vec![ESC, 0x7f]
        );
        // Alt+arrow is a modified cursor key, not an ESC-prefixed one.
        assert_eq!(
            encode_key(Key::Left, Modifiers::alt(), &modes()),
            b"\x1b[1;3D"
        );
    }

    #[test]
    fn ctrl_backspace_and_delete_are_distinct() {
        assert_eq!(
            encode_key(Key::Backspace, Modifiers::ctrl(), &modes()),
            vec![0x08]
        );
        assert_eq!(
            encode_key(Key::Delete, Modifiers::none(), &modes()),
            b"\x1b[3~"
        );
    }

    #[test]
    fn modified_function_keys() {
        // F1-F4 switch from SS3 to the CSI form once a modifier is held.
        assert_eq!(
            encode_key(Key::F(1), Modifiers::ctrl(), &modes()),
            b"\x1b[1;5P"
        );
        assert_eq!(
            encode_key(Key::F(5), Modifiers::shift(), &modes()),
            b"\x1b[15;2~"
        );
        // Out-of-range function keys produce nothing rather than garbage.
        assert!(encode_key(Key::F(13), Modifiers::none(), &modes()).is_empty());
    }

    #[test]
    fn bracketed_vs_plain_paste() {
        let mut m = modes();
        assert_eq!(paste("hi", &m), b"hi");

        m.bracketed_paste = true;
        assert_eq!(paste("hi", &m), b"\x1b[200~hi\x1b[201~");
        // Embedded terminator is stripped.
        assert_eq!(paste("a\x1b[201~b", &m), b"\x1b[200~ab\x1b[201~");
    }

    #[test]
    fn sgr_mouse_encode() {
        let mut m = modes();
        m.mouse_mode = MouseMode::Normal;
        m.mouse_sgr = true;

        // Left press at (col=0,row=0) => 1-based 1;1, press 'M'.
        assert_eq!(
            encode_mouse(
                MouseButton::Left,
                MouseEventKind::Press,
                0,
                0,
                Modifiers::none(),
                &m
            )
            .unwrap(),
            b"\x1b[<0;1;1M"
        );
        // Left release => same coords, 'm'.
        assert_eq!(
            encode_mouse(
                MouseButton::Left,
                MouseEventKind::Release,
                4,
                2,
                Modifiers::none(),
                &m
            )
            .unwrap(),
            b"\x1b[<0;5;3m"
        );
    }

    #[test]
    fn normal_mouse_encode_and_off_gate() {
        let mut m = modes();
        // Off => None.
        assert!(encode_mouse(
            MouseButton::Left,
            MouseEventKind::Press,
            0,
            0,
            Modifiers::none(),
            &m
        )
        .is_none());

        m.mouse_mode = MouseMode::Normal;
        // Left press at (0,0): ESC [ M, 32+0, 32+1, 32+1.
        assert_eq!(
            encode_mouse(
                MouseButton::Left,
                MouseEventKind::Press,
                0,
                0,
                Modifiers::none(),
                &m
            )
            .unwrap(),
            vec![0x1b, b'[', b'M', 32, 33, 33]
        );
    }

    #[test]
    fn motion_is_reported_only_in_the_modes_that_asked_for_it() {
        let mut m = modes();
        m.mouse_sgr = true;

        // 1000 (Normal) is press/release only: motion must not be sent.
        m.mouse_mode = MouseMode::Normal;
        assert!(
            encode_mouse(
                MouseButton::Left,
                MouseEventKind::Motion,
                3,
                4,
                Modifiers::none(),
                &m
            )
            .is_none(),
            "mode 1000 must not report motion"
        );
        // Press/release still work in 1000.
        assert!(encode_mouse(
            MouseButton::Left,
            MouseEventKind::Press,
            3,
            4,
            Modifiers::none(),
            &m
        )
        .is_some());

        // 1002 (ButtonEvent) reports drag motion, with the +32 motion bit.
        m.mouse_mode = MouseMode::ButtonEvent;
        assert_eq!(
            encode_mouse(
                MouseButton::Left,
                MouseEventKind::Motion,
                3,
                4,
                Modifiers::none(),
                &m
            )
            .unwrap(),
            b"\x1b[<32;4;5M"
        );

        // 1003 (AnyEvent) reports motion too.
        m.mouse_mode = MouseMode::AnyEvent;
        assert!(encode_mouse(
            MouseButton::Left,
            MouseEventKind::Motion,
            0,
            0,
            Modifiers::none(),
            &m
        )
        .is_some());
    }

    #[test]
    fn sgr_encodes_modifiers_and_wheel_buttons() {
        let mut m = modes();
        m.mouse_mode = MouseMode::Normal;
        m.mouse_sgr = true;

        // Wheel up is button 64; shift(+4)/alt(+8)/ctrl(+16) stack on top.
        assert_eq!(
            encode_mouse(
                MouseButton::WheelUp,
                MouseEventKind::Press,
                0,
                0,
                Modifiers::none(),
                &m
            )
            .unwrap(),
            b"\x1b[<64;1;1M"
        );
        assert_eq!(
            encode_mouse(
                MouseButton::Right,
                MouseEventKind::Press,
                0,
                0,
                Modifiers {
                    ctrl: true,
                    alt: false,
                    shift: true
                },
                &m
            )
            .unwrap(),
            // 2 (right) + 4 (shift) + 16 (ctrl) = 22
            b"\x1b[<22;1;1M"
        );
    }

    #[test]
    fn normal_encoding_releases_as_button_three() {
        let mut m = modes();
        m.mouse_mode = MouseMode::Normal;
        // Legacy (non-SGR) encoding cannot express *which* button was released,
        // so every release is button 3.
        assert_eq!(
            encode_mouse(
                MouseButton::Right,
                MouseEventKind::Release,
                0,
                0,
                Modifiers::none(),
                &m
            )
            .unwrap(),
            vec![0x1b, b'[', b'M', 32 + 3, 33, 33]
        );
    }
}
