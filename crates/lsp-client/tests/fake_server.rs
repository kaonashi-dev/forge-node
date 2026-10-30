//! Drives [`lsp_client::Server`] against a fake language server that is this
//! test binary re-executed with `LSP_CLIENT_FAKE_SERVER=<mode>`.
//!
//! libtest prints its banner on stdout before any test runs, so the fake is
//! launched through `sh` with stdout moved to stderr and speaks LSP on fd 3.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use lsp_client::{Limits, Location, LspError, Server, ServerConfig, Status};
use serde_json::{json, Value};

const MODE_VAR: &str = "LSP_CLIENT_FAKE_SERVER";

#[test]
fn fake_server_entry() {
    let Ok(mode) = std::env::var(MODE_VAR) else {
        return;
    };
    Fake::new(&mode).run();
    std::process::exit(0);
}

struct Fake {
    mode: String,
    input: BufReader<std::io::Stdin>,
    output: File,
    initialized: bool,
    violations: Vec<String>,
    versions: HashMap<String, i64>,
    last_opened: Option<String>,
    cancels: u64,
    waiting_definition: Option<Value>,
    grandchild: Option<std::process::Child>,
}

impl Fake {
    fn new(mode: &str) -> Self {
        Self {
            mode: mode.to_owned(),
            input: BufReader::new(std::io::stdin()),
            output: File::options()
                .write(true)
                .open("/dev/fd/3")
                .expect("fd 3 is the LSP channel"),
            initialized: false,
            violations: Vec::new(),
            versions: HashMap::new(),
            last_opened: None,
            cancels: 0,
            waiting_definition: None,
            grandchild: None,
        }
    }

    fn run(&mut self) {
        eprintln!("fake server {} up", self.mode);
        while let Some(message) = self.read() {
            self.handle(message);
        }
    }

    fn read(&mut self) -> Option<Value> {
        let mut length = None;
        loop {
            let mut line = String::new();
            if self.input.read_line(&mut line).ok()? == 0 {
                return None;
            }
            let line = line.trim_end();
            if line.is_empty() {
                break;
            }
            if let Some(value) = line.strip_prefix("Content-Length:") {
                length = value.trim().parse::<usize>().ok();
            }
        }
        let mut body = vec![0; length?];
        self.input.read_exact(&mut body).ok()?;
        serde_json::from_slice(&body).ok()
    }

    fn send(&mut self, message: &Value) {
        let body = serde_json::to_vec(message).expect("serialize fake message");
        self.send_raw(&body);
    }

    fn send_raw(&mut self, body: &[u8]) {
        let mut frame = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
        frame.extend_from_slice(body);
        self.output.write_all(&frame).expect("write fake frame");
        self.output.flush().expect("flush fake frame");
    }

    fn reply(&mut self, id: &Value, result: Value) {
        self.send(&json!({"jsonrpc": "2.0", "id": id, "result": result}));
    }

    fn notify(&mut self, method: &str, params: Value) {
        self.send(&json!({"jsonrpc": "2.0", "method": method, "params": params}));
    }

    fn handle(&mut self, message: Value) {
        let id = message.get("id").cloned();
        let params = message.get("params").cloned().unwrap_or(Value::Null);
        let Some(method) = message.get("method").and_then(Value::as_str) else {
            self.on_response(id, &message);
            return;
        };
        match (method, id) {
            ("initialize", Some(id)) => self.initialize(&id, &params),
            ("initialized", None) => self.on_initialized(),
            ("textDocument/didOpen", None) => {
                if !self.initialized {
                    self.violations.push("didOpen before initialized".into());
                }
                let doc = &params["textDocument"];
                let uri = doc["uri"].as_str().unwrap_or_default().to_owned();
                self.versions
                    .insert(uri.clone(), doc["version"].as_i64().unwrap_or(-1));
                self.last_opened = Some(uri);
            }
            ("textDocument/didChange", None) => {
                let doc = &params["textDocument"];
                let uri = doc["uri"].as_str().unwrap_or_default().to_owned();
                self.versions
                    .insert(uri, doc["version"].as_i64().unwrap_or(-1));
            }
            ("textDocument/didSave", None) => match self.mode.as_str() {
                "progress" => self.notify(
                    "$/progress",
                    json!({"token": "idx", "value": {"kind": "end"}}),
                ),
                "status" => self.notify(
                    "experimental/serverStatus",
                    json!({"health": "ok", "quiescent": true}),
                ),
                _ => {}
            },
            ("$/cancelRequest", None) => {
                self.cancels += 1;
                eprintln!("cancel {}", params["id"]);
            }
            ("textDocument/definition", Some(id)) => self.definition(id, &params),
            ("shutdown", Some(id)) => {
                if self.mode != "stubborn" {
                    self.reply(&id, Value::Null);
                }
            }
            ("exit", None) => {
                if self.mode != "stubborn" {
                    std::process::exit(0);
                }
            }
            _ => {}
        }
    }

