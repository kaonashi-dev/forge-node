//! Engine behavior tests: damage tracking, snapshot/delta building, title/bell,
//! and mode parsing (§11.4, ADR-011). These are deterministic and drive the
//! engine directly (no PTY).

#![cfg(feature = "engine-alacritty")]

use domain::{Damage, MouseMode, PtySize, DEFAULT_SCROLLBACK_TAIL};
use terminal_core::{
    AlacrittyEngine, DeltaBuilder, TerminalEngine, DEFAULT_SCROLLBACK_LINES, MAX_SCROLLBACK_LINES,
};

fn engine(cols: u16, rows: u16) -> AlacrittyEngine {
    AlacrittyEngine::new(PtySize {
        cols,
        rows,
        pixel_width: 0,
        pixel_height: 0,
    })
}

#[test]
fn feed_reports_damage() {
    let mut e = engine(10, 5);
    // A freshly constructed terminal reports Full damage once.
    assert!(matches!(e.take_damage(), Damage::Full));

    e.feed(b"a\r\nb\r\nc");
    match e.take_damage() {
        Damage::Partial(rows) => {
            assert!(!rows.is_empty(), "expected damaged rows after feed");
            assert!(rows.contains(&0), "row 0 should be damaged, got {rows:?}");
        }
        other => panic!("expected Partial damage, got {other:?}"),
    }
}

#[test]
fn damage_clears_after_poll() {
    let mut e = engine(10, 5);
    let _ = e.take_damage(); // consume initial Full
    e.feed(b"a\r\nb\r\nc");
    let _ = e.take_damage(); // poll 1 drains the write damage

    // Poll 2 without any new input: the previously damaged top rows are cleared
    // (only the cursor's own row may remain).
    match e.take_damage() {
        Damage::Partial(rows) => {
            assert!(
                !rows.contains(&0),
                "row 0 damage should have cleared, got {rows:?}"
            );
        }
        other => panic!("expected Partial after clear, got {other:?}"),
    }
}

#[test]
fn resize_yields_full_damage() {
    let mut e = engine(10, 5);
    let _ = e.take_damage();
    e.feed(b"hi");
    let _ = e.take_damage();

    e.resize(PtySize {
        cols: 12,
        rows: 6,
        pixel_width: 0,
        pixel_height: 0,
    });
    assert!(matches!(e.take_damage(), Damage::Full));
}

#[test]
fn seq_increments_on_feed() {
    let mut e = engine(10, 2);
    assert_eq!(e.seq(), 0);
    e.feed(b"x");
    e.feed(b"y");
    assert_eq!(e.seq(), 2);
}

#[test]
fn snapshot_shape() {
    let mut e = engine(8, 3);
    e.feed(b"hello");
    let snap = e.snapshot(50);
    assert_eq!(snap.visible.len(), 3);
    assert_eq!(snap.visible[0].cells.len(), 8);
    assert_eq!(snap.size.cols, 8);
    assert_eq!(snap.seq, 1);
    assert_eq!(snap.scrollback_len, 0);
    assert!(snap.scrollback_tail.is_empty());
}

#[test]
fn delta_carries_changed_rows() {
    let mut e = engine(10, 3);
    let mut db = DeltaBuilder::new();

    let snap = db.snapshot(&e, 100);
    assert_eq!(snap.seq, 0);

    e.feed(b"hello");
    let delta = db.delta(&mut e);
    assert_eq!(delta.seq, 1);
    assert!(!delta.rows.is_empty(), "delta should carry damaged rows");
    assert!(
        delta.rows.iter().any(|(i, _)| *i == 0),
        "row 0 should be in the delta"
    );
    // The changed row's text should render "hello".
    let (_, row) = delta.rows.iter().find(|(i, _)| *i == 0).unwrap();
    let text: String = row.cells.iter().map(|c| c.text.as_str()).collect();
    assert!(text.starts_with("hello"), "got {text:?}");
}

#[test]
fn delta_reports_scrolled_lines() {
    let mut e = engine(10, 2); // only 2 visible rows
    let mut db = DeltaBuilder::new();
    let _ = db.snapshot(&e, 0);

    // Feeding 5 lines into a 2-row viewport pushes lines into scrollback.
    e.feed(b"1\r\n2\r\n3\r\n4\r\n5\r\n");
    let delta = db.delta(&mut e);
    assert!(
        delta.scrolled_lines >= 1,
        "expected lines to scroll into history, got {}",
        delta.scrolled_lines
    );
    assert!(e.scrollback_len() >= 1);
}

