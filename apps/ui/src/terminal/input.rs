use client::CellGrid;
use client::{
    Key as TKey, Modifiers, MouseButton, MouseEventKind, encode_key, encode_mouse, paste,
};
use domain::{MouseMode, TermModes};
use rune::{Ime, Key, KeyEvent, Mods, WheelEvent};

use crate::terminal::pane::{FetchRequest, Gesture, TerminalUi, Viewport};
use crate::terminal::selection::{CopyError, Granularity, Selection, copy_text};

pub const PREEDIT_MAX_CHARS: usize = 64;
pub const MAX_PASTE_BYTES: usize = 1 << 20;
const MAX_WHEEL_REPORTS: i64 = 10;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Press {
    Down,
    Move,
    Up,
}

#[derive(Clone, Copy, Debug)]
pub struct Pointer {
    pub phase: Press,
    pub x: f32,
    pub y: f32,
    pub clicks: u32,
    pub cancelled: bool,
    pub shift: bool,
}

pub struct Entry {
    pub pane: usize,
    pub bytes: Vec<u8>,
    pub label: String,
}

/// What a handler wants done: PTY bytes (one batch per frame), history fetches and notes for the log.
#[derive(Default)]
pub struct Outbox {
    pub entries: Vec<Entry>,
    pub fetches: Vec<(usize, FetchRequest)>,
    pub notes: Vec<String>,
    pub sent_bytes: u64,
}

#[derive(Default)]
pub struct Effects {
    pub copy: Option<String>,
    pub request_paste: bool,
}

impl Outbox {
    fn emit(&mut self, pane: usize, bytes: Vec<u8>, label: impl Into<String>) {
        if !bytes.is_empty() {
            self.sent_bytes = self.sent_bytes.saturating_add(bytes.len() as u64);
            self.entries.push(Entry {
                pane,
                bytes,
                label: label.into(),
            });
        }
    }

    fn fetch(&mut self, pane: usize, request: Option<FetchRequest>, ui: &mut TerminalUi) {
        if let Some(request) = request
            && ui.fetch_pending != Some(request)
        {
            ui.fetch_pending = Some(request);
            self.fetches.push((pane, request));
        }
    }

    /// Entries for one pane that follow each other travel as one write, as the editor input batch does.
    pub fn drain_batches(&mut self) -> Vec<(usize, Vec<u8>, Vec<String>)> {
        let mut out: Vec<(usize, Vec<u8>, Vec<String>)> = Vec::new();
        for entry in self.entries.drain(..) {
            match out.last_mut() {
                Some((pane, bytes, labels)) if *pane == entry.pane => {
                    bytes.extend_from_slice(&entry.bytes);
                    labels.push(entry.label);
                }
                _ => out.push((entry.pane, entry.bytes, vec![entry.label])),
            }
        }
        out
    }
}

pub fn modifiers(m: Mods) -> Modifiers {
    Modifiers {
        ctrl: m.ctrl,
        alt: m.alt,
        shift: m.shift,
    }
}

pub fn render_bytes(bytes: &[u8]) -> String {
    bytes
        .iter()
        .flat_map(|&b| std::ascii::escape_default(b))
        .map(char::from)
        .collect()
}

fn snap_to_live(ui: &mut TerminalUi) {
    ui.scroll = 0;
    ui.selection = None;
}

fn reporting(modes: &TermModes, shift: bool) -> bool {
    modes.mouse_mode != MouseMode::Off && !shift
}

pub fn copy_selection(
    grid: &CellGrid,
    ui: &TerminalUi,
    fx: &mut Effects,
    out: &mut Outbox,
) -> bool {
    let Some(selection) = ui.selection else {
        return false;
    };
    let viewport = Viewport {
        grid,
        scroll: ui.scroll,
    };
    match copy_text(&viewport, &selection) {
        Ok(text) => {
            out.notes
                .push(format!("copy {} bytes {:?}", text.len(), text));
            fx.copy = Some(text);
        }
        Err(CopyError::Unavailable) => out
            .notes
            .push("copy refused: history rows not cached".into()),
        Err(CopyError::TooLarge) => out
            .notes
            .push("copy refused: over the line or byte limit".into()),
    }
    true
}