    fn initialize(&mut self, id: &Value, params: &Value) {
        if self.mode == "silent_init" {
            return;
        }
        if self.mode == "slow" {
            std::thread::sleep(Duration::from_millis(300));
        }
        let expected = [
            params["processId"].is_u64(),
            params["capabilities"]["general"]["positionEncodings"] == json!(["utf-16"]),
            params["capabilities"]["textDocument"]["definition"]["linkSupport"] == json!(true),
            params["capabilities"]["window"]["workDoneProgress"] == json!(true),
            params["rootUri"] == params["workspaceFolders"][0]["uri"],
            params["initializationOptions"] == json!({"fake": true}),
        ];
        if expected.contains(&false) {
            self.send(&json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32602, "message": format!("bad initialize params: {expected:?}")}}));
            return;
        }
        let encoding = if self.mode == "utf8" {
            "utf-8"
        } else {
            "utf-16"
        };
        self.reply(
            id,
            json!({"capabilities": {"positionEncoding": encoding, "definitionProvider": true}}),
        );
    }

    fn on_initialized(&mut self) {
        self.initialized = true;
        match self.mode.as_str() {
            "progress" => self.send(&json!({
                "jsonrpc": "2.0", "id": "create-1",
                "method": "window/workDoneProgress/create", "params": {"token": "idx"},
            })),
            "status" => {
                self.notify(
                    "experimental/serverStatus",
                    json!({"health": "ok", "quiescent": false, "message": "Loading"}),
                );
                self.notify(
                    "$/progress",
                    json!({"token": 9, "value": {"kind": "begin", "title": "Building"}}),
                );
            }
            "stubborn" => {
                // Inherits fd 3, so it holds the client's stdout pipe open
                // after this process is gone; it is never waited for because
                // the client must kill it.
                let child = std::process::Command::new("/bin/sleep")
                    .arg("30")
                    .spawn()
                    .expect("spawn sleep");
                eprintln!("grandchild {}", child.id());
                self.grandchild = Some(child);
            }
            "deaf" => std::thread::sleep(Duration::from_secs(60)),
            _ => {}
        }
    }

    fn on_response(&mut self, id: Option<Value>, message: &Value) {
        match id.as_ref().and_then(Value::as_str) {
            Some("create-1") => {
                self.notify(
                    "$/progress",
                    json!({"token": "idx", "value": {"kind": "begin", "title": "Indexing", "percentage": 0}}),
                );
                self.notify(
                    "$/progress",
                    json!({"token": "idx", "value": {"kind": "report", "percentage": 50}}),
                );
            }
            Some("config-1") => {
                let Some(definition) = self.waiting_definition.take() else {
                    return;
                };
                let answers = &message["result"];
                let line = u64::from(answers[1].is_null());
                let uri = answers[0].as_str().unwrap_or("file:///wrong").to_owned();
                self.reply(&definition, location(&uri, line, 0));
            }
            _ => {}
        }
    }

    fn definition(&mut self, id: Value, params: &Value) {
        match params["position"]["line"].as_u64().unwrap_or(u64::MAX) {
            0 => self.reply(&id, location("file:///tmp/a%20b/%C3%A9.rs", 3, 7)),
            1 => self.reply(
                &id,
                json!([
                    location("file:///x/one.rs", 1, 2),
                    location("untitled:Untitled-1", 0, 0),
                    location("file:///x/two.rs", 3, 4),
                ]),
            ),
            2 => self.reply(
                &id,
                json!([{
                    "originSelectionRange": range(0, 0),
                    "targetUri": "file:///x/link.rs",
                    "targetRange": range(10, 0),
                    "targetSelectionRange": range(12, 5),
                }]),
            ),
            3 => self.reply(&id, Value::Null),
            5 => {
                self.waiting_definition = Some(id);
                self.send(&json!({
                    "jsonrpc": "2.0", "id": "config-1", "method": "workspace/configuration",
                    "params": {"items": [{"section": "fake.answer"}, {"section": "missing.key"}]},
                }));
            }
            6 => {
                let mut body =
                    serde_json::to_vec(&json!({"jsonrpc": "2.0", "id": id, "result": null}))
                        .expect("serialize fake reply");
                body.extend(std::iter::repeat_n(b' ', 16 * 1024));
                self.send_raw(&body);
            }
            7 => std::process::exit(3),
            8 => match (&self.last_opened, self.violations.is_empty()) {
                (Some(uri), true) => {
                    let version = self.versions.get(uri).copied().unwrap_or(-1);
                    let uri = uri.clone();
                    self.reply(&id, location(&uri, u64::try_from(version).unwrap_or(999), 0));
                }
                _ => self.send(&json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32603, "message": format!("{:?}", self.violations)}})),
            },
            9 => self.reply(&id, location("file:///cancels", self.cancels, 0)),
            10 => {
                let many: Vec<Value> = (0..100).map(|n| location("file:///m.rs", n, 0)).collect();
                self.reply(&id, Value::Array(many));
            }
            11 => self.send(&json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32801, "message": "content modified"}})),
            _ => {}
        }
    }
}

