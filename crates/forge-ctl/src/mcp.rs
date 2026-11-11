use std::io::{self, BufRead, Write};
use std::sync::{mpsc, Arc, Condvar, Mutex};

use super::*;

const MAX_MCP_LINE: usize = 65_536;

const INSTRUCTIONS: &str = "Forge daemon tools. Identity is this session's own: a worker reports, asks and \
reads its inbox for its task; a controller runs the task loop (task_add, task_start, run_wait, act). \
Results are {ok, result} or {ok: false, error: {code, message, details, next}}; next is CLI argv for a human. \
Pass the same request_id when retrying a ledger mutation that timed out, so it is not applied twice. \
Read guide (topic worker or controller) for the full contract.";

/// A call that carries a progress token hears from the server at this cadence,
/// so a client that resets its timeout on progress does not cut a 600 s wait.
const PROGRESS_EVERY: Duration = Duration::from_secs(10);

const MAX_REQUEST_ID: u64 = 128;

/// Who the caller is, from the identity the daemon injected. It narrows the
/// advertised catalog only: the daemon enforces the rails, so a call is never
/// refused for the role.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Role {
    Session,
    Controller,
    Worker,
}

impl Role {
    pub(crate) fn from_env() -> Self {
        if env_attempt().is_some() {
            Self::Worker
        } else if env_run().is_some() {
            Self::Controller
        } else {
            Self::Session
        }
    }
}

const EVERY_ROLE: &[Role] = &[Role::Session, Role::Controller, Role::Worker];
const NOT_A_WORKER: &[Role] = &[Role::Session, Role::Controller];

/// Bounds concurrent tool calls; each holds its own daemon connection, so a
/// long `wait_session` no longer stalls a `list_sessions` behind it.
const MAX_IN_FLIGHT: usize = 8;

type Output = Arc<Mutex<dyn Write + Send>>;

struct Slots {
    used: Mutex<usize>,
    freed: Condvar,
}

struct Slot(Arc<Slots>);

impl Slots {
    /// Blocks until a slot is free, calling `waiting` every `every` meanwhile
    /// so a queued call can report progress before it runs.
    fn acquire(self: &Arc<Self>, every: Duration, mut waiting: impl FnMut()) -> Slot {
        let lock = || {
            self.used
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
        };
        let mut used = lock();
        while *used >= MAX_IN_FLIGHT {
            let (guard, wait) = self
                .freed
                .wait_timeout(used, every)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            used = guard;
            if wait.timed_out() && *used >= MAX_IN_FLIGHT {
                drop(used);
                waiting();
                used = lock();
            }
        }
        *used += 1;
        Slot(Arc::clone(self))
    }
}

/// Released on drop, so a panicking tool call cannot leak its slot.
impl Drop for Slot {
    fn drop(&mut self) {
        let mut used = self
            .0
            .used
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *used = used.saturating_sub(1);
        self.0.freed.notify_one();
    }
}

fn write(output: &Output, response: &Value) -> Result<(), u8> {
    let mut writer = output
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    serde_json::to_writer(&mut *writer, response).map_err(|_| 3)?;
    writer
        .write_all(b"\n")
        .and_then(|_| writer.flush())
        .map_err(|_| 3)
}

fn progress(token: &Value, elapsed: Duration) -> Value {
    let secs = elapsed.as_secs();
    json!({ "jsonrpc": "2.0", "method": "notifications/progress", "params": {
        "progressToken": token, "progress": secs, "message": format!("still running after {secs} s"),
    } })
}

/// Runs `call` while reporting progress to `token`; the wait blocks on the
/// call's completion, so a short call sends nothing.
fn with_progress<R: Send>(
    output: &Output,
    token: &Value,
    started: Instant,
    every: Duration,
    call: impl FnOnce() -> R + Send,
) -> R {
    std::thread::scope(|scope| {
        let (done, finished) = mpsc::channel::<()>();
        let worker = scope.spawn(move || {
            let result = call();
            let _ = done.send(());
            result
        });
        while let Err(mpsc::RecvTimeoutError::Timeout) = finished.recv_timeout(every) {
            let _ = write(output, &progress(token, started.elapsed()));
        }
        match worker.join() {
            Ok(result) => result,
            Err(panic) => std::panic::resume_unwind(panic),
        }
    })
}

pub(crate) fn serve(app: &App) -> Result<(), u8> {
    let role = Role::from_env();
    let stdin = io::stdin();
    let mut reader = stdin.lock();
    let output: Output = Arc::new(Mutex::new(io::stdout()));
    let slots = Arc::new(Slots {
        used: Mutex::new(0),
        freed: Condvar::new(),
    });
    let mut workers: Vec<std::thread::JoinHandle<()>> = Vec::new();
    let result = loop {
        let line = match read_line(&mut reader) {
            Ok(Some(line)) => line,
            Ok(None) => break Ok(()),
            Err(error) => {
                eprintln!("MCP input: {error}");
                break Err(3);
            }
        };
        let request = match serde_json::from_slice::<Value>(&line) {
            Ok(request) => request,
            Err(_) => {
                write(
                    &output,
                    &error(
                        Value::Null,
                        -32700,
                        "invalid JSON or request exceeds 64 KiB",
                    ),
                )?;
                continue;
            }
        };
        if request.get("method").and_then(Value::as_str) == Some("tools/call") {
            let started = Instant::now();
            let token = request
                .pointer("/params/_meta/progressToken")
                .filter(|token| token.is_string() || token.is_i64())
                .cloned();
            let slot = slots.acquire(PROGRESS_EVERY, || {
                if let Some(token) = &token {
                    let _ = write(&output, &progress(token, started.elapsed()));
                }
            });
            let app = app.clone();
            let output = Arc::clone(&output);
            workers.retain(|worker| !worker.is_finished());
            workers.push(std::thread::spawn(move || {
                let _slot = slot;
                let response = match token {
                    Some(token) => {
                        with_progress(&output, &token, started, PROGRESS_EVERY, move || {
                            handle(&app, role, request)
                        })
                    }
                    None => handle(&app, role, request),
                };
                if let Some(response) = response {
                    let _ = write(&output, &response);
                }
            }));
        } else if let Some(response) = handle(app, role, request) {
            write(&output, &response)?;
        }
    };
    for worker in workers {
        let _ = worker.join();
    }
    result
}

