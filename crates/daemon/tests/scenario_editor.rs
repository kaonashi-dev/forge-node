//! Feature 19: a daemon-supervised `forge-editor` under a real PTY.
//!
//! The harness PATH is a single directory. These tests write a small Python
//! stand-in that speaks the control protocol, so the suite does not depend on
//! a previously built `forge-editor` binary, and so a crash is just `sys.exit`.

mod common;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::Duration;

use domain::{PtySize, SessionKind, SessionState};
use protocol::{DaemonEvent, ErrorCode, Request, Response};

const FAKE_EDITOR: &str = r#"#!/usr/bin/python3
import os, socket, struct, sys, time

def pack_int(n):
    if 0 <= n <= 127:
        return bytes([n])
    if n <= 0xFF:
        return bytes([0xCC, n])
    if n <= 0xFFFF:
        return bytes([0xCD]) + n.to_bytes(2, "big")
    if n <= 0xFFFFFFFF:
        return bytes([0xCE]) + n.to_bytes(4, "big")
    return bytes([0xCF]) + n.to_bytes(8, "big")

def pack_str(s):
    b = s.encode()
    n = len(b)
    if n < 32:
        return bytes([0xA0 | n]) + b
    if n < 256:
        return bytes([0xD9, n]) + b
    return bytes([0xDA]) + n.to_bytes(2, "big") + b

def pack_bool(v):
    return b"\xc3" if v else b"\xc2"

def pack_nil():
    return b"\xc0"

def pack_map(items):
    n = len(items)
    header = bytes([0x80 | n]) if n < 16 else bytes([0xDE]) + n.to_bytes(2, "big")
    body = b""
    for k, v in items:
        body += pack_str(k) + v
    return header + body

def frame(payload):
    return struct.pack(">I", len(payload)) + payload

def hello(session_id, pid):
    inner = pack_map([
        ("version", pack_int(1)),
        ("session_id", pack_str(session_id)),
        ("pid", pack_int(pid)),
    ])
    return frame(pack_map([("Hello", inner)]))

def opened(request_id):
    inner = pack_map([
        ("request_id", pack_int(request_id)),
        ("document_version", pack_int(1)),
    ])
    return frame(pack_map([("Opened", inner)]))

def state(path, line=1):
    st = pack_map([
        ("path", pack_str(path)),
        ("line", pack_int(line)),
        ("column", pack_int(1)),
        ("dirty", pack_bool(False)),
        ("read_only", pack_bool(True)),
        ("document_version", pack_int(1)),
    ])
    inner = pack_map([("request_id", pack_nil()), ("state", st)])
    return frame(pack_map([("State", inner)]))

def read_frame(sock):
    header = b""
    while len(header) < 4:
        chunk = sock.recv(4 - len(header))
        if not chunk:
            raise EOFError("control eof")
        header += chunk
    n = struct.unpack(">I", header)[0]
    payload = b""
    while len(payload) < n:
        chunk = sock.recv(n - len(payload))
        if not chunk:
            raise EOFError("control eof")
        payload += chunk
    return payload

def unpack(buf, i=0):
    b = buf[i]
    if b <= 0x7F:
        return b, i + 1
    if 0xA0 <= b <= 0xBF:
        n = b & 0x1F
        return buf[i+1:i+1+n].decode(), i + 1 + n
    if 0x80 <= b <= 0x8F:
        n = b & 0x0F
        i += 1
        out = {}
        for _ in range(n):
            k, i = unpack(buf, i)
            v, i = unpack(buf, i)
            out[k] = v
        return out, i
    if b == 0xC0:
        return None, i + 1
    if b == 0xC2:
        return False, i + 1
    if b == 0xC3:
        return True, i + 1
    if b == 0xCC:
        return buf[i+1], i + 2
    if b == 0xCD:
        return int.from_bytes(buf[i+1:i+3], "big"), i + 3
    if b == 0xCE:
        return int.from_bytes(buf[i+1:i+5], "big"), i + 5
    if b == 0xCF:
        return int.from_bytes(buf[i+1:i+9], "big"), i + 9
    if b == 0xD9:
        n = buf[i+1]
        return buf[i+2:i+2+n].decode(), i + 2 + n
    if b == 0xDA:
        n = int.from_bytes(buf[i+1:i+3], "big")
        return buf[i+3:i+3+n].decode(), i + 3 + n
    if b == 0xDE:
        n = int.from_bytes(buf[i+1:i+3], "big")
        i += 3
        out = {}
        for _ in range(n):
            k, i = unpack(buf, i)
            v, i = unpack(buf, i)
            out[k] = v
        return out, i
    raise ValueError("unsupported msgpack 0x%02x" % b)

sock_path = os.environ["FORGE_EDITOR_CONTROL"]
session_id = os.environ.get("FORGE_SESSION_ID", "")
with open(".forge-editor-env", "w") as fh:
    for key in ("FORGE_EDITOR_CONTROL", "FORGE_SESSION_ID", "FORGE_WORKSPACE"):
        fh.write("%s=%s\n" % (key, os.environ.get(key, "")))