fn range(line: u64, character: u64) -> Value {
    json!({"start": {"line": line, "character": character}, "end": {"line": line, "character": character}})
}

fn location(uri: &str, line: u64, character: u64) -> Value {
    json!({"uri": uri, "range": range(line, character)})
}

struct Fixture {
    server: Server,
    root: tempfile::TempDir,
}

fn start(mode: &str) -> Fixture {
    start_with(mode, Limits::default(), json!({}))
}

fn start_with(mode: &str, limits: Limits, settings: Value) -> Fixture {
    let root = tempfile::tempdir().expect("fake server tempdir");
    let trap = if mode == "stubborn" {
        "trap '' TERM; "
    } else {
        ""
    };
    let script = format!(
        "{trap}exec 3>&1 1>&2; exec \"$0\" fake_server_entry --exact --nocapture --test-threads=1"
    );
    let config = ServerConfig {
        id: format!("fake-{mode}"),
        command: PathBuf::from("/bin/sh"),
        args: vec![
            "-c".into(),
            script,
            std::env::current_exe()
                .expect("current exe")
                .to_string_lossy()
                .into_owned(),
        ],
        root: root.path().to_path_buf(),
        env: vec![(MODE_VAR.into(), mode.into())],
        initialization_options: json!({"fake": true}),
        settings,
        limits,
    };
    Fixture {
        server: Server::start(config).expect("start fake server"),
        root,
    }
}

fn soon() -> Instant {
    Instant::now() + Duration::from_secs(10)
}

fn define(server: &Server, line: u32) -> Result<Vec<Location>, LspError> {
    server.definition(
        Path::new("/src/lib.rs"),
        line,
        0,
        soon(),
        &AtomicBool::new(false),
    )
}

fn at(path: &str, line: u32, character: u32) -> Location {
    Location {
        path: PathBuf::from(path),
        line,
        character,
    }
}