pub fn key(
    idx: usize,
    ui: &mut TerminalUi,
    grid: &CellGrid,
    ev: &KeyEvent,
    out: &mut Outbox,
    fx: &mut Effects,
) -> bool {
    let mods = modifiers(ev.mods);
    let modes = grid.modes;
    let bytes = match &ev.key {
        Key::Command(c) => {
            return match c {
                'c' => copy_selection(grid, ui, fx, out),
                'v' => {
                    fx.request_paste = true;
                    true
                }
                _ => false,
            };
        }
        Key::Text(text) => {
            if ev.mods.cmd {
                return false;
            }
            let mut bytes = Vec::new();
            for c in text.chars() {
                let m = if mods.alt && !c.is_ascii() {
                    Modifiers { alt: false, ..mods }
                } else {
                    mods
                };
                bytes.extend(encode_key(TKey::Char(c), m, &modes));
            }
            bytes
        }
        Key::PageUp | Key::PageDown | Key::Home | Key::End if mods.shift => {
            let page = grid.visible.len().saturating_sub(1) as i64;
            let lines = match &ev.key {
                Key::PageUp => page,
                Key::PageDown => -page,
                Key::Home => i64::MAX / 2,
                _ => i64::MIN / 2,
            };
            let viewport = Viewport {
                grid,
                scroll: ui.scroll,
            };
            let target = (ui.scroll as i64 + lines).clamp(0, viewport.max_scroll() as i64);
            ui.scroll = target as usize;
            let request = missing_history(ui, &viewport);
            out.fetch(idx, request, ui);
            ui.request_frame();
            return true;
        }
        key => {
            let k = match key {
                Key::Enter => TKey::Enter,
                Key::Escape => TKey::Escape,
                Key::Backspace => TKey::Backspace,
                Key::Delete => TKey::Delete,
                Key::Tab => TKey::Tab,
                Key::Up => TKey::Up,
                Key::Down => TKey::Down,
                Key::Left => TKey::Left,
                Key::Right => TKey::Right,
                Key::Home => TKey::Home,
                Key::End => TKey::End,
                Key::PageUp => TKey::PageUp,
                Key::PageDown => TKey::PageDown,
                Key::Insert => TKey::Insert,
                Key::Function(n) if (1..=12).contains(n) => TKey::F(*n),
                _ => return false,
            };
            encode_key(k, mods, &modes)
        }
    };
    snap_to_live(ui);
    out.emit(idx, bytes, format!("{:?}{}", ev.key, mod_suffix(ev.mods)));
    true
}

pub(crate) fn mod_suffix(m: Mods) -> String {
    let mut s = String::new();
    for (on, name) in [(m.ctrl, "ctrl"), (m.alt, "alt"), (m.shift, "shift")] {
        if on {
            s.push('+');
            s.push_str(name);
        }
    }
    s
}

pub fn wheel(
    idx: usize,
    ui: &mut TerminalUi,
    grid: &CellGrid,
    ev: &WheelEvent,
    cell: (f32, f32),
    out: &mut Outbox,
) -> bool {
    let modes = grid.modes;
    let total = ui.wheel_rem + ev.dy / cell.1;
    let lines = total.trunc() as i64;
    ui.wheel_rem = total - lines as f32;
    if lines == 0 {
        return true;
    }
    if reporting(&modes, ev.mods.shift) {
        let viewport = Viewport {
            grid,
            scroll: ui.scroll,
        };
        let (col, row) = viewport.viewport_cell(ev.x, ev.y, cell);
        let button = if lines > 0 {
            MouseButton::WheelUp
        } else {
            MouseButton::WheelDown
        };
        for _ in 0..lines.abs().min(MAX_WHEEL_REPORTS) {
            if let Some(bytes) = encode_mouse(
                button,
                MouseEventKind::Press,
                col,
                row,
                modifiers(ev.mods),
                &modes,
            ) {
                out.emit(idx, bytes, format!("wheel {button:?}"));
            }
        }
        return true;
    }
    if modes.alt_screen {
        return true;
    }
    let viewport = Viewport {
        grid,
        scroll: ui.scroll,
    };
    let target = (ui.scroll as i64 + lines).clamp(0, viewport.max_scroll() as i64);
    ui.scroll = target as usize;
    let request = missing_history(ui, &viewport);
    out.fetch(idx, request, ui);
    ui.request_frame();
    true
}

