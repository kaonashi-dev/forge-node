//! End-to-end proof: the binary really runs in a PTY.
//!
//! Unit tests cannot see the terminal handshake. These open a real PTY, run
//! `forge-editor` against a temp file, send keystrokes and check what comes
//! back — the parts no viewport test can answer: raw mode, bracketed paste,
//! resize, an atomic save, and that a control byte in the file is drawn rather
//! than executed.

use std::fs;
use std::io::{Read, Write};
use std::sync::mpsc::{channel, Receiver};
use std::thread;
use std::time::{Duration, Instant};

use portable_pty::{native_pty_system, Child, CommandBuilder, PtySize};

const WAIT: Duration = Duration::from_secs(10);

struct ChildGuard(Box<dyn Child + Send + Sync>);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
    }
}

fn spawn_reader(mut reader: Box<dyn Read + Send>) -> Receiver<Vec<u8>> {
    let (sender, receiver) = channel();
    thread::spawn(move || {
        let mut buffer = [0_u8; 4096];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(count) => {
                    if sender.send(buffer[..count].to_vec()).is_err() {
                        break;
                    }
                }
            }
        }
    });
    receiver
}

fn wait_for(
    receiver: &Receiver<Vec<u8>>,
    seen: &mut Vec<u8>,
    needle: &str,
    timeout: Duration,
) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if String::from_utf8_lossy(seen).contains(needle) {
            return true;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return false;
        }
        match receiver.recv_timeout(remaining) {
            Ok(chunk) => seen.extend_from_slice(&chunk),
            Err(_) => return String::from_utf8_lossy(seen).contains(needle),
        }
    }
}

fn wait_for_exit(child: &mut (dyn Child + Send + Sync), timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return true,
            Ok(None) => {}
            Err(_) => return false,
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(Duration::from_millis(25));
    }
}

