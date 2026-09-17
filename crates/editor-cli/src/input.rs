//! Turns the GUI's structured input into the events `App` already handles.
//!
//! The DOM surface has no PTY, so a keystroke arrives named rather than as an
//! escape sequence. Everything here is a translation: the key table, the
//! commands and the selection model stay in `app` and `editor-core`, so the
//! two surfaces cannot drift into two editors.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use editor_control::{modifiers as mods, EditorInput, WireKey, WireMouseKind};

use crate::app::App;

/// Apply one event from the GUI.
pub fn apply(app: &mut App, event: EditorInput) {
    match event {
        EditorInput::Key { key, modifiers } => {
            if let Some(code) = key_code(key) {
                app.handle_key(KeyEvent::new(code, key_modifiers(modifiers)));
            }
        }
        // A paste and a resolved IME composition are the same thing here: text
        // the person committed, which `paste` inserts without running the key
        // table over it.
        EditorInput::Text(text) => app.paste(&text),
        EditorInput::Mouse {
            kind,
            line,
            column,
            modifiers,
        } => {
            if let Some(event) = mouse_event(app, kind, line, column, modifiers) {
                app.handle_mouse(event);
            }
        }
        EditorInput::Wheel { lines } => wheel(app, lines),
        // The GUI's container is what scrolls and its textarea is what holds
        // the keyboard; there is nothing for the host to do with either.
        EditorInput::Focus(_) => {}
        _ => {}
    }
}

fn key_code(key: WireKey) -> Option<KeyCode> {
    Some(match key {
        WireKey::Char(ch) => KeyCode::Char(ch),
        WireKey::Enter => KeyCode::Enter,
        WireKey::Tab => KeyCode::Tab,
        WireKey::Backspace => KeyCode::Backspace,
        WireKey::Delete => KeyCode::Delete,
        WireKey::Escape => KeyCode::Esc,
        WireKey::Left => KeyCode::Left,
        WireKey::Right => KeyCode::Right,
        WireKey::Up => KeyCode::Up,
        WireKey::Down => KeyCode::Down,
        WireKey::Home => KeyCode::Home,
        WireKey::End => KeyCode::End,
        WireKey::PageUp => KeyCode::PageUp,
        WireKey::PageDown => KeyCode::PageDown,
        WireKey::Insert => KeyCode::Insert,
        WireKey::Function(n) if (1..=12).contains(&n) => KeyCode::F(n),
        // A key a later GUI knows and this build does not is ignored, never
        // guessed at: a wrong guess is an edit nobody asked for.
        _ => return None,
    })
}

fn key_modifiers(bits: u8) -> KeyModifiers {
    let mut out = KeyModifiers::NONE;
    if bits & mods::SHIFT != 0 {
        out |= KeyModifiers::SHIFT;
    }
    if bits & mods::CONTROL != 0 {
        out |= KeyModifiers::CONTROL;
    }
    if bits & mods::ALT != 0 {
        out |= KeyModifiers::ALT;
    }
    if bits & mods::META != 0 {
        out |= KeyModifiers::SUPER;
    }
    out
}

/// Most lines one wheel event may scroll.
///
/// A notch is a line and the surface scrolls its own container, so anything
/// past a screenful here is a number from the wire deciding how long this loop
/// runs — the anchor walk is per line and there is no shortcut through a fold.
const MAX_WHEEL_LINES: u32 = 256;

/// Scroll by whole lines, the way a wheel notch does in the TUI.
fn wheel(app: &mut App, lines: i32) {
    let step = if lines > 0 {
        MouseEventKind::ScrollDown
    } else {
        MouseEventKind::ScrollUp
    };
    for _ in 0..lines.unsigned_abs().min(MAX_WHEEL_LINES) {
        app.handle_mouse(MouseEvent {
            kind: step,
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        });
    }
}

/// A click in document coordinates, as the cell `App::handle_mouse` expects.
///
/// The GUI knows which line a node is, so it says so; the host converts back
/// rather than the GUI guessing a row, because folds and the window's anchor
/// are the host's state and not the GUI's. The column arrives in UTF-16 like
/// every other column on this wire and becomes a display cell here, which is
/// the one place that knows a tab is four of them and a 中 is two.
fn mouse_event(
    app: &App,
    kind: WireMouseKind,
    line: u32,
    column: u32,
    modifiers: u8,
) -> Option<MouseEvent> {
    let row = screen_row(app, line as usize)?;
    let text = app.document().text();
    let display = display_column(text.line(line as usize), column);
    let cell = app.gutter_width() + display.saturating_sub(app.left());
    Some(MouseEvent {
        kind: match kind {
            WireMouseKind::Down => MouseEventKind::Down(MouseButton::Left),
            WireMouseKind::Drag => MouseEventKind::Drag(MouseButton::Left),
            WireMouseKind::Up => MouseEventKind::Up(MouseButton::Left),
            _ => return None,
        },
        column: u16::try_from(cell).unwrap_or(u16::MAX),
        row,
        modifiers: key_modifiers(modifiers),
    })
}

/// A UTF-16 column within `line` as the display cell it sits at.
///
/// Two conversions, not one: UTF-16 units to a byte offset, then bytes to
/// display cells. A tab is one code unit and four cells, so collapsing them
/// would put the caret inside the tab it was clicked past.
fn display_column(line: &str, utf16: u32) -> usize {
    let mut units = 0_u32;
    let mut bytes = line.len();
    for (offset, ch) in line.char_indices() {
        if units >= utf16 {
            bytes = offset;
            break;
        }
        units += ch.len_utf16() as u32;
    }
    if units < utf16 {
        bytes = line.len();
    }
    editor_core::metrics::display_column(line, bytes)
}

/// Which screen row a 0-based line sits on, or `None` when it is off-window.
///
/// A linear walk of the viewport, which is bounded by what fits on a screen:
/// folds make the mapping non-arithmetic, and there is no second index to
/// keep in step with them.
fn screen_row(app: &App, line: usize) -> Option<u16> {
    (0..app.content_height())
        .find(|row| app.row_line_sub(*row).is_some_and(|(at, _)| at == line))
        .and_then(|row| u16::try_from(row).ok())
}

#[cfg(test)]
mod tests {
    use super::display_column;

    /// The conversion a pointer depends on. A tab is one UTF-16 unit and four
    /// cells; an astral character is two units and two cells; a combining mark
    /// is a unit and no cell at all.
    #[test]
    fn a_utf16_column_becomes_the_cell_it_sits_at() {
        assert_eq!(display_column("ab", 0), 0);
        assert_eq!(display_column("ab", 2), 2);
        assert_eq!(display_column("\tx", 1), editor_core::metrics::TAB_WIDTH);
        // 😀 is two UTF-16 units wide and two cells wide.
        assert_eq!(display_column("a😀b", 3), 3);
        // 中 is one UTF-16 unit and two cells.
        assert_eq!(display_column("a中b", 2), 3);
        // Past the end clamps to the line's width rather than panicking.
        assert_eq!(display_column("ab", 99), 2);
        assert_eq!(display_column("", 4), 0);
    }
}