pub fn pointer(
    idx: usize,
    ui: &mut TerminalUi,
    grid: &CellGrid,
    p: Pointer,
    cell: (f32, f32),
    out: &mut Outbox,
    fx: &mut Effects,
) {
    let modes = grid.modes;
    let viewport = Viewport {
        grid,
        scroll: ui.scroll,
    };
    let (col, row) = viewport.viewport_cell(p.x, p.y, cell);
    let mods = Modifiers {
        shift: p.shift,
        ..Modifiers::none()
    };
    match p.phase {
        Press::Down => {
            ui.preedit.clear();
            ui.request_frame();
            if reporting(&modes, p.shift) {
                ui.gesture = Some(Gesture::Report);
                ui.last_cell = Some((col, row));
                if let Some(b) = encode_mouse(
                    MouseButton::Left,
                    MouseEventKind::Press,
                    col,
                    row,
                    mods,
                    &modes,
                ) {
                    out.emit(idx, b, "mouse press");
                }
            } else {
                ui.gesture = Some(Gesture::Select);
                let granularity = match p.clicks {
                    0 | 1 => Granularity::Cell,
                    2 => Granularity::Word,
                    _ => Granularity::Line,
                };
                ui.selection = Some(Selection::new(
                    viewport.cell_at(p.x, p.y, cell),
                    granularity,
                ));
            }
        }
        Press::Move => match ui.gesture {
            Some(Gesture::Report) => {
                if ui.last_cell != Some((col, row)) {
                    ui.last_cell = Some((col, row));
                    if let Some(b) = encode_mouse(
                        MouseButton::Left,
                        MouseEventKind::Motion,
                        col,
                        row,
                        mods,
                        &modes,
                    ) {
                        out.emit(idx, b, "mouse motion");
                    }
                }
            }
            Some(Gesture::Select) => {
                let at = viewport.cell_at(p.x, p.y, cell);
                if let Some(selection) = ui.selection.as_mut() {
                    selection.head = at;
                    ui.request_frame();
                }
            }
            None => {}
        },
        Press::Up => {
            let gesture = ui.gesture.take();
            if p.cancelled {
                ui.selection = None;
                ui.request_frame();
                return;
            }
            match gesture {
                Some(Gesture::Report) => {
                    if let Some(b) = encode_mouse(
                        MouseButton::Left,
                        MouseEventKind::Release,
                        col,
                        row,
                        mods,
                        &modes,
                    ) {
                        out.emit(idx, b, "mouse release");
                    }
                }
                Some(Gesture::Select) => {
                    if let Some(selection) = ui.selection
                        && selection.granularity == Granularity::Cell
                        && selection.anchor == selection.head
                    {
                        ui.selection = None;
                    }
                    if let Some(selection) = ui.selection {
                        let range = selection.range(&viewport);
                        out.notes.push(format!(
                            "selected {:?} {:?}..{:?}",
                            selection.granularity, range.0, range.1
                        ));
                    }
                    ui.request_frame();
                }
                None => {}
            }
            let _ = fx;
        }
    }
}

pub fn secondary(
    idx: usize,
    ui: &mut TerminalUi,
    grid: &CellGrid,
    p: Pointer,
    cell: (f32, f32),
    out: &mut Outbox,
    fx: &mut Effects,
) {
    let modes = grid.modes;
    if reporting(&modes, p.shift) {
        let viewport = Viewport {
            grid,
            scroll: ui.scroll,
        };
        let (col, row) = viewport.viewport_cell(p.x, p.y, cell);
        for kind in [MouseEventKind::Press, MouseEventKind::Release] {
            if let Some(bytes) = encode_mouse(
                MouseButton::Right,
                kind,
                col,
                row,
                Modifiers::none(),
                &modes,
            ) {
                out.emit(idx, bytes, format!("mouse right {kind:?}"));
            }
        }
    } else if !copy_selection(grid, ui, fx, out) {
        fx.request_paste = true;
    }
}