#[test]
fn rows_addresses_scrollback_with_negative_indices() {
    let mut e = engine(10, 2);
    e.feed(b"top\r\nmid\r\nbot");
    // "top" scrolled into history; -1 is the most recent scrollback line.
    let scroll = e.rows(-1..0);
    assert_eq!(scroll.len(), 1);
    let text: String = scroll[0].cells.iter().map(|c| c.text.as_str()).collect();
    assert!(
        text.trim_end() == "mid" || text.trim_end() == "top",
        "got {text:?}"
    );

    // Visible rows 0..2.
    let vis = e.rows(0..2);
    assert_eq!(vis.len(), 2);

    // Way out of range yields a blank row, not a panic.
    let blank = e.rows(1_000..1_001);
    assert_eq!(blank.len(), 1);
}

#[test]
fn title_and_bell() {
    let mut e = engine(10, 2);
    assert_eq!(e.title(), None);

    // OSC 0 set title, ST-terminated (avoids the BEL/bell ambiguity).
    e.feed(b"\x1b]0;My Title\x1b\\");
    assert_eq!(e.title(), Some("My Title"));
    assert!(!e.take_bell(), "OSC terminator must not ring the bell");

    // A standalone BEL rings the bell exactly once.
    e.feed(b"\x07");
    assert!(e.take_bell());
    assert!(!e.take_bell());
}

#[test]
fn scrollback_limits_match_the_plan() {
    // §11.4: 10 000 lines by default, 100 000 as the hard configurable cap.
    assert_eq!(DEFAULT_SCROLLBACK_LINES, 10_000);
    assert_eq!(MAX_SCROLLBACK_LINES, 100_000);
}

#[test]
fn scrollback_is_bounded_by_the_configured_limit() {
    // A tiny limit makes the bound observable without feeding 10 000 lines.
    let mut e = AlacrittyEngine::with_scrollback(
        PtySize {
            cols: 8,
            rows: 2,
            pixel_width: 0,
            pixel_height: 0,
        },
        5,
    );
    for i in 0..50 {
        e.feed(format!("line{i}\r\n").as_bytes());
    }
    assert_eq!(
        e.scrollback_len(),
        5,
        "history must saturate at the configured limit, never grow unbounded"
    );
}

#[test]
fn snapshot_tail_is_capped_at_the_requested_length() {
    let mut e = engine(8, 2);
    for i in 0..500 {
        e.feed(format!("l{i}\r\n").as_bytes());
    }
    // The default tail is 200 rows (§11.4), and the engine never returns more
    // than the caller asked for even when far more history exists.
    assert!(e.scrollback_len() > DEFAULT_SCROLLBACK_TAIL as u64);
    let snap = e.snapshot(DEFAULT_SCROLLBACK_TAIL);
    assert_eq!(snap.scrollback_tail.len(), DEFAULT_SCROLLBACK_TAIL);
    assert_eq!(DEFAULT_SCROLLBACK_TAIL, 200);

    // A tail longer than the history is clamped to the history, not padded.
    let short = engine(8, 2).snapshot(DEFAULT_SCROLLBACK_TAIL);
    assert!(short.scrollback_tail.is_empty());
}

#[test]
fn osc_52_clipboard_writes_are_ignored() {
    // §11.4: OSC 52 is disabled by default in the MVP. The engine must neither
    // act on a clipboard *store* nor answer a clipboard *query* — answering
    // would leak the user's clipboard into the child process.
    let mut e = engine(20, 2);
    e.feed(b"\x1b]52;c;aGVsbG8=\x07"); // store "hello"
    assert!(
        e.take_pty_writes().is_empty(),
        "a clipboard store must produce no PTY reply"
    );

    e.feed(b"\x1b]52;c;?\x07"); // query the clipboard
    assert!(
        e.take_pty_writes().is_empty(),
        "a clipboard query must not be answered while OSC 52 is disabled"
    );

    // The grid itself is untouched by either sequence.
    let snap = e.snapshot(0);
    let text: String = snap.visible[0]
        .cells
        .iter()
        .map(|c| c.text.as_str())
        .collect();
    assert!(
        text.trim().is_empty(),
        "OSC 52 must not render, got {text:?}"
    );
}

#[test]
fn modes_parse_private_modes() {
    let mut e = engine(10, 2);
    assert_eq!(e.modes(), Default::default());

    e.feed(b"\x1b[?1h"); // DECCKM app cursor keys
    assert!(e.modes().app_cursor_keys);

    e.feed(b"\x1b[?2004h"); // bracketed paste
    assert!(e.modes().bracketed_paste);

    e.feed(b"\x1b[?1000h"); // normal mouse
    assert_eq!(e.modes().mouse_mode, MouseMode::Normal);
    e.feed(b"\x1b[?1002h"); // button-event motion
    assert_eq!(e.modes().mouse_mode, MouseMode::ButtonEvent);
    e.feed(b"\x1b[?1003h"); // any-event motion
    assert_eq!(e.modes().mouse_mode, MouseMode::AnyEvent);

    e.feed(b"\x1b[?1006h"); // SGR mouse
    assert!(e.modes().mouse_sgr);

    e.feed(b"\x1b[?1049h"); // alt screen
    assert!(e.modes().alt_screen);
}
