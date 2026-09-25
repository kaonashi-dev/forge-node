//! End-to-end proof that `--headless` is an editor with no terminal.
//!
//! No PTY is opened anywhere in this file: the process is spawned with its
//! stdio closed, and everything the surface needs arrives as `ViewFrame`s on
//! the control socket. That is the claim the DOM surface rests on — if this
//! needed a tty, the GUI would still be painting cells.

use std::io::Write;
use std::os::unix::net::UnixListener;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use editor_control::{
    encode, modifiers, read_frame, write_frame, DaemonMessage, EditorInput, EditorMessage,
    ViewFrame, ViewRequest, WireFindCommand, WireKey, WireMarkKind, WireMouseKind, CONTROL_VERSION,
    MAX_VIEW_FRAME_BYTES,
};

const WAIT: Duration = Duration::from_secs(10);
const SESSION_ID: &str = "ses-headless";
/// The window the fake GUI mounts: a screenful plus the overscan it would
/// keep above and below it.
const WINDOW: u32 = 40 + 2 * editor_control::VIEW_OVERSCAN;

struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// The daemon side: handshake, `Open`, then the traffic in both directions.
struct Host {
    _directory: tempfile::TempDir,
    requests: Sender<DaemonMessage>,
    messages: Receiver<EditorMessage>,
    _child: ChildGuard,
}

impl Host {
    fn send(&self, request: DaemonMessage) {
        self.requests.send(request).expect("control requests");
    }

    fn input(&self, events: Vec<EditorInput>) {
        self.send(DaemonMessage::Input {
            request_id: None,
            events,
        });
    }

    fn key(&self, key: WireKey, bits: u8) {
        self.input(vec![EditorInput::Key {
            key,
            modifiers: bits,
        }]);
    }

    /// The next frame, skipping the `State` notifications that travel with it.
    fn frame(&self) -> ViewFrame {
        let deadline = Instant::now() + WAIT;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            assert!(!left.is_zero(), "no ViewFrame arrived");
            match self.messages.recv_timeout(left) {
                Ok(EditorMessage::ViewFrame { frame }) => return frame,
                Ok(_) => continue,
                Err(error) => panic!("the control channel ended: {error}"),
            }
        }
    }

    /// The first frame at or past `version`; earlier ones are what the host
    /// had already published when the input landed.
    fn frame_at_version(&self, version: u64) -> ViewFrame {
        let deadline = Instant::now() + WAIT;
        loop {
            assert!(
                Instant::now() < deadline,
                "no frame reached version {version}"
            );
            let frame = self.frame();
            if frame.doc_version >= version {
                return frame;
            }
        }
    }
}

fn start(text: &str, display_path: &str) -> Host {
    let directory = tempfile::tempdir().expect("temp dir");
    let socket = directory.path().join("editor.sock");
    let listener = UnixListener::bind(&socket).expect("bind control socket");
    let (message_tx, message_rx) = channel();
    let (request_tx, request_rx) = channel::<DaemonMessage>();
    let text = text.to_string();
    let path = display_path.to_string();

    thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("the editor connected");
        stream.set_read_timeout(Some(WAIT)).expect("read timeout");
        let mut writer = stream.try_clone().expect("clone the control stream");
        match read_frame::<EditorMessage>(&mut stream).expect("Hello") {
            EditorMessage::Hello {
                version,
                session_id,
                ..
            } => {
                assert_eq!(version, CONTROL_VERSION);
                assert_eq!(session_id, SESSION_ID);
            }
            other => panic!("expected Hello, got {other:?}"),
        }
        write_frame(
            &mut writer,
            &DaemonMessage::Welcome {
                version: CONTROL_VERSION,
                session_id: SESSION_ID.to_string(),
                buffer_id: 9,
            },
        )
        .expect("Welcome");
        write_frame(
            &mut writer,
            &DaemonMessage::Open {
                request_id: 101,
                buffer_id: 9,
                path,
                text,
                revision: Some("rev-1".to_string()),
                line: None,
                read_only: false,
                autosave: false,
            },
        )
        .expect("Open");
        match read_frame::<EditorMessage>(&mut stream).expect("Opened") {
            EditorMessage::Opened { request_id, .. } => assert_eq!(request_id, 101),
            other => panic!("expected Opened, got {other:?}"),
        }
        thread::spawn(move || {
            for request in request_rx.iter() {
                if write_frame(&mut writer, &request).is_err() {
                    break;
                }
            }
        });
        // No deadline once the handshake is done: a headless host that nobody
        // is typing in publishes nothing, which is the point.
        stream.set_read_timeout(None).expect("clear read timeout");
        while let Ok(message) = read_frame::<EditorMessage>(&mut stream) {
            if message_tx.send(message).is_err() {
                break;
            }
        }
    });

    let child = Command::new(env!("CARGO_BIN_EXE_forge-editor"))
        .arg("--control")
        .arg(&socket)
        .arg("--headless")
        .arg(display_path)
        .env("FORGE_SESSION_ID", SESSION_ID)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn the headless editor");

    let host = Host {
        _directory: directory,
        requests: request_tx,
        messages: message_rx,
        _child: ChildGuard(child),
    };
    host.send(DaemonMessage::SetView {
        request_id: 1,
        view: ViewRequest {
            first_line: 0,
            line_count: WINDOW,
        },
    });
    host
}