pub fn paste_text(idx: usize, ui: &mut TerminalUi, grid: &CellGrid, text: &str, out: &mut Outbox) {
    if text.len() > MAX_PASTE_BYTES {
        out.notes.push(format!(
            "paste refused: {} bytes over {MAX_PASTE_BYTES}",
            text.len()
        ));
        return;
    }
    snap_to_live(ui);
    let bytes = paste(text, &grid.modes);
    out.emit(
        idx,
        bytes,
        if grid.modes.bracketed_paste {
            "paste (bracketed)"
        } else {
            "paste"
        },
    );
    ui.request_frame();
}

pub fn focus(idx: usize, ui: &mut TerminalUi, grid: &CellGrid, focused: bool, out: &mut Outbox) {
    if !focused {
        ui.gesture = None;
        ui.preedit.clear();
        ui.request_frame();
    }
    if grid.modes.focus_events {
        let bytes: &[u8] = if focused { b"\x1b[I" } else { b"\x1b[O" };
        out.emit(
            idx,
            bytes.to_vec(),
            if focused { "focus in" } else { "focus out" },
        );
    }
}

/// Preedit draws at the cursor and inserts nothing; only a commit becomes PTY bytes.
pub fn ime(idx: usize, ui: &mut TerminalUi, ev: &Ime, out: &mut Outbox) -> bool {
    match ev {
        Ime::Preedit(text, _) => {
            ui.preedit = text.chars().take(PREEDIT_MAX_CHARS).collect();
            ui.request_frame();
            true
        }
        Ime::Commit(text) => {
            ui.preedit.clear();
            snap_to_live(ui);
            let clean: String = text.chars().filter(|c| !c.is_control()).collect();
            out.emit(idx, clean.into_bytes(), "ime commit");
            ui.request_frame();
            true
        }
        Ime::Enabled | Ime::Disabled => true,
    }
}