fn read_line(reader: &mut impl BufRead) -> io::Result<Option<Vec<u8>>> {
    let mut line = Vec::new();
    let mut oversized = false;
    loop {
        let buf = reader.fill_buf()?;
        if buf.is_empty() {
            return Ok((!line.is_empty() || oversized).then_some(line));
        }
        let n = buf
            .iter()
            .position(|b| *b == b'\n')
            .map_or(buf.len(), |i| i + 1);
        let ended = buf[n - 1] == b'\n';
        if !oversized && line.len().saturating_add(n) <= MAX_MCP_LINE {
            line.extend_from_slice(&buf[..n]);
        } else {
            oversized = true;
            line.clear();
        }
        reader.consume(n);
        if ended {
            return Ok(Some(line));
        }
    }
}

fn error(id: Value, code: i32, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn handle(app: &App, role: Role, request: Value) -> Option<Value> {
    if !request.is_object()
        || request.get("jsonrpc").and_then(Value::as_str) != Some("2.0")
        || !request.get("method").is_some_and(Value::is_string)
    {
        return Some(error(
            request.get("id").cloned().unwrap_or(Value::Null),
            -32600,
            "expected a JSON-RPC 2.0 request",
        ));
    }
    let id = request.get("id")?.clone();
    if !matches!(id, Value::Null | Value::String(_) | Value::Number(_)) {
        return Some(error(Value::Null, -32600, "invalid request id"));
    }
    let result = match request.get("method").and_then(Value::as_str) {
        Some("initialize") => json!({
            "protocolVersion": request.pointer("/params/protocolVersion")
                .and_then(Value::as_str)
                .filter(|v| matches!(*v, "2024-11-05" | "2025-03-26" | "2025-06-18"))
                .unwrap_or("2025-06-18"),
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "forgectl", "version": env!("CARGO_PKG_VERSION") },
            "instructions": INSTRUCTIONS,
        }),
        Some("ping") => json!({}),
        Some("tools/list") => json!({ "tools": tools(role) }),
        Some("tools/call") => {
            let Some(name) = request.pointer("/params/name").and_then(Value::as_str) else {
                return Some(error(id, -32602, "tool name required"));
            };
            let args = request
                .pointer("/params/arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            let argv = match tool_argv(name, &args) {
                Ok(argv) => argv,
                Err(message) => return Some(error(id, -32602, &message)),
            };
            let cli = match Cli::try_parse_from(argv) {
                Ok(cli) => cli,
                Err(error_message) => return Some(error(id, -32602, &error_message.to_string())),
            };
            let tool_app = App {
                json: true,
                request_id: cli.request_id,
                capture: Some(std::cell::RefCell::new(Value::Null)),
                ..app.clone()
            };
            let outcome = tool_app.execute(cli.command);
            let value = tool_app
                .capture
                .as_ref()
                .map(|capture| capture.borrow().clone())
                .unwrap_or(Value::Null);
            json!({ "content": [{ "type": "text", "text": value.to_string() }], "structuredContent": value, "isError": outcome.is_err() })
        }
        _ => return Some(error(id, -32601, "method not found")),
    };
    Some(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
}

struct Tool {
    name: &'static str,
    description: &'static str,
    argv: &'static [&'static str],
    properties: Value,
    required: &'static [&'static str],
    /// Passed after `--`, in this order, so a value can never become a flag.
    positional: &'static [&'static str],
    /// Enum-valued properties whose value is itself the flag (`outcome: done` is `--done`).
    flags: &'static [&'static str],
    roles: &'static [Role],
}

impl Tool {
    fn new(
        name: &'static str,
        description: &'static str,
        argv: &'static [&'static str],
        properties: Value,
    ) -> Self {
        Self {
            name,
            description,
            argv,
            properties,
            required: &[],
            positional: &[],
            flags: &[],
            roles: EVERY_ROLE,
        }
    }

    fn roles(mut self, roles: &'static [Role]) -> Self {
        self.roles = roles;
        self
    }

    /// A ledger mutation: the daemon stores its result under `(request_id, caller)`
    /// and answers a retry with it. Session dispatch has no receipt, so it never
    /// offers the key.
    fn idempotent(mut self) -> Self {
        if let Some(properties) = self.properties.as_object_mut() {
            properties.insert(
                "request_id".into(),
                json!({ "type": "string", "minLength": 1, "maxLength": MAX_REQUEST_ID,
                    "description": "Idempotency key: reuse it when retrying this call after a timeout." }),
            );
        }
        self
    }

    fn required(mut self, required: &'static [&'static str]) -> Self {
        self.required = required;
        self
    }

    fn positional(mut self, positional: &'static [&'static str]) -> Self {
        self.positional = positional;
        self
    }

    fn flags(mut self, flags: &'static [&'static str]) -> Self {
        self.flags = flags;
        self
    }

    fn schema(&self) -> Value {
        json!({ "name": self.name, "description": self.description, "inputSchema": {
            "type": "object", "properties": self.properties, "required": self.required,
            "additionalProperties": false,
        } })
    }
}

const MAX_LIST_ITEMS: u64 = 64;