#[test]
fn retarget_over_control_keeps_the_live_document_and_undo() {
    let host = start("hello", "old.txt");
    let before = host.frame();
    host.input(vec![EditorInput::Text("draft ".into())]);
    let edited = host.frame_at_version(before.doc_version + 1);
    host.send(DaemonMessage::Retarget {
        path: "new.rs".into(),
    });
    host.send(DaemonMessage::GetState { request_id: 900 });
    let deadline = Instant::now() + WAIT;
    loop {
        let message = host
            .messages
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .unwrap();
        if let EditorMessage::State {
            request_id: Some(900),
            state,
        } = message
        {
            assert_eq!(state.path, "new.rs");
            assert_eq!(state.document_version, edited.doc_version);
            assert_eq!(state.column, edited.caret.column + 1);
            assert!(state.dirty);
            break;
        }
    }
    host.key(WireKey::Char('z'), modifiers::CONTROL);
    let undone = host.frame_at_version(edited.doc_version + 1);
    assert_eq!(
        undone.rows[0]
            .spans
            .iter()
            .map(|s| s.text.as_str())
            .collect::<String>(),
        "hello"
    );
}

fn numbered(lines: usize) -> String {
    let mut out = String::new();
    for line in 0..lines {
        out.push_str(&format!("fn line_{line}() {{ let value = {line}; }}\n"));
    }
    out
}

#[test]
fn a_headless_editor_publishes_a_window_and_never_the_file() {
    let host = start(&numbered(10_000), "src/big.rs");
    let deadline = Instant::now() + WAIT;
    let frame = loop {
        assert!(Instant::now() < deadline, "no window-sized frame arrived");
        let frame = host.frame();
        if frame.rows.len() == WINDOW as usize {
            break frame;
        }
    };
    assert_eq!(frame.buffer_id, 9);
    assert_eq!(frame.first_line, 0);
    assert_eq!(frame.total_lines, 10_001, "the trailing newline is a line");
    assert_eq!(frame.rows[0].line, 0);
    assert_eq!(frame.rows[0].text(), "fn line_0() { let value = 0; }");
    assert!(!frame.clipped);
    // The colouring came with the window: a DOM row is spans, not a string the
    // GUI has to re-lex.
    assert!(
        frame.rows[0].spans.len() > 1,
        "the row arrived unscoped: {:?}",
        frame.rows[0].spans
    );

    let bytes = encode(&EditorMessage::ViewFrame {
        frame: frame.clone(),
    })
    .expect("encodes")
    .len();
    // The number the surface decision rests on: a ten-thousand-line file
    // costs one window, and the window is kilobytes.
    assert!(
        bytes < MAX_VIEW_FRAME_BYTES / 8,
        "a {WINDOW}-row window of a 10k-line file cost {bytes} bytes"
    );
    println!("window of {} rows: {bytes} bytes", frame.rows.len());
}