/// Wait for a condition a test cannot be woken by (stderr arriving, a
/// reparented grandchild being reaped by init).
fn eventually(what: &str, check: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !check() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn group_gone(pgid: u32) -> bool {
    let pgid = i32::try_from(pgid).expect("pgid fits i32");
    nix::sys::signal::kill(nix::unistd::Pid::from_raw(-pgid), None) == Err(nix::errno::Errno::ESRCH)
}

fn exited_reason(status: &Status) -> &str {
    match status {
        Status::Exited { reason } => reason,
        other => panic!("expected Exited, got {other:?}"),
    }
}

#[test]
fn handshake_reaches_ready_and_keeps_stderr() {
    let fixture = start("basic");
    let server = &fixture.server;
    assert_eq!(server.id(), "fake-basic");
    assert_eq!(server.root(), fixture.root.path());
    assert_eq!(server.wait_ready(soon()), Status::Ready);
    eventually("stderr banner", || {
        server.stderr_tail().contains("fake server basic up")
    });
}

#[test]
fn every_definition_shape_is_normalised() {
    let fixture = start("basic");
    let server = &fixture.server;
    assert_eq!(define(server, 0).unwrap(), vec![at("/tmp/a b/é.rs", 3, 7)]);
    assert_eq!(
        define(server, 1).unwrap(),
        vec![at("/x/one.rs", 1, 2), at("/x/two.rs", 3, 4)]
    );
    assert_eq!(define(server, 2).unwrap(), vec![at("/x/link.rs", 12, 5)]);
    assert_eq!(define(server, 3).unwrap(), Vec::new());
    assert_eq!(define(server, 10).unwrap().len(), lsp_client::MAX_LOCATIONS);
    match define(server, 11) {
        Err(LspError::ResponseError { code: -32801, .. }) => {}
        other => panic!("{other:?}"),
    }
}

#[test]
fn workspace_configuration_is_answered_from_settings() {
    let fixture = start_with(
        "basic",
        Limits::default(),
        json!({"fake": {"answer": "file:///from%20settings/x.rs"}}),
    );
    assert_eq!(
        define(&fixture.server, 5).unwrap(),
        vec![at("/from settings/x.rs", 1, 0)]
    );
}

#[test]
fn progress_tokens_report_indexing_until_they_end() {
    let fixture = start("progress");
    let server = &fixture.server;
    let indexing = Status::Indexing {
        title: "Indexing".into(),
        percentage: Some(50),
    };
    assert_eq!(server.wait_for(soon(), |s| *s == indexing), indexing);
    server.did_save(Path::new("/src/lib.rs"));
    assert_eq!(server.wait_ready(soon()), Status::Ready);
}

#[test]
fn server_status_outranks_open_progress() {
    let fixture = start("status");
    let server = &fixture.server;
    let building = Status::Indexing {
        title: "Building".into(),
        percentage: None,
    };
    assert_eq!(server.wait_for(soon(), |s| *s == building), building);
    server.did_save(Path::new("/src/lib.rs"));
    assert_eq!(server.wait_ready(soon()), Status::Ready);
}

#[test]
fn documents_opened_while_starting_are_replayed_after_initialized() {
    let fixture = start("slow");
    let server = &fixture.server;
    assert_eq!(server.status(), Status::Starting);
    let file = fixture.root.path().join("src/a b.rs");
    server.did_open(&file, "rust", 1, "fn a() {}");
    server.did_change_full(&file, 2, "fn a() { }");
    server.did_open(Path::new("other.rs"), "rust", 1, "");
    server.did_close(Path::new("other.rs"));
    assert_eq!(
        define(server, 8).unwrap(),
        vec![at(&file.to_string_lossy(), 2, 0)]
    );
    server.did_change_full(&file, 3, "fn a() {  }");
    assert_eq!(
        define(server, 8).unwrap(),
        vec![at(&file.to_string_lossy(), 3, 0)]
    );
}

#[test]
fn cancellation_and_deadline_withdraw_the_request() {
    let fixture = start("basic");
    let server = &fixture.server;
    assert_eq!(server.wait_ready(soon()), Status::Ready);

    let cancel = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&cancel);
    let canceller = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(100));
        flag.store(true, Ordering::Relaxed);
    });
    let started = Instant::now();
    let result = server.definition(Path::new("/a.rs"), 4, 0, soon(), &cancel);
    assert!(matches!(result, Err(LspError::Cancelled)), "{result:?}");
    assert!(started.elapsed() < Duration::from_secs(2));
    canceller.join().unwrap();

    let deadline = Instant::now() + Duration::from_millis(200);
    let result = server.definition(Path::new("/a.rs"), 4, 0, deadline, &AtomicBool::new(false));
    assert!(matches!(result, Err(LspError::Timeout)), "{result:?}");

    assert_eq!(define(server, 9).unwrap(), vec![at("/cancels", 2, 0)]);
}