fn missing_history(ui: &TerminalUi, viewport: &Viewport<'_>) -> Option<FetchRequest> {
    let top = viewport.top_abs();
    let base = viewport.base();
    let end = (top + viewport.rows() as i64).min(base);
    let first = (top.max(0)..end).find(|&abs| viewport.row_at_abs(abs).is_none())?;
    let from_line = (first - crate::terminal::pane::FETCH_BLOCK / 2).max(0);
    let _ = ui;
    Some(FetchRequest {
        from_line,
        count: crate::terminal::pane::FETCH_BLOCK as u32,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::SharedTerminal;
    use crate::terminal::fixture::{ModeSet, grid};

    const CELL: (f32, f32) = (7.0, 14.0);

    fn surface(mode: ModeSet) -> (TerminalUi, client::CellGrid) {
        let grid = grid(100, 30, mode, true);
        let shared = SharedTerminal::new();
        let ui = TerminalUi::new(shared, true);
        (ui, grid)
    }

    fn press(key: Key, mods: Mods, ui: &mut TerminalUi, grid: &client::CellGrid) -> Vec<u8> {
        let (mut out, mut fx) = (Outbox::default(), Effects::default());
        key_(key, mods, ui, grid, &mut out, &mut fx);
        out.entries.into_iter().flat_map(|e| e.bytes).collect()
    }

    fn key_(
        key: Key,
        mods: Mods,
        ui: &mut TerminalUi,
        grid: &client::CellGrid,
        out: &mut Outbox,
        fx: &mut Effects,
    ) -> bool {
        super::key(0, ui, grid, &KeyEvent { key, mods }, out, fx)
    }

    fn ctrl() -> Mods {
        Mods {
            ctrl: true,
            ..Mods::default()
        }
    }

    #[test]
    fn printable_modifiers_and_named_keys() {
        let (mut ui, grid) = surface(ModeSet::Plain);
        let none = Mods::default();
        assert_eq!(press(Key::Text("a".into()), none, &mut ui, &grid), b"a");
        assert_eq!(
            press(Key::Text("é".into()), none, &mut ui, &grid),
            "é".as_bytes()
        );
        assert_eq!(press(Key::Text("c".into()), ctrl(), &mut ui, &grid), [0x03]);
        assert_eq!(
            press(
                Key::Text("x".into()),
                Mods { alt: true, ..none },
                &mut ui,
                &grid
            ),
            b"\x1bx"
        );
        assert_eq!(
            press(
                Key::Text("å".into()),
                Mods { alt: true, ..none },
                &mut ui,
                &grid
            ),
            "å".as_bytes(),
            "Option types its own character"
        );
        assert_eq!(press(Key::Enter, none, &mut ui, &grid), b"\r");
        assert_eq!(
            press(
                Key::Enter,
                Mods {
                    shift: true,
                    ..none
                },
                &mut ui,
                &grid
            ),
            b"\x1b\r"
        );
        assert_eq!(press(Key::Tab, none, &mut ui, &grid), b"\t");
        assert_eq!(
            press(
                Key::Tab,
                Mods {
                    shift: true,
                    ..none
                },
                &mut ui,
                &grid
            ),
            b"\x1b[Z"
        );
        assert_eq!(press(Key::Backspace, none, &mut ui, &grid), [0x7f]);
        assert_eq!(press(Key::Up, none, &mut ui, &grid), b"\x1b[A");
        assert_eq!(press(Key::Left, ctrl(), &mut ui, &grid), b"\x1b[1;5D");
        assert_eq!(press(Key::Home, none, &mut ui, &grid), b"\x1b[H");
        assert_eq!(press(Key::End, none, &mut ui, &grid), b"\x1b[F");
        assert_eq!(press(Key::PageUp, none, &mut ui, &grid), b"\x1b[5~");
        assert_eq!(press(Key::PageDown, none, &mut ui, &grid), b"\x1b[6~");
        assert_eq!(press(Key::Function(5), none, &mut ui, &grid), b"\x1b[15~");
        assert!(
            press(
                Key::Command('q'),
                Mods { cmd: true, ..none },
                &mut ui,
                &grid
            )
            .is_empty()
        );
    }

    #[test]
    fn application_cursor_keys_follow_the_mode() {
        let (mut ui, grid) = surface(ModeSet::App);
        assert_eq!(press(Key::Up, Mods::default(), &mut ui, &grid), b"\x1bOA");
    }

    #[test]
    fn cmd_chords_never_become_bytes() {
        let (mut ui, grid) = surface(ModeSet::Plain);
        let (mut out, mut fx) = (Outbox::default(), Effects::default());
        let cmd = Mods {
            cmd: true,
            ..Mods::default()
        };
        assert!(!key_(
            Key::Text(" ".into()),
            cmd,
            &mut ui,
            &grid,
            &mut out,
            &mut fx
        ));
        assert!(key_(
            Key::Command('v'),
            cmd,
            &mut ui,
            &grid,
            &mut out,
            &mut fx
        ));
        assert!(fx.request_paste && out.entries.is_empty());
    }

    #[test]
    fn bracketed_paste_only_when_the_mode_is_set() {
        let mut out = Outbox::default();
        let (mut ui, grid) = surface(ModeSet::Plain);
        paste_text(0, &mut ui, &grid, "a\x1b[201~b", &mut out);
        assert_eq!(out.entries[0].bytes, b"\x1b[200~ab\x1b[201~");
        let (mut ui, grid) = surface(ModeSet::App);
        paste_text(0, &mut ui, &grid, "x y", &mut out);
        assert_eq!(out.entries[1].bytes, b"x y");
    }

    #[test]
    fn wheel_scrolls_history_unless_the_program_asked_for_the_mouse() {
        let mut out = Outbox::default();
        let (mut ui, grid) = surface(ModeSet::Plain);
        wheel(
            0,
            &mut ui,
            &grid,
            &WheelEvent::new(0.0, 28.0).at(10.0, 10.0),
            CELL,
            &mut out,
        );
        assert_eq!(ui.scroll, 2);
        assert!(out.entries.is_empty());
        wheel(
            0,
            &mut ui,
            &grid,
            &WheelEvent::new(0.0, -14.0),
            CELL,
            &mut out,
        );
        assert_eq!(ui.scroll, 1);
        let (mut ui, grid) = surface(ModeSet::Mouse);
        wheel(
            0,
            &mut ui,
            &grid,
            &WheelEvent::new(0.0, 14.0).at(21.0, 28.0),
            CELL,
            &mut out,
        );
        assert_eq!(out.entries[0].bytes, b"\x1b[<64;4;3M");
        assert_eq!(ui.scroll, 0);
        let shift = Mods {
            shift: true,
            ..Mods::default()
        };
        wheel(
            0,
            &mut ui,
            &grid,
            &WheelEvent::new(0.0, 14.0).mods(shift),
            CELL,
            &mut out,
        );
        assert_eq!(ui.scroll, 1, "shift bypasses reporting");
    }

    #[test]
    fn scrolling_past_the_cache_asks_for_one_history_block() {
        let mut out = Outbox::default();
        let (mut ui, grid) = surface(ModeSet::Plain);
        let lines = ui.scroll as i64 + 300;
        ui.scroll = lines as usize;
        let viewport = Viewport {
            grid: &grid,
            scroll: ui.scroll,
        };
        let request = missing_history(&ui, &viewport);
        out.fetch(0, request, &mut ui);
        let again = missing_history(&ui, &viewport);
        out.fetch(0, again, &mut ui);
        assert_eq!(out.fetches.len(), 1, "coalesced");
        assert!(out.fetches[0].1.from_line < 800);
    }

    fn drag(
        ui: &mut TerminalUi,
        grid: &client::CellGrid,
        phase: Press,
        x: f32,
        y: f32,
        clicks: u32,
        out: &mut Outbox,
    ) {
        let pt = Pointer {
            phase,
            x,
            y,
            clicks,
            cancelled: false,
            shift: false,
        };
        pointer(0, ui, grid, pt, CELL, out, &mut Effects::default());
    }

    #[test]
    fn sgr_mouse_reports_press_motion_release_only_in_a_reporting_mode() {
        let mut out = Outbox::default();
        let (mut ui, grid) = surface(ModeSet::Mouse);
        drag(&mut ui, &grid, Press::Down, 7.0, 14.0, 1, &mut out);
        drag(&mut ui, &grid, Press::Move, 8.0, 14.0, 1, &mut out);
        drag(&mut ui, &grid, Press::Move, 15.0, 14.0, 1, &mut out);
        drag(&mut ui, &grid, Press::Up, 15.0, 14.0, 1, &mut out);
        let got: Vec<_> = out.entries.iter().map(|e| render_bytes(&e.bytes)).collect();
        assert_eq!(got, ["\\x1b[<0;2;2M", "\\x1b[<32;3;2M", "\\x1b[<0;3;2m"]);
        assert!(ui.selection.is_none(), "reporting never selects");
        let mut out = Outbox::default();
        let (mut ui, grid) = surface(ModeSet::Plain);
        drag(&mut ui, &grid, Press::Down, 7.0, 14.0, 1, &mut out);
        drag(&mut ui, &grid, Press::Move, 40.0, 28.0, 1, &mut out);
        drag(&mut ui, &grid, Press::Up, 40.0, 28.0, 1, &mut out);
        assert!(out.entries.is_empty(), "no bytes without a mouse mode");
        assert!(ui.selection.is_some());
    }

    #[test]
    fn focus_events_only_when_the_mode_is_set() {
        let mut out = Outbox::default();
        let (mut ui, grid) = surface(ModeSet::Plain);
        focus(0, &mut ui, &grid, true, &mut out);
        assert!(out.entries.is_empty());
        let (mut ui, grid) = surface(ModeSet::Mouse);
        focus(0, &mut ui, &grid, true, &mut out);
        focus(0, &mut ui, &grid, false, &mut out);
        assert_eq!(out.entries[0].bytes, b"\x1b[I");
        assert_eq!(out.entries[1].bytes, b"\x1b[O");
    }

    #[test]
    fn word_line_and_soft_wrap_selection_and_copy() {
        let mut out = Outbox::default();
        let (mut ui, grid) = surface(ModeSet::Plain);
        let word_row = 13;
        let (x, y) = (30.0 * CELL.0, word_row as f32 * CELL.1 + 1.0);
        drag(&mut ui, &grid, Press::Down, x, y, 2, &mut out);
        drag(&mut ui, &grid, Press::Up, x, y, 2, &mut out);
        let viewport = Viewport {
            grid: &grid,
            scroll: ui.scroll,
        };
        let text = copy_text(&viewport, &ui.selection.unwrap()).unwrap();
        assert_eq!(text, "/Users/kaonashi/dev/forge-node/src/main.rs:42");
        let wrapped_row = 14;
        let y = wrapped_row as f32 * CELL.1 + 1.0;
        drag(&mut ui, &grid, Press::Down, 10.0, y, 3, &mut out);
        drag(&mut ui, &grid, Press::Up, 10.0, y, 3, &mut out);
        let viewport = Viewport {
            grid: &grid,
            scroll: ui.scroll,
        };
        let text = copy_text(&viewport, &ui.selection.unwrap()).unwrap();
        assert!(text.starts_with("a soft wrapped logical line"), "{text}");
        assert!(text.ends_with("ending here."), "{text}");
        assert!(!text.contains('\n'), "soft wraps are joined: {text:?}");
    }

    #[test]
    fn copy_spans_scrollback_and_refuses_missing_history() {
        let (mut ui, grid) = surface(ModeSet::Plain);
        ui.scroll = 3;
        let viewport = Viewport {
            grid: &grid,
            scroll: ui.scroll,
        };
        let top = viewport.top_abs();
        ui.selection = Some(Selection {
            anchor: (0, top),
            head: (10, top + 4),
            granularity: Granularity::Cell,
        });
        let text = copy_text(&viewport, &ui.selection.unwrap()).unwrap();
        assert!(text.starts_with("history line"), "{text}");
        assert_eq!(text.lines().count(), 5);
        ui.selection = Some(Selection {
            anchor: (0, 10),
            head: (5, 20),
            granularity: Granularity::Cell,
        });
        assert_eq!(
            copy_text(&viewport, &ui.selection.unwrap()),
            Err(CopyError::Unavailable)
        );
        ui.selection = Some(Selection {
            anchor: (0, 0),
            head: (5, 9000),
            granularity: Granularity::Cell,
        });
        assert_eq!(
            copy_text(&viewport, &ui.selection.unwrap()),
            Err(CopyError::TooLarge)
        );
    }

    #[test]
    fn preedit_never_inserts_and_commit_does() {
        let mut out = Outbox::default();
        let (mut ui, grid) = surface(ModeSet::Plain);
        assert!(ime(0, &mut ui, &Ime::Enabled, &mut out));
        assert!(ime(
            0,
            &mut ui,
            &Ime::Preedit("かん".into(), Some((0, 6))),
            &mut out
        ));
        assert_eq!(ui.preedit, "かん");
        assert!(out.entries.is_empty(), "preedit is not input");
        assert!(ime(
            0,
            &mut ui,
            &Ime::Preedit(String::new(), None),
            &mut out
        ));
        assert!(ui.preedit.is_empty() && out.entries.is_empty());
        ime(0, &mut ui, &Ime::Preedit("かん".into(), None), &mut out);
        ime(0, &mut ui, &Ime::Commit("漢".into()), &mut out);
        assert!(ui.preedit.is_empty());
        assert_eq!(out.entries[0].bytes, "漢".as_bytes());
        assert_eq!(grid.last_seq, 1, "the replica is never edited locally");
    }

    #[test]
    fn typing_returns_the_viewport_to_the_live_screen() {
        let (mut ui, grid) = surface(ModeSet::Plain);
        ui.scroll = 40;
        press(Key::Text("a".into()), Mods::default(), &mut ui, &grid);
        assert_eq!(ui.scroll, 0);
    }
}