#[test]
fn scrolling_ten_thousand_lines_moves_the_window_and_not_the_file() {
    let host = start(&numbered(10_000), "src/big.rs");
    host.send(DaemonMessage::SetView {
        request_id: 2,
        view: ViewRequest {
            first_line: 9_000,
            line_count: WINDOW,
        },
    });
    let deadline = Instant::now() + WAIT;
    let frame = loop {
        assert!(Instant::now() < deadline, "the window never moved");
        let frame = host.frame();
        if frame.first_line == 9_000 {
            break frame;
        }
    };
    assert_eq!(frame.rows.len(), WINDOW as usize);
    assert_eq!(frame.rows[0].text(), "fn line_9000() { let value = 9000; }");
    assert_eq!(frame.total_lines, 10_001);
}

#[test]
fn typing_advances_the_document_and_republishes_the_row() {
    let host = start("alpha\nbeta\n", "src/note.txt");
    let before = host.frame().doc_version;
    host.key(WireKey::Char('X'), 0);
    let frame = host.frame_at_version(before + 1);
    assert_eq!(frame.rows[0].text(), "Xalpha");
    assert_eq!(frame.caret.line, 0);
    assert_eq!(frame.caret.column, 1);
}

/// The GUI owns the scroll container, but a caret it cannot see has to bring
/// the window with it, or an arrow key at the bottom edge does nothing.
#[test]
fn a_caret_driven_off_the_window_moves_the_window() {
    let host = start(&numbered(500), "src/big.rs");
    host.send(DaemonMessage::SetView {
        request_id: 3,
        view: ViewRequest {
            first_line: 0,
            line_count: 10,
        },
    });
    // Ctrl-End is the end of the buffer; the window has to follow the caret
    // there even though the GUI never scrolled.
    host.key(WireKey::End, modifiers::CONTROL);
    let deadline = Instant::now() + WAIT;
    loop {
        assert!(Instant::now() < deadline, "the window never followed");
        let frame = host.frame();
        if frame.first_line > 0 {
            assert!(
                frame.caret.line >= frame.first_line,
                "the caret at {} is above the window at {}",
                frame.caret.line,
                frame.first_line
            );
            assert!(frame.rows.len() <= 10);
            break;
        }
    }
}

/// An input burst is one frame, not one per key: the emit floor is what keeps
/// a held key off the socket as fifty windows.
#[test]
fn a_burst_of_keys_does_not_become_a_frame_each() {
    let host = start(&numbered(200), "src/big.rs");
    let before = host.frame().doc_version;
    let events: Vec<EditorInput> = (0..32)
        .map(|_| EditorInput::Key {
            key: WireKey::Char('z'),
            modifiers: 0,
        })
        .collect();
    host.input(events);
    let frame = host.frame_at_version(before + 32);
    assert!(frame.rows[0].text().starts_with("zzzz"));
    let mut extra = 0;
    while let Ok(message) = host.messages.recv_timeout(Duration::from_millis(200)) {
        if matches!(message, EditorMessage::ViewFrame { .. }) {
            extra += 1;
        }
    }
    assert!(extra <= 1, "{extra} extra frames for one 32-key burst");
}

/// A `Text` event is a paste or a resolved IME composition; the host clamps
/// it the same way the key path clamps everything else from the wire.
#[test]
fn committed_text_lands_as_one_edit() {
    let host = start("\n", "src/note.txt");
    let before = host.frame().doc_version;
    host.input(vec![EditorInput::Text("héllo 😀".to_string())]);
    let frame = host.frame_at_version(before + 1);
    assert_eq!(frame.rows[0].text(), "héllo 😀");
    // Two UTF-16 units for the emoji, so the caret a browser would place is
    // past them both.
    assert_eq!(frame.caret.column, 8);
}

