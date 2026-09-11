//! Terminal golden tests (§21 "Terminal golden"): feed ANSI byte sequences into
//! [`AlacrittyEngine`] and snapshot the resulting grid with `insta`. Covers plain
//! text, SGR colors (16/256/truecolor), cursor movement, clear, alt-screen,
//! wide/CJK characters, and resize-with-reflow.

#![cfg(feature = "engine-alacritty")]

use domain::{CellFlags, Color, PtySize};
use terminal_core::{AlacrittyEngine, TerminalEngine};

fn engine(cols: u16, rows: u16) -> AlacrittyEngine {
    AlacrittyEngine::new(PtySize {
        cols,
        rows,
        pixel_width: 0,
        pixel_height: 0,
    })
}

/// Render the visible grid as plain text, trailing spaces trimmed per row.
fn render_plain(e: &AlacrittyEngine) -> String {
    let snap = e.snapshot(0);
    let mut out = String::new();
    for row in &snap.visible {
        let mut line = String::new();
        for cell in row.cells.iter() {
            line.push_str(&cell.text);
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out
}

/// List every cell whose style differs from the default (text, fg, bg, flags).
fn render_styled(e: &AlacrittyEngine) -> String {
    let snap = e.snapshot(0);
    let mut out = String::new();
    for (r, row) in snap.visible.iter().enumerate() {
        for (c, cell) in row.cells.iter().enumerate() {
            let styled = cell.fg != Color::Default
                || cell.bg != Color::Default
                || cell.flags != CellFlags::empty();
            if styled {
                out.push_str(&format!(
                    "({r},{c}) {:?} fg={:?} bg={:?} flags=0b{:09b}\n",
                    cell.text, cell.fg, cell.bg, cell.flags.0
                ));
            }
        }
    }
    out
}

#[test]
fn plain_text() {
    let mut e = engine(20, 3);
    e.feed(b"hello world");
    insta::assert_snapshot!(render_plain(&e));
}

#[test]
fn wrapping_plain_text() {
    let mut e = engine(10, 4);
    e.feed(b"abcdefghijklmnop");
    let snap = e.snapshot(0);
    let wrapped_flags: Vec<bool> = snap.visible.iter().map(|r| r.wrapped).collect();
    insta::assert_snapshot!(format!("{}\nwrapped={:?}", render_plain(&e), wrapped_flags));
}

#[test]
fn sgr_16_color() {
    let mut e = engine(12, 2);
    // Red foreground "RED", reset, green background "GRN".
    e.feed(b"\x1b[31mRED\x1b[0m\x1b[42mGRN\x1b[0m");
    insta::assert_snapshot!(render_styled(&e));
}

#[test]
fn sgr_256_color() {
    let mut e = engine(8, 1);
    e.feed(b"\x1b[38;5;196mX\x1b[48;5;21mY");
    insta::assert_snapshot!(render_styled(&e));
}

#[test]
fn sgr_truecolor() {
    let mut e = engine(8, 1);
    e.feed(b"\x1b[38;2;10;20;30mZ");
    insta::assert_snapshot!(render_styled(&e));
}

#[test]
fn sgr_attributes() {
    let mut e = engine(16, 1);
    // Bold, italic, underline, then inverse.
    e.feed(b"\x1b[1mB\x1b[0m\x1b[3mI\x1b[0m\x1b[4mU\x1b[0m\x1b[7mR\x1b[0m");
    insta::assert_snapshot!(render_styled(&e));
}

#[test]
fn cursor_movement() {
    let mut e = engine(10, 4);
    // Write "abc", then jump to row 2, col 3 (1-based) and write "xy".
    e.feed(b"abc\x1b[2;3Hxy");
    let cur = e.cursor();
    insta::assert_snapshot!(format!(
        "{}\ncursor: line={} col={} shape={:?} visible={}",
        render_plain(&e),
        cur.line,
        cur.col,
        cur.shape,
        cur.visible
    ));
}

#[test]
fn clear_screen() {
    let mut e = engine(10, 3);
    e.feed(b"line1\r\nline2\r\nline3");
    let before = render_plain(&e);
    // Clear entire screen and home the cursor.
    e.feed(b"\x1b[2J\x1b[H");
    let after = render_plain(&e);
    insta::assert_snapshot!(format!("BEFORE:\n{before}\nAFTER:\n{after}"));
}

#[test]
fn alt_screen_enter_leave() {
    let mut e = engine(10, 2);
    e.feed(b"main");
    let main_before = render_plain(&e);
    let alt_off = e.modes().alt_screen;

    // Enter alternate screen (1049), draw, check.
    e.feed(b"\x1b[?1049h");
    e.feed(b"ALT");
    let alt_view = render_plain(&e);
    let alt_on = e.modes().alt_screen;

    // Leave alternate screen; the primary content returns.
    e.feed(b"\x1b[?1049l");
    let main_after = render_plain(&e);
    let alt_off_again = e.modes().alt_screen;

    insta::assert_snapshot!(format!(
        "main_before(alt={alt_off}):\n{main_before}\n\
         alt_view(alt={alt_on}):\n{alt_view}\n\
         main_after(alt={alt_off_again}):\n{main_after}"
    ));
}

#[test]
fn wide_cjk_char() {
    let mut e = engine(10, 1);
    // 'a', a full-width CJK char, then 'b'.
    e.feed("a世b".as_bytes());
    insta::assert_snapshot!(format!(
        "plain:\n{}\nstyled:\n{}",
        render_plain(&e),
        render_styled(&e)
    ));
}

#[test]
fn resize_with_reflow() {
    let mut e = engine(10, 3);
    // 16 chars wrap across two rows at width 10.
    e.feed(b"abcdefghijklmnop");
    let narrow = render_plain(&e);

    // Grow to width 20: the soft-wrapped line reflows back onto one row.
    e.resize(PtySize {
        cols: 20,
        rows: 3,
        pixel_width: 0,
        pixel_height: 0,
    });
    let wide = render_plain(&e);

    insta::assert_snapshot!(format!("width10:\n{narrow}\nwidth20:\n{wide}"));
}
