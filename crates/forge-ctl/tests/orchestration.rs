//! Scenario, contract, and launch proofs for `forgectl`.
//!
//! Each test boots a real daemon on a file database and a short socket under
//! `/tmp`, then drives the `forgectl` binary. The client library is used only
//! where the CLI has no command: adding a project, opening a shell, and
//! observing events the CLI does not print.

use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::{Arc, Mutex, Once};
use std::time::{Duration, Instant};

use client::Client;
use daemon::core::Daemon;
use domain::{SessionRole, WorkspaceId};
use flume::RecvTimeoutError;
use protocol::{ClientKind, DaemonEvent, Request, Response};
use serde_json::{json, Value};

const DEADLINE: Duration = Duration::from_secs(45);

fn scratch_dir() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("FORGE_GOAL_SCRATCH") {
        let path = PathBuf::from(path);
        fs::create_dir_all(&path).ok()?;
        return Some(path);
    }
    let path = PathBuf::from(
        "/var/folders/sq/wn3p2b8j4292071rl49wpvm40000gn/T/grok-goal-4ee1d712dc72/implementer",
    );
    path.is_dir().then_some(path)
}

fn append_log(name: &str, body: &str) {
    static CLEAR: Once = Once::new();
    static LOCK: Mutex<()> = Mutex::new(());
    let Some(dir) = scratch_dir() else {
        return;
    };
    let _guard = LOCK.lock().unwrap_or_else(|error| error.into_inner());
    if name == "scenario.log" {
        CLEAR.call_once(|| {
            let _ = fs::remove_file(dir.join(name));
        });
    }
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join(name))
        .expect("open scratch log");
    file.write_all(body.as_bytes()).expect("write scratch log");
}

fn write_log(name: &str, body: &str) {
    let Some(dir) = scratch_dir() else {
        return;
    };
    fs::write(dir.join(name), body).expect("write scratch log");
}

struct Notes {
    name: &'static str,
    body: String,
    overwrite: bool,
}

impl Notes {
    fn scenario() -> Self {
        Self {
            name: "scenario.log",
            body: String::new(),
            overwrite: false,
        }
    }

    fn file(name: &'static str) -> Self {
        Self {
            name,
            body: String::new(),
            overwrite: true,
        }
    }

    fn line(&mut self, line: impl AsRef<str>) {
        self.body.push_str(line.as_ref());
        self.body.push('\n');
    }
}

impl Drop for Notes {
    fn drop(&mut self) {
        if self.overwrite {
            write_log(self.name, &self.body);
        } else {
            append_log(self.name, &self.body);
        }
    }
}

fn sh_quote(s: &str) -> String {
    let mut out = String::from("'");
    for ch in s.chars() {
        if ch == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}

fn install_fake_agent(bin: &Path, capture: &Path, command: &str, provider: &str, version: &str) {
    write_executable(
        bin,
        command,
        &format!(
            "#!/bin/sh\n\
             if [ \"$1\" = --version ]; then\n\
             \x20 printf '%s\\n' '{version}'\n\
             \x20 exit 0\n\
             fi\n\
             if [ -z \"$FORGE_SESSION_ID\" ]; then\n\
             \x20 exit 0\n\
             fi\n\
             /bin/mkdir -p {cap}\n\
             name=${{FORGE_ATTEMPT_ID:-session-$FORGE_SESSION_ID}}\n\
             out={cap}/$name\n\
             {{\n\
             \x20 printf 'PROVIDER=%s\\n' '{provider}'\n\
             \x20 printf 'BIN=%s\\n' '{command}'\n\
             \x20 printf 'SOCKET=%s\\n' \"$FORGE_SOCKET\"\n\
             \x20 printf 'SESSION=%s\\n' \"$FORGE_SESSION_ID\"\n\
             \x20 printf 'RUN=%s\\n' \"$FORGE_RUN_ID\"\n\
             \x20 printf 'TASK=%s\\n' \"$FORGE_TASK_ID\"\n\
             \x20 printf 'ATTEMPT=%s\\n' \"$FORGE_ATTEMPT_ID\"\n\
             \x20 printf 'JSON=%s\\n' \"$FORGECTL_JSON\"\n\
             \x20 printf 'ARGS=%s\\n' \"$*\"\n\
             }} > \"$out\"\n\
             printf 'ready %s\\n' \"$FORGE_ATTEMPT_ID\"\n\
             exec /bin/cat\n",
            version = version,
            provider = provider,
            command = command,
            cap = sh_quote(&capture.to_string_lossy()),
        ),
    );
}

fn write_executable(dir: &Path, name: &str, script: &str) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, script).expect("write script");
    let mut perms = fs::metadata(&path).expect("stat").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&path, perms).expect("chmod");
    path
}

struct Harness {
    tmp: tempfile::TempDir,
    socket: PathBuf,
    db: PathBuf,
    worktrees: PathBuf,
    shell: PathBuf,
    capture: PathBuf,
}

struct Running {
    daemon: Arc<Daemon>,
    socket: PathBuf,
    _thread: std::thread::JoinHandle<()>,
}

impl Drop for Running {
    fn drop(&mut self) {
        self.daemon.request_shutdown();
    }
}

impl Harness {
    fn new() -> Self {
        let tmp = tempfile::tempdir_in("/tmp").expect("tempdir");
        let bin = tmp.path().join("bin");
        fs::create_dir_all(&bin).unwrap();
        std::os::unix::fs::symlink("/usr/bin/env", bin.join("env")).unwrap();
        fs::create_dir_all(tmp.path().join("home")).unwrap();
        let capture = tmp.path().join("capture");
        fs::create_dir_all(&capture).unwrap();
        let shell = write_executable(
            tmp.path(),
            "forge-test-shell",
            &format!(
                "#!/bin/sh\nPATH={path}\nexport PATH\nHOME={home}\nexport HOME\nunset ENV\nunset BASH_ENV\nwhile [ \"$1\" = -l ] || [ \"$1\" = -i ]; do shift; done\nexec /bin/sh \"$@\"\n",
                path = sh_quote(&bin.to_string_lossy()),
                home = sh_quote(&tmp.path().join("home").to_string_lossy()),
            ),
        );
        for (command, provider, version) in [
            ("claude", "claude", "claude 0.0.0-test"),
            ("codex", "codex", "codex 0.0.0-test"),
            ("opencode", "opencode", "opencode 0.0.0-test"),
            ("agent", "cursor", "cursor 0.0.0-test"),
            ("grok", "grok", "grok 0.0.0-test"),
        ] {
            install_fake_agent(&bin, &capture, command, provider, version);
        }
        Self {
            socket: tmp.path().join("d.sock"),
            db: tmp.path().join("forge.db"),
            worktrees: tmp.path().join("worktrees"),
            shell,
            capture,
            tmp,
        }
    }

    fn boot(&self) -> Running {
        self.boot_with(|_| {})
    }

    fn boot_with(&self, configure: impl FnOnce(&mut daemon::config::Config)) -> Running {
        let db = persistence::Db::open(&self.db).expect("open db");
        let mut cfg = daemon::config::Config::default();
        cfg.sessions.shell = self.shell.to_string_lossy().into_owned();
        // The idle sweeper is off so a quiet fake agent is not a writer.
        cfg.sessions.idle_warn_after_secs = 0;
        cfg.sessions.long_running_warn_after_secs = 0;
        cfg.github.refresh_secs = 0;
        configure(&mut cfg);
        let daemon = Daemon::start(
            db,
            cfg,
            self.worktrees.clone(),
            "test-instance".into(),
            "0.0.0-test".into(),
        )
        .expect("start daemon");
        let socket = self.socket.clone();
        let d = daemon.clone();
        let sock = socket.clone();
        let thread = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .unwrap();
            rt.block_on(async move {
                let listener = daemon::server::bind(&sock).expect("bind");
                daemon::server::serve(d, listener, sock).await;
            });
        });
        let deadline = Instant::now() + Duration::from_secs(10);
        while !socket.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(socket.exists(), "daemon socket was not bound");
        Running {
            daemon,
            socket,
            _thread: thread,
        }
    }

    fn crash(running: &Running) {
        running.daemon.request_shutdown();
        let deadline = Instant::now() + Duration::from_secs(10);
        while running.socket.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(!running.socket.exists(), "socket should be unlinked");
    }
}

struct Ctl {
    bin: PathBuf,
    socket: PathBuf,
}

struct Ran {
    code: i32,
    stdout: String,
    stderr: String,
}

impl Ctl {
    fn new(socket: &Path) -> Self {
        Self {
            bin: PathBuf::from(env!("CARGO_BIN_EXE_forgectl")),
            socket: socket.to_path_buf(),
        }
    }