mode = os.environ.get("FORGE_TEST_EDITOR_MODE", "serve")
if mode == "hang":
    time.sleep(30)
    sys.exit(0)

sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
sock.connect(sock_path)
sock.sendall(hello(session_id, os.getpid()))
welcome = unpack(read_frame(sock))[0]
opened_msg = unpack(read_frame(sock))[0]
open_body = opened_msg.get("Open") or {}
text = open_body.get("text") or ""
path = open_body.get("path") or "buffer"
sys.stdout.write(text)
sys.stdout.flush()
sock.sendall(opened(int(open_body.get("request_id") or 1)))
sock.sendall(state(path, int(open_body.get("line") or 1)))

if mode == "crash":
    time.sleep(0.2)
    sys.exit(1)

while True:
    time.sleep(0.25)
"#;

fn install_fake_editor(harness: &common::Harness) {
    let path = harness.bin().join("forge-editor");
    fs::write(&path, FAKE_EDITOR).expect("write fake editor");
    let mut perms = fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&path, perms).unwrap();
}

fn create_editor(
    client: &client::Client,
    events: &flume::Receiver<DaemonEvent>,
    workspace_id: domain::WorkspaceId,
    path: &str,
    line: Option<u32>,
) -> (domain::SessionId, domain::TerminalId) {
    let response = client
        .request(Request::CreateEditorSession {
            workspace_id,
            path: path.into(),
            line,
            read_only: true,
        })
        .expect("CreateEditorSession");
    let Response::SessionCreated {
        session_id,
        terminal_id,
    } = response
    else {
        panic!("expected SessionCreated, got {response:?}");
    };
    common::wait_for_running(events, |session| session.id == session_id);
    (session_id, terminal_id)
}

#[test]
fn editor_session_process_sees_the_control_env() {
    let harness = common::Harness::new();
    install_fake_editor(&harness);
    let repo = test_support::init_repo().expect("git repo");
    fs::write(repo.path().join("src.rs"), "fn main() {}\n").unwrap();
    let running = harness.boot();
    let client = running.connect("editor-env");
    let events = client.events();
    let workspace = common::add_main_workspace(&client, repo.path());
    let (_session, _terminal) = create_editor(&client, &events, workspace, "src.rs", Some(1));

    assert!(
        common::poll_until(common::DEADLINE, || repo
            .path()
            .join(".forge-editor-env")
            .exists()),
        "the editor should dump the launch environment"
    );
    let dumped = fs::read_to_string(repo.path().join(".forge-editor-env")).unwrap();
    assert!(
        dumped.contains("FORGE_EDITOR_CONTROL="),
        "control socket must travel in the environment:\n{dumped}"
    );
    assert!(dumped.contains("FORGE_SESSION_ID="), "{dumped}");
    assert!(dumped.contains("FORGE_WORKSPACE="), "{dumped}");
    let socket = dumped
        .lines()
        .find_map(|l| l.strip_prefix("FORGE_EDITOR_CONTROL="))
        .unwrap();
    assert!(
        Path::new(socket).exists() || dumped.contains("editor-"),
        "named a control socket: {socket}"
    );
}

#[test]
fn editor_session_opens_with_the_service_text() {
    let harness = common::Harness::new();
    install_fake_editor(&harness);
    let repo = test_support::init_repo().expect("git repo");
    fs::write(repo.path().join("note.txt"), "service-text-xyz\n").unwrap();
    let running = harness.boot();
    let client = running.connect("editor-open");
    let events = client.events();
    let workspace = common::add_main_workspace(&client, repo.path());
    let (_session, terminal) = create_editor(&client, &events, workspace, "note.txt", None);
    // `create_editor` already waited for `Running`, which the handshake sets
    // after the editor painted the buffer: the text is in the attach snapshot,
    // not in a delta that follows it.
    assert!(
        common::attach_and_wait_for_row(&client, &events, terminal, |t| t
            .contains("service-text-xyz"))
        .is_some(),
        "the PTY must show the text fs-service read, not a local open"
    );
}

#[test]
fn editor_state_survives_a_client_reconnect() {
    let harness = common::Harness::new();
    install_fake_editor(&harness);
    let repo = test_support::init_repo().expect("git repo");
    fs::write(repo.path().join("a.rs"), "hello\n").unwrap();
    let running = harness.boot();
    let client = running.connect("editor-state");
    let events = client.events();
    let workspace = common::add_main_workspace(&client, repo.path());
    let (session_id, _terminal) = create_editor(&client, &events, workspace, "a.rs", Some(1));
    assert!(
        common::poll_until(common::DEADLINE, || {
            common::session(&client, session_id)
                .and_then(|s| s.editor)
                .is_some()
        }),
        "state should land on the session after the handshake"
    );
    let before = common::session(&client, session_id)
        .unwrap()
        .editor
        .unwrap();
    assert_eq!(before.path, "a.rs");
    assert!(before.read_only);

    drop(client);
    let again = running.connect("editor-state-2");
    let after = common::session(&again, session_id)
        .and_then(|s| s.editor)
        .expect("snapshot recovers editor state");
    assert_eq!(after.path, before.path);
    assert_eq!(after.document_version, before.document_version);
}