#[test]
fn opens_edits_saves_resizes_and_quits_under_a_real_pty() {
    let directory = tempfile::tempdir().expect("temp dir");
    let path = directory.path().join("note.txt");
    fs::write(&path, "hello\nworld\n").expect("fixture write");

    let pair = native_pty_system()
        .openpty(PtySize {
            rows: 12,
            cols: 60,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("open pty");
    let mut command = CommandBuilder::new(env!("CARGO_BIN_EXE_forge-editor"));
    command.arg(&path);
    command.env("TERM", "xterm-256color");
    let child = pair.slave.spawn_command(command).expect("spawn editor");
    let mut child = ChildGuard(child);
    drop(pair.slave);

    let reader = pair.master.try_clone_reader().expect("pty reader");
    let mut writer = pair.master.take_writer().expect("pty writer");
    let receiver = spawn_reader(reader);
    let mut seen = Vec::new();

    assert!(
        wait_for(&receiver, &mut seen, "hello", WAIT),
        "the editor never rendered the file"
    );

    writer.write_all(b"X").expect("write key");
    writer.flush().expect("flush key");
    assert!(
        wait_for(&receiver, &mut seen, "Xhello", WAIT),
        "typing was not rendered"
    );

    // A resize must produce a fresh frame; the previous buffer is cleared so
    // a stale match cannot stand in for a redraw.
    seen.clear();
    pair.master
        .resize(PtySize {
            rows: 8,
            cols: 40,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("resize pty");
    assert!(
        wait_for(&receiver, &mut seen, "hello", WAIT),
        "the editor did not redraw after a resize"
    );

    writer.write_all(&[0x13]).expect("write Ctrl-S");
    writer.flush().expect("flush Ctrl-S");
    let deadline = Instant::now() + WAIT;
    while !fs::read_to_string(&path)
        .expect("read back")
        .contains("Xhello")
    {
        assert!(Instant::now() < deadline, "the file was not saved");
        thread::sleep(Duration::from_millis(25));
    }

    writer.write_all(&[0x11]).expect("write Ctrl-Q");
    writer.flush().expect("flush Ctrl-Q");
    assert!(
        wait_for_exit(&mut *child.0, WAIT),
        "the editor did not quit after Ctrl-Q"
    );
}

/// A running editor under a PTY, torn down by `Drop`.
struct Harness {
    _directory: tempfile::TempDir,
    path: std::path::PathBuf,
    master: Box<dyn portable_pty::MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    receiver: Receiver<Vec<u8>>,
    seen: Vec<u8>,
    child: ChildGuard,
}

impl Harness {
    fn start(contents: &str, args: &[&str]) -> Self {
        let directory = tempfile::tempdir().expect("temp dir");
        let path = directory.path().join("note.txt");
        fs::write(&path, contents).expect("fixture write");

        let pair = native_pty_system()
            .openpty(PtySize {
                rows: 12,
                cols: 60,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("open pty");
        let mut command = CommandBuilder::new(env!("CARGO_BIN_EXE_forge-editor"));
        for arg in args {
            command.arg(arg);
        }
        command.arg(&path);
        command.env("TERM", "xterm-256color");
        let child = ChildGuard(pair.slave.spawn_command(command).expect("spawn editor"));
        drop(pair.slave);

        let reader = pair.master.try_clone_reader().expect("pty reader");
        let writer = pair.master.take_writer().expect("pty writer");
        Self {
            _directory: directory,
            path,
            master: pair.master,
            writer,
            receiver: spawn_reader(reader),
            seen: Vec::new(),
            child,
        }
    }

    fn send(&mut self, bytes: &[u8]) {
        self.writer.write_all(bytes).expect("write to pty");
        self.writer.flush().expect("flush pty");
    }

    fn expect(&mut self, needle: &str) {
        assert!(
            wait_for(&self.receiver, &mut self.seen, needle, WAIT),
            "never saw {needle:?}; screen was:\n{}",
            String::from_utf8_lossy(&self.seen)
        );
    }

    fn forget(&mut self) {
        self.seen.clear();
    }

    fn on_disk(&self) -> String {
        fs::read_to_string(&self.path).expect("read back")
    }

    fn wait_for_disk(&self, needle: &str) {
        let deadline = Instant::now() + WAIT;
        while !self.on_disk().contains(needle) {
            assert!(
                Instant::now() < deadline,
                "the file never contained {needle:?}"
            );
            thread::sleep(Duration::from_millis(25));
        }
    }
}

#[test]
fn a_control_byte_in_the_file_is_drawn_and_never_executed() {
    // The file holds a real CSI that would clear the screen if it reached the
    // terminal; the editor must show the control picture instead.
    let mut editor = Harness::start("before\u{1b}[2Jafter\n", &[]);
    editor.expect("␛[2Jafter");
}

#[test]
fn a_bracketed_paste_arrives_as_one_undo_step() {
    let mut editor = Harness::start("\n", &[]);
    editor.expect("1 ");
    editor.send(b"\x1b[200~one\ntwo\x1b[201~");
    editor.expect("two");
    editor.send(&[0x13]); // Ctrl-S
    editor.wait_for_disk("two");

    editor.forget();
    editor.send(&[0x1a]); // Ctrl-Z
    editor.expect("[modified]");
    editor.send(&[0x13]);
    let deadline = Instant::now() + WAIT;
    while editor.on_disk() != "\n" {
        assert!(
            Instant::now() < deadline,
            "undo did not remove the whole paste: {:?}",
            editor.on_disk()
        );
        thread::sleep(Duration::from_millis(25));
    }
}

#[test]
fn a_resize_during_a_paste_still_lands_the_whole_text() {
    let mut editor = Harness::start("\n", &[]);
    editor.expect("1 ");
    editor.send(b"\x1b[200~alpha bravo charlie\x1b[201~");
    editor
        .master
        .resize(PtySize {
            rows: 8,
            cols: 30,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("resize pty");
    editor.send(&[0x13]);
    editor.wait_for_disk("alpha bravo charlie");
}

#[test]
fn read_only_refuses_to_write_and_says_so() {
    let mut editor = Harness::start("locked\n", &["--read-only"]);
    editor.expect("locked");
    editor.send(b"X");
    editor.expect("read-only");
    editor.send(&[0x13]);
    editor.expect("read-only");
    assert_eq!(editor.on_disk(), "locked\n");
    editor.send(&[0x11]); // Ctrl-Q: clean, so it closes without asking
    assert!(wait_for_exit(&mut *editor.child.0, WAIT));
}

#[test]
fn a_dirty_buffer_asks_before_closing() {
    let mut editor = Harness::start("hello\n", &[]);
    editor.expect("hello");
    editor.send(b"X");
    editor.expect("Xhello");
    editor.send(&[0x11]); // Ctrl-Q
    editor.expect("unsaved changes");
    assert!(
        !wait_for_exit(&mut *editor.child.0, Duration::from_millis(300)),
        "Ctrl-Q discarded unsaved changes without asking"
    );
    editor.send(b"d");
    assert!(wait_for_exit(&mut *editor.child.0, WAIT));
}

#[test]
fn a_crlf_file_keeps_its_terminators_through_an_edit_and_a_save() {
    let mut editor = Harness::start("alpha\r\nbeta\r\n", &[]);
    editor.expect("alpha");
    editor.send(b"X");
    editor.send(&[0x13]);
    editor.wait_for_disk("Xalpha");
    assert_eq!(
        fs::read(&editor.path).expect("read back"),
        b"Xalpha\r\nbeta\r\n",
        "the line terminators changed"
    );
}

#[test]
fn opening_with_a_line_number_starts_there() {
    let mut editor = Harness::start("one\ntwo\nthree\n", &["+3"]);
    editor.expect("3:1");
}

// ---- integrated mode (feature 19) ------------------------------------------
//
// A fake daemon speaks the real `editor-control` protocol over a Unix socket:
// handshake, Open, then the request traffic. The editor must build its buffer
// from the socket — never from disk — and refuse to write the checkout.

mod integrated {
    use super::*;
    use editor_control::{
        read_frame, write_frame, DaemonMessage, EditorMessage, EditorStateWire, CONTROL_VERSION,
    };
    use std::os::unix::net::UnixListener;
    use std::sync::mpsc::Sender;

    const SESSION_ID: &str = "ses-editor-19";

    /// The daemon side of the control socket.
    ///
    /// A server thread performs the handshake and forwards everything the
    /// editor sends; daemon requests the test writes are applied by a writer
    /// thread so the two directions never block each other in the test.
    struct FakeDaemon {
        _directory: tempfile::TempDir,
        /// The path the editor was launched with, for `--control`.
        socket: std::path::PathBuf,
        requests: Sender<DaemonMessage>,
        messages: Receiver<EditorMessage>,
    }

    impl FakeDaemon {
        fn send(&self, request: DaemonMessage) {
            self.requests
                .send(request)
                .expect("control requests channel");
        }

        fn next(&self, timeout: Duration) -> Option<EditorMessage> {
            self.messages.recv_timeout(timeout).ok()
        }

        /// The first `State` whose `line` matches.
        fn wait_for_state_with_line(&self, line: u32) -> EditorStateWire {
            let deadline = Instant::now() + WAIT;
            loop {
                assert!(Instant::now() < deadline, "no State with line {line}");
                match self.next(WAIT) {
                    Some(EditorMessage::State { state, .. }) if state.line == line => return state,
                    Some(_) => continue,
                    None => panic!("the control channel closed while waiting for line {line}"),
                }
            }
        }
    }

    /// Bind the socket and start serving.
    fn fake_daemon(text: &str, path: &str) -> FakeDaemon {
        let directory = tempfile::tempdir().expect("temp dir");
        let socket = directory.path().join("editor.sock");
        let listener = UnixListener::bind(&socket).expect("bind control socket");
        let (message_tx, message_rx) = channel();
        let (request_tx, request_rx) = channel();
        let text = text.to_string();
        let path = path.to_string();

        thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("the editor connected");
            stream
                .set_read_timeout(Some(WAIT))
                .expect("read timeout on the control socket");
            let mut writer = stream.try_clone().expect("clone the control stream");

            let hello = match read_frame::<EditorMessage>(&mut stream).expect("Hello") {
                EditorMessage::Hello {
                    version,
                    session_id,
                    pid: _,
                } => (version, session_id),
                other => panic!("expected Hello, got {other:?}"),
            };
            assert_eq!(hello.0, CONTROL_VERSION, "handshake version");
            assert_eq!(hello.1, SESSION_ID, "handshake session identity");

            write_frame(
                &mut writer,
                &DaemonMessage::Welcome {
                    version: CONTROL_VERSION,
                    session_id: hello.1.clone(),
                    buffer_id: 1,
                },
            )
            .expect("Welcome");
            write_frame(
                &mut writer,
                &DaemonMessage::Open {
                    request_id: 101,
                    buffer_id: 1,
                    path: path.clone(),
                    text: text.clone(),
                    revision: Some("rev-1".to_string()),
                    line: Some(2),
                    read_only: true,
                },
            )
            .expect("Open");
            match read_frame::<EditorMessage>(&mut stream).expect("Opened") {
                EditorMessage::Opened {
                    request_id,
                    document_version: _,
                } => assert_eq!(request_id, 101, "the open was echoed"),
                other => panic!("expected Opened, got {other:?}"),
            }

            // The writer thread applies the test's daemon requests; a send is
            // possible the moment the handshake is done.
            thread::spawn(move || {
                for request in request_rx.iter() {
                    write_frame(&mut writer, &request).expect("write control frame");
                }
            });
            while let Ok(message) = read_frame::<EditorMessage>(&mut stream) {
                if message_tx.send(message).is_err() {
                    break;
                }
            }
        });

        FakeDaemon {
            _directory: directory,
            socket,
            requests: request_tx,
            messages: message_rx,
        }
    }

    struct IntegratedEditor {
        // Kept for its lifetime: dropping the master would close the PTY
        // beneath a still-running child.
        _master: Box<dyn portable_pty::MasterPty + Send>,
        writer: Box<dyn Write + Send>,
        receiver: Receiver<Vec<u8>>,
        seen: Vec<u8>,
        child: ChildGuard,
        daemon: FakeDaemon,
    }

    impl IntegratedEditor {
        fn start(text: &str, display_path: &str) -> Self {
            let daemon = fake_daemon(text, display_path);
            let socket = daemon.socket.clone();
            let pair = native_pty_system()
                .openpty(PtySize {
                    rows: 12,
                    cols: 60,
                    pixel_width: 0,
                    pixel_height: 0,
                })
                .expect("open pty");
            let mut command = CommandBuilder::new(env!("CARGO_BIN_EXE_forge-editor"));
            command.arg("--control");
            command.arg(&socket);
            command.arg(display_path);
            command.env("TERM", "xterm-256color");
            command.env("FORGE_SESSION_ID", SESSION_ID);
            let child = ChildGuard(pair.slave.spawn_command(command).expect("spawn editor"));
            drop(pair.slave);

            let reader = pair.master.try_clone_reader().expect("pty reader");
            let writer = pair.master.take_writer().expect("pty writer");
            Self {
                _master: pair.master,
                writer,
                receiver: spawn_reader(reader),
                seen: Vec::new(),
                child,
                daemon,
            }
        }

        fn send(&mut self, bytes: &[u8]) {
            self.writer.write_all(bytes).expect("write to pty");
            self.writer.flush().expect("flush pty");
        }

        fn expect(&mut self, needle: &str) {
            assert!(
                wait_for(&self.receiver, &mut self.seen, needle, WAIT),
                "never saw {needle:?}; screen was:\n{}",
                String::from_utf8_lossy(&self.seen)
            );
        }
    }

    #[test]
    fn integrated_open_shows_the_line_from_control() {
        let mut editor = IntegratedEditor::start("alpha\nworld\nthird\n", "harness/tsconfig.json");
        // The buffer came from the socket, and line 2 was revealed.
        editor.expect("world");
        let state = editor.daemon.wait_for_state_with_line(2);
        assert_eq!(state.path, "harness/tsconfig.json");
        assert!(state.read_only);
        assert!(!state.dirty);
    }

    #[test]
    fn a_reveal_request_moves_the_caret() {
        let mut editor = IntegratedEditor::start("alpha\nworld\nthird\n", "harness/tsconfig.json");
        editor.expect("world");
        editor.daemon.send(DaemonMessage::Reveal {
            request_id: 7,
            line: 3,
            column: Some(4),
        });
        // Both directions of the wire count from 1, so the reported position
        // is the one that was asked for.
        let state = editor.daemon.wait_for_state_with_line(3);
        assert_eq!(state.column, 4);
    }

    #[test]
    fn integrated_mode_reads_no_disk() {
        // The display path is unreadable: if integrated mode opened it, the
        // child would fail before painting the buffer the socket handed over.
        let fixture = tempfile::NamedTempFile::new().expect("fixture");
        std::fs::write(fixture.path(), "from-disk\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(fixture.path()).unwrap().permissions();
            perms.set_mode(0o000);
            std::fs::set_permissions(fixture.path(), perms).unwrap();
        }
        let display = fixture.path().to_string_lossy().into_owned();
        let mut editor = IntegratedEditor::start("from-control\n", &display);
        editor.expect("from-control");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(fixture.path()).unwrap().permissions();
            perms.set_mode(0o644);
            std::fs::set_permissions(fixture.path(), perms).unwrap();
        }
    }

    #[test]
    fn integrated_save_is_refused_without_touching_disk() {
        // The display path does not exist: if the editor touched the disk for
        // save or revision, it would fail loudly or write here.
        let mut editor = IntegratedEditor::start("alpha\nworld\n", "display/only.txt");
        editor.expect("world");
        editor.send(&[0x13]); // Ctrl-S
        editor.expect("integrated save is not available yet");
        editor.send(&[0x11]); // Ctrl-Q quits (read-only buffers are never dirty)
        assert!(
            wait_for_exit(&mut *editor.child.0, WAIT),
            "the editor hung after quit"
        );
    }

    /// A state snapshot can be requested at any time; it does not wait for a
    /// keystroke.
    #[test]
    fn a_state_request_answers_with_a_snapshot() {
        let mut editor = IntegratedEditor::start("alpha\nworld\n", "display/only.txt");
        editor.expect("world");
        editor
            .daemon
            .send(DaemonMessage::GetState { request_id: 9 });
        let deadline = Instant::now() + WAIT;
        loop {
            assert!(Instant::now() < deadline, "no answer to GetState");
            match editor.daemon.next(WAIT) {
                Some(EditorMessage::State {
                    request_id: Some(9),
                    state,
                }) => {
                    assert_eq!(state.path, "display/only.txt");
                    assert!(state.read_only);
                    return;
                }
                Some(_) => continue,
                None => panic!("the control channel closed"),
            }
        }
    }
}
