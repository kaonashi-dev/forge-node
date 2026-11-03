use std::io::{self, BufRead, Write};
use std::sync::{Arc, Condvar, Mutex};

use super::*;

const MAX_MCP_LINE: usize = 65_536;

/// Bounds concurrent tool calls; each holds its own daemon connection, so a
/// long `wait_session` no longer stalls a `list_sessions` behind it.
const MAX_IN_FLIGHT: usize = 8;

type Output = Arc<Mutex<io::Stdout>>;

struct Slots {
    used: Mutex<usize>,
    freed: Condvar,
}

struct Slot(Arc<Slots>);

impl Slots {
    fn acquire(self: &Arc<Self>) -> Slot {
        let mut used = self
            .used
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while *used >= MAX_IN_FLIGHT {
            used = self
                .freed
                .wait(used)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
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

pub(crate) fn serve(app: &App) -> Result<(), u8> {
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
            let slot = slots.acquire();
            let app = app.clone();
            let output = Arc::clone(&output);
            workers.retain(|worker| !worker.is_finished());
            workers.push(std::thread::spawn(move || {
                let _slot = slot;
                if let Some(response) = handle(&app, request) {
                    let _ = write(&output, &response);
                }
            }));
        } else if let Some(response) = handle(app, request) {
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

fn handle(app: &App, request: Value) -> Option<Value> {
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
        }),
        Some("ping") => json!({}),
        Some("tools/list") => json!({ "tools": tools() }),
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
                request_id: None,
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
}

impl Tool {
    fn schema(&self) -> Value {
        json!({ "name": self.name, "description": self.description, "inputSchema": {
            "type": "object", "properties": self.properties, "required": self.required,
            "additionalProperties": false,
        } })
    }
}

fn catalog() -> Vec<Tool> {
    let string = json!({ "type": "string" });
    let boolean = json!({ "type": "boolean" });
    let role = json!({ "type": "string", "enum": ["generic", "orchestrator", "planner", "researcher", "executor", "reviewer", "tester"] });
    let session = json!({ "session": string });
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
        Tool { name: "list_projects", description: "List projects by name, root path and id.",
            argv: &["project", "list"], properties: json!({}), required: &[], positional: &[] },
        Tool { name: "list_providers", description: "List installed agent providers and what each supports.",
            argv: &["providers"], properties: json!({}), required: &[], positional: &[] },
        Tool { name: "list_workspaces", description: "List checkouts (main and worktrees), optionally of one project.",
            argv: &["workspace", "list"], properties: json!({ "project": string }), required: &[], positional: &[] },
        Tool { name: "list_sessions", description: "List daemon-owned sessions with project, title, provider, state and activity.",
            argv: &["session", "list"],
            properties: json!({ "project": string, "kind": { "type": "string", "enum": ["agent", "shell", "editor", "task"] }, "active": boolean }),
            required: &[], positional: &[] },
        Tool { name: "show_session", description: "Show one session as the daemon stores it.",
            argv: &["session", "show"], properties: session.clone(), required: &["session"], positional: &["session"] },
        Tool { name: "read_session", description: "Read a bounded terminal transcript, newest lines last.",
            argv: &["session", "read"],
            properties: json!({ "session": string,
                "max_bytes": { "type": "integer", "minimum": 1, "maximum": 1_048_576 },
                "lines": { "type": "integer", "minimum": 1, "maximum": 100_000 } }),
            required: &["session"], positional: &["session"] },
        Tool { name: "session_changes", description: "Files a session changed since its base commit.",
            argv: &["session", "changes"], properties: session.clone(), required: &["session"], positional: &["session"] },
        Tool { name: "spawn_session", description: "Launch an agent. Called from a Forge session, project, checkout, provider and profile default to the caller's. context is appended to the prompt as a handover; together at most 64 KiB. A parent must be in the same project; otherwise the agent is an independent root.",
            argv: &["session", "spawn"],
            properties: placement(json!({ "provider": string, "profile": string, "prompt": string,
                "model": string, "effort": string, "read_only": boolean, "resume": string, "context": string })),
            required: &[], positional: &[] },
        Tool { name: "spawn_shell", description: "Open an interactive shell. Called from a Forge session, project and checkout default to the caller's.",
            argv: &["session", "shell"], properties: placement(json!({})), required: &[], positional: &[] },
        Tool { name: "exec_shell", description: "Type one command line into a shell and press Enter. Refused unless the shell is at its prompt; delivery is not proof it succeeded: read the transcript.",
            argv: &["session", "exec"], properties: json!({ "session": string, "text": string }),
            required: &["session", "text"], positional: &["session"] },
        Tool { name: "send_prompt", description: "Paste up to 8 KiB into an agent. Default requires hook-confirmed idle; queue has one slot. Now never submits while non-idle.",
            argv: &["session", "send"],
            properties: json!({ "session": string, "text": string, "no_submit": boolean, "queue": boolean, "now": boolean }),
            required: &["session", "text"], positional: &["session"] },
        Tool { name: "wait_session", description: "Wait by events until the first of a comma-separated list holds: idle, waiting, working (agents only) or exit (any session). Wait for idle,waiting,exit so a question does not look like a hang. Idle is not evidence of completed work.",
            argv: &["session", "wait"],
            properties: json!({ "session": string,
                "for": { "type": "string", "pattern": "^(idle|waiting|working|exit)(,(idle|waiting|working|exit))*$" },
                "timeout": { "type": "integer", "minimum": 0, "maximum": 600 } }),
            required: &["session", "for"], positional: &["session"] },
        Tool { name: "rename_session", description: "Set a non-empty human title. Titles need not be unique.",
            argv: &["session", "rename"], properties: json!({ "session": string, "title": string }),
            required: &["session", "title"], positional: &["session", "title"] },
        Tool { name: "signal_session", description: "Deliver a signal to a session's process group, or with foreground to the job in front, as a typed Ctrl-C would.",
            argv: &["session", "signal"],
            properties: json!({ "session": string, "signal": { "type": "string", "enum": ["int", "term", "hup", "kill"] }, "foreground": boolean }),
            required: &["session", "signal"], positional: &["session"] },
        Tool { name: "kill_session", description: "Kill a session's process group. The row stays until closed.",
            argv: &["session", "kill"], properties: session.clone(), required: &["session"], positional: &["session"] },
        Tool { name: "restart_session", description: "Relaunch an ended session; an agent's initial prompt is not re-sent.",
            argv: &["session", "restart"], properties: session.clone(), required: &["session"], positional: &["session"] },
        Tool { name: "close_session", description: "Remove an ended session from the tree.",
            argv: &["session", "close"], properties: session, required: &["session"], positional: &["session"] },
        Tool { name: "run_spec", description: "Run a spec with two agents: launch a planner (Claude at high effort) that reads the spec, writes a plan, launches the coder, waits, reviews and sends its review. spec is a Claude artifact link, a Markdown file path or the full spec text. preset claude codes with Claude at medium effort; mix codes with Cursor, or with coder (for example opencode).",
            argv: &["spec", "run"], properties: spec_properties(true), required: &["spec"], positional: &["spec"] },
        Tool { name: "spec_brief", description: "Return the planner brief for a spec, for the calling session to act as the planner itself instead of launching one.",
            argv: &["spec", "brief"], properties: spec_properties(false), required: &["spec"], positional: &["spec"] },
        Tool { name: "list_actions", description: "List a project's approved actions.",
            argv: &["action", "list"], properties: json!({ "project": string }), required: &["project"], positional: &[] },
        Tool { name: "run_action", description: "Run an approved project action as a task session.",
            argv: &["action", "run"], properties: json!({ "project": string, "action": string, "workspace": string }),
            required: &["project", "action"], positional: &["action"] },
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

fn tools() -> Vec<Value> {
    catalog().iter().map(Tool::schema).collect()
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
        let within = |v: u64| {
            schema["minimum"].as_u64().is_none_or(|min| v >= min)
                && schema["maximum"].as_u64().is_none_or(|max| v <= max)
        };
        let valid = match schema["type"].as_str() {
            Some("string") => value.is_string(),
            Some("boolean") => value.is_boolean(),
            Some("integer") => value.as_u64().is_some_and(within),
            _ => false,
        };
        if !valid
            || schema
                .get("enum")
                .and_then(Value::as_array)
                .is_some_and(|allowed| !allowed.contains(value))
        {
            return Err(format!("invalid {key}"));
        }
    }
    let mut argv: Vec<String> = std::iter::once("forgectl")
        .chain(tool.argv.iter().copied())
        .map(str::to_owned)
        .collect();
    for (key, value) in args {
        if tool.positional.contains(&key.as_str()) {
            continue;
        }
        let flag = key.replace('_', "-");
        match value {
            Value::Bool(true) => argv.push(format!("--{flag}")),
            Value::Bool(false) => {}
            Value::String(s) => argv.push(format!("--{flag}={s}")),
            Value::Number(n) => argv.push(format!("--{flag}={n}")),
            _ => return Err("invalid argument".into()),
        }
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
            ("list_actions", json!({ "project": "api" })),
            ("run_action", json!({ "project": "api", "action": "--dev" })),
        ];
        assert_eq!(samples.len(), catalog().len());
        for (name, args) in samples {
            let argv = tool_argv(name, &args).unwrap();
            assert!(Cli::try_parse_from(&argv).is_ok(), "{name}: {argv:?}");
        }
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
        let initialize = handle(&app, json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05"}})).unwrap();
        assert_eq!(initialize["result"]["protocolVersion"], "2024-11-05");
        let catalog = handle(&app, json!({"jsonrpc":"2.0","id":2,"method":"tools/list"})).unwrap();
        assert_eq!(
            catalog["result"]["tools"].as_array().unwrap().len(),
            super::catalog().len()
        );
        assert!(handle(
            &app,
            json!({"jsonrpc":"2.0","method":"notifications/initialized"})
        )
        .is_none());
        assert_eq!(handle(&app, json!({})).unwrap()["error"]["code"], -32600);
        assert_eq!(
            handle(
                &app,
                json!({"jsonrpc":"2.0","id":3,"method":"no-such-method"})
            )
            .unwrap()["error"]["code"],
            -32601
        );
    }
}