/// The surface is one editor with two sinks: a save is still a request the
/// daemon answers, not a write the headless host makes.
#[test]
fn a_headless_save_is_still_a_request() {
    let host = start("alpha\n", "src/note.txt");
    host.key(WireKey::Char('!'), 0);
    host.key(WireKey::Char('s'), modifiers::CONTROL);
    let deadline = Instant::now() + WAIT;
    loop {
        assert!(Instant::now() < deadline, "no SaveRequest arrived");
        match host.messages.recv_timeout(WAIT) {
            Ok(EditorMessage::SaveRequest { text, .. }) => {
                assert_eq!(text, "!alpha\n");
                return;
            }
            Ok(_) => continue,
            Err(error) => panic!("the control channel ended: {error}"),
        }
    }
}

/// A frame the GUI cannot fit is clipped by the producer, not by the socket.
#[test]
fn a_window_of_very_long_lines_is_clipped_to_the_budget() {
    let line = "x".repeat(6000);
    let text: String = std::iter::repeat_n(line.as_str(), 200)
        .collect::<Vec<_>>()
        .join("\n");
    let host = start(&text, "src/wide.txt");
    host.send(DaemonMessage::SetView {
        request_id: 4,
        view: ViewRequest {
            first_line: 0,
            line_count: 200,
        },
    });
    let deadline = Instant::now() + WAIT;
    loop {
        assert!(Instant::now() < deadline, "no clipped frame arrived");
        let frame = host.frame();
        if !frame.clipped {
            continue;
        }
        assert!(frame.rows.len() < 200, "the budget did not bite");
        let bytes = encode(&EditorMessage::ViewFrame { frame })
            .expect("encodes")
            .len();
        assert!(bytes <= MAX_VIEW_FRAME_BYTES, "{bytes} bytes over the cap");
        return;
    }
}

/// `std::io::Write` is used by the harness only; the editor never wrote a byte
/// to a terminal here, which is the whole claim.
#[allow(dead_code)]
fn _harness_writes_nothing(_: &mut dyn Write) {}

/// Extending a selection publishes it, in the same units the caret uses.
#[test]
fn a_selection_travels_as_ranges_in_utf16_columns() {
    let host = start("héllo 😀 world\nsecond\n", "src/note.txt");
    host.frame();
    // Home, then shift-End: the whole first line, including the astral pair.
    host.key(WireKey::Home, 0);
    host.key(WireKey::End, modifiers::SHIFT);
    let deadline = Instant::now() + WAIT;
    loop {
        assert!(Instant::now() < deadline, "no frame carried the selection");
        let frame = host.frame();
        let Some(range) = frame.selection.first() else {
            continue;
        };
        assert_eq!(range.from.line, 0);
        assert_eq!(range.from.column, 0);
        assert_eq!(range.to.line, 0);
        // "héllo 😀 world" is 14 UTF-16 units: the emoji is two of them, and
        // a surface that counted chars or bytes would land somewhere else.
        assert_eq!(range.to.column, 14);
        return;
    }
}

/// A click reports a UTF-16 column and the caret lands on it.
///
/// The conversion this exercises is the one nothing else would catch: the
/// column is UTF-16 on the wire, display cells inside `App`, and a tab is one
/// of the first and four of the second.
#[test]
fn a_click_past_a_tab_puts_the_caret_where_it_was_clicked() {
    let host = start("\tabc\n", "src/note.txt");
    host.frame();
    host.input(vec![EditorInput::Mouse {
        kind: WireMouseKind::Down,
        line: 0,
        // Two UTF-16 units in: past the tab and past the "a".
        column: 2,
        modifiers: 0,
    }]);
    let deadline = Instant::now() + WAIT;
    loop {
        assert!(Instant::now() < deadline, "the caret never moved");
        let frame = host.frame();
        if frame.caret.column == 2 {
            assert_eq!(frame.caret.line, 0);
            return;
        }
    }
}

/// A key the surface does not know does nothing, rather than being guessed at.
#[test]
fn an_unknown_function_key_is_ignored_and_not_inserted() {
    let host = start("alpha\n", "src/note.txt");
    let before = host.frame();
    host.key(WireKey::Function(99), 0);
    host.key(WireKey::Char('!'), 0);
    let frame = host.frame_at_version(before.doc_version + 1);
    assert_eq!(frame.rows[0].text(), "!alpha", "only the real key landed");
}