fn catalog() -> Vec<Tool> {
    let string = json!({ "type": "string" });
    let boolean = json!({ "type": "boolean" });
    let strings =
        json!({ "type": "array", "items": { "type": "string" }, "maxItems": MAX_LIST_ITEMS });
    let version = json!({ "type": "integer", "minimum": 0 });
    let wait_secs = json!({ "type": "integer", "minimum": 0, "maximum": 600 });
    let role = json!({ "type": "string", "enum": ["generic", "orchestrator", "planner", "researcher", "executor", "reviewer", "tester"] });
    let session = json!({ "session": string });
    let task = json!({ "task": string });
    let placement = |extra: Value| {
        let mut properties = json!({
            "project": string, "workspace": string, "worktree": string, "base": string,
            "name": string, "role": role, "parent": string,
        });
        if let (Some(properties), Value::Object(extra)) = (properties.as_object_mut(), extra) {
            properties.extend(extra);
        }
        properties
    };
    vec![
        // Places and providers.
        Tool::new("list_projects", "List projects by name, root path and id.",
            &["project", "list"], json!({})),
        Tool::new("list_providers", "List installed agent providers and what each supports.",
            &["providers"], json!({})),
        Tool::new("list_workspaces", "List checkouts (main and worktrees), optionally of one project.",
            &["workspace", "list"], json!({ "project": string })),
        Tool::new("list_actions", "List a project's approved actions.",
            &["action", "list"], json!({ "project": string })).required(&["project"]),
        Tool::new("run_action", "Run an approved project action as a task session.",
            &["action", "run"], json!({ "project": string, "action": string, "workspace": string }))
            .required(&["project", "action"]).positional(&["action"]),
        Tool::new("status", "Protocol version, the caller's identity (session, run, task, attempt) and the orchestration limits.",
            &["status"], json!({})),
        Tool::new("guide", "The full contract for a role or topic, as text.",
            &["guide"],
            json!({ "topic": { "type": "string", "enum": ["controller", "worker", "messages", "board", "integration", "errors", "sessions", "spec"] } }))
            .positional(&["topic"]),
        // Sessions.
        Tool::new("list_sessions", "List daemon-owned sessions with project, title, provider, state and activity.",
            &["session", "list"],
            json!({ "project": string, "kind": { "type": "string", "enum": ["agent", "shell", "editor", "task"] }, "active": boolean })),
        Tool::new("show_session", "Show one session as the daemon stores it.",
            &["session", "show"], session.clone()).required(&["session"]).positional(&["session"]),
        Tool::new("read_session", "Read a bounded terminal transcript, newest lines last.",
            &["session", "read"],
            json!({ "session": string,
                "max_bytes": { "type": "integer", "minimum": 1, "maximum": 1_048_576 },
                "lines": { "type": "integer", "minimum": 1, "maximum": 100_000 } }))
            .required(&["session"]).positional(&["session"]),
        Tool::new("session_changes", "Files a session changed since its base commit.",
            &["session", "changes"], session.clone()).required(&["session"]).positional(&["session"]),
        Tool::new("spawn_session", "Launch an agent. Called from a Forge session, project, checkout, provider and profile default to the caller's. context is appended to the prompt as a handover; together at most 64 KiB. A parent must be in the same project; otherwise the agent is an independent root.",
            &["session", "spawn"],
            placement(json!({ "provider": string, "profile": string, "prompt": string,
                "model": string, "effort": string, "read_only": boolean, "resume": string, "context": string }))),
        Tool::new("spawn_shell", "Open an interactive shell. Called from a Forge session, project and checkout default to the caller's.",
            &["session", "shell"], placement(json!({}))),
        Tool::new("exec_shell", "Type one command line into a shell and press Enter. Refused unless the shell is at its prompt; delivery is not proof it succeeded: read the transcript.",
            &["session", "exec"], json!({ "session": string, "text": string }))
            .required(&["session", "text"]).positional(&["session"]),
        Tool::new("send_prompt", "Paste up to 8 KiB into an agent. Default requires hook-confirmed idle; queue has one slot. Now never submits while non-idle.",
            &["session", "send"],
            json!({ "session": string, "text": string, "no_submit": boolean, "queue": boolean, "now": boolean }))
            .required(&["session", "text"]).positional(&["session"]),
        Tool::new("wait_session", "Wait by events until the first of a comma-separated list holds: idle, waiting, working (agents only) or exit (any session). Wait for idle,waiting,exit so a question does not look like a hang. Idle is not evidence of completed work.",
            &["session", "wait"],
            json!({ "session": string,
                "for": { "type": "string", "pattern": "^(idle|waiting|working|exit)(,(idle|waiting|working|exit))*$" },
                "timeout": wait_secs }))
            .required(&["session", "for"]).positional(&["session"]),
        Tool::new("rename_session", "Set a non-empty human title. Titles need not be unique.",
            &["session", "rename"], json!({ "session": string, "title": string }))
            .required(&["session", "title"]).positional(&["session", "title"]),
        Tool::new("signal_session", "Deliver a signal to a session's process group, or with foreground to the job in front, as a typed Ctrl-C would.",
            &["session", "signal"],
            json!({ "session": string, "signal": { "type": "string", "enum": ["int", "term", "hup", "kill"] }, "foreground": boolean }))
            .required(&["session", "signal"]).positional(&["session"]),
        Tool::new("kill_session", "Kill a session's process group. The row stays until closed.",
            &["session", "kill"], session.clone()).required(&["session"]).positional(&["session"]),
        Tool::new("restart_session", "Relaunch an ended session; an agent's initial prompt is not re-sent.",
            &["session", "restart"], session.clone()).required(&["session"]).positional(&["session"]),
        Tool::new("close_session", "Remove an ended session from the tree.",
            &["session", "close"], session).required(&["session"]).positional(&["session"]),
        Tool::new("run_spec", "Run a spec with two agents: launch a planner (Claude at high effort) that reads the spec, writes a plan, launches the coder, waits, reviews and sends its review. spec is a Claude artifact link, a Markdown file path or the full spec text. preset claude codes with Claude at medium effort; mix codes with Cursor, or with coder (for example opencode).",
            &["spec", "run"], spec_properties(true)).required(&["spec"]).positional(&["spec"]).roles(NOT_A_WORKER),
        Tool::new("spec_brief", "Return the planner brief for a spec, for the calling session to act as the planner itself instead of launching one.",
            &["spec", "brief"], spec_properties(false)).required(&["spec"]).positional(&["spec"]).roles(NOT_A_WORKER),
        // A worker's side of a run. Identity is the session's FORGE_* environment.
        Tool::new("context", "This task's current context: run objective, brief, spec, acceptance, accepted dependencies, board keys and the unread message count.",
            &["context"], json!({})).roles(&[Role::Worker]),
        Tool::new("report", "Settle this attempt, exactly once, then stop and wait. summary says what changed, how it was verified and what is left. result_file is an optional path in this checkout.",
            &["report"],
            json!({ "outcome": { "type": "string", "enum": ["done", "failed", "blocked"] },
                "summary": string, "verification": string, "result_file": string }))
            .required(&["outcome", "summary"]).flags(&["outcome"]).roles(&[Role::Worker]).idempotent(),
        Tool::new("report_show", "Show a task's report, its latest attempt unless attempt is given.",
            &["report", "show"], json!({ "task": string, "attempt": { "type": "integer", "minimum": 1 } }))
            .required(&["task"]).positional(&["task"]),
        Tool::new("ask", "Ask the controller a question on scope, ownership or architecture. With wait, block until the answer (timeout in seconds).",
            &["ask"], json!({ "text": string, "wait": boolean, "timeout": wait_secs }))
            .required(&["text"]).positional(&["text"]).roles(&[Role::Worker]).idempotent(),
        Tool::new("inbox", "Read this session's stored messages. With wait, block until one arrives. ack takes message ids to mark read.",
            &["inbox"],
            json!({ "wait": boolean, "timeout": wait_secs, "ack": strings, "ack_all": boolean, "run": string })),
        Tool::new("send_message", "Store a message for controller, task:<id>, session:<id> or run:<id>. It is never typed into a terminal.",
            &["send"], json!({ "to": string, "kind": string, "text": string }))
            .required(&["to", "text"]).positional(&["text"]).idempotent(),
        Tool::new("state_list", "List the run's board entries, optionally under a key prefix.",
            &["state", "list"], json!({ "prefix": string })),
        Tool::new("state_get", "Read one board key with its version.",
            &["state", "get"], json!({ "key": string })).required(&["key"]).positional(&["key"]),
        Tool::new("state_set", "Write a board key. value is JSON text. if_version is the version you read; 0 creates the key. A mismatch is a conflict carrying the current version.",
            &["state", "set"], json!({ "key": string, "value": string, "if_version": version }))
            .required(&["key", "value", "if_version"]).positional(&["key", "value"]).idempotent(),
        Tool::new("state_delete", "Delete a board key at the version you read.",
            &["state", "del"], json!({ "key": string, "if_version": version }))
            .required(&["key", "if_version"]).positional(&["key"]).idempotent(),
        Tool::new("state_watch", "Block until a board key changes past from_version, or the timeout in seconds.",
            &["state", "watch"], json!({ "key": string, "from_version": version, "timeout": wait_secs }))
            .required(&["key"]).positional(&["key"]),
        Tool::new("pr_draft", "Hand this session's pull-request text to Forge Node, which shows it for editing, commits, pushes and opens the pull request. branch is used only when the checkout is on its default branch; commit is the commit subject and commit_body its body; body is the pull request in Markdown.",
            &["pr", "draft"],
            json!({ "title": string, "branch": string, "commit": string, "commit_body": string, "body": string }))
            .required(&["title", "branch", "commit", "body"]).roles(&[Role::Session]),
        // A controller's side of a run.
        Tool::new("run_start", "Start a run. controller is agent:<provider>[:profile], self or none; integration is new (default), current or a workspace id.",
            &["run", "start"],
            json!({ "objective": string, "controller": string, "integration": string, "base": string, "project": string }))
            .required(&["objective"]).roles(NOT_A_WORKER).idempotent(),
        Tool::new("run_list", "List runs, optionally of one project or only active ones.",
            &["run", "list"], json!({ "project": string, "active": boolean })).roles(NOT_A_WORKER),
        Tool::new("run_show", "Show a run with its tasks and attempts; defaults to the caller's run.",
            &["run", "show"], json!({ "run": string })).positional(&["run"]).roles(NOT_A_WORKER),
        Tool::new("run_brief", "Replace the run brief with set, at the brief version you read.",
            &["run", "brief"], json!({ "run": string, "set": string, "if_version": version }))
            .required(&["set", "if_version"]).positional(&["run"]).roles(NOT_A_WORKER).idempotent(),
        Tool::new("run_wait", "Block until the first of a comma-separated list holds: attention, reported, question or settled, optionally for some tasks. Never poll with sleep.",
            &["run", "wait"],
            json!({ "run": string, "for": { "type": "string", "pattern": "^[a-z]+(,[a-z]+)*$" }, "task": strings, "timeout": wait_secs }))
            .required(&["for"]).positional(&["run"]).roles(NOT_A_WORKER),
        Tool::new("run_close", "Close a run. With pr, open one pull request from the integration workspace (title, body, draft, base); cancel stops live attempts; cleanup removes task worktrees.",
            &["run", "close"],
            json!({ "run": string, "pr": boolean, "title": string, "body": string, "draft": boolean,
                "base": string, "cancel": boolean, "cleanup": boolean }))
            .positional(&["run"]).roles(NOT_A_WORKER).idempotent(),
        Tool::new("run_resume", "Start a new controller for an interrupted run from its ledger. Workers are not revived.",
            &["run", "resume"], json!({ "run": string, "provider": string, "profile": string }))
            .required(&["run", "provider"]).positional(&["run"]).roles(NOT_A_WORKER).idempotent(),
        Tool::new("task_add", "Add a task to the run. after names tasks whose output this one needs.",
            &["task", "add"],
            json!({ "title": string, "spec": string, "acceptance": string, "after": strings,
                "read_only": boolean, "allow_subruns": boolean, "run": string }))
            .required(&["title"]).roles(NOT_A_WORKER).idempotent(),
        Tool::new("task_list", "List the run's tasks, optionally only ready ones.",
            &["task", "list"], json!({ "run": string, "ready": boolean })).roles(NOT_A_WORKER),
        Tool::new("task_show", "Show one task.", &["task", "show"], task.clone())
            .required(&["task"]).positional(&["task"]).roles(NOT_A_WORKER),
        Tool::new("task_start", "Start an attempt of a ready task with a provider. placement is worktree (default for write), integration, same or a workspace id.",
            &["task", "start"],
            json!({ "task": string, "provider": string, "profile": string, "placement": string, "branch": string }))
            .required(&["task", "provider"]).positional(&["task"]).roles(NOT_A_WORKER).idempotent(),
        Tool::new("task_review", "The evidence for a decision: the latest report and the attempt's diff stat, or the patch.",
            &["task", "review"], json!({ "task": string, "patch": boolean }))
            .required(&["task"]).positional(&["task"]).roles(NOT_A_WORKER),
        Tool::new("task_accept", "Accept a reported task.", &["task", "accept"], json!({ "task": string, "note": string }))
            .required(&["task"]).positional(&["task"]).roles(NOT_A_WORKER).idempotent(),
        Tool::new("task_reject", "Reject a reported task with feedback; retry starts a new attempt, optionally with another provider.",
            &["task", "reject"],
            json!({ "task": string, "feedback": string, "retry": boolean, "provider": string, "profile": string }))
            .required(&["task", "feedback"]).positional(&["task"]).roles(NOT_A_WORKER).idempotent(),
        Tool::new("task_cancel", "Cancel a task; kill also ends its live session.",
            &["task", "cancel"], json!({ "task": string, "kill": boolean }))
            .required(&["task"]).positional(&["task"]).roles(NOT_A_WORKER).idempotent(),
        Tool::new("task_integrate", "Merge an accepted task into the integration worktree (merge or squash), or continue or abort a conflicted integration.",
            &["task", "integrate"],
            json!({ "task": string, "how": { "type": "string", "enum": ["merge", "squash", "continue", "abort"] } }))
            .required(&["task", "how"]).positional(&["task"]).flags(&["how"]).roles(NOT_A_WORKER).idempotent(),
        Tool::new("task_cleanup", "Remove a settled task's worktree unless keep_worktree.",
            &["task", "cleanup"], json!({ "task": string, "keep_worktree": boolean }))
            .required(&["task"]).positional(&["task"]).roles(NOT_A_WORKER).idempotent(),
        // Forge Review.
        Tool::new("review_start", "Start a structured, read-only review of a workspace's changes against its merge base. With wait, block until it ends and return its findings.",
            &["review", "start"],
            json!({ "workspace": string, "base": string, "tier": { "type": "string", "enum": ["base", "plus", "apex"] },
                "provider": string, "preset": string, "instructions": string, "model": string,
                "effort": string, "wait": boolean })).roles(NOT_A_WORKER),
        Tool::new("review_show", "A review run and its findings.", &["review", "show"], json!({ "review": string }))
            .required(&["review"]).positional(&["review"]).roles(NOT_A_WORKER),
        Tool::new("review_status", "A workspace's review runs, newest first.",
            &["review", "status"], json!({ "workspace": string })).roles(NOT_A_WORKER),
        Tool::new("review_cancel", "Stop a queued or running review.", &["review", "cancel"], json!({ "review": string }))
            .required(&["review"]).positional(&["review"]).roles(NOT_A_WORKER),
    ]
}