#[test]
fn closing_the_editor_view_detaches_and_kill_ends_it() {
    let harness = common::Harness::new();
    install_fake_editor(&harness);
    let repo = test_support::init_repo().expect("git repo");
    fs::write(repo.path().join("a.rs"), "hello\n").unwrap();
    let running = harness.boot();
    let client = running.connect("editor-kill");
    let events = client.events();
    let workspace = common::add_main_workspace(&client, repo.path());
    let (session_id, terminal) = create_editor(&client, &events, workspace, "a.rs", None);
    common::attach(&client, terminal, 80, 24);
    assert_eq!(
        client
            .request(Request::DetachTerminal {
                terminal_id: terminal
            })
            .expect("detach"),
        Response::Ack
    );
    let still = common::session(&client, session_id).unwrap();
    assert_eq!(still.state, SessionState::Running, "detach is not kill");
    assert_eq!(still.kind, SessionKind::Editor);
    assert!(common::kill_and_wait(&client, &events, session_id));
    let gone = common::session(&client, session_id).unwrap();
    assert!(gone.state.is_terminal());
}

#[test]
fn editor_crash_marks_the_session_and_releases_state() {
    let harness = common::Harness::new();
    install_fake_editor(&harness);
    let repo = test_support::init_repo().expect("git repo");
    fs::write(repo.path().join("a.rs"), "hello\n").unwrap();
    let running = harness.boot_with(|cfg| {
        // The fake honours FORGE_TEST_EDITOR_MODE from the login-shell env.
        // Injected via the process environment the harness shell exports? The
        // fake reads it itself; set it on the harness bin wrapper instead.
        let _ = cfg;
    });
    // Rewrite the fake to crash after handshake.
    let path = harness.bin().join("forge-editor");
    let script = FAKE_EDITOR.replace(
        "mode = os.environ.get(\"FORGE_TEST_EDITOR_MODE\", \"serve\")",
        "mode = \"crash\"",
    );
    fs::write(&path, script).unwrap();
    let mut perms = fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&path, perms).unwrap();

    let client = running.connect("editor-crash");
    let events = client.events();
    let workspace = common::add_main_workspace(&client, repo.path());
    let response = client
        .request(Request::CreateEditorSession {
            workspace_id: workspace,
            path: "a.rs".into(),
            line: None,
            read_only: true,
        })
        .expect("create");
    let Response::SessionCreated { session_id, .. } = response else {
        panic!("expected SessionCreated");
    };
    assert!(
        common::wait_for(&events, common::DEADLINE, |event| {
            matches!(event, DaemonEvent::SessionUpdated(session)
                if session.id == session_id && session.state.is_terminal())
        })
        .is_some(),
        "a crashed editor must leave a terminal state"
    );
    let socket = daemon::editor::control_socket_path(session_id).expect("path");
    assert!(
        common::poll_until(Duration::from_secs(2), || !socket.exists()),
        "Drop must remove the control socket: {}",
        socket.display()
    );
}

#[test]
fn editor_session_missing_binary_fails_the_spawn() {
    let harness = common::Harness::new();
    // No forge-editor on the hermetic PATH.
    let repo = test_support::init_repo().expect("git repo");
    fs::write(repo.path().join("a.rs"), "hello\n").unwrap();
    let running = harness.boot();
    let client = running.connect("editor-missing");
    let workspace = common::add_main_workspace(&client, repo.path());
    let error = client
        .request(Request::CreateEditorSession {
            workspace_id: workspace,
            path: "a.rs".into(),
            line: None,
            read_only: true,
        })
        .expect_err("missing binary");
    match error {
        client::ClientError::Protocol(p) => assert_eq!(p.code, ErrorCode::SpawnError),
        other => panic!("expected SpawnError, got {other:?}"),
    }
    assert!(common::sessions(&client).is_empty());
}

#[test]
fn editor_session_refuses_binary_and_missing() {
    let harness = common::Harness::new();
    install_fake_editor(&harness);
    let repo = test_support::init_repo().expect("git repo");
    fs::write(repo.path().join("ok.rs"), "ok\n").unwrap();
    fs::write(repo.path().join("blob.bin"), b"ok\0nope").unwrap();
    fs::create_dir(repo.path().join("lib")).unwrap();
    let running = harness.boot();
    let client = running.connect("editor-refuse");
    let workspace = common::add_main_workspace(&client, repo.path());
    for path in ["missing.rs", "lib", "blob.bin"] {
        let error = client
            .request(Request::CreateEditorSession {
                workspace_id: workspace,
                path: path.into(),
                line: None,
                read_only: true,
            })
            .expect_err(path);
        match error {
            client::ClientError::Protocol(p) => assert_eq!(p.code, ErrorCode::InvalidRequest),
            other => panic!("{path}: expected InvalidRequest, got {other:?}"),
        }
    }
    assert!(common::sessions(&client).is_empty());
}

#[allow(dead_code)]
fn _size() -> PtySize {
    PtySize {
        cols: 80,
        rows: 24,
        pixel_width: 0,
        pixel_height: 0,
    }
}
