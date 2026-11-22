//! `forgectl review` against a real daemon, for the commands that need no
//! reviewer: the codebase graph, the knowledge base, the sandbox and stats.

#![allow(
    clippy::unwrap_used,
    reason = "a panicking helper is the test's assertion"
)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use client::Client;
use daemon::core::Daemon;
use domain::WorkspaceId;
use protocol::{Request, Response};
use serde_json::{json, Value};

struct Harness {
    tmp: tempfile::TempDir,
    socket: PathBuf,
    shell: PathBuf,
}

struct Running {
    daemon: Arc<Daemon>,
    _thread: std::thread::JoinHandle<()>,
}

impl Drop for Running {
    fn drop(&mut self) {
        self.daemon.request_shutdown();
    }
}

fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn write_executable(dir: &Path, name: &str, script: &str) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, script).unwrap();
    let mut perms = fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&path, perms).unwrap();
    path
}

impl Harness {
    fn new() -> Self {
        let tmp = tempfile::tempdir_in("/tmp").expect("tempdir");
        let bin = tmp.path().join("bin");
        fs::create_dir_all(&bin).unwrap();
        std::os::unix::fs::symlink("/usr/bin/env", bin.join("env")).unwrap();
        fs::create_dir_all(tmp.path().join("home")).unwrap();
        let shell = write_executable(
            tmp.path(),
            "forge-test-shell",
            &format!(
                "#!/bin/sh\nPATH={path}\nexport PATH\nHOME={home}\nexport HOME\nCLAUDE_CONFIG_DIR={claude}\nexport CLAUDE_CONFIG_DIR\nunset ENV\nunset BASH_ENV\nwhile [ \"$1\" = -l ] || [ \"$1\" = -i ]; do shift; done\nexec /bin/sh \"$@\"\n",
                path = sh_quote(&bin.to_string_lossy()),
                home = sh_quote(&tmp.path().join("home").to_string_lossy()),
                claude = sh_quote(&tmp.path().join("claude").to_string_lossy()),
            ),
        );
        Self {
            socket: tmp.path().join("d.sock"),
            shell,
            tmp,
        }
    }

    fn boot(&self) -> Running {
        self.boot_with(|_| {})
    }

    fn boot_with(&self, configure: impl FnOnce(&mut daemon::config::Config)) -> Running {
        let db = persistence::Db::open(&self.tmp.path().join("forge.db")).expect("open db");
        let mut cfg = daemon::config::Config::default();
        cfg.sessions.shell = self.shell.to_string_lossy().into_owned();
        cfg.sessions.idle_warn_after_secs = 0;
        cfg.sessions.long_running_warn_after_secs = 0;
        cfg.github.refresh_secs = 0;
        configure(&mut cfg);
        let forge = client::paths::resolve(&client::paths::PathInputs {
            home: Some(self.tmp.path().to_path_buf()),
            forge_home: Some(self.tmp.path().join("forge-home").into_os_string()),
            ..client::paths::PathInputs::default()
        })
        .expect("forge paths");
        let daemon = Daemon::start(
            db,
            cfg,
            daemon::core::DaemonPaths {
                forge,
                worktrees_root: self.tmp.path().join("worktrees"),
            },
            "test-instance".into(),
            "0.0.0-test".into(),
        )
        .expect("start daemon");
        let d = daemon.clone();
        let sock = self.socket.clone();
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
        while !self.socket.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(self.socket.exists(), "daemon socket was not bound");
        Running {
            daemon,
            _thread: thread,
        }
    }

    fn ctl(&self, args: &[&str], env: &[(&str, &str)]) -> Ran {
        ctl(&self.socket, args, env, None)
    }
}

