//! Minimal PTY smoke test: spawn `/bin/echo hi`, read its output, and reap it.
//! Bounded by timeouts so it can never hang CI (§21). Uses a real PTY, so it is
//! skipped when `/bin/echo` is absent.

use std::io::Read;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use domain::{PtySize, SpawnSpec};
use terminal_core::{PortablePtyBackend, PtyBackend};

#[test]
fn spawn_echo_read_and_reap() {
    let program = std::path::PathBuf::from("/bin/echo");
    if !program.exists() {
        eprintln!("skipping pty smoke test: /bin/echo not found");
        return;
    }

    let spec = SpawnSpec {
        program,
        args: vec!["hi".to_string()],
        cwd: std::env::temp_dir(),
        env: vec![("PATH".to_string(), "/usr/bin:/bin".to_string())],
    };

    let backend = PortablePtyBackend::new();
    let mut handle = backend
        .spawn(&spec, PtySize::default())
        .expect("spawn /bin/echo");

    assert!(handle.child_pid() > 0, "child pid should be set");
    assert!(
        handle.process_group() > 0,
        "process group leader should be reported"
    );
    // §11.2 criterion (b): the child is its own session/group leader after
    // `setsid`, so its pgid is its pid — and stays that value for the life of
    // the handle rather than following the terminal's foreground job (§11.3
    // signals `-pgid`).
    let pgid = handle.process_group();
    assert_eq!(
        pgid,
        i32::try_from(handle.child_pid()).unwrap(),
        "pgid of a setsid child equals its pid"
    );
    assert_eq!(handle.process_group(), pgid, "pgid must be stable");

    // Read on a worker thread; the main thread bounds the wait with recv_timeout.
    let mut reader = handle.reader();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut buf = [0u8; 4096];
        let mut acc = Vec::new();
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break, // EOF: child exited and slave closed
                Ok(n) => {
                    acc.extend_from_slice(&buf[..n]);
                    if acc.windows(2).any(|w| w == b"hi") {
                        let _ = tx.send(acc.clone());
                    }
                }
                Err(_) => break,
            }
        }
        let _ = tx.send(acc);
    });

    let output = rx
        .recv_timeout(Duration::from_secs(5))
        .expect("should read echo output within 5s");
    let text = String::from_utf8_lossy(&output);
    assert!(text.contains("hi"), "expected 'hi' in output, got {text:?}");

    // The child should exit; poll try_wait for up to ~2.5s.
    let mut exited = false;
    for _ in 0..50 {
        if let Some(status) = handle.try_wait().expect("try_wait should not error") {
            assert_eq!(status.code, Some(0), "echo should exit 0");
            exited = true;
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }
    assert!(exited, "echo child should have exited");
}