    fn run(&self, args: &[&str], env: &[(&str, &str)]) -> Ran {
        let mut cmd = Command::new(&self.bin);
        cmd.arg("--json")
            .arg("--socket")
            .arg(&self.socket)
            .arg("--timeout")
            .arg("60s")
            .args(args)
            .env("FORGE_SOCKET", &self.socket)
            .env_remove("FORGE_SESSION_ID")
            .env_remove("FORGE_RUN_ID")
            .env_remove("FORGE_TASK_ID")
            .env_remove("FORGE_ATTEMPT_ID")
            .stdin(Stdio::null());
        for (key, value) in env {
            cmd.env(key, value);
        }
        finish(cmd.output().expect("spawn forgectl"))
    }
}

fn finish(output: Output) -> Ran {
    Ran {
        code: output.status.code().unwrap_or(1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

fn parse_stdout(ran: &Ran) -> Value {
    serde_json::from_str(ran.stdout.trim()).unwrap_or_else(|error| {
        panic!(
            "stdout is not json ({error}): {}\nstderr: {}",
            ran.stdout, ran.stderr
        )
    })
}

fn ok(ran: &Ran) -> Value {
    assert_eq!(
        ran.code, 0,
        "expected exit 0, got {}\nstdout {}\nstderr {}",
        ran.code, ran.stdout, ran.stderr
    );
    let value = parse_stdout(ran);
    assert_eq!(value["ok"], json!(true), "{}", ran.stdout);
    value["result"].clone()
}

fn err(ran: &Ran, code: i32) -> Value {
    assert_eq!(
        ran.code, code,
        "expected exit {code}, got {}\nstdout {}\nstderr {}",
        ran.code, ran.stdout, ran.stderr
    );
    let value = parse_stdout(ran);
    assert_eq!(value["ok"], json!(false), "{}", ran.stdout);
    let next = value["error"]["next"].as_array().expect("next");
    assert!(
        next.iter().all(|argv| argv
            .as_array()
            .is_some_and(|argv| argv.iter().all(|part| part.is_string()))),
        "next must be argv, got {}",
        ran.stdout
    );
    value
}

fn git(repo: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .expect("git");
    assert!(
        output.status.success(),
        "git {args:?} in {} failed: {}",
        repo.display(),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn init_repo(root: &Path) -> PathBuf {
    let repo = root.join("repo");
    fs::create_dir_all(&repo).unwrap();
    git(&repo, &["-c", "init.defaultBranch=main", "init"]);
    git(&repo, &["config", "user.email", "t@example.com"]);
    git(&repo, &["config", "user.name", "Test"]);
    git(&repo, &["config", "commit.gpgsign", "false"]);
    fs::write(repo.join("file.txt"), "base\n").unwrap();
    git(&repo, &["add", "file.txt"]);
    git(&repo, &["commit", "-m", "init"]);
    repo
}

fn commit_file(repo: &Path, name: &str, body: &str, message: &str) {
    fs::write(repo.join(name), body).unwrap();
    git(repo, &["add", "--", name]);
    git(repo, &["commit", "-m", message]);
}

fn rev_parse(repo: &Path) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

fn connect(socket: &Path) -> Client {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(client) = Client::connect(socket, "orchestration-test") {
            return client;
        }
        if Instant::now() >= deadline {
            panic!("could not connect to the test daemon");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn add_project(client: &Client, repo: &Path) -> WorkspaceId {
    let response = client
        .request(Request::AddProject {
            path: repo.to_path_buf(),
        })
        .expect("AddProject");
    assert_eq!(response, Response::Ack);
    match client.request(Request::GetSnapshot).expect("snapshot") {
        Response::Snapshot { workspaces, .. } => {
            workspaces
                .into_iter()
                .find(|workspace| workspace.kind == domain::WorkspaceKind::Main)
                .expect("main workspace")
                .id
        }
        other => panic!("snapshot: {other:?}"),
    }
}

fn workspace_path(client: &Client, id: &str) -> PathBuf {
    let id: WorkspaceId = id.parse().expect("workspace id");
    match client.request(Request::GetSnapshot).expect("snapshot") {
        Response::Snapshot { workspaces, .. } => {
            workspaces
                .into_iter()
                .find(|workspace| workspace.id == id)
                .unwrap_or_else(|| panic!("no workspace {id}"))
                .path
        }
        other => panic!("snapshot: {other:?}"),
    }
}

fn poll<T>(what: &str, mut body: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + DEADLINE;
    loop {
        if let Some(value) = body() {
            return value;
        }
        if Instant::now() >= deadline {
            panic!("timed out waiting for {what}");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn field(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(|item| item.as_str())
        .unwrap_or_else(|| panic!("missing {key} in {value}"))
        .to_owned()
}

fn task_named<'a>(view: &'a Value, title: &str) -> &'a Value {
    view["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|task| task["title"] == title)
        .unwrap_or_else(|| panic!("no task {title} in {view}"))
}

fn latest_attempt<'a>(view: &'a Value, task_id: &str) -> &'a Value {
    view["attempts"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|attempt| attempt["task_id"] == task_id)
        .max_by_key(|attempt| attempt["n"].as_u64().unwrap_or(0))
        .unwrap_or_else(|| panic!("no attempt for {task_id}"))
}

fn show(ctl: &Ctl) -> Value {
    ok(&ctl.run(&["run", "show"], &[]))
}

fn transcript(ctl: &Ctl, session: &str) -> String {
    ok(&ctl.run(&["session", "read", session, "--max-bytes", "200000"], &[]))["text"]
        .as_str()
        .unwrap_or("")
        .to_owned()
}

fn wait_text(ctl: &Ctl, session: &str, needle: &str) -> String {
    poll(&format!("transcript {needle}"), || {
        let text = transcript(ctl, session);
        text.contains(needle).then_some(text)
    })
}

fn assert_stays_out(ctl: &Ctl, session: &str, needle: &str) {
    let start = Instant::now();
    while start.elapsed() < Duration::from_millis(1200) {
        let text = transcript(ctl, session);
        assert!(!text.contains(needle), "{needle} appeared in {text}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn capture_of(harness: &Harness, attempt: &str) -> String {
    poll("capture file", || {
        fs::read_to_string(harness.capture.join(attempt)).ok()
    })
}

fn db_stamp(db: &Path) -> String {
    fn one(path: &Path) -> String {
        match fs::metadata(path) {
            Ok(meta) => format!(
                "{} {} {:?}",
                path.display(),
                meta.len(),
                meta.modified().ok()
            ),
            Err(_) => format!("{} missing", path.display()),
        }
    }
    format!(
        "{}\n{}\n{}",
        one(db),
        one(&PathBuf::from(format!("{}-wal", db.display()))),
        one(&PathBuf::from(format!("{}-shm", db.display())))
    )
}

fn start_run(ctl: &Ctl, objective: &str) -> String {
    let started = ok(&ctl.run(
        &[
            "run",
            "start",
            "--controller",
            "none",
            "--objective",
            objective,
        ],
        &[],
    ));
    field(&started, "run_id")
}

fn add_task_in(ctl: &Ctl, run: &str, title: &str, spec: &str, read_only: bool) -> String {
    let mut args = vec![
        "task", "add", "--run", run, "--title", title, "--spec", spec,
    ];
    if read_only {
        args.push("--read-only");
    }
    field(&ok(&ctl.run(&args, &[])), "task_id")
}

fn add_task(ctl: &Ctl, title: &str, spec: &str, after: Option<&str>, read_only: bool) -> String {
    let mut args = vec!["task", "add", "--title", title, "--spec", spec];
    if read_only {
        args.push("--read-only");
    }
    if let Some(after) = after {
        args.push("--after");
        args.push(after);
    }
    field(&ok(&ctl.run(&args, &[])), "task_id")
}

fn start_task(ctl: &Ctl, task: &str) -> Value {
    ok(&ctl.run(&["task", "start", task, "--provider", "claude"], &[]))
}

fn hook_state(ctl: &Ctl, session: &str, state: &str) {
    let ran = ctl.run(
        &["hook", "--state", state],
        &[("FORGE_SESSION_ID", session)],
    );
    assert_eq!(
        ran.code, 0,
        "hook exits 0 with no stdout\nstderr {}",
        ran.stderr
    );
}

fn report_done(ctl: &Ctl, started: &Value, run_id: &str, summary: &str) -> Value {
    let env = [
        ("FORGE_SESSION_ID", field(started, "session_id")),
        ("FORGE_RUN_ID", run_id.to_owned()),
        ("FORGE_TASK_ID", field(started, "task_id")),
        ("FORGE_ATTEMPT_ID", field(started, "attempt_id")),
    ];
    let refs: Vec<(&str, &str)> = env.iter().map(|(k, v)| (*k, v.as_str())).collect();
    ok(&ctl.run(&["report", "--done", "--summary", summary], &refs))
}

fn await_ready(ctl: &Ctl, harness: &Harness, started: &Value) -> String {
    let attempt = field(started, "attempt_id");
    let session = field(started, "session_id");
    wait_text(ctl, &session, &format!("ready {attempt}"));
    capture_of(harness, &attempt)
}

#[test]
fn scenario_ledger_report_wait_and_restart() {
    let mut notes = Notes::scenario();
    notes.line("== ledger ==");
    let harness = Harness::new();
    let repo = init_repo(harness.tmp.path());
    let running = harness.boot();
    let client = connect(&running.socket);
    add_project(&client, &repo);
    let ctl = Ctl::new(&running.socket);
    let run_id = start_run(&ctl, "ship the ledger");

    let once = ok(&ctl.run(
        &[
            "--request-id",
            "req-task-once",
            "task",
            "add",
            "--title",
            "idempotent task",
            "--spec",
            "once",
        ],
        &[],
    ));
    let again = ok(&ctl.run(
        &[
            "--request-id",
            "req-task-once",
            "task",
            "add",
            "--title",
            "idempotent task",
            "--spec",
            "a second body that must not be stored",
        ],
        &[],
    ));
    assert_eq!(once["task_id"], again["task_id"]);
    let listed = ok(&ctl.run(&["task", "list"], &[]));
    let copies = listed["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|task| task["title"] == "idempotent task")
        .count();
    assert_eq!(copies, 1, "same request id must not create a second task");
    notes.line(format!(
        "request_id_once task={} copies={copies}",
        once["task_id"]
    ));

    let report_task = add_task(&ctl, "report me", "PROMPT_TOKEN_report", None, false);
    let started = start_task(&ctl, &report_task);
    let capture = await_ready(&ctl, &harness, &started);
    assert!(
        capture.contains(&field(&started, "attempt_id")),
        "launch env missing attempt: {capture}"
    );
    assert!(
        capture.contains("PROMPT_TOKEN_report"),
        "brief was not the launch argv: {capture}"
    );
    assert!(
        capture.contains("SOCKET=") && !capture.lines().any(|line| line == "SOCKET="),
        "FORGE_SOCKET was not injected: {capture}"
    );
    let text = transcript(&ctl, &field(&started, "session_id"));
    assert!(
        !text.contains("PROMPT_TOKEN_report"),
        "prompt was pasted into the terminal: {text}"
    );
    report_done(&ctl, &started, &run_id, "REPORT_TOKEN_done");
    let shown = ok(&ctl.run(&["task", "show", &report_task], &[]));
    assert_eq!(shown["status"], "review", "{shown}");
    assert_eq!(shown["report"]["outcome"], "done", "{shown}");
    notes.line("report_moves_task_to_review status=review outcome=done");

    let waited = Instant::now();
    let wake = ok(&ctl.run(
        &["run", "wait", "--for", "reported", "--timeout", "8s"],
        &[],
    ));
    let elapsed = waited.elapsed();
    assert!(
        elapsed < Duration::from_secs(3),
        "wait was not immediate: {elapsed:?} {wake}"
    );
    let triggered = wake["triggered"].as_array().expect("triggered");
    assert!(
        triggered.iter().any(|item| item["kind"] == "reported"),
        "triggering item missing: {wake}"
    );
    notes.line(format!(
        "wait_immediate_ms={} kind=reported",
        elapsed.as_millis()
    ));

    let foreign = add_task(&ctl, "foreign report", "foreign", None, false);
    let foreign_started = start_task(&ctl, &foreign);
    await_ready(&ctl, &harness, &foreign_started);
    let foreign_env = [
        (
            "FORGE_SESSION_ID",
            "00000000-0000-0000-0000-000000000099".to_owned(),
        ),
        ("FORGE_ATTEMPT_ID", field(&foreign_started, "attempt_id")),
    ];
    let foreign_refs: Vec<(&str, &str)> = foreign_env
        .iter()
        .map(|(key, value)| (*key, value.as_str()))
        .collect();
    let refused = err(
        &ctl.run(
            &["report", "--done", "--summary", "not my attempt"],
            &foreign_refs,
        ),
        5,
    );
    assert_eq!(refused["error"]["code"], "conflict");
    notes.line("foreign_report_exit=5 code=conflict");
    ok(&ctl.run(
        &["session", "kill", &field(&foreign_started, "session_id")],
        &[],
    ));

    let retry = ok(&ctl.run(
        &[
            "task",
            "reject",
            &report_task,
            "--feedback",
            "FEEDBACK_TOKEN_retry",
            "--retry",
            "--provider",
            "claude",
        ],
        &[],
    ));
    let retry_attempt = field(&retry, "attempt_id");
    let retry_capture = await_ready(&ctl, &harness, &retry);
    assert!(
        retry_capture.contains("FEEDBACK_TOKEN_retry"),
        "retry prompt missing feedback: {retry_capture}"
    );
    assert!(
        retry_capture.contains("REPORT_TOKEN_done"),
        "retry prompt missing previous report: {retry_capture}"
    );
    notes.line("reject_retry_prompt_includes_feedback_and_previous_report");
    let old_env = [
        ("FORGE_SESSION_ID", field(&started, "session_id")),
        ("FORGE_ATTEMPT_ID", field(&started, "attempt_id")),
    ];
    let old_refs: Vec<(&str, &str)> = old_env
        .iter()
        .map(|(key, value)| (*key, value.as_str()))
        .collect();
    let superseded = err(
        &ctl.run(&["report", "--done", "--summary", "too late"], &old_refs),
        5,
    );
    assert_eq!(superseded["error"]["code"], "conflict");
    notes.line("superseded_report_exit=5 code=conflict");

    let die = add_task(&ctl, "exit quietly", "die", None, false);
    let die_started = start_task(&ctl, &die);
    await_ready(&ctl, &harness, &die_started);
    ok(&ctl.run(
        &["session", "kill", &field(&die_started, "session_id")],
        &[],
    ));
    let lost = poll("exited without report", || {
        let view = show(&ctl);
        let attempt = latest_attempt(&view, &die);
        (attempt["phase"] == "lost"
            && attempt["lost_reason"] == "exited_without_report"
            && view["attention"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item["kind"] == "exited_without_report"))
        .then_some(view)
    });
    notes.line(format!(
        "exit_without_report phase=lost reason=exited_without_report attention={}",
        lost["attention"]
    ));

    Harness::crash(&running);
    let restarted = harness.boot();
    let ctl = Ctl::new(&restarted.socket);
    let view = show(&ctl);
    assert_eq!(view["run"]["status"], "interrupted", "{view}");
    let attempt = latest_attempt(&view, &field(&retry, "task_id"));
    assert_eq!(attempt["id"], retry_attempt);
    assert_eq!(attempt["phase"], "lost", "{attempt}");
    assert_eq!(attempt["lost_reason"], "daemon_restarted", "{attempt}");
    let quiet = latest_attempt(&view, &die);
    assert_eq!(quiet["lost_reason"], "exited_without_report");
    notes.line("restart run=interrupted attempt=lost reason=daemon_restarted nothing_resumed");
}

#[test]
fn scenario_integrate_cleanup_and_pull_request() {
    let mut notes = Notes::scenario();
    notes.line("== integrate ==");
    let harness = Harness::new();
    let repo = init_repo(harness.tmp.path());
    let origin = harness.tmp.path().join("origin.git");
    let output = Command::new("git")
        .args(["init", "--bare", "-b", "main"])
        .arg(&origin)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "bare repo: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    git(
        &repo,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    git(&repo, &["push", "-u", "origin", "main"]);

    let gh_log = harness.tmp.path().join("gh.log");
    let gh = write_executable(
        harness.tmp.path(),
        "fake-gh",
        &format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> {}\nprintf '%s\\n' 'https://github.com/acme/forge/pull/7'\nexit 0\n",
            sh_quote(&gh_log.to_string_lossy()),
        ),
    );
    let running = harness.boot_with(|cfg| {
        cfg.github.executable = gh.to_string_lossy().into_owned();
    });
    let client = connect(&running.socket);
    add_project(&client, &repo);
    let ctl = Ctl::new(&running.socket);
    let run_id = start_run(&ctl, "ship the merge");
    let view = show(&ctl);
    let integration = workspace_path(
        &client,
        view["run"]["integration_workspace_id"].as_str().unwrap(),
    );
    let base = rev_parse(&integration);

    let alpha = add_task(&ctl, "alpha", "change alpha", None, false);
    let delta = add_task(&ctl, "delta", "change delta", None, false);
    let later = add_task(&ctl, "later", "PROMPT_TOKEN_later", Some(&alpha), false);
    let ready = ok(&ctl.run(&["task", "list", "--ready"], &[]));
    assert!(
        !ready["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|task| task["id"] == later),
        "dependent task was ready too early: {ready}"
    );
    assert_eq!(task_named(&show(&ctl), "later")["status"], "pending");
    notes.line("dependent_unready_until_accept status=pending");

    let alpha_started = start_task(&ctl, &alpha);
    let delta_started = start_task(&ctl, &delta);
    await_ready(&ctl, &harness, &alpha_started);
    await_ready(&ctl, &harness, &delta_started);
    let alpha_path = workspace_path(&client, &field(&alpha_started, "workspace_id"));
    let delta_path = workspace_path(&client, &field(&delta_started, "workspace_id"));
    commit_file(&alpha_path, "file.txt", "alpha\n", "alpha");
    commit_file(&delta_path, "file.txt", "delta\n", "delta");
    report_done(&ctl, &alpha_started, &run_id, "ALPHA_REPORT_TOKEN");
    report_done(&ctl, &delta_started, &run_id, "delta done");
    ok(&ctl.run(&["task", "accept", &alpha], &[]));
    assert_eq!(task_named(&show(&ctl), "later")["status"], "ready");
    notes.line("dependent_ready_after_accept status=ready");
    ok(&ctl.run(
        &[
            "state",
            "set",
            "plan",
            "{\"ship\":true}",
            "--if-version",
            "0",
        ],
        &[],
    ));
    let digest = ok(&ctl.run(
        &["context"],
        &[
            ("FORGE_RUN_ID", run_id.as_str()),
            ("FORGE_TASK_ID", later.as_str()),
        ],
    ));
    let digest_text = digest["digest"].as_str().unwrap_or("");
    assert!(
        digest_text.contains("ALPHA_REPORT_TOKEN"),
        "context digest missing the accepted summary: {digest_text}"
    );
    assert!(
        digest_text.contains("plan="),
        "context digest missing the board: {digest_text}"
    );
    notes.line("context_includes_accepted_summary_and_board");

    let merged = ok(&ctl.run(&["task", "integrate", &alpha], &[]));
    assert_eq!(merged["state"], "integrated", "{merged}");
    let head = rev_parse(&integration);
    assert_ne!(head, base, "merge did not move the integration head");
    ok(&ctl.run(&["task", "accept", &delta], &[]));
    let later_started = start_task(&ctl, &later);
    let later_capture = await_ready(&ctl, &harness, &later_started);
    assert_eq!(later_started["base_commit"].as_str(), Some(head.as_str()));
    assert!(
        later_capture.contains("ALPHA_REPORT_TOKEN"),
        "new attempt prompt missing the dependency summary: {later_capture}"
    );
    notes.line(format!(
        "new_attempt_branched_from_integration_head base={head}"
    ));

    let conflict = ok(&ctl.run(&["task", "integrate", &delta], &[]));
    assert_eq!(conflict["state"], "conflict", "{conflict}");
    assert!(!conflict["conflicts"].as_array().unwrap().is_empty());
    let aborted = ok(&ctl.run(&["task", "integrate", &delta, "--abort"], &[]));
    assert_eq!(aborted["state"], "aborted", "{aborted}");
    let merge_head = Command::new("git")
        .arg("-C")
        .arg(&integration)
        .args(["rev-parse", "-q", "--verify", "MERGE_HEAD"])
        .output()
        .unwrap();
    assert!(!merge_head.status.success(), "abort left MERGE_HEAD");
    let again = ok(&ctl.run(&["task", "integrate", &delta], &[]));
    assert_eq!(again["state"], "conflict", "{again}");
    fs::write(integration.join("file.txt"), "resolved\n").unwrap();
    git(&integration, &["add", "--", "file.txt"]);
    let continued = ok(&ctl.run(&["task", "integrate", &delta, "--continue"], &[]));
    assert_eq!(continued["state"], "integrated", "{continued}");
    notes.line("merge_conflict_abort_then_continue state=integrated");

    ok(&ctl.run(
        &["session", "kill", &field(&alpha_started, "session_id")],
        &[],
    ));
    ok(&ctl.run(
        &["session", "kill", &field(&delta_started, "session_id")],
        &[],
    ));
    poll("alpha session exited", || {
        let list = ok(&ctl.run(&["session", "list"], &[]));
        list["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|session| {
                session["id"] == field(&alpha_started, "session_id")
                    && session["state"].as_str().unwrap_or("").contains("Exited")
            })
            .then_some(())
    });
    fs::write(delta_path.join("leftover.txt"), "keep me\n").unwrap();
    let cleaned = ok(&ctl.run(&["task", "cleanup", &delta], &[]));
    let residual = cleaned["residual"].as_array().cloned().unwrap_or_default();
    assert!(
        !residual.is_empty(),
        "dirty worktree was removed without a residual: {cleaned}"
    );
    assert!(
        delta_path.exists(),
        "dirty worktree should still be on disk"
    );
    let alpha_branch = field(&alpha_started, "branch");
    let delta_branch = field(&delta_started, "branch");
    let removed = ok(&ctl.run(&["task", "cleanup", &alpha], &[]));
    assert!(
        removed["residual"].as_array().unwrap().is_empty(),
        "clean worktree reported a residual: {removed}"
    );
    assert!(
        !alpha_path.exists(),
        "clean managed worktree should have been removed"
    );
    for branch in [&alpha_branch, &delta_branch] {
        let listed = Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(["branch", "--list", branch])
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&listed.stdout);
        assert!(
            text.contains(branch.as_str()),
            "cleanup deleted branch {branch}: {text}"
        );
    }
    notes.line(format!("cleanup_residual={} branches_kept", residual.len()));

    let events = client.events();
    while events.try_recv().is_ok() {}
    let closed = ok(&ctl.run(
        &[
            "run",
            "close",
            "--pr",
            "--draft",
            "--title",
            "Ship the merge",
            "--base",
            "main",
        ],
        &[],
    ));
    assert_eq!(
        closed["pull_request"]["url"], "https://github.com/acme/forge/pull/7",
        "close hid the pull request: {closed}"
    );
    let opened = poll("pull request", || {
        match events.recv_timeout(Duration::from_millis(200)) {
            Ok(DaemonEvent::PullRequestOpened { url, error, .. }) => Some((url, error)),
            Ok(_) => None,
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => Some((None, Some("disconnected".into()))),
        }
    });
    assert_eq!(
        opened.0.as_deref(),
        Some("https://github.com/acme/forge/pull/7"),
        "pull request failed: {opened:?}"
    );
    let gh_text = fs::read_to_string(&gh_log).unwrap_or_default();
    let creates = gh_text
        .lines()
        .filter(|line| line.contains("pr create"))
        .count();
    assert_eq!(creates, 1, "gh log:\n{gh_text}");
    assert!(
        gh_text.contains("--draft"),
        "draft was not passed to gh:\n{gh_text}"
    );
    notes.line("one_pull_request url=https://github.com/acme/forge/pull/7 draft=true");
}

#[test]
fn scenario_pointer_broadcast_and_cli_events() {
    let mut notes = Notes::scenario();
    notes.line("== pointer ==");
    let harness = Harness::new();
    let repo = init_repo(harness.tmp.path());
    let running = harness.boot();
    let client = connect(&running.socket);
    let main = add_project(&client, &repo);
    let ctl = Ctl::new(&running.socket);
    let run_id = start_run(&ctl, "ship the messages");
    let left = add_task(&ctl, "left reader", "left", None, true);
    let right = add_task(&ctl, "right reader", "right", None, true);
    let left_started = start_task(&ctl, &left);
    let right_started = start_task(&ctl, &right);
    await_ready(&ctl, &harness, &left_started);
    await_ready(&ctl, &harness, &right_started);
    let left_session = field(&left_started, "session_id");
    let right_session = field(&right_started, "session_id");
    let pointer = domain::orchestration::pointer_line(1);

    // The process is in `cat` (it printed `ready`) and activity is still
    // Starting, so a message must not be typed.
    ok(&ctl.run(
        &[
            "send",
            "--to",
            &format!("session:{left_session}"),
            "STARTING_SECRET",
        ],
        &[],
    ));
    assert_stays_out(&ctl, &left_session, &pointer);
    assert_stays_out(&ctl, &left_session, "STARTING_SECRET");
    hook_state(&ctl, &left_session, "waiting");
    ok(&ctl.run(
        &[
            "send",
            "--to",
            &format!("session:{left_session}"),
            "WAITING_SECRET",
        ],
        &[],
    ));
    assert_stays_out(&ctl, &left_session, &pointer);
    assert_stays_out(&ctl, &left_session, "WAITING_SECRET");
    notes.line("pointer_suppressed_while_starting_and_waiting");

    let shell = match client
        .request(Request::CreateShellSession {
            workspace_id: main,
            parent: None,
            role: SessionRole::Generic,
        })
        .expect("shell")
    {
        Response::SessionCreated {
            session_id,
            terminal_id,
        } => (session_id, terminal_id),
        other => panic!("shell: {other:?}"),
    };
    poll("shell running", || {
        let list = ok(&ctl.run(&["session", "list"], &[]));
        list["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|session| {
                session["id"] == shell.0.to_string()
                    && session["state"].as_str().unwrap_or("").contains("Running")
                    && session["activity"] == "unknown"
            })
            .then_some(())
    });
    client
        .request(Request::WriteTerminalInput {
            terminal_id: shell.1,
            bytes: b"printf 'SHELL_READY\\n'\n".to_vec(),
        })
        .expect("write shell");
    wait_text(&ctl, &shell.0.to_string(), "SHELL_READY");
    ok(&ctl.run(
        &[
            "send",
            "--to",
            &format!("session:{}", shell.0),
            "UNKNOWN_SECRET",
        ],
        &[],
    ));
    assert_stays_out(&ctl, &shell.0.to_string(), &pointer);
    assert_stays_out(&ctl, &shell.0.to_string(), "UNKNOWN_SECRET");
    notes.line("pointer_suppressed_while_unknown");

    hook_state(&ctl, &right_session, "working");
    hook_state(&ctl, &right_session, "idle");
    ok(&ctl.run(
        &[
            "send",
            "--to",
            &format!("session:{right_session}"),
            "IDLE_SECRET",
        ],
        &[],
    ));
    let idle_text = wait_text(&ctl, &right_session, &pointer);
    // The tty echoes the typed line and `cat` writes it again, so one delivery
    // shows up twice. A second delivery would be four.
    assert_eq!(idle_text.matches(&pointer).count(), 2, "{idle_text}");
    assert!(!idle_text.contains("IDLE_SECRET"), "{idle_text}");
    notes.line("pointer_typed_once_while_idle");

    // A session with an empty inbox, so `inbox --wait` stays blocked and the
    // watch is still registered when the message is stored.
    let watched = add_task(&ctl, "watched reader", "watched", None, true);
    let watched_started = start_task(&ctl, &watched);
    await_ready(&ctl, &harness, &watched_started);
    let watched_session = field(&watched_started, "session_id");
    hook_state(&ctl, &watched_session, "working");
    hook_state(&ctl, &watched_session, "idle");
    let mut waiting = Command::new(env!("CARGO_BIN_EXE_forgectl"))
        .args([
            "--json",
            "--socket",
            running.socket.to_str().unwrap(),
            "--timeout",
            "30s",
            "inbox",
            "--wait",
            "--timeout",
            "30s",
        ])
        .env("FORGE_SOCKET", &running.socket)
        .env("FORGE_SESSION_ID", &watched_session)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("inbox wait");
    std::thread::sleep(Duration::from_millis(800));
    assert!(
        waiting.try_wait().unwrap().is_none(),
        "inbox wait exited before a message arrived"
    );
    ok(&ctl.run(
        &[
            "send",
            "--to",
            &format!("session:{watched_session}"),
            "WATCH_SECRET",
        ],
        &[],
    ));
    assert_stays_out(&ctl, &watched_session, &pointer);
    assert_stays_out(&ctl, &watched_session, "WATCH_SECRET");
    let _ = waiting.kill();
    let _ = waiting.wait();
    notes.line("pointer_suppressed_while_inbox_wait");

    hook_state(&ctl, &watched_session, "working");
    hook_state(&ctl, &watched_session, "idle");
    ok(&ctl.run(
        &[
            "send",
            "--to",
            &format!("session:{watched_session}"),
            "SECOND_IDLE_SECRET",
        ],
        &[],
    ));
    // The watched inbox already holds the suppressed message, so this delivery
    // says "2 new messages", not the one-message line.
    let twice = poll("second pointer", || {
        let text = transcript(&ctl, &watched_session);
        text.contains("run: forgectl inbox").then_some(text)
    });
    assert!(!twice.contains("SECOND_IDLE_SECRET"), "{twice}");
    notes.line("pointer_typed_again_after_inbox_wait_ended");

    let posted = ok(&ctl.run(
        &[
            "send",
            "--to",
            &format!("run:{run_id}"),
            "RUN_BROADCAST_TOKEN",
        ],
        &[],
    ));
    assert_eq!(
        posted["ids"].as_array().map(|ids| ids.len()),
        Some(3),
        "one run message should store one envelope per live attempt: {posted}"
    );
    for session in [&left_session, &right_session, &watched_session] {
        let inbox = ok(&ctl.run(&["inbox"], &[("FORGE_SESSION_ID", session.as_str())]));
        let hits = inbox["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|message| message["instructions"] == "RUN_BROADCAST_TOKEN")
            .count();
        assert_eq!(hits, 1, "session {session} inbox: {inbox}");
    }
    notes.line("run_message_one_envelope_per_live_attempt count=3");

    let gui = Client::connect(&running.socket, "gui-observer").unwrap();
    let cli = Client::connect_as(&running.socket, "cli-observer", ClientKind::Cli).unwrap();
    let gui_events = gui.events();
    let cli_events = cli.events();
    for _ in 0..20 {
        client
            .request(Request::WriteTerminalInput {
                terminal_id: shell.1,
                bytes: b"printf 'burst\\n'\n".to_vec(),
            })
            .unwrap();
    }
    let start = Instant::now();
    let mut gui_saw = false;
    let mut cli_saw = false;
    while start.elapsed() < Duration::from_secs(4) {
        match gui_events.try_recv() {
            Ok(DaemonEvent::TerminalActivity { .. }) => gui_saw = true,
            Ok(_) => {}
            Err(_) => {}
        }
        match cli_events.try_recv() {
            Ok(DaemonEvent::TerminalActivity { .. }) => cli_saw = true,
            Ok(_) => {}
            Err(_) => {}
        }
        if gui_saw && start.elapsed() > Duration::from_millis(500) {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(gui_saw, "a GUI client saw no terminal activity");
    assert!(!cli_saw, "a CLI connection observed terminal activity");
    notes.line("cli_connection_saw_no_terminal_activity");
}

#[test]
fn scenario_hooks_do_not_write_sqlite() {
    let mut notes = Notes::scenario();
    notes.line("== hooks ==");
    let harness = Harness::new();
    let repo = init_repo(harness.tmp.path());
    let running = harness.boot();
    let client = connect(&running.socket);
    add_project(&client, &repo);
    let ctl = Ctl::new(&running.socket);
    let _run = start_run(&ctl, "ship the hooks");
    let task = add_task(&ctl, "hook me", "hook", None, true);
    let started = start_task(&ctl, &task);
    await_ready(&ctl, &harness, &started);
    let session = field(&started, "session_id");
    // Let the spawn's own session upsert finish, then require a quiet stamp.
    std::thread::sleep(Duration::from_millis(300));
    let first = db_stamp(&harness.db);
    std::thread::sleep(Duration::from_millis(200));
    let stable = db_stamp(&harness.db);
    assert_eq!(
        first, stable,
        "database was still being written before the burst"
    );
    for _ in 0..40 {
        let ran = ctl.run(
            &["hook", "--state", "idle", "--source", "test"],
            &[("FORGE_SESSION_ID", session.as_str())],
        );
        assert_eq!(ran.code, 0, "hook must exit 0: {}", ran.stderr);
    }
    let after = db_stamp(&harness.db);
    assert_eq!(
        stable, after,
        "hook burst wrote SQLite\nbefore {stable}\nafter {after}"
    );
    notes.line("hook_burst_sqlite_writes=0 exit=0");
}

#[test]
fn contract_schema_usage_not_found_policy_cas_and_timeout() {
    let mut notes = Notes::file("contract.log");
    let bin = env!("CARGO_BIN_EXE_forgectl");
    let schema = finish(
        Command::new(bin)
            .args(["--json", "schema"])
            .output()
            .unwrap(),
    );
    notes.line(format!("schema exit={}", schema.code));
    notes.line(&schema.stdout);
    let schema_value = ok(&schema);
    let locked: Value = serde_json::from_str(include_str!("fixtures/schema.json")).unwrap();
    assert_eq!(schema_value, locked);

    let usage = finish(
        Command::new(bin)
            .args(["--json", "guide", "no-such-guide"])
            .output()
            .unwrap(),
    );
    notes.line(format!("usage exit={}", usage.code));
    notes.line(&usage.stdout);
    let usage_value = err(&usage, 2);
    assert_eq!(usage_value["error"]["code"], "usage");
    assert!(usage_value["error"]["next"][0]
        .as_array()
        .unwrap()
        .iter()
        .any(|part| part == "forgectl"));

    let harness = Harness::new();
    let repo = init_repo(harness.tmp.path());
    let running = harness.boot_with(|cfg| {
        cfg.orchestration.max_tasks_per_run = 1;
    });
    let client = connect(&running.socket);
    add_project(&client, &repo);
    let ctl = Ctl::new(&running.socket);

    let missing = ctl.run(
        &["task", "show", "00000000-0000-0000-0000-000000000001"],
        &[],
    );
    notes.line(format!("not_found exit={}", missing.code));
    notes.line(&missing.stdout);
    let missing_value = err(&missing, 4);
    assert_eq!(missing_value["error"]["code"], "not_found");

    let _run = start_run(&ctl, "contract run");
    ok(&ctl.run(&["task", "add", "--title", "only", "--spec", "one"], &[]));
    let policy = ctl.run(&["task", "add", "--title", "second", "--spec", "two"], &[]);
    notes.line(format!("policy exit={}", policy.code));
    notes.line(&policy.stdout);
    let policy_value = err(&policy, 6);
    assert_eq!(policy_value["error"]["code"], "policy");
    let reason = policy_value["error"]["details"]["reason"]
        .as_str()
        .unwrap_or("");
    assert!(
        reason.starts_with("policy:"),
        "policy details: {}",
        policy.stdout
    );

    ok(&ctl.run(
        &["state", "set", "plan", "{\"n\":1}", "--if-version", "0"],
        &[],
    ));
    let conflict = ctl.run(
        &["state", "set", "plan", "{\"n\":2}", "--if-version", "0"],
        &[],
    );
    notes.line(format!("conflict exit={}", conflict.code));
    notes.line(&conflict.stdout);
    let conflict_value = err(&conflict, 5);
    assert_eq!(conflict_value["error"]["code"], "conflict");
    assert_eq!(
        conflict_value["error"]["details"]["current_version"],
        json!(1),
        "{}",
        conflict.stdout
    );

    let waited = ctl.run(
        &["run", "wait", "--for", "reported", "--timeout", "200ms"],
        &[],
    );
    notes.line(format!("timeout exit={}", waited.code));
    notes.line(&waited.stdout);
    let waited_value = err(&waited, 124);
    assert_eq!(waited_value["error"]["code"], "timeout");
}

#[test]
fn launch_human_controller_twice() {
    let mut first = String::new();
    let mut second = String::new();
    let mut shapes = Vec::new();
    for (index, log) in [&mut first, &mut second].into_iter().enumerate() {
        let harness = Harness::new();
        let repo = init_repo(harness.tmp.path());
        let running = harness.boot();
        let client = connect(&running.socket);
        add_project(&client, &repo);
        let ctl = Ctl::new(&running.socket);
        let status = ctl.run(
            &["status"],
            &[
                ("FORGE_SESSION_ID", "11111111-1111-4111-8111-111111111111"),
                ("FORGE_RUN_ID", "22222222-2222-4222-8222-222222222222"),
                ("FORGE_TASK_ID", "33333333-3333-4333-8333-333333333333"),
                ("FORGE_ATTEMPT_ID", "44444444-4444-4444-8444-444444444444"),
            ],
        );
        log.push_str(&format!("status exit={}\n{}\n", status.code, status.stdout));
        let status_result = ok(&status);
        assert!(status_result["protocol_version"].as_u64().is_some());
        assert_eq!(
            status_result["caller"]["session_id"],
            "11111111-1111-4111-8111-111111111111"
        );
        assert!(status_result["limits"]["enabled"].as_bool().unwrap());

        let run = ok(&ctl.run(
            &[
                "run",
                "start",
                "--controller",
                "none",
                "--objective",
                "launch the loop",
            ],
            &[],
        ));
        log.push_str(&format!("run exit=0\n{run}\n"));
        let run_id = field(&run, "run_id");
        let task = add_task(&ctl, "launch task", "PROMPT_TOKEN_launch", None, false);
        log.push_str(&format!("task {task}\n"));
        let started = start_task(&ctl, &task);
        log.push_str(&format!("start {started}\n"));
        await_ready(&ctl, &harness, &started);
        let reported = report_done(&ctl, &started, &run_id, "launch done");
        log.push_str(&format!("report exit=0\n{reported}\n"));
        let shown = ok(&ctl.run(&["task", "show", &task], &[]));
        log.push_str(&format!("show exit=0\n{shown}\n"));
        assert_eq!(shown["status"], "review", "{shown}");
        assert_eq!(shown["report"]["outcome"], "done", "{shown}");
        shapes.push((json_shape(&status_result), json_shape(&shown)));
        write_log(
            if index == 0 {
                "launch-1.log"
            } else {
                "launch-2.log"
            },
            log,
        );
    }
    assert_eq!(shapes[0].0, shapes[1].0, "status shape diverged");
    assert_eq!(shapes[0].1, shapes[1].1, "show shape diverged");
}

#[test]
fn scenario_answer_resumes_blocked_and_close_keeps_residual() {
    let mut notes = Notes::scenario();
    notes.line("== answer ==");
    let harness = Harness::new();
    let repo = init_repo(harness.tmp.path());
    let running = harness.boot();
    let client = connect(&running.socket);
    add_project(&client, &repo);
    let ctl = Ctl::new(&running.socket);
    let run_id = start_run(&ctl, "ship the answer");
    let task = add_task(&ctl, "ask me", "PROMPT_TOKEN_ask", None, false);
    let started = start_task(&ctl, &task);
    await_ready(&ctl, &harness, &started);
    let session = field(&started, "session_id");
    let attempt = field(&started, "attempt_id");
    let workspace = workspace_path(&client, &field(&started, "workspace_id"));
    commit_file(&workspace, "patch.txt", "PATCH_TOKEN\n", "patch");
    let review = ok(&ctl.run(&["task", "review", &task, "--patch"], &[]));
    let patch_text = review["patches"].to_string();
    assert!(
        patch_text.contains("PATCH_TOKEN"),
        "review --patch dropped the patch: {review}"
    );
    let bare = ok(&ctl.run(&["task", "review", &task], &[]));
    assert!(
        bare.get("patches").is_none()
            || bare["patches"]
                .as_array()
                .is_some_and(|patches| patches.is_empty()),
        "review without --patch attached a patch: {bare}"
    );
    notes.line("review_patch_includes_diff");

    let mut asking = Command::new(env!("CARGO_BIN_EXE_forgectl"))
        .args([
            "--json",
            "--socket",
            running.socket.to_str().unwrap(),
            "--timeout",
            "20s",
            "ask",
            "--wait",
            "--timeout",
            "12s",
            "NEED_ANSWER_TOKEN",
        ])
        .env("FORGE_SOCKET", &running.socket)
        .env("FORGE_SESSION_ID", &session)
        .env("FORGE_RUN_ID", &run_id)
        .env("FORGE_TASK_ID", &task)
        .env("FORGE_ATTEMPT_ID", &attempt)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("ask --wait");
    let question = poll("question attention", || {
        let view = show(&ctl);
        view["attention"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| {
                item["kind"] == "question"
                    && item["task_id"] == task
                    && item["text"] == "NEED_ANSWER_TOKEN"
            })
            .cloned()
    });
    notes.line(format!("question_has_task_id {}", question["task_id"]));
    let waited = ok(&ctl.run(
        &[
            "run",
            "wait",
            "--for",
            "question",
            "--task",
            &task,
            "--timeout",
            "2s",
        ],
        &[],
    ));
    assert!(
        waited["triggered"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["kind"] == "question" && item["task_id"] == task),
        "run wait --task dropped the question: {waited}"
    );
    ok(&ctl.run(
        &[
            "send",
            "--to",
            &format!("session:{session}"),
            "--kind",
            "answer",
            "ANSWER_TOKEN",
        ],
        &[],
    ));
    let finished = poll("ask --wait", || asking.try_wait().ok().flatten());
    assert!(finished.success(), "ask --wait exit {finished:?}");
    let mut stdout = String::new();
    asking
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut stdout)
        .unwrap();
    assert!(
        stdout.contains("ANSWER_TOKEN"),
        "ask --wait did not return the answer: {stdout}"
    );
    notes.line("ask_wait_unblocked_by_answer");

    let env = [
        ("FORGE_SESSION_ID", session.clone()),
        ("FORGE_RUN_ID", run_id.clone()),
        ("FORGE_TASK_ID", task.clone()),
        ("FORGE_ATTEMPT_ID", attempt.clone()),
    ];
    let refs: Vec<(&str, &str)> = env
        .iter()
        .map(|(key, value)| (*key, value.as_str()))
        .collect();
    ok(&ctl.run(
        &["report", "--blocked", "--summary", "need a decision"],
        &refs,
    ));
    let blocked = ok(&ctl.run(&["task", "show", &task], &[]));
    assert_eq!(blocked["status"], "blocked", "{blocked}");
    assert_eq!(blocked["attempt"]["phase"], "reported", "{blocked}");
    ok(&ctl.run(
        &[
            "send",
            "--to",
            &format!("session:{session}"),
            "--kind",
            "answer",
            "CONTINUE_TOKEN",
        ],
        &[],
    ));
    let resumed = ok(&ctl.run(&["task", "show", &task], &[]));
    assert_eq!(resumed["status"], "active", "{resumed}");
    assert_eq!(resumed["attempt"]["phase"], "running", "{resumed}");
    ok(&ctl.run(&["report", "--done", "--summary", "continued"], &refs));
    notes.line("blocked_answer_returns_same_attempt_to_active");

    let digest = ok(&ctl.run(
        &["context"],
        &[
            ("FORGE_SESSION_ID", session.as_str()),
            ("FORGE_RUN_ID", run_id.as_str()),
            ("FORGE_TASK_ID", task.as_str()),
        ],
    ));
    let digest_text = digest["digest"].as_str().unwrap_or("");
    assert!(
        digest_text.contains("Unread messages: ") && !digest_text.contains("Unread messages: 0"),
        "context hid unread messages: {digest_text}"
    );
    notes.line("context_counts_unread");

    ok(&ctl.run(&["task", "accept", &task], &[]));
    ok(&ctl.run(&["task", "integrate", &task], &[]));
    fs::write(workspace.join("leftover.txt"), "keep\n").unwrap();
    let closed = ok(&ctl.run(&["run", "close", "--cleanup"], &[]));
    let residual = closed["residual"].as_array().cloned().unwrap_or_default();
    assert!(
        !residual.is_empty(),
        "close --cleanup claimed a dirty worktree was removed: {closed}"
    );
    assert!(workspace.exists(), "dirty worktree was removed");
    notes.line(format!("close_cleanup_residual={}", residual.len()));
}

#[test]
fn scenario_overlapping_starts_keep_one_writer() {
    let mut notes = Notes::scenario();
    notes.line("== one writer ==");
    let harness = Harness::new();
    let repo = init_repo(harness.tmp.path());
    let running = harness.boot();
    let client = connect(&running.socket);
    add_project(&client, &repo);
    let ctl = Ctl::new(&running.socket);
    let _run = start_run(&ctl, "ship one writer");
    let first = add_task(&ctl, "writer one", "one", None, false);
    let second = add_task(&ctl, "writer two", "two", None, false);
    let socket = running.socket.clone();
    let left = first.clone();
    let right = second.clone();
    let socket_left = socket.clone();
    let started = std::thread::spawn(move || {
        Ctl::new(&socket_left).run(
            &[
                "task",
                "start",
                &left,
                "--provider",
                "claude",
                "--placement",
                "integration",
            ],
            &[],
        )
    });
    let other = Ctl::new(&socket).run(
        &[
            "task",
            "start",
            &right,
            "--provider",
            "claude",
            "--placement",
            "integration",
        ],
        &[],
    );
    let one = started.join().expect("start thread");
    let codes = [one.code, other.code];
    assert!(
        codes.contains(&0) && codes.contains(&6),
        "overlapping writers both passed or both failed: codes {codes:?}\n{} / {}\n{} / {}",
        one.stdout,
        other.stdout,
        one.stderr,
        other.stderr
    );
    let view = show(&ctl);
    let live = view["attempts"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|attempt| attempt["phase"] == "running" || attempt["phase"] == "starting")
        .count();
    assert_eq!(live, 1, "two writing attempts were stored: {view}");
    notes.line("overlapping_starts_one_writer");
}

#[test]
fn scenario_overlapping_starts_of_one_task_store_one_attempt() {
    let mut notes = Notes::scenario();
    notes.line("== same task ==");
    let harness = Harness::new();
    let repo = init_repo(harness.tmp.path());
    let running = harness.boot();
    let client = connect(&running.socket);
    add_project(&client, &repo);
    let ctl = Ctl::new(&running.socket);

    for round in 0..4 {
        let run_id = start_run(&ctl, &format!("read only overlap {round}"));
        let task = add_task_in(&ctl, &run_id, &format!("read {round}"), "once", true);
        let (one, other) = overlap_start(
            &running.socket,
            &["task", "start", &task, "--provider", "claude"],
            &["task", "start", &task, "--provider", "claude"],
        );
        assert_one_stored(&ctl, &run_id, &task, &one, &other);
    }
    notes.line("same_task_read_only_one_attempt");

    for round in 0..4 {
        let run_id = start_run(&ctl, &format!("branch overlap {round}"));
        let task = add_task_in(&ctl, &run_id, &format!("write {round}"), "once", false);
        let left = format!("forge/left-{round}");
        let right = format!("forge/right-{round}");
        let (one, other) = overlap_start(
            &running.socket,
            &[
                "task",
                "start",
                &task,
                "--provider",
                "claude",
                "--branch",
                &left,
            ],
            &[
                "task",
                "start",
                &task,
                "--provider",
                "claude",
                "--branch",
                &right,
            ],
        );
        assert_one_stored(&ctl, &run_id, &task, &one, &other);
        let winner = if one.code == 0 { &one } else { &other };
        let branch = field(&ok(winner), "branch");
        let loser = if branch == left { &right } else { &left };
        let listed = worktree_list(&repo);
        assert!(
            listed.contains(&branch),
            "winning branch missing from worktree list: {listed}"
        );
        assert!(
            !listed.contains(loser),
            "refused start left worktree {loser}: {listed}"
        );
        let view = ok(&ctl.run(&["run", "show", &run_id], &[]));
        let stored = view["attempts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|attempt| attempt["task_id"] == task)
            .unwrap();
        assert_eq!(stored["branch"], json!(branch), "{view}");
    }
    notes.line("same_task_distinct_branches_one_worktree");
}

fn overlap_start(socket: &Path, left: &[&str], right: &[&str]) -> (Ran, Ran) {
    let socket_left = socket.to_path_buf();
    let left: Vec<String> = left.iter().map(|arg| (*arg).to_owned()).collect();
    let started = std::thread::spawn(move || {
        let args: Vec<&str> = left.iter().map(String::as_str).collect();
        Ctl::new(&socket_left).run(&args, &[])
    });
    let other = Ctl::new(socket).run(right, &[]);
    (started.join().expect("start thread"), other)
}

fn assert_one_stored(ctl: &Ctl, run_id: &str, task: &str, one: &Ran, other: &Ran) {
    let codes = [one.code, other.code];
    assert!(
        codes.contains(&0) && codes.contains(&5),
        "same-task overlap did not keep a single start: codes {codes:?}\n{} / {}\n{} / {}",
        one.stdout,
        other.stdout,
        one.stderr,
        other.stderr
    );
    let view = ok(&ctl.run(&["run", "show", run_id], &[]));
    let attempts: Vec<&Value> = view["attempts"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|attempt| attempt["task_id"] == task)
        .collect();
    assert_eq!(attempts.len(), 1, "stored more than one attempt: {view}");
    assert_eq!(attempts[0]["n"], json!(1), "{view}");
}

fn worktree_list(repo: &Path) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["worktree", "list", "--porcelain"])
        .output()
        .expect("git worktree list");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn scenario_configured_stall_is_attention() {
    let mut notes = Notes::scenario();
    notes.line("== stall ==");
    let harness = Harness::new();
    let repo = init_repo(harness.tmp.path());
    let running = harness.boot_with(|cfg| {
        cfg.orchestration.stall_after_secs = 1;
    });
    let client = connect(&running.socket);
    add_project(&client, &repo);
    let ctl = Ctl::new(&running.socket);
    let _run = start_run(&ctl, "ship the stall");
    let task = add_task(&ctl, "idle worker", "idle", None, true);
    let started = start_task(&ctl, &task);
    await_ready(&ctl, &harness, &started);
    hook_state(&ctl, &field(&started, "session_id"), "idle");
    let view = poll("stalled attention", || {
        let view = show(&ctl);
        view["attention"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["kind"] == "stalled" && item["task_id"] == task)
            .then_some(view)
    });
    notes.line(format!("configured_stall_attention {}", view["attention"]));
}

#[test]
fn scenario_providers_hand_work_to_each_other() {
    let mut notes = Notes::file("providers.log");
    let harness = Harness::new();
    let repo = init_repo(harness.tmp.path());
    let running = harness.boot();
    let client = connect(&running.socket);
    add_project(&client, &repo);
    let ctl = Ctl::new(&running.socket);

    let providers = poll("five providers detected", || {
        let providers = ok(&ctl.run(&["providers"], &[]));
        let ready = ["claude", "codex", "opencode", "cursor", "grok"]
            .iter()
            .all(|id| {
                providers.as_array().unwrap().iter().any(|row| {
                    row["descriptor"]["id"] == *id
                        && row["detection"]["status"].get("Installed").is_some()
                })
            });
        ready.then_some(providers)
    });
    let versions: Vec<String> = providers
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|row| {
            let id = row["descriptor"]["id"].as_str()?;
            let version = row["detection"]["status"]["Installed"]["version"]
                .as_str()
                .unwrap_or("");
            Some(format!("{id}={version}"))
        })
        .collect();
    notes.line(format!("providers installed={}", versions.join(",")));

    let started = ok(&ctl.run(
        &[
            "run",
            "start",
            "--controller",
            "agent:claude",
            "--objective",
            "CROSS_PROVIDER_OBJECTIVE",
        ],
        &[],
    ));
    let run_id = field(&started, "run_id");
    let controller = field(&started, "controller_session_id");
    let controller_capture = capture_of(&harness, &format!("session-{controller}"));
    assert!(
        controller_capture.contains("PROVIDER=claude\n"),
        "controller did not launch claude: {controller_capture}"
    );
    assert!(
        controller_capture.contains("BIN=claude\n"),
        "{controller_capture}"
    );
    assert!(
        controller_capture.contains(&format!("RUN={run_id}\n")),
        "controller missing its run: {controller_capture}"
    );
    assert!(
        controller_capture.contains("TASK=\n") && controller_capture.contains("ATTEMPT=\n"),
        "controller was given a worker identity: {controller_capture}"
    );
    assert!(
        controller_capture.contains("JSON=1\n"),
        "agent was not forced onto JSON: {controller_capture}"
    );
    assert!(
        controller_capture.contains("You are the controller"),
        "controller prompt was not the launch argv: {controller_capture}"
    );
    assert!(
        !transcript(&ctl, &controller).contains("CROSS_PROVIDER_OBJECTIVE"),
        "controller prompt was pasted into the terminal"
    );
    notes.line(format!(
        "controller provider=claude session={controller} prompt=argv json=1"
    ));

    let task = add_task_in(&ctl, &run_id, "codex writes", "PROMPT_TOKEN_codex", false);
    let codex = ok(&ctl.run(
        &["task", "start", &task, "--provider", "codex"],
        &[("FORGE_SESSION_ID", controller.as_str())],
    ));
    let codex_attempt = field(&codex, "attempt_id");
    let codex_session = field(&codex, "session_id");
    let codex_capture = await_ready(&ctl, &harness, &codex);
    assert!(
        codex_capture.contains("PROVIDER=codex\n"),
        "{codex_capture}"
    );
    assert!(codex_capture.contains("BIN=codex\n"), "{codex_capture}");
    assert!(
        codex_capture.contains("PROMPT_TOKEN_codex"),
        "codex launch argv dropped the spec: {codex_capture}"
    );
    assert!(
        !codex_capture.contains("--prompt"),
        "codex was given OpenCode's prompt flag: {codex_capture}"
    );
    assert!(
        codex_capture.contains(&format!("TASK={task}\n")),
        "{codex_capture}"
    );
    assert!(
        codex_capture.contains(&format!("ATTEMPT={codex_attempt}\n")),
        "{codex_capture}"
    );
    assert!(
        !transcript(&ctl, &codex_session).contains("PROMPT_TOKEN_codex"),
        "worker prompt was pasted into the terminal"
    );
    let integration = show(&ctl)["run"]["integration_workspace_id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_ne!(
        field(&codex, "workspace_id"),
        integration,
        "write attempt shared the integration checkout"
    );
    notes.line(format!(
        "worker provider=codex session={codex_session} attempt={codex_attempt} prompt=positional not-pasted worktree=own"
    ));

    let mut asking = Command::new(env!("CARGO_BIN_EXE_forgectl"))
        .args([
            "--json",
            "--socket",
            running.socket.to_str().unwrap(),
            "--timeout",
            "20s",
            "ask",
            "--wait",
            "--timeout",
            "12s",
            "CODEX_QUESTION",
        ])
        .env("FORGE_SOCKET", &running.socket)
        .env("FORGE_SESSION_ID", &codex_session)
        .env("FORGE_RUN_ID", &run_id)
        .env("FORGE_TASK_ID", &task)
        .env("FORGE_ATTEMPT_ID", &codex_attempt)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("codex ask");
    poll("codex question", || {
        let view = show(&ctl);
        view["attention"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["kind"] == "question" && item["text"] == "CODEX_QUESTION")
            .then_some(())
    });
    ok(&ctl.run(
        &[
            "send",
            "--to",
            &format!("session:{codex_session}"),
            "--kind",
            "answer",
            "CLAUDE_ANSWER",
        ],
        &[("FORGE_SESSION_ID", controller.as_str())],
    ));
    let finished = poll("codex ask returned", || asking.try_wait().ok().flatten());
    assert!(finished.success(), "ask --wait exit {finished:?}");
    let mut stdout = String::new();
    asking
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut stdout)
        .unwrap();
    assert!(stdout.contains("CLAUDE_ANSWER"), "{stdout}");
    notes.line("codex asked; claude controller answered");

    report_done(&ctl, &codex, &run_id, "REPORT_FROM_CODEX");
    let reviewed = ok(&ctl.run(&["task", "show", &task], &[]));
    assert_eq!(reviewed["status"], "review");
    assert_eq!(reviewed["attempt"]["provider_id"], "codex");
    assert_eq!(reviewed["report"]["summary"], "REPORT_FROM_CODEX");
    notes.line("codex report settled the task status=review");

    let retried = ok(&ctl.run(
        &[
            "task",
            "reject",
            &task,
            "--feedback",
            "FEEDBACK_FOR_GROK",
            "--retry",
            "--provider",
            "grok",
        ],
        &[("FORGE_SESSION_ID", controller.as_str())],
    ));
    let grok_attempt = field(&retried, "attempt_id");
    let grok_capture = await_ready(&ctl, &harness, &retried);
    assert!(grok_capture.contains("PROVIDER=grok\n"), "{grok_capture}");
    assert!(grok_capture.contains("BIN=grok\n"), "{grok_capture}");
    assert!(
        grok_capture.contains("FEEDBACK_FOR_GROK"),
        "grok launch dropped the claude feedback: {grok_capture}"
    );
    assert!(
        grok_capture.contains("REPORT_FROM_CODEX"),
        "grok launch dropped the codex report: {grok_capture}"
    );
    assert_ne!(
        field(&retried, "workspace_id"),
        field(&codex, "workspace_id")
    );
    notes.line(format!(
        "retry provider=grok attempt={grok_attempt} prompt includes codex report and claude feedback"
    ));

    let reviewer = add_task_in(&ctl, &run_id, "opencode reads", "OPENCODE_TOKEN", true);
    let opencode = ok(&ctl.run(
        &["task", "start", &reviewer, "--provider", "opencode"],
        &[("FORGE_SESSION_ID", controller.as_str())],
    ));
    let opencode_capture = await_ready(&ctl, &harness, &opencode);
    assert!(
        opencode_capture.contains("PROVIDER=opencode\n"),
        "{opencode_capture}"
    );
    assert!(
        opencode_capture.contains("--prompt") && opencode_capture.contains("OPENCODE_TOKEN"),
        "opencode did not take the prompt as --prompt: {opencode_capture}"
    );
    assert!(
        opencode_capture.contains("--agent") && opencode_capture.contains("plan"),
        "read-only opencode was not put in its plan agent: {opencode_capture}"
    );
    assert_eq!(field(&opencode, "workspace_id"), integration);
    notes.line("reviewer provider=opencode args=--agent plan --prompt placement=integration");

    let cursor_task = add_task_in(&ctl, &run_id, "cursor reads", "CURSOR_TOKEN", true);
    let cursor = ok(&ctl.run(
        &["task", "start", &cursor_task, "--provider", "cursor"],
        &[("FORGE_SESSION_ID", controller.as_str())],
    ));
    let cursor_capture = await_ready(&ctl, &harness, &cursor);
    assert!(
        cursor_capture.contains("PROVIDER=cursor\n"),
        "{cursor_capture}"
    );
    assert!(cursor_capture.contains("BIN=agent\n"), "{cursor_capture}");
    assert!(
        cursor_capture.contains("--mode") && cursor_capture.contains("ask"),
        "read-only cursor was not put in ask mode: {cursor_capture}"
    );
    assert!(cursor_capture.contains("CURSOR_TOKEN"), "{cursor_capture}");
    assert_eq!(field(&cursor, "workspace_id"), integration);
    notes.line("reviewer provider=cursor bin=agent args=--mode ask placement=integration");
}

fn json_shape(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for (key, child) in map {
                out.insert(key.clone(), json_shape(child));
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(items.iter().map(json_shape).collect()),
        Value::String(_) => json!("string"),
        Value::Number(_) => json!("number"),
        Value::Bool(_) => json!("bool"),
        Value::Null => Value::Null,
    }
}
