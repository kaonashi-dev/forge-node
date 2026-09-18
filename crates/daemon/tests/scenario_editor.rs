//! A daemon-supervised `forge-editor` under a real PTY.
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

def pack_array(items):
    n = len(items)
    header = bytes([0x90 | n]) if n < 16 else bytes([0xDC]) + n.to_bytes(2, "big")
    return header + b"".join(items)

def pack_map(items):
    n = len(items)
    header = bytes([0x80 | n]) if n < 16 else bytes([0xDE]) + n.to_bytes(2, "big")
    body = b""
    for k, v in items:
        body += pack_str(k) + v
    return header + body

def frame(payload):
    return struct.pack(">I", len(payload)) + payload

CONTROL_VERSION = __CONTROL_VERSION__

def hello(session_id, pid):
    inner = pack_map([
        ("version", pack_int(CONTROL_VERSION)),
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

def view_frame(rows, first_line=0, total_lines=1, doc_version=1, caret=(0, 0)):
    packed = []
    for line, spans in rows:
        packed.append(pack_map([
            ("line", pack_int(line)),
            ("truncated", pack_bool(False)),
            ("spans", pack_array([
                pack_map([("text", pack_str(t)), ("scope", pack_str(sc))])
                for t, sc in spans
            ])),
        ]))
    frame_body = pack_map([
        ("buffer_id", pack_int(1)),
        ("doc_version", pack_int(doc_version)),
        ("first_line", pack_int(first_line)),
        ("total_lines", pack_int(total_lines)),
        ("rows", pack_array(packed)),
        ("clipped", pack_bool(False)),
        ("folded", pack_array([])),
        ("caret", pack_map([("line", pack_int(caret[0])), ("column", pack_int(caret[1]))])),
        ("selection", pack_array([])),
        ("extra_carets", pack_array([])),
        ("decorations", pack_array([])),
    ])
    return frame(pack_map([("ViewFrame", pack_map([("frame", frame_body)]))]))

def find_definition(request_id, symbol):
    inner = pack_map([
        ("request_id", pack_int(request_id)),
        ("symbol", pack_str(symbol)),
    ])
    return frame(pack_map([("FindDefinition", inner)]))

def save_request(request_id, text, document_version):
    inner = pack_map([
        ("request_id", pack_int(request_id)),
        ("text", pack_str(text)),
        ("document_version", pack_int(document_version)),
    ])
    return frame(pack_map([("SaveRequest", inner)]))

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
    if 0x90 <= b <= 0x9F:
        n = b & 0x0F
        i += 1
        out = []
        for _ in range(n):
            v, i = unpack(buf, i)
            out.append(v)
        return out, i
    if b in (0xDC, 0xDD):
        width = 2 if b == 0xDC else 4
        n = int.from_bytes(buf[i+1:i+1+width], "big")
        i += 1 + width
        out = []
        for _ in range(n):
            v, i = unpack(buf, i)
            out.append(v)
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

def await_message(sock, *names):
    global path
    # The daemon sends GitMarks unprompted; a fake that reads blindly would
    # take it for the answer it asked for. Dispatch by name, like the real one.
    deadline = time.time() + 20
    while time.time() < deadline:
        msg = unpack(read_frame(sock))[0]
        if "Retarget" in msg:
            path = msg["Retarget"]["path"]
        for name in names:
            if name in msg:
                return name, msg[name]
    raise SystemExit("no %s arrived" % (names,))

if mode == "marks":
    _, body = await_message(sock, "GitMarks")
    with open(".forge-editor-marks", "w") as fh:
        for mark in body["marks"]:
            fh.write("%s %s\n" % (mark["line"], mark["kind"]))
        fh.write("end\n")

if mode == "save_stale":
    # One save, after the test has made the file stale: the conflict stands
    # so it can be read back.
    for _ in range(600):
        if os.path.exists(".forge-go"):
            break
        time.sleep(0.05)
    sock.sendall(save_request(1001, "from the editor\n", 2))
    name, _ = await_message(sock, "Saved", "SaveRefused")
    with open(".forge-editor-save", "w") as fh:
        fh.write(name + "\n")

if mode == "save_twice":
    # The test makes the file stale on disk, then drops the sentinel: the
    # first save must be refused and the second must go through.
    for _ in range(600):
        if os.path.exists(".forge-go"):
            break
        time.sleep(0.05)
    answers = []
    for attempt in (1, 2):
        sock.sendall(save_request(1000 + attempt, "from the editor\n", 2))
        name, _ = await_message(sock, "Saved", "SaveRefused")
        answers.append(name)
    with open(".forge-editor-save", "w") as fh:
        fh.write(" ".join(answers) + "\n")

if mode == "definition":
    sock.sendall(find_definition(1500, "answer"))
    _, body = await_message(sock, "Definitions")
    with open(".forge-editor-definitions", "w") as fh:
        for place in body["places"]:
            fh.write("%s:%s %s\n" % (place["path"], place["line"], place["text"].strip()))
        fh.write("end\n")

if mode == "copy":
    # Written only once the test has attached: the escape is a single burst,
    # and a client that subscribed after it would wait for a store that has
    # already been broadcast.
    for _ in range(600):
        if os.path.exists(".forge-go"):
            break
        time.sleep(0.05)
    # What `Ctrl-C` leaves on the wire: the clipboard *store*, base64 of
    # "picked up". The daemon's VT engine is the only thing that reads it.
    sys.stdout.write("\x1b]52;c;cGlja2VkIHVw\x07")
    sys.stdout.flush()

if mode == "headless":
    # The DOM surface: no raw mode, no ANSI, and the window is what the
    # daemon is expected to broadcast. `--headless` must have reached argv,
    # or the surface flag never made it to the spawn.
    with open(".forge-editor-argv", "w") as fh:
        fh.write(" ".join(sys.argv[1:]) + "\n")
    sock.sendall(view_frame([(0, [("fn", "Keyword"), (" main", "Function")])],
                            total_lines=2, doc_version=1))
    seen = []
    while len(seen) < 2:
        msg = unpack(read_frame(sock))[0]
        if "Input" in msg:
            events = msg["Input"]["events"]
            keys = []
            for event in events:
                if isinstance(event, dict) and "Key" in event:
                    key = event["Key"]["key"]
                    name = key["Char"] if isinstance(key, dict) else key
                    keys.append("%s+%d" % (name, event["Key"]["modifiers"]))
                elif isinstance(event, dict) and "Text" in event:
                    keys.append("text:%s" % event["Text"])
            seen.append("input " + ",".join(keys))
        elif "SetView" in msg:
            view = msg["SetView"]["view"]
            seen.append("view %d %d" % (view["first_line"], view["line_count"]))
    with open(".forge-editor-surface", "w") as fh:
        fh.write("\n".join(seen) + "\nend\n")
    # A second window, so the test can tell a frame that answered the input
    # from the one that was already on screen.
    sock.sendall(view_frame([(0, [("Xfn main", "Plain")])],
                            total_lines=2, doc_version=2, caret=(0, 1)))

if mode == "save":
    body = os.environ.get("FORGE_TEST_SAVE_TEXT", "saved by the editor\n")
    sock.sendall(save_request(1000, body, 2))
    name, payload = await_message(sock, "Saved", "SaveRefused")
    with open(".forge-editor-save", "w") as fh:
        detail = payload.get("revision") or payload.get("reason") or ""
        fh.write("%s %s\n" % (name, detail))

if mode == "move":
    original = path
    receipt = ".forge-move-" + os.path.basename(original)
    count = 0
    while True:
        msg = unpack(read_frame(sock))[0]
        if "Retarget" in msg:
            path = msg["Retarget"]["path"]
        elif "GetState" in msg:
            # An old queued state must never redirect the daemon's save destination.
            sock.sendall(state(original, 7))
        elif "Save" in msg:
            count += 1
            sock.sendall(save_request(2000 + count, "draft " + original + "\n", count + 1))
            name, _ = await_message(sock, "Saved", "SaveRefused")
            with open(receipt, "w") as fh:
                fh.write("%s %s %d\n" % (name, path, count))

while True:
    time.sleep(0.25)
"#;

/// The fake's source, speaking whatever control version this build speaks.
///
/// Baked in rather than hardcoded: a bumped `CONTROL_VERSION` would otherwise
/// fail every test here with "a session should reach Running" and say nothing
/// about the handshake being the reason.
fn fake_editor_source() -> String {
    FAKE_EDITOR.replace(
        "__CONTROL_VERSION__",
        &editor_control::CONTROL_VERSION.to_string(),
    )
}

fn install_fake_editor(harness: &common::Harness) {
    let path = harness.bin().join("forge-editor");
    fs::write(&path, fake_editor_source()).expect("write fake editor");
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
            autosave: false,
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
    let script = fake_editor_source().replace(
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
            autosave: false,
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

/// The whole integrated save, end to end: the editor asks, `fs-service`
/// writes, and the answer carries the revision the next save must present.
///
/// The editor never opens the checkout — this is the only path a keystroke in
/// an integrated buffer can reach disk by.
#[test]
fn an_integrated_save_writes_through_fs_service() {
    let harness = common::Harness::new();
    let repo = test_support::init_repo().expect("git repo");
    fs::write(repo.path().join("a.rs"), "before\n").unwrap();
    install_saving_editor(&harness, "after the save\n");
    let running = harness.boot();
    let client = running.connect("editor-save");
    let events = client.events();
    let workspace = common::add_main_workspace(&client, repo.path());

    let response = client
        .request(Request::CreateEditorSession {
            workspace_id: workspace,
            path: "a.rs".into(),
            line: None,
            autosave: false,
            read_only: false,
        })
        .expect("create");
    let Response::SessionCreated { session_id, .. } = response else {
        panic!("expected SessionCreated");
    };
    assert!(
        common::wait_for(&events, common::DEADLINE, |event| {
            matches!(event, DaemonEvent::SessionUpdated(session)
                if session.id == session_id && session.state == SessionState::Running)
        })
        .is_some(),
        "the buffer must open"
    );

    let target = repo.path().join("a.rs");
    assert!(
        common::poll_until(Duration::from_secs(5), || {
            fs::read_to_string(&target).unwrap_or_default() == "after the save\n"
        }),
        "the daemon must write the editor's text: {:?}",
        fs::read_to_string(&target)
    );

    let answer = await_receipt(&repo.path().join(".forge-editor-save"));
    assert!(
        answer.starts_with("Saved "),
        "the answer must carry the new revision, got {answer:?}"
    );
}

/// A read-only session refuses the save rather than writing it.
#[test]
fn a_read_only_editor_session_refuses_the_save() {
    let harness = common::Harness::new();
    let repo = test_support::init_repo().expect("git repo");
    fs::write(repo.path().join("a.rs"), "before\n").unwrap();
    install_saving_editor(&harness, "should never land\n");
    let running = harness.boot();
    let client = running.connect("editor-save-ro");
    let events = client.events();
    let workspace = common::add_main_workspace(&client, repo.path());

    let (session_id, _) = create_editor(&client, &events, workspace, "a.rs", None);
    let answer = await_receipt(&repo.path().join(".forge-editor-save"));
    assert!(
        answer.starts_with("SaveRefused"),
        "a read-only buffer must not reach the disk, got {answer:?}"
    );
    assert_eq!(
        fs::read_to_string(repo.path().join("a.rs")).unwrap(),
        "before\n",
        "the file is untouched"
    );
    let _ = session_id;
}

/// The fake's written answer, once it is actually on disk.
///
/// Waits for a complete line rather than for the path to exist: the file is
/// created when the fake opens it and the bytes land on close, so `exists()`
/// alone races an empty read under a loaded suite.
fn await_receipt(path: &Path) -> String {
    assert!(
        common::poll_until(Duration::from_secs(10), || {
            fs::read_to_string(path).is_ok_and(|body| body.contains('\n'))
        }),
        "the editor must get an answer at {}",
        path.display()
    );
    fs::read_to_string(path).expect("receipt")
}

#[test]
fn moving_a_directory_retargets_all_editors_and_serializes_saves() {
    let harness = common::Harness::new();
    install_editor_mode(&harness, "move", None);
    let repo = test_support::init_repo().unwrap();
    fs::create_dir(repo.path().join("src")).unwrap();
    for name in ["a.rs", "b.rs"] {
        fs::write(repo.path().join("src").join(name), "before\n").unwrap();
    }
    let running = harness.boot();
    let client = running.connect("editor-moves");
    let observer = running.connect("editor-moves-observer");
    let events = client.events();
    let observed = observer.events();
    let workspace = common::add_main_workspace(&client, repo.path());
    let mut ids = Vec::new();
    for name in ["a.rs", "b.rs"] {
        let Response::SessionCreated { session_id, .. } = client
            .request(Request::CreateEditorSession {
                workspace_id: workspace,
                path: format!("src/{name}"),
                line: None,
                autosave: false,
                read_only: false,
            })
            .unwrap()
        else {
            panic!("editor session")
        };
        common::wait_for_running(&events, |s| s.id == session_id);
        ids.push(session_id);
    }
    // The save may reach disk on either side of the move; neither order may recreate src.
    client
        .request(Request::OverwriteEditorBuffer { session_id: ids[0] })
        .unwrap();
    client
        .request(Request::RenamePath {
            workspace_id: workspace,
            from: "src".into(),
            to: "moved".into(),
        })
        .unwrap();
    assert!(common::wait_for(&observed, common::DEADLINE, |event| matches!(event,
        DaemonEvent::SessionUpdated(s) if s.id == ids[1] && s.editor.as_ref().is_some_and(|e| e.path == "moved/b.rs")
    )).is_some());
    assert!(await_receipt(&repo.path().join(".forge-move-a.rs")).starts_with("Saved "));
    for (id, name) in ids.iter().zip(["a.rs", "b.rs"]) {
        client
            .request(Request::OverwriteEditorBuffer { session_id: *id })
            .unwrap();
        assert!(common::poll_until(common::DEADLINE, || {
            fs::read_to_string(repo.path().join(format!(".forge-move-{name}")))
                .is_ok_and(|s| s.starts_with(&format!("Saved moved/{name}")))
        }));
        assert_eq!(
            fs::read_to_string(repo.path().join("moved").join(name)).unwrap(),
            format!("draft src/{name}\n")
        );
        assert_eq!(
            common::session(&client, *id).unwrap().editor.unwrap().path,
            format!("moved/{name}")
        );
    }
    assert!(!repo.path().join("src").exists());
    let error = client.request(Request::RenamePath {
        workspace_id: workspace,
        from: "moved/a.rs".into(),
        to: "moved/b.rs".into(),
    });
    assert!(error.is_err());
    assert_eq!(
        common::session(&client, ids[0])
            .unwrap()
            .editor
            .unwrap()
            .path,
        "moved/a.rs"
    );
    client
        .request(Request::RenamePath {
            workspace_id: workspace,
            from: "moved/a.rs".into(),
            to: "moved/A.rs".into(),
        })
        .unwrap();
    client
        .request(Request::OverwriteEditorBuffer { session_id: ids[0] })
        .unwrap();
    assert!(common::poll_until(common::DEADLINE, || {
        fs::read_to_string(repo.path().join(".forge-move-a.rs"))
            .is_ok_and(|s| s.starts_with("Saved moved/A.rs"))
    }));
    assert_eq!(
        common::session(&observer, ids[0])
            .unwrap()
            .editor
            .unwrap()
            .path,
        "moved/A.rs"
    );
}

/// A refusal must not be a dead end.
///
/// The daemon conditions the write on the revision it last saw. When an agent
/// writes the same path, that revision is stale *forever* unless the refusal
/// teaches the daemon the new one — and a buffer nobody can ever save is worse
/// than a save that asks twice. The second `Ctrl-S` overwrites, which is what
/// the standalone editor has always done.
#[test]
fn a_refused_save_arms_the_next_one_instead_of_trapping_the_buffer() {
    let harness = common::Harness::new();
    let repo = test_support::init_repo().expect("git repo");
    fs::write(repo.path().join("a.rs"), "before\n").unwrap();
    install_editor_mode(&harness, "save_twice", None);
    let running = harness.boot();
    let client = running.connect("editor-save-twice");
    let events = client.events();
    let workspace = common::add_main_workspace(&client, repo.path());

    let response = client
        .request(Request::CreateEditorSession {
            workspace_id: workspace,
            path: "a.rs".into(),
            line: None,
            autosave: false,
            read_only: false,
        })
        .expect("create");
    let Response::SessionCreated { session_id, .. } = response else {
        panic!("expected SessionCreated");
    };
    assert!(
        common::wait_for(&events, common::DEADLINE, |event| {
            matches!(event, DaemonEvent::SessionUpdated(session)
                if session.id == session_id && session.state == SessionState::Running)
        })
        .is_some(),
        "the buffer must open"
    );

    // Somebody else writes the same path, then the editor is let go.
    fs::write(repo.path().join("a.rs"), "an agent wrote this\n").unwrap();
    fs::write(repo.path().join(".forge-go"), b"").unwrap();

    let answers = await_receipt(&repo.path().join(".forge-editor-save"));
    assert_eq!(
        answers.trim(),
        "SaveRefused Saved",
        "the first save is refused and the second overwrites"
    );
    assert_eq!(
        fs::read_to_string(repo.path().join("a.rs")).unwrap(),
        "from the editor\n"
    );
}

/// The conflict is readable, and both answers resolve it.
///
/// `GetEditorConflict` is the only place the two sides travel: the flag rides
/// `SessionUpdated`, the documents never do.
#[test]
fn a_refused_save_offers_both_sides_and_take_disk_resolves_it() {
    let harness = common::Harness::new();
    let repo = test_support::init_repo().expect("git repo");
    fs::write(repo.path().join("a.rs"), "before\n").unwrap();
    install_editor_mode(&harness, "save_stale", None);
    let running = harness.boot();
    let client = running.connect("editor-conflict");
    let events = client.events();
    let workspace = common::add_main_workspace(&client, repo.path());

    let response = client
        .request(Request::CreateEditorSession {
            workspace_id: workspace,
            path: "a.rs".into(),
            line: None,
            autosave: false,
            read_only: false,
        })
        .expect("create");
    let Response::SessionCreated { session_id, .. } = response else {
        panic!("expected SessionCreated");
    };
    assert!(
        common::wait_for(&events, common::DEADLINE, |event| {
            matches!(event, DaemonEvent::SessionUpdated(session)
                if session.id == session_id && session.state == SessionState::Running)
        })
        .is_some(),
        "the buffer must open"
    );

    // Nothing to compare before a save is refused.
    match client
        .request(Request::GetEditorConflict { session_id })
        .expect_err("no conflict yet")
    {
        client::ClientError::Protocol(p) => assert_eq!(p.code, ErrorCode::NotFound),
        other => panic!("expected NotFound, got {other:?}"),
    }

    fs::write(repo.path().join("a.rs"), "an agent wrote this\n").unwrap();
    fs::write(repo.path().join(".forge-go"), b"").unwrap();
    await_receipt(&repo.path().join(".forge-editor-save"));

    // The flag is on the session; the texts are only in the answer.
    assert!(
        common::poll_until(Duration::from_secs(5), || {
            matches!(
                client.request(Request::GetEditorConflict { session_id }),
                Ok(Response::EditorConflict { .. })
            )
        }),
        "the refusal must leave something to compare"
    );
    let Response::EditorConflict { path, disk, mine } = client
        .request(Request::GetEditorConflict { session_id })
        .expect("conflict")
    else {
        panic!("expected EditorConflict");
    };
    assert_eq!(path, "a.rs");
    assert_eq!(disk, "an agent wrote this\n", "what was there instead");
    assert_eq!(mine, "from the editor\n", "the draft that was refused");
    client
        .request(Request::RenamePath {
            workspace_id: workspace,
            from: "a.rs".into(),
            to: "renamed.rs".into(),
        })
        .unwrap();
    let Response::EditorConflict {
        path,
        disk: moved_disk,
        mine: moved_mine,
    } = client
        .request(Request::GetEditorConflict { session_id })
        .unwrap()
    else {
        panic!("the move must retain the conflict")
    };
    assert_eq!(path, "renamed.rs");
    assert_eq!(moved_disk, disk);
    assert_eq!(moved_mine, mine);
    let editor = common::session(&client, session_id)
        .unwrap()
        .editor
        .unwrap();
    assert_eq!(editor.path, "renamed.rs");
    assert!(editor.conflict);
}

/// The gutter's marks are the daemon's `git diff`, not the editor's.
///
/// The editor never opens the checkout, so a mark it draws had to arrive over
/// the control channel — this is the whole of that path, against a real repo.
#[test]
fn the_daemon_sends_git_marks_for_the_open_buffer() {
    let harness = common::Harness::new();
    let repo = test_support::init_repo().expect("git repo");
    // Committed, then changed: line 2 modified, a line appended.
    fs::write(repo.path().join("a.rs"), "one\ntwo\nthree\n").unwrap();
    commit_all(repo.path(), "base");
    fs::write(repo.path().join("a.rs"), "one\nTWO\nthree\nfour\n").unwrap();

    install_editor_mode(&harness, "marks", None);
    let running = harness.boot();
    let client = running.connect("editor-marks");
    let events = client.events();
    let workspace = common::add_main_workspace(&client, repo.path());
    let (session_id, _) = create_editor(&client, &events, workspace, "a.rs", None);
    let _ = session_id;

    let recorded = await_receipt(&repo.path().join(".forge-editor-marks"));
    let lines: Vec<&str> = recorded.lines().filter(|line| *line != "end").collect();
    assert_eq!(
        lines,
        ["2 Modified", "4 Added"],
        "the changed line and the new one, and nothing else"
    );
}

/// Commit everything in `repo`, so a diff has a base to speak against.
fn commit_all(repo: &Path, message: &str) {
    for args in [vec!["add", "-A"], vec!["commit", "-m", message]] {
        let status = std::process::Command::new("git")
            .args(&args)
            .current_dir(repo)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.com")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.com")
            .status()
            .expect("git");
        assert!(status.success(), "git {args:?} failed");
    }
}

/// Install the fake in its save mode, with the text it should ask to write.
fn install_saving_editor(harness: &common::Harness, text: &str) {
    install_editor_mode(harness, "save", Some(text));
}

/// Install the fake pinned to one mode, optionally with the text it saves.
fn install_editor_mode(harness: &common::Harness, mode: &str, text: Option<&str>) {
    let path = harness.bin().join("forge-editor");
    let mut script = fake_editor_source().replace(
        "mode = os.environ.get(\"FORGE_TEST_EDITOR_MODE\", \"serve\")",
        &format!("mode = {mode:?}"),
    );
    if let Some(text) = text {
        script = script.replace(
            "os.environ.get(\"FORGE_TEST_SAVE_TEXT\", \"saved by the editor\\n\")",
            &format!("{text:?}"),
        );
    }
    fs::write(&path, script).unwrap();
    let mut perms = fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&path, perms).unwrap();
}

/// Go-to-definition is a request, because the editor never opens the checkout.
///
/// The symbol travels, the daemon greps the tree through `fs-service` — which
/// validates it as an identifier first, so a caret cannot become a regex — and
/// the candidates come back ranked. Candidates and not a jump: the ranking is a
/// heuristic, so the editor offers the list.
#[test]
fn the_editor_asks_the_daemon_where_a_symbol_is_declared() {
    let harness = common::Harness::new();
    install_editor_mode(&harness, "definition", None);
    let repo = test_support::init_repo().expect("git repo");
    fs::write(repo.path().join("a.rs"), "let x = answer();\n").unwrap();
    fs::write(
        repo.path().join("lib.rs"),
        "pub fn answer() -> u8 {\n    42\n}\n",
    )
    .unwrap();
    commit_all(repo.path(), "seed");
    let running = harness.boot();
    let client = running.connect("editor-definition");
    let events = client.events();
    let workspace = common::add_main_workspace(&client, repo.path());
    let (_session, _terminal) = create_editor(&client, &events, workspace, "a.rs", None);

    let found = await_receipt(&repo.path().join(".forge-editor-definitions"));
    assert!(
        found.contains("lib.rs:1") && found.contains("pub fn answer"),
        "the declaration was not offered:\n{found}"
    );
}

/// A copy in the editor reaches the host, not just the editor's own register.
///
/// The whole chain in one test, because every hop already exists on its own:
/// the editor writes an OSC 52 into its PTY, the daemon's engine is the only
/// VT that parses it, and the store arrives as a broadcast keyed on the
/// *editor's* terminal — which is what lets the Tauri host refuse one from a
/// background agent and forward this one.
#[test]
fn a_copy_in_the_editor_reaches_the_host_as_a_clipboard_store() {
    let harness = common::Harness::new();
    install_editor_mode(&harness, "copy", None);
    let repo = test_support::init_repo().expect("git repo");
    fs::write(repo.path().join("a.rs"), "picked up\n").unwrap();
    let running = harness.boot();
    let client = running.connect("editor-copy");
    let events = client.events();
    let workspace = common::add_main_workspace(&client, repo.path());
    let (_session, terminal) = create_editor(&client, &events, workspace, "a.rs", None);
    common::attach(&client, terminal, 80, 24);
    fs::write(repo.path().join(".forge-go"), "").unwrap();

    let event = common::wait_for(&events, common::DEADLINE, |event| {
        matches!(event, DaemonEvent::ClipboardStore { terminal_id, .. } if *terminal_id == terminal)
    })
    .expect("the editor's OSC 52 never became a clipboard store");
    let DaemonEvent::ClipboardStore { text, .. } = event else {
        unreachable!()
    };
    assert_eq!(text, "picked up");
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
            autosave: false,
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
                autosave: false,
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

/// Install the fake pinned to one mode, for a test that cannot set the
/// environment the spawn will carry.
fn install_fake_editor_in_mode(harness: &common::Harness, mode: &str) {
    let path = harness.bin().join("forge-editor");
    let script = fake_editor_source().replace(
        "mode = os.environ.get(\"FORGE_TEST_EDITOR_MODE\", \"serve\")",
        &format!("mode = {mode:?}"),
    );
    fs::write(&path, script).expect("write fake editor");
    let mut perms = fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&path, perms).unwrap();
}

/// The DOM surface, end to end through the daemon.
///
/// The half `crates/editor-cli/tests/headless.rs` cannot reach: that one proves
/// the host publishes windows, this one proves the daemon spawns it headless,
/// forwards a client's input to its socket, and turns the window it gets back
/// into a `DaemonEvent` a surface can mount.
#[test]
fn the_dom_surface_carries_input_down_and_windows_back_up() {
    let harness = common::Harness::new();
    install_fake_editor_in_mode(&harness, "headless");
    let repo = test_support::init_repo().expect("git repo");
    fs::write(repo.path().join("main.rs"), "fn main\n").unwrap();
    let running = harness.boot_with(|cfg| {
        cfg.editor.surface = daemon::config::EditorSurface::Dom;
    });
    let client = running.connect("editor-dom");
    let events = client.events();
    let workspace = common::add_main_workspace(&client, repo.path());
    let (session_id, _terminal) = create_editor(&client, &events, workspace, "main.rs", None);

    // The window the host published, as a domain event and not a cell grid.
    let first = wait_for_frame(&events, session_id, 1);
    assert_eq!(first.total_lines, 2);
    assert_eq!(first.rows[0].text(), "fn main");
    assert_eq!(
        first.rows[0].spans[0].scope,
        domain::EditorScope::Keyword,
        "the colouring travels with the window"
    );

    client
        .request(Request::SetEditorView {
            session_id,
            first_line: 0,
            line_count: 88,
        })
        .expect("SetEditorView");
    client
        .request(Request::SendEditorInput {
            session_id,
            events: vec![
                domain::EditorInputEvent::Key {
                    key: domain::EditorKey::Char('X'),
                    modifiers: domain::editor_modifiers::CONTROL,
                },
                domain::EditorInputEvent::Text("hi".into()),
            ],
        })
        .expect("SendEditorInput");

    // The fake closes its receipt before publishing the response frame.
    let answer = wait_for_frame(&events, session_id, 2);
    let seen = fs::read_to_string(repo.path().join(".forge-editor-surface")).unwrap();
    assert!(
        seen.contains("view 0 88"),
        "the window the surface mounted must reach the host:\n{seen}"
    );
    assert!(
        seen.contains("input X+2,text:hi"),
        "a named key with its modifiers, and committed text, in one batch:\n{seen}"
    );

    // The spawn itself: the surface flag is what put `--headless` in argv.
    let argv = fs::read_to_string(repo.path().join(".forge-editor-argv")).unwrap();
    assert!(argv.contains("--headless"), "argv was {argv:?}");

    assert_eq!(answer.rows[0].text(), "Xfn main");
    assert_eq!(answer.caret.column, 1);
}

/// The first `EditorFrame` for `session` at or past `version`.
fn wait_for_frame(
    events: &flume::Receiver<DaemonEvent>,
    session: domain::SessionId,
    version: u64,
) -> domain::EditorFrame {
    let event = common::wait_for(events, common::DEADLINE, |event| {
        matches!(event, DaemonEvent::EditorFrame { session_id, frame }
            if *session_id == session && frame.doc_version >= version)
    })
    .expect("an EditorFrame should arrive");
    match event {
        DaemonEvent::EditorFrame { frame, .. } => frame,
        _ => unreachable!(),
    }
}
