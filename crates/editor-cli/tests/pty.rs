//! End-to-end spike proof: the binary really runs in a PTY.
//!
//! Unit tests cannot see the terminal handshake. This test opens a real PTY,
//! runs `forge-editor` against a temp file, sends keystrokes, verifies they
//! reach the screen, saves, resizes and quits — the F1 route in one place.

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