/// The gutter travels with the row it belongs to.
///
/// A mark list arriving on its own would point at lines that had already
/// moved, which is the whole reason these are row fields.
#[test]
fn a_git_mark_arrives_on_the_row_it_marks() {
    let host = start("one\ntwo\nthree\n", "src/note.txt");
    host.frame();
    host.send(DaemonMessage::GitMarks {
        request_id: 7,
        marks: vec![
            editor_control::WireMark {
                line: 2,
                kind: WireMarkKind::Modified,
            },
            editor_control::WireMark {
                line: 3,
                kind: WireMarkKind::Added,
            },
        ],
    });
    let deadline = Instant::now() + WAIT;
    loop {
        assert!(Instant::now() < deadline, "no frame carried the gutter");
        let frame = host.frame();
        let Some(second) = frame.rows.get(1) else {
            continue;
        };
        if second.mark.is_none() {
            continue;
        }
        assert_eq!(second.mark, Some(WireMarkKind::Modified));
        assert_eq!(frame.rows[2].mark, Some(WireMarkKind::Added));
        assert_eq!(frame.rows[0].mark, None, "an unchanged line carries none");
        return;
    }
}

/// Find highlights travel as ranges the surface can paint, not as attributes
/// baked into the text.
#[test]
fn a_find_marks_every_hit_in_the_window() {
    let host = start("alpha beta alpha\n", "src/note.txt");
    host.frame();
    // Ctrl-F opens the GUI's panel; the query arrives from its field.
    host.key(WireKey::Char('f'), modifiers::CONTROL);
    host.send(find_set("alpha"));
    let deadline = Instant::now() + WAIT;
    loop {
        assert!(Instant::now() < deadline, "no frame carried a decoration");
        let frame = host.frame();
        if frame.decorations.is_empty() {
            continue;
        }
        assert!(
            frame.decorations.iter().all(|d| d.range.from.line == 0),
            "every hit is on the one line: {:?}",
            frame.decorations
        );
        return;
    }
}

/// A surface with no status row still sees what it is typing into.
///
/// The prompt is the thing being interacted with, so it travels on the state
/// the chrome already reads rather than needing a row nobody paints.
#[test]
fn an_open_prompt_reaches_a_surface_that_has_no_status_row() {
    let host = start("alpha beta\n", "src/note.txt");
    host.frame();
    host.key(WireKey::Char('g'), modifiers::CONTROL);
    host.key(WireKey::Char('1'), 0);
    let deadline = Instant::now() + WAIT;
    loop {
        assert!(
            Instant::now() < deadline,
            "the prompt never reached a State"
        );
        match host.messages.recv_timeout(WAIT) {
            Ok(EditorMessage::State { state, .. }) if state.status.starts_with("go to line: 1") => {
                return;
            }
            Ok(_) => continue,
            Err(error) => panic!("the control channel ended: {error}"),
        }
    }
}

/// Find is the GUI's panel: its count and position travel on the state, and
/// no prompt line does.
#[test]
fn find_reports_its_count_on_the_state_instead_of_a_prompt() {
    let host = start("alpha beta beta\n", "src/note.txt");
    host.frame();
    host.key(WireKey::Char('f'), modifiers::CONTROL);
    host.send(find_set("bet"));
    let deadline = Instant::now() + WAIT;
    loop {
        assert!(Instant::now() < deadline, "the find never reached a State");
        match host.messages.recv_timeout(WAIT) {
            Ok(EditorMessage::State { state, .. }) => {
                let Some(find) = state.find else { continue };
                if find.pattern != "bet" {
                    continue;
                }
                assert_eq!((find.total, find.index), (2, 1));
                assert!(!state.status.starts_with("find:"), "{}", state.status);
                return;
            }
            Ok(_) => continue,
            Err(error) => panic!("the control channel ended: {error}"),
        }
    }
}

fn find_set(pattern: &str) -> DaemonMessage {
    DaemonMessage::Find {
        command: WireFindCommand::Set {
            pattern: pattern.into(),
            case_sensitive: false,
            whole_word: false,
            regex: false,
        },
    }
}