#[test]
fn a_full_pending_map_is_busy() {
    let limits = Limits {
        max_pending: 1,
        ..Limits::default()
    };
    let fixture = start_with("basic", limits, json!({}));
    let server = fixture.server.clone();
    assert_eq!(server.wait_ready(soon()), Status::Ready);
    let cancel = Arc::new(AtomicBool::new(false));
    let blocked = {
        let server = server.clone();
        let cancel = Arc::clone(&cancel);
        // Retries because the main thread's probe may hold the only slot.
        std::thread::spawn(move || loop {
            match server.definition(Path::new("/a.rs"), 4, 0, soon(), &cancel) {
                Err(LspError::Busy) => std::thread::yield_now(),
                other => return other,
            }
        })
    };
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match define(&server, 0) {
            Err(LspError::Busy) => break,
            Ok(_) => assert!(Instant::now() < deadline, "never saw Busy"),
            Err(other) => panic!("{other:?}"),
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    cancel.store(true, Ordering::Relaxed);
    assert!(matches!(blocked.join().unwrap(), Err(LspError::Cancelled)));
    assert!(define(&server, 0).is_ok());
}

#[test]
fn an_oversized_response_fails_its_request_and_the_stream_survives() {
    let limits = Limits {
        max_message_bytes: 4096,
        ..Limits::default()
    };
    let fixture = start_with("basic", limits, json!({}));
    let server = &fixture.server;
    match define(server, 6) {
        Err(LspError::Protocol(message)) => assert!(message.contains("4096"), "{message}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(define(server, 0).unwrap(), vec![at("/tmp/a b/é.rs", 3, 7)]);
}

#[test]
fn a_crash_fails_every_pending_request() {
    let fixture = start("basic");
    let server = fixture.server.clone();
    assert_eq!(server.wait_ready(soon()), Status::Ready);
    let waiting = {
        let server = server.clone();
        std::thread::spawn(move || define(&server, 4))
    };
    match define(&server, 7) {
        Err(LspError::Exited(reason)) => assert!(reason.contains("exit status: 3"), "{reason}"),
        other => panic!("{other:?}"),
    }
    assert!(matches!(waiting.join().unwrap(), Err(LspError::Exited(_))));
    let status = server.status();
    assert!(
        exited_reason(&status).contains("fake server basic up"),
        "{status:?}"
    );
    assert!(matches!(define(&server, 0), Err(LspError::Exited(_))));
    server.did_save(Path::new("/a.rs"));
}

#[test]
fn a_foreign_position_encoding_is_refused() {
    let fixture = start("utf8");
    let status = fixture.server.wait_ready(soon());
    assert!(exited_reason(&status).contains("utf-8"), "{status:?}");
    eventually("group exit", || group_gone(fixture.server.process_id()));
}

#[test]
fn an_unanswered_initialize_times_out() {
    let limits = Limits {
        initialize_timeout: Duration::from_millis(200),
        ..Limits::default()
    };
    let fixture = start_with("silent_init", limits, json!({}));
    let status = fixture.server.wait_ready(soon());
    assert!(exited_reason(&status).contains("initialize"), "{status:?}");
    assert!(matches!(
        define(&fixture.server, 0),
        Err(LspError::Exited(_))
    ));
}

#[test]
fn a_server_that_stops_reading_is_declared_stalled() {
    let limits = Limits {
        writer_stall: Duration::from_millis(200),
        ..Limits::default()
    };
    let fixture = start_with("deaf", limits, json!({}));
    let server = &fixture.server;
    assert_eq!(server.wait_ready(soon()), Status::Ready);
    let text = "x".repeat(64 * 1024);
    let started = Instant::now();
    let path = Path::new("/big.rs");
    server.did_open(path, "rust", 0, &text);
    for version in 1..2000 {
        if matches!(server.status(), Status::Exited { .. }) {
            break;
        }
        server.did_change_full(path, version, &text);
    }
    let status = server.status();
    assert!(exited_reason(&status).contains("stalled"), "{status:?}");
    assert!(started.elapsed() < Duration::from_secs(5));
    eventually("group exit", || group_gone(server.process_id()));
}

#[test]
fn shutdown_is_cooperative_when_the_server_cooperates() {
    let fixture = start("basic");
    let server = &fixture.server;
    assert_eq!(server.wait_ready(soon()), Status::Ready);
    let started = Instant::now();
    server.shutdown(Duration::from_secs(5));
    assert!(started.elapsed() < Duration::from_secs(2));
    let status = server.status();
    assert!(
        exited_reason(&status).contains("exit status: 0"),
        "{status:?}"
    );
    assert!(group_gone(server.process_id()));
}

#[test]
fn shutdown_kills_a_server_that_ignores_it_and_its_grandchildren() {
    let fixture = start("stubborn");
    let server = &fixture.server;
    assert_eq!(server.wait_ready(soon()), Status::Ready);
    assert_eq!(define(server, 3).unwrap(), Vec::new());
    eventually("grandchild", || server.stderr_tail().contains("grandchild"));
    let started = Instant::now();
    server.shutdown(Duration::from_millis(300));
    assert!(started.elapsed() < Duration::from_secs(4));
    assert!(matches!(server.status(), Status::Exited { .. }));
    eventually("group exit", || group_gone(server.process_id()));
}

#[test]
fn dropping_the_last_handle_kills_the_group() {
    let fixture = start("stubborn");
    let pgid = fixture.server.process_id();
    assert_eq!(fixture.server.wait_ready(soon()), Status::Ready);
    assert_eq!(define(&fixture.server, 3).unwrap(), Vec::new());
    let clone = fixture.server.clone();
    drop(fixture.server);
    assert!(!group_gone(pgid));
    drop(clone);
    eventually("group exit", || group_gone(pgid));
}