fn spec_properties(launch: bool) -> Value {
    let string = json!({ "type": "string" });
    let mut properties = json!({
        "spec": string, "preset": { "type": "string", "enum": ["claude", "mix"] },
        "coder": string, "coder_effort": string, "coder_model": string,
        "worktree": string, "base": string, "name": string,
        "rounds": { "type": "integer", "minimum": 1, "maximum": 10 },
    });
    if launch {
        if let Some(properties) = properties.as_object_mut() {
            for key in [
                "project",
                "workspace",
                "planner",
                "planner_effort",
                "planner_model",
            ] {
                properties.insert(key.into(), string.clone());
            }
        }
    }
    properties
}

fn tools(role: Role) -> Vec<Value> {
    catalog()
        .iter()
        .filter(|tool| tool.roles.contains(&role))
        .map(Tool::schema)
        .collect()
}

fn valid_scalar(schema: &Value, value: &Value) -> bool {
    let within = |v: u64| {
        schema["minimum"].as_u64().is_none_or(|min| v >= min)
            && schema["maximum"].as_u64().is_none_or(|max| v <= max)
    };
    let typed = match schema["type"].as_str() {
        Some("string") => value.as_str().is_some_and(|s| {
            let chars = s.chars().count() as u64;
            schema["minLength"].as_u64().is_none_or(|min| chars >= min)
                && schema["maxLength"].as_u64().is_none_or(|max| chars <= max)
        }),
        Some("boolean") => value.is_boolean(),
        Some("integer") => value.as_u64().is_some_and(within),
        _ => false,
    };
    typed
        && schema
            .get("enum")
            .and_then(Value::as_array)
            .is_none_or(|allowed| allowed.contains(value))
}