impl Harness {
    /// A fake `claude` that writes `answer` where the real one writes its
    /// transcript, then exits.
    fn install_reviewer(&self, answer: &str) {
        let record = json!({
            "type": "assistant",
            "message": {"role": "assistant", "content": [{"type": "text", "text": answer}]},
        })
        .to_string();
        let answer_file = self.tmp.path().join("answer.jsonl");
        fs::write(&answer_file, format!("{record}\n")).unwrap();
        let script = format!(
            "#!/bin/sh\n\
             if [ \"$1\" = --version ]; then echo '2.1.0 (Claude Code)'; exit 0; fi\n\
             id=''\nprev=''\n\
             for a in \"$@\"; do [ \"$prev\" = --session-id ] && id=$a; prev=$a; done\n\
             slug=$(printf '%s' \"$FORGE_WORKSPACE\" | /usr/bin/tr -c 'A-Za-z0-9' '-')\n\
             dir=\"$CLAUDE_CONFIG_DIR/projects/$slug\"\n\
             /bin/mkdir -p \"$dir\"\n\
             /bin/cat {answer} > \"$dir/$id.jsonl\"\n",
            answer = sh_quote(&answer_file.to_string_lossy()),
        );
        write_executable(&self.tmp.path().join("bin"), "claude", &script);
    }
}

fn wait_for_claude(client: &Client) {
    let deadline = Instant::now() + Duration::from_secs(45);
    loop {
        if let Ok(Response::Providers(providers)) = client.request(Request::ListAgentProviders) {
            if providers
                .iter()
                .any(|p| p.descriptor.id.as_str() == "claude" && p.detection.status.is_installed())
            {
                return;
            }
        }
        assert!(Instant::now() < deadline, "claude was never detected");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn v1_answer(findings: &[Value]) -> String {
    json!({ "schema": "forge.review/v1", "summary": "Changes `total`.", "findings": findings })
        .to_string()
}

struct Ran {
    code: i32,
    stdout: String,
    stderr: String,
}

fn ctl(socket: &Path, args: &[&str], env: &[(&str, &str)], cwd: Option<&Path>) -> Ran {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_forgectl"));
    cmd.arg("--json")
        .arg("--socket")
        .arg(socket)
        .arg("--timeout")
        .arg("60s")
        .args(args)
        .env("FORGE_SOCKET", socket)
        .env_remove("FORGE_SESSION_ID")
        .env_remove("FORGE_WORKSPACE")
        .stdin(Stdio::null());
    if let Some(cwd) = cwd {
        cmd.current_dir(cwd);
    }
    for (key, value) in env {
        cmd.env(key, value);
    }
    let output = cmd.output().expect("spawn forgectl");
    Ran {
        code: output.status.code().unwrap_or(1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

fn parsed(ran: &Ran) -> Value {
    serde_json::from_str(ran.stdout.trim()).unwrap_or_else(|error| {
        panic!(
            "stdout is not json ({error}): {}\nstderr: {}",
            ran.stdout, ran.stderr
        )
    })
}

fn ok(ran: &Ran) -> Value {
    assert_eq!(ran.code, 0, "stdout {}\nstderr {}", ran.stdout, ran.stderr);
    let value = parsed(ran);
    assert_eq!(value["ok"], json!(true), "{}", ran.stdout);
    value["result"].clone()
}

fn err(ran: &Ran, code: i32) -> Value {
    assert_eq!(
        ran.code, code,
        "stdout {}\nstderr {}",
        ran.stdout, ran.stderr
    );
    let value = parsed(ran);
    assert_eq!(value["ok"], json!(false), "{}", ran.stdout);
    value["error"].clone()
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
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// `lib.rs` defines `total`; `main.rs` calls it.
fn graph_repo(root: &Path) -> PathBuf {
    let repo = root.join("repo");
    fs::create_dir_all(&repo).unwrap();
    git(&repo, &["-c", "init.defaultBranch=main", "init", "-q"]);
    git(&repo, &["config", "user.email", "t@example.com"]);
    git(&repo, &["config", "user.name", "Test"]);
    git(&repo, &["config", "commit.gpgsign", "false"]);
    fs::write(
        repo.join("lib.rs"),
        "pub fn total(a: u32, b: u32) -> u32 {\n    a + b\n}\n",
    )
    .unwrap();
    fs::write(
        repo.join("main.rs"),
        "mod lib;\n\nfn main() {\n    let sum = lib::total(2, 3);\n    report(sum);\n}\n\nfn report(n: u32) {\n    println(n);\n}\n",
    )
    .unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-qm", "init"]);
    repo
}

fn connect(socket: &Path) -> Client {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(client) = Client::connect(socket, "review-cli-test") {
            return client;
        }
        assert!(Instant::now() < deadline, "could not connect");
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

fn names(nodes: &Value) -> Vec<String> {
    nodes
        .as_array()
        .expect("nodes")
        .iter()
        .map(|node| {
            format!(
                "{}:{}",
                node["path"].as_str().unwrap(),
                node["name"].as_str().unwrap()
            )
        })
        .collect()
}

#[test]
fn graph_rebuild_query_and_show_answer_from_the_daemon() {
    let harness = Harness::new();
    let repo = graph_repo(harness.tmp.path());
    let _running = harness.boot();
    let client = connect(&harness.socket);
    let workspace = add_project(&client, &repo).to_string();
    let ws = ["--workspace", workspace.as_str()];

    let rebuilt = ok(&harness.ctl(
        &[&["review", "graph", "rebuild"][..], &ws, &["--full"]].concat(),
        &[],
    ));
    assert!(rebuilt["parsed"].as_u64().unwrap() >= 2, "{rebuilt}");
    assert!(rebuilt["edges"].as_u64().unwrap() > 0, "{rebuilt}");

    let callers = ok(&harness.ctl(
        &[
            &["review", "graph", "query"][..],
            &ws,
            &["--callers", "total"],
        ]
        .concat(),
        &[],
    ));
    assert_eq!(callers["source"], "daemon");
    assert_eq!(names(&callers["nodes"]), ["main.rs:main"]);
    let callees = ok(&harness.ctl(
        &[
            &["review", "graph", "query"][..],
            &ws,
            &["--callees", "main"],
        ]
        .concat(),
        &[],
    ));
    assert_eq!(names(&callees["nodes"]), ["lib.rs:total", "main.rs:report"]);
    let path = ok(&harness.ctl(
        &[
            &["review", "graph", "query"][..],
            &ws,
            &["--path", "main", "report"],
        ]
        .concat(),
        &[],
    ));
    assert_eq!(names(&path["nodes"]), ["main.rs:main", "main.rs:report"]);
    let external = ok(&harness.ctl(
        &[
            &["review", "graph", "query"][..],
            &ws,
            &["--external-calls", "main.rs"],
        ]
        .concat(),
        &[],
    ));
    assert_eq!(names(&external["nodes"]), [":println"]);
    let imports = ok(&harness.ctl(
        &[
            &["review", "graph", "query"][..],
            &ws,
            &["--imports", "main.rs"],
        ]
        .concat(),
        &[],
    ));
    assert_eq!(names(&imports["nodes"]), ["lib.rs:lib.rs"]);
    let similar = ok(&harness.ctl(
        &[
            &["review", "graph", "query"][..],
            &ws,
            &["--similar", "main"],
        ]
        .concat(),
        &[],
    ));
    assert!(similar["nodes"].as_array().unwrap().is_empty());

    let dot = ok(&harness.ctl(&[&["review", "graph", "show"][..], &ws].concat(), &[]));
    assert!(dot["text"]
        .as_str()
        .unwrap()
        .starts_with("digraph codebase {"));
    let mermaid = ok(&harness.ctl(
        &[
            &["review", "graph", "show"][..],
            &ws,
            &["--format", "mermaid"],
        ]
        .concat(),
        &[],
    ));
    assert!(mermaid["text"]
        .as_str()
        .unwrap()
        .starts_with("flowchart LR"));

    let usage = err(
        &harness.ctl(
            &[&["review", "graph", "query"][..], &ws, &["total"]].concat(),
            &[],
        ),
        2,
    );
    assert!(
        usage["message"]
            .as_str()
            .unwrap()
            .contains("which question"),
        "{usage}"
    );
}

#[test]
fn graph_query_reads_the_cache_without_a_daemon_and_refuses_a_stale_one() {
    let harness = Harness::new();
    let repo = graph_repo(harness.tmp.path());
    {
        let _running = harness.boot();
        let client = connect(&harness.socket);
        let workspace = add_project(&client, &repo).to_string();
        ok(&harness.ctl(
            &["review", "graph", "rebuild", "--workspace", &workspace],
            &[],
        ));
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    while Client::connect(&harness.socket, "probe").is_ok() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    let gone = harness.tmp.path().join("gone.sock");
    let offline = ok(&ctl(
        &gone,
        &["review", "graph", "query", "--callers", "total"],
        &[],
        Some(&repo),
    ));
    assert_eq!(offline["source"], "cache");
    assert_eq!(names(&offline["nodes"]), ["main.rs:main"]);

    fs::write(repo.join("other.rs"), "fn other() {}\n").unwrap();
    git(&repo, &["add", "other.rs"]);
    git(&repo, &["commit", "-qm", "move head"]);
    let stale = err(
        &ctl(
            &gone,
            &["review", "graph", "query", "--callers", "total"],
            &[("FORGE_WORKSPACE", repo.to_str().unwrap())],
            None,
        ),
        1,
    );
    assert_eq!(stale["code"], "stale_graph", "{stale}");
}

#[test]
fn review_show_draws_a_diagram_of_what_the_run_reviewed() {
    let harness = Harness::new();
    harness.install_reviewer(&v1_answer(&[]));
    let repo = graph_repo(harness.tmp.path());
    let branchy = "pub fn total(a: u32, b: u32) -> u32 {\n    if a == 0 {\n        return b;\n    } else if b == 0 {\n        return a;\n    }\n    if a > b { a } else if b > a { b } else { 0 }\n}\n";
    fs::write(repo.join("lib.rs"), branchy).unwrap();
    let _running = harness.boot();
    let client = connect(&harness.socket);
    wait_for_claude(&client);
    let workspace = add_project(&client, &repo).to_string();

    let done = ok(&harness.ctl(
        &["review", "start", "--workspace", &workspace, "--wait"],
        &[],
    ));
    assert_eq!(done["run"]["status"], "Completed", "{done}");
    let kinds: Vec<&str> = done["run"]["diagrams"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["sequence", "flow"], "{done}");
    let review = done["run"]["id"].as_str().unwrap().to_owned();

    let mermaid = ok(&harness.ctl(
        &[
            "review",
            "show",
            &review,
            "--diagram",
            "flow",
            "--format",
            "mermaid",
        ],
        &[],
    ));
    assert!(
        mermaid["mermaid"]
            .as_str()
            .unwrap()
            .starts_with("flowchart TD\n"),
        "{mermaid}"
    );
    let forge = ok(&harness.ctl(&["review", "show", &review, "--diagram", "sequence"], &[]));
    assert_eq!(forge["kind"], "sequence");
    assert_eq!(forge["edges"][0]["label"], "total");
    let none = err(
        &harness.ctl(&["review", "show", &review, "--diagram", "er"], &[]),
        4,
    );
    assert_eq!(none["code"], "not_found");
}

#[test]
fn kb_show_edit_and_export_go_through_the_daemon() {
    let harness = Harness::new();
    let repo = graph_repo(harness.tmp.path());
    let _running = harness.boot();
    let client = connect(&harness.socket);
    let workspace = add_project(&client, &repo).to_string();
    ok(&harness.ctl(
        &["review", "graph", "rebuild", "--workspace", &workspace],
        &[],
    ));

    let shown = ok(&harness.ctl(&["review", "kb", "show", "--workspace", &workspace], &[]));
    let names: Vec<&str> = shown["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["file"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "architecture.md",
            "patterns.json",
            "learned-rules.md",
            "false-positives.json"
        ]
    );
    let architecture = ok(&harness.ctl(
        &[
            "review",
            "kb",
            "show",
            "--workspace",
            &workspace,
            "--file",
            "architecture.md",
        ],
        &[],
    ));
    assert_eq!(architecture["source"], "daemon");
    assert!(
        architecture["content"]
            .as_str()
            .unwrap()
            .contains("## (root)"),
        "{architecture}"
    );

    let editor = write_executable(
        harness.tmp.path(),
        "append-editor",
        "#!/bin/sh\nprintf 'key ghp_0123456789abcdefghijklmnopqrstuvwxyz\\n## edited by hand\\n' >> \"$1\"\n",
    );
    let editor = editor.to_str().unwrap();
    let edited = ok(&harness.ctl(
        &[
            "review",
            "kb",
            "edit",
            "--workspace",
            &workspace,
            "architecture.md",
        ],
        &[("EDITOR", editor), ("VISUAL", "")],
    ));
    assert_eq!(edited["changed"], true);
    let architecture = ok(&harness.ctl(
        &[
            "review",
            "kb",
            "show",
            "--workspace",
            &workspace,
            "--file",
            "architecture.md",
        ],
        &[],
    ));
    assert_eq!(architecture["source"], "user");
    let content = architecture["content"].as_str().unwrap();
    assert!(content.ends_with("## edited by hand\n"), "{content}");
    assert!(
        content.contains("key «redacted»") && !content.contains("ghp_"),
        "redacted on write"
    );
    let refused = err(
        &harness.ctl(
            &[
                "review",
                "kb",
                "edit",
                "--workspace",
                &workspace,
                "learned-rules.md",
            ],
            &[("EDITOR", editor), ("VISUAL", "")],
        ),
        2,
    );
    assert!(
        refused["message"].as_str().unwrap().contains("derived"),
        "{refused}"
    );

    let exported = ok(&harness.ctl(&["review", "kb", "export", "--workspace", &workspace], &[]));
    let text = exported["text"].as_str().unwrap();
    assert!(text.starts_with("# Forge Review knowledge base\n"));
    assert!(text.contains("## edited by hand") && text.contains("false-positives.json"));
    let out = harness.tmp.path().join("kb.md");
    ok(&harness.ctl(
        &[
            "review",
            "kb",
            "export",
            "--workspace",
            &workspace,
            "--output",
            out.to_str().unwrap(),
        ],
        &[],
    ));
    assert_eq!(fs::read_to_string(&out).unwrap(), text);
}

#[cfg(target_os = "macos")]
#[test]
fn sandbox_run_waits_for_its_targets_and_fails_on_a_failing_one() {
    let harness = Harness::new();
    let repo = graph_repo(harness.tmp.path());
    fs::create_dir_all(repo.join(".forge/review")).unwrap();
    fs::write(
        repo.join(".forge/review/checks.toml"),
        "[[targets]]\nname = \"pass\"\ncommand = \"echo ok\"\n[[targets]]\nname = \"fail\"\ncommand = \"echo nope >&2; exit 4\"\n",
    )
    .unwrap();
    let _running = harness.boot();
    let client = connect(&harness.socket);
    let workspace = add_project(&client, &repo).to_string();
    ok(&harness.ctl(
        &[
            "review",
            "checks",
            "approve",
            "--workspace",
            &workspace,
            "--yes",
        ],
        &[],
    ));

    let passed = ok(&harness.ctl(
        &[
            "review",
            "checks",
            "run",
            "--workspace",
            &workspace,
            "--target",
            "pass",
        ],
        &[],
    ));
    assert_eq!(passed["targets"][0]["exit_code"], 0);
    assert_eq!(passed["targets"][0]["stdout"], "ok\n");
    let id = passed["id"].as_str().unwrap().to_owned();

    let failed = err(
        &harness.ctl(&["review", "checks", "run", "--workspace", &workspace], &[]),
        1,
    );
    assert_eq!(failed["code"], "sandbox_failed");
    let targets = failed["details"]["targets"].as_array().unwrap();
    assert_eq!(targets.len(), 2);
    assert_eq!(targets[1]["exit_code"], 4);
    assert_eq!(targets[1]["stderr"], "nope\n");

    let results = ok(&harness.ctl(&["review", "checks", "results", &id], &[]));
    assert_eq!(results["id"], id.as_str());
    assert!(
        results["review_id"].is_null(),
        "a manual run belongs to no review"
    );
}

#[cfg(target_os = "macos")]
#[test]
fn only_an_approved_sandbox_file_runs_and_approving_asks_first() {
    let harness = Harness::new();
    let repo = graph_repo(harness.tmp.path());
    fs::create_dir_all(repo.join(".forge/review")).unwrap();
    let file = repo.join(".forge/review/checks.toml");
    fs::write(
        &file,
        "[[targets]]\nname = \"t\"\ncommand = \"echo ran > ran\"\n",
    )
    .unwrap();
    let _running = harness.boot();
    let client = connect(&harness.socket);
    let workspace = add_project(&client, &repo).to_string();
    let run = || harness.ctl(&["review", "checks", "run", "--workspace", &workspace], &[]);

    let refused = err(&run(), 1);
    assert!(
        refused["message"]
            .as_str()
            .unwrap()
            .contains("is not approved"),
        "{refused}"
    );
    assert!(refused["details"]["unapproved_hash"].is_string());
    assert!(!repo.join("ran").exists(), "nothing ran");

    let asked = err(
        &harness.ctl(
            &["review", "checks", "approve", "--workspace", &workspace],
            &[],
        ),
        2,
    );
    assert!(asked["message"].as_str().unwrap().contains("--yes"));

    let approved = ok(&harness.ctl(
        &[
            "review",
            "checks",
            "approve",
            "--workspace",
            &workspace,
            "--yes",
        ],
        &[],
    ));
    assert_eq!(approved["file_hash"], refused["details"]["unapproved_hash"]);
    ok(&run());
    assert!(repo.join("ran").exists());

    fs::write(
        &file,
        "[[targets]]\nname = \"t\"\ncommand = \"echo again > again\"\n",
    )
    .unwrap();
    let changed = err(&run(), 1);
    assert!(
        changed["message"]
            .as_str()
            .unwrap()
            .contains("changed since it was approved"),
        "{changed}"
    );
    assert!(!repo.join("again").exists());
}

#[test]
fn fix_loop_asks_first_and_starts_only_with_yes() {
    let harness = Harness::new();
    harness.install_reviewer(&v1_answer(&[json!({
        "path": "lib.rs", "line": 2, "severity": "P1", "category": "logic", "confidence": 0.9,
        "title": "subtracts", "body": "b", "evidence": "a - b",
    })]));
    let repo = graph_repo(harness.tmp.path());
    fs::write(
        repo.join("lib.rs"),
        "pub fn total(a: u32, b: u32) -> u32 {\n    a - b\n}\n",
    )
    .unwrap();
    let _running = harness.boot();
    let client = connect(&harness.socket);
    wait_for_claude(&client);
    let workspace = add_project(&client, &repo);
    let ws = workspace.to_string();
    let done = ok(&harness.ctl(&["review", "start", "--workspace", &ws, "--wait"], &[]));
    assert_eq!(done["findings"].as_array().unwrap().len(), 1, "{done}");

    let refused = err(
        &harness.ctl(&["review", "fix-loop", "--workspace", &ws], &[]),
        2,
    );
    assert!(
        refused["message"].as_str().unwrap().contains("--yes"),
        "{refused}"
    );
    let idle = client.request(Request::CancelFixLoop {
        workspace_id: workspace,
    });
    assert!(idle.is_err(), "nothing was started: {idle:?}");

    let started = ok(&harness.ctl(
        &[
            "review",
            "fix-loop",
            "--workspace",
            &ws,
            "--yes",
            "--max-iters",
            "1",
        ],
        &[],
    ));
    assert_eq!(started["findings"], 1);
    assert_eq!(started["max_iterations"], 1);
    assert_eq!(started["started"]["workspace_id"], ws.as_str());
}

fn seeded_run(
    workspace: WorkspaceId,
    tier: domain::Tier,
    seconds: u32,
    reviewers: u8,
) -> domain::ReviewRun {
    let at = |s: u32| {
        domain::Timestamp::parse_rfc3339(&format!("2026-01-01T00:{:02}:{:02}Z", s / 60, s % 60))
            .unwrap()
    };
    domain::ReviewRun {
        id: domain::ReviewId::new(),
        workspace_id: workspace,
        target: domain::ReviewTarget::Workspace {
            workspace_id: workspace,
            base: None,
        },
        head_oid: "h".into(),
        base_oid: None,
        tier,
        preset: "standard".into(),
        provider: "claude".into(),
        status: domain::ReviewStatus::Completed,
        stage: None,
        summary: String::new(),
        score: Some(5.0),
        verdict: Some(domain::Verdict::Ok),
        error: None,
        session_id: None,
        coverage: domain::Coverage::default(),
        created_at: at(0),
        finished_at: Some(at(seconds)),
        branch: None,
        tree_oid: None,
        diagrams: Vec::new(),
        auto_approvable: false,
        reviewers,
        schema_version: domain::SchemaVersion::V1,
        sandbox_run_id: None,
    }
}

fn seeded_finding(
    id: u32,
    hypothesis: Option<domain::Hypothesis>,
    kb_ref: Option<&str>,
) -> domain::Finding {
    domain::Finding {
        id,
        fingerprint: format!("f{id}"),
        path: "lib.rs".into(),
        line: None,
        end_line: None,
        severity: domain::Severity::P2,
        category: "logic".into(),
        rule_id: None,
        confidence: 0.9,
        title: "t".into(),
        body: "b".into(),
        evidence: "e".into(),
        suggestion: None,
        lineage: domain::Lineage::New,
        reaction: None,
        hypothesis,
        kb_ref: kb_ref.map(str::to_owned),
    }
}

#[test]
fn stats_report_latency_and_rates_over_the_stored_runs() {
    let harness = Harness::new();
    let repo = graph_repo(harness.tmp.path());
    let _running = harness.boot();
    let client = connect(&harness.socket);
    let workspace = add_project(&client, &repo);
    let store = persistence::Db::open(&harness.tmp.path().join("forge.db"))
        .unwrap()
        .review_store()
        .unwrap();
    for seconds in [10, 30, 20] {
        store
            .insert_run(&seeded_run(workspace, domain::Tier::Quick, seconds, 1))
            .unwrap();
    }
    let cited = seeded_run(workspace, domain::Tier::Quick, 20, 1);
    store.insert_run(&cited).unwrap();
    store
        .insert_findings(
            cited.id,
            &[seeded_finding(1, None, Some("architecture.md#core"))],
        )
        .unwrap();
    let parallel = seeded_run(workspace, domain::Tier::Deep, 40, 3);
    store.insert_run(&parallel).unwrap();
    store
        .insert_findings(
            parallel.id,
            &[
                seeded_finding(1, Some(domain::Hypothesis::AuthBypass), None),
                seeded_finding(2, Some(domain::Hypothesis::AuthBypass), None),
            ],
        )
        .unwrap();
    store
        .save_sandbox_run(&domain::SandboxRun {
            id: "s".into(),
            workspace_id: workspace,
            review_id: Some(parallel.id),
            targets: vec![domain::SandboxTarget {
                name: "test".into(),
                exit_code: Some(1),
                duration_ms: 1,
                timed_out: false,
                stdout: String::new(),
                stderr: String::new(),
            }],
            error: None,
            unapproved_hash: None,
            started_at: domain::Timestamp::now(),
            finished_at: None,
        })
        .unwrap();

    let stats = ok(&harness.ctl(
        &["review", "stats", "--workspace", &workspace.to_string()],
        &[],
    ));
    assert_eq!(stats["runs"], 5);
    assert_eq!(
        stats["latency"][0],
        json!({ "tier": "quick", "runs": 4, "p50_secs": 20, "p95_secs": 30 })
    );
    assert_eq!(
        stats["latency"][1],
        json!({ "tier": "deep", "runs": 1, "p50_secs": 40, "p95_secs": 40 })
    );
    assert!(
        (stats["hypothesis_coverage"].as_f64().unwrap() - 33.3).abs() < 0.01,
        "{stats}"
    );
    assert_eq!(stats["kb_hit_rate"], 20.0);
    assert_eq!(stats["sandbox_checks_failure_rate"], 100.0);
    assert!(stats["graph_hit_rate"].is_null(), "no query asked yet");
    ok(&harness.ctl(
        &[
            "review",
            "graph",
            "rebuild",
            "--workspace",
            &workspace.to_string(),
        ],
        &[],
    ));
    ok(&harness.ctl(
        &[
            "review",
            "graph",
            "query",
            "--workspace",
            &workspace.to_string(),
            "--callers",
            "total",
        ],
        &[],
    ));
    let stats = ok(&harness.ctl(&["review", "stats"], &[]));
    assert_eq!(
        stats["graph_hit_rate"], 50.0,
        "one answered query, one rebuild"
    );
    assert!(
        stats["kb_bytes"].as_u64().unwrap() > 0,
        "the rebuild wrote the architecture"
    );
}