fn tool_argv(name: &str, args: &Value) -> Result<Vec<String>, String> {
    let tool = catalog()
        .into_iter()
        .find(|t| t.name == name)
        .ok_or_else(|| "unknown tool".to_owned())?;
    let args = args
        .as_object()
        .ok_or_else(|| "arguments must be an object".to_owned())?;
    let properties = tool
        .properties
        .as_object()
        .ok_or_else(|| "invalid schema".to_owned())?;
    for required in tool.required {
        if !args.contains_key(*required) {
            return Err(format!("missing {required}"));
        }
    }
    for (key, value) in args {
        let schema = properties
            .get(key)
            .ok_or_else(|| format!("unknown argument {key}"))?;
        let valid = if schema["type"] == "array" {
            value.as_array().is_some_and(|items| {
                items.len() as u64 <= MAX_LIST_ITEMS
                    && items
                        .iter()
                        .all(|item| valid_scalar(&schema["items"], item))
            })
        } else {
            valid_scalar(schema, value)
        };
        if !valid {
            return Err(format!("invalid {key}"));
        }
    }
    let mut argv: Vec<String> = std::iter::once("forgectl")
        .chain(tool.argv.iter().copied())
        .map(str::to_owned)
        .collect();
    let flag_value = |key: &str, value: &Value| -> Result<Option<String>, String> {
        let flag = key.replace('_', "-");
        Ok(match value {
            Value::Bool(true) => Some(format!("--{flag}")),
            Value::Bool(false) => None,
            Value::String(s) => Some(format!("--{flag}={s}")),
            Value::Number(n) => Some(format!("--{flag}={n}")),
            _ => return Err("invalid argument".into()),
        })
    };
    for (key, value) in args {
        if tool.positional.contains(&key.as_str()) {
            continue;
        }
        if tool.flags.contains(&key.as_str()) {
            // The schema's enum already admitted this value, so it cannot name another flag.
            let flag = value.as_str().ok_or_else(|| format!("invalid {key}"))?;
            argv.push(format!("--{flag}"));
            continue;
        }
        if let Value::Array(items) = value {
            for item in items {
                argv.extend(flag_value(key, item)?);
            }
            continue;
        }
        argv.extend(flag_value(key, value)?);
    }
    // `-` would make the command read stdin, which is this server's request stream.
    if tool
        .positional
        .iter()
        .any(|key| args.get(*key).and_then(Value::as_str) == Some("-"))
    {
        return Err("stdin (-) is not available over MCP".into());
    }
    if !tool.positional.is_empty() {
        argv.push("--".into());
        for key in tool.positional {
            if let Some(value) = args.get(*key).and_then(Value::as_str) {
                argv.push(value.into());
            }
        }
    }
    Ok(argv)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_lines_resynchronize_after_oversize_input() {
        let body = format!("{}\n{{}}\n", "x".repeat(MAX_MCP_LINE + 1));
        let mut reader = io::Cursor::new(body);
        assert!(read_line(&mut reader).unwrap().unwrap().is_empty());
        assert_eq!(read_line(&mut reader).unwrap().unwrap(), b"{}\n");
        assert!(read_line(&mut reader).unwrap().is_none());
    }

    #[test]
    fn tool_arguments_cannot_inject_cli_flags_or_unknown_fields() {
        let argv = tool_argv(
            "rename_session",
            &json!({ "session": "s", "title": "--kill" }),
        )
        .unwrap();
        assert!(
            matches!(Cli::try_parse_from(argv).unwrap().command, Command::Session(SessionCmd::Rename { title, .. }) if title == "--kill")
        );
        assert!(tool_argv(
            "send_prompt",
            &json!({ "session": "s", "text": "hello", "raw_keys": true })
        )
        .is_err());
        assert!(tool_argv(
            "wait_session",
            &json!({ "session": "s", "for": "idle", "timeout": 601 })
        )
        .is_err());
        assert!(tool_argv("read_session", &json!({ "session": "s", "lines": 0 })).is_err());
        assert!(tool_argv("spec_brief", &json!({ "spec": "-" })).is_err());
    }

    #[test]
    fn every_tool_parses_as_a_cli_command() {
        let samples = [
            ("list_projects", json!({})),
            ("list_providers", json!({})),
            ("list_workspaces", json!({ "project": "api" })),
            ("list_actions", json!({ "project": "api" })),
            ("run_action", json!({ "project": "api", "action": "--dev" })),
            ("status", json!({})),
            ("guide", json!({ "topic": "worker" })),
            (
                "list_sessions",
                json!({ "project": "api", "kind": "shell", "active": true }),
            ),
            ("show_session", json!({ "session": "s" })),
            (
                "read_session",
                json!({ "session": "s", "max_bytes": 4096, "lines": 40 }),
            ),
            ("session_changes", json!({ "session": "s" })),
            (
                "spawn_session",
                json!({ "project": "api", "provider": "claude", "worktree": "w", "base": "main", "role": "tester", "parent": "self", "resume": "abc", "context": "--we agreed" }),
            ),
            ("spawn_shell", json!({ "name": "build" })),
            ("exec_shell", json!({ "session": "s", "text": "-rf" })),
            (
                "send_prompt",
                json!({ "session": "s", "text": "hi", "queue": true }),
            ),
            (
                "wait_session",
                json!({ "session": "s", "for": "idle,waiting,exit", "timeout": 5 }),
            ),
            ("rename_session", json!({ "session": "s", "title": "t" })),
            (
                "signal_session",
                json!({ "session": "s", "signal": "int", "foreground": true }),
            ),
            ("kill_session", json!({ "session": "s" })),
            ("restart_session", json!({ "session": "s" })),
            ("close_session", json!({ "session": "s" })),
            (
                "run_spec",
                json!({ "spec": "https://claude.ai/public/artifacts/x", "preset": "mix", "coder": "opencode", "rounds": 2, "project": "api", "planner_effort": "high" }),
            ),
            (
                "spec_brief",
                json!({ "spec": "--docs/spec.md", "worktree": "spec/x", "base": "main" }),
            ),
            ("context", json!({})),
            (
                "report",
                json!({ "outcome": "done", "summary": "--failed", "verification": "cargo test", "result_file": "out.md" }),
            ),
            ("report_show", json!({ "task": "t", "attempt": 2 })),
            (
                "ask",
                json!({ "text": "--which?", "wait": true, "timeout": 30 }),
            ),
            (
                "inbox",
                json!({ "wait": true, "timeout": 5, "ack": ["a", "b"], "run": "r" }),
            ),
            ("inbox", json!({ "ack_all": true })),
            (
                "send_message",
                json!({ "to": "controller", "kind": "message", "text": "--done" }),
            ),
            ("state_list", json!({ "prefix": "claim/" })),
            ("state_get", json!({ "key": "decision/db" })),
            (
                "state_set",
                json!({ "key": "decision/db", "value": "{\"use\":\"sqlite\"}", "if_version": 0 }),
            ),
            (
                "state_delete",
                json!({ "key": "decision/db", "if_version": 3 }),
            ),
            (
                "state_watch",
                json!({ "key": "decision/db", "from_version": 3, "timeout": 10 }),
            ),
            (
                "pr_draft",
                json!({ "title": "Fix it", "branch": "fix/it", "commit": "Fix it", "commit_body": "Why.", "body": "--body" }),
            ),
            (
                "run_start",
                json!({ "objective": "ship", "controller": "self", "integration": "current", "base": "main", "project": "api" }),
            ),
            ("run_list", json!({ "project": "api", "active": true })),
            ("run_show", json!({})),
            ("run_show", json!({ "run": "r" })),
            (
                "run_brief",
                json!({ "run": "r", "set": "--brief", "if_version": 1 }),
            ),
            (
                "run_wait",
                json!({ "for": "attention,reported,question", "task": ["t1", "t2"], "timeout": 60 }),
            ),
            (
                "run_close",
                json!({ "pr": true, "title": "Run", "body": "--x", "draft": true, "base": "main", "cleanup": true }),
            ),
            (
                "run_resume",
                json!({ "run": "r", "provider": "claude", "profile": "p" }),
            ),
            (
                "task_add",
                json!({ "title": "t", "spec": "s", "acceptance": "a", "after": ["x"], "read_only": true, "allow_subruns": true, "run": "r" }),
            ),
            ("task_list", json!({ "run": "r", "ready": true })),
            ("task_show", json!({ "task": "t" })),
            (
                "task_start",
                json!({ "task": "t", "provider": "claude", "placement": "worktree", "branch": "b" }),
            ),
            ("task_review", json!({ "task": "t", "patch": true })),
            ("task_accept", json!({ "task": "t", "note": "ok" })),
            (
                "task_reject",
                json!({ "task": "t", "feedback": "--no", "retry": true, "provider": "opencode" }),
            ),
            ("task_cancel", json!({ "task": "t", "kill": true })),
            ("task_integrate", json!({ "task": "t", "how": "squash" })),
            (
                "task_cleanup",
                json!({ "task": "t", "keep_worktree": true }),
            ),
            (
                "review_start",
                json!({ "workspace": "w", "base": "main", "tier": "plus", "provider": "claude", "preset": "p", "instructions": "--x", "model": "m", "effort": "high", "wait": true }),
            ),
            ("review_show", json!({ "review": "r" })),
            ("review_status", json!({ "workspace": "w" })),
            ("review_cancel", json!({ "review": "r" })),
        ];
        let covered: std::collections::BTreeSet<&str> =
            samples.iter().map(|(name, _)| *name).collect();
        assert_eq!(covered.len(), catalog().len());
        for (name, args) in samples {
            let argv = tool_argv(name, &args).unwrap();
            assert!(Cli::try_parse_from(&argv).is_ok(), "{name}: {argv:?}");
        }
    }

    #[test]
    fn values_reach_the_cli_as_values() {
        let parse = |name: &str, args: Value| {
            Cli::try_parse_from(tool_argv(name, &args).unwrap())
                .unwrap()
                .command
        };
        assert!(matches!(
            parse("report", json!({ "outcome": "blocked", "summary": "--done" })),
            Command::Report { done: false, blocked: true, summary: Some(summary), .. } if summary == "--done"
        ));
        assert!(matches!(
            parse("pr_draft", json!({ "title": "t", "branch": "b", "commit": "c", "body": "--title=x" })),
            Command::Pr(PrCmd::Draft { body: Some(body), title, .. }) if body == "--title=x" && title == "t"
        ));
        assert!(matches!(
            parse("inbox", json!({ "ack": ["--ack-all", "m2"] })),
            Command::Inbox { ack, ack_all: false, .. } if ack == ["--ack-all", "m2"]
        ));
        assert!(matches!(
            parse("task_integrate", json!({ "task": "t", "how": "abort" })),
            Command::Task(TaskCmd::Integrate {
                abort: true,
                squash: false,
                ..
            })
        ));
    }

    #[test]
    fn enum_flags_and_lists_are_bounded_by_their_schema() {
        assert!(tool_argv("report", &json!({ "outcome": "kill", "summary": "s" })).is_err());
        assert!(tool_argv("report", &json!({ "outcome": true, "summary": "s" })).is_err());
        assert!(tool_argv(
            "task_integrate",
            &json!({ "task": "t", "how": "merge --x" })
        )
        .is_err());
        assert!(tool_argv("inbox", &json!({ "ack": "m1" })).is_err());
        assert!(tool_argv("inbox", &json!({ "ack": [1] })).is_err());
        let many: Vec<String> = (0..=MAX_LIST_ITEMS).map(|n| n.to_string()).collect();
        assert!(tool_argv("inbox", &json!({ "ack": many })).is_err());
    }

    #[test]
    fn mcp_handshake_catalog_and_notifications_do_not_need_a_daemon() {
        let app = App {
            json: true,
            socket: None,
            timeout: Duration::from_secs(1),
            wait_timeout: Duration::from_secs(1),
            review_wait: Duration::from_secs(1),
            request_id: None,
            capture: None,
        };
        let initialize = handle(&app, Role::Session, json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05"}})).unwrap();
        assert_eq!(initialize["result"]["protocolVersion"], "2024-11-05");
        let catalog = handle(
            &app,
            Role::Session,
            json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
        )
        .unwrap();
        assert_eq!(
            catalog["result"]["tools"].as_array().unwrap().len(),
            tools(Role::Session).len()
        );
        assert!(handle(
            &app,
            Role::Session,
            json!({"jsonrpc":"2.0","method":"notifications/initialized"})
        )
        .is_none());
        assert_eq!(
            handle(&app, Role::Session, json!({})).unwrap()["error"]["code"],
            -32600
        );
        assert_eq!(
            handle(
                &app,
                Role::Session,
                json!({"jsonrpc":"2.0","id":3,"method":"no-such-method"})
            )
            .unwrap()["error"]["code"],
            -32601
        );
    }

    #[test]
    fn ledger_mutations_carry_an_idempotency_key_and_dispatch_does_not() {
        let cli = Cli::try_parse_from(
            tool_argv(
                "state_set",
                &json!({ "key": "k", "value": "1", "if_version": 0, "request_id": "retry-1" }),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(cli.request_id.as_deref(), Some("retry-1"));
        let long = "x".repeat(MAX_REQUEST_ID as usize + 1);
        assert!(tool_argv(
            "report",
            &json!({ "outcome": "done", "summary": "s", "request_id": long })
        )
        .is_err());
        assert!(tool_argv(
            "report",
            &json!({ "outcome": "done", "summary": "s", "request_id": "" })
        )
        .is_err());
        // The daemon keeps no receipt for session dispatch, so a key there would promise a dedup that never happens.
        assert!(tool_argv("spawn_session", &json!({ "request_id": "r" })).is_err());
        assert!(tool_argv(
            "send_prompt",
            &json!({ "session": "s", "text": "t", "request_id": "r" })
        )
        .is_err());
    }

    #[test]
    fn the_catalog_follows_the_callers_role() {
        let names = |role| -> Vec<String> {
            tools(role)
                .iter()
                .map(|tool| tool["name"].as_str().unwrap().to_owned())
                .collect()
        };
        let worker = names(Role::Worker);
        let controller = names(Role::Controller);
        let session = names(Role::Session);
        for name in [
            "context",
            "report",
            "ask",
            "state_set",
            "inbox",
            "read_session",
        ] {
            assert!(worker.iter().any(|n| n == name), "worker lacks {name}");
        }
        for name in [
            "task_add",
            "run_wait",
            "review_start",
            "pr_draft",
            "run_spec",
        ] {
            assert!(!worker.iter().any(|n| n == name), "worker sees {name}");
        }
        for name in [
            "task_add",
            "task_start",
            "run_wait",
            "state_set",
            "report_show",
        ] {
            assert!(
                controller.iter().any(|n| n == name),
                "controller lacks {name}"
            );
        }
        for name in ["report", "ask", "context", "pr_draft"] {
            assert!(
                !controller.iter().any(|n| n == name),
                "controller sees {name}"
            );
        }
        for name in [
            "pr_draft",
            "run_start",
            "spawn_session",
            "state_set",
            "report_show",
        ] {
            assert!(session.iter().any(|n| n == name), "session lacks {name}");
        }
        for name in ["report", "ask", "context"] {
            assert!(!session.iter().any(|n| n == name), "session sees {name}");
        }
        let every: std::collections::BTreeSet<String> = [worker, controller, session]
            .into_iter()
            .flatten()
            .collect();
        assert_eq!(every.len(), catalog().len(), "a tool no role can see");
    }

    #[test]
    fn a_hidden_tool_is_still_callable() {
        assert!(tool_argv("run_start", &json!({ "objective": "nested" })).is_ok());
    }

    struct Signalling {
        bytes: Vec<u8>,
        flushed: mpsc::Sender<()>,
    }

    impl Write for Signalling {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.bytes.extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            let _ = self.flushed.send(());
            Ok(())
        }
    }

    #[test]
    fn a_call_queued_behind_full_slots_reports_while_it_waits() {
        let slots = Arc::new(Slots {
            used: Mutex::new(0),
            freed: Condvar::new(),
        });
        let mut held: Vec<Slot> = (0..MAX_IN_FLIGHT)
            .map(|_| slots.acquire(Duration::from_secs(60), || unreachable!()))
            .collect();
        let (reported, first_report) = mpsc::channel();
        let queued = {
            let slots = Arc::clone(&slots);
            std::thread::spawn(move || {
                let mut reports = 0;
                let _slot = slots.acquire(Duration::from_millis(1), || {
                    reports += 1;
                    let _ = reported.send(());
                });
                reports
            })
        };
        // A slot frees only after the queued call has reported, so the report cannot be skipped by a fast release.
        first_report.recv().unwrap();
        held.pop();
        assert!(queued.join().unwrap() >= 1);
    }

    #[test]
    fn a_long_call_reports_progress_and_still_returns_its_result() {
        let (flushed, first_progress) = mpsc::channel();
        let sink = Arc::new(Mutex::new(Signalling {
            bytes: Vec::new(),
            flushed,
        }));
        let output: Output = sink.clone();
        let token = json!("tok-7");
        // The call cannot finish until a progress line has been written, so the test cannot pass by racing it.
        let result = with_progress(
            &output,
            &token,
            Instant::now(),
            Duration::from_millis(1),
            move || {
                first_progress.recv().unwrap();
                42
            },
        );
        assert_eq!(result, 42);
        let bytes = sink.lock().unwrap().bytes.clone();
        let first: Value =
            serde_json::from_slice(bytes.split(|b| *b == b'\n').next().unwrap()).unwrap();
        assert_eq!(first["method"], "notifications/progress");
        assert_eq!(first["params"]["progressToken"], "tok-7");
        assert!(first.get("id").is_none());
    }
}
