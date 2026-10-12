use std::io::{self, BufRead, Write};

use super::*;

const MAX_MCP_LINE: usize = 65_536;

pub(crate) fn serve(app: &App) -> Result<(), u8> {
    let stdin = io::stdin();
    let mut reader = stdin.lock();
    let stdout = io::stdout();
    let mut writer = stdout.lock();
    loop {
        let line = read_line(&mut reader).map_err(|error| {
            eprintln!("MCP input: {error}");
            3
        })?;
        let Some(line) = line else {
            return Ok(());
        };
        let response = match serde_json::from_slice::<Value>(&line) {
            Ok(request) => handle(app, request),
            Err(_) => Some(error(
                Value::Null,
                -32700,
                "invalid JSON or request exceeds 64 KiB",
            )),
        };
        if let Some(response) = response {
            serde_json::to_writer(&mut writer, &response).map_err(|_| 3)?;
            writer
                .write_all(b"\n")
                .and_then(|_| writer.flush())
                .map_err(|_| 3)?;
        }
    }
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
                socket: app.socket.clone(),
                timeout: app.timeout,
                request_id: None,
                capture: Some(std::cell::RefCell::new(Value::Null)),
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

fn tool(name: &str, description: &str, properties: Value, required: &[&str]) -> Value {
    json!({ "name": name, "description": description, "inputSchema": {
        "type": "object", "properties": properties, "required": required, "additionalProperties": false,
    } })
}

fn tools() -> Vec<Value> {
    let string = json!({ "type": "string" });
    let boolean = json!({ "type": "boolean" });
    vec![
        tool("list_sessions", "List daemon-owned sessions with project, title, provider and activity.", json!({}), &[]),
        tool("spawn_session", "Launch an independent agent in a project. Does not create a cross-project graph parent.", json!({
            "project": string, "provider": string, "workspace": string, "worktree": string, "base": string,
            "profile": string, "name": string, "prompt": string, "model": string, "effort": string, "read_only": boolean,
        }), &["project", "provider"]),
        tool("send_prompt", "Paste up to 8 KiB. Default requires hook-confirmed idle; queue has one slot. Now never submits while non-idle.", json!({
            "session": string, "text": string, "no_submit": boolean, "queue": boolean, "now": boolean,
        }), &["session", "text"]),
        tool("wait_session", "Wait for an activity state by events, not polling. Idle is not evidence of completed work.", json!({
            "session": string, "for": { "type": "string", "enum": ["idle", "waiting", "working"] },
            "timeout": { "type": "integer", "minimum": 0, "maximum": 600 },
        }), &["session", "for"]),
        tool("read_session", "Read a bounded terminal transcript.", json!({ "session": string }), &["session"]),
        tool("rename_session", "Set a non-empty human title. Titles need not be unique.", json!({ "session": string, "title": string }), &["session", "title"]),
    ]
}

fn tool_argv(name: &str, args: &Value) -> Result<Vec<String>, String> {
    let definition = tools()
        .into_iter()
        .find(|t| t["name"] == name)
        .ok_or_else(|| "unknown tool".to_owned())?;
    let args = args
        .as_object()
        .ok_or_else(|| "arguments must be an object".to_owned())?;
    let properties = definition["inputSchema"]["properties"]
        .as_object()
        .ok_or_else(|| "invalid schema".to_owned())?;
    for required in definition["inputSchema"]["required"]
        .as_array()
        .into_iter()
        .flatten()
    {
        if !args.contains_key(required.as_str().unwrap_or("")) {
            return Err(format!("missing {required}"));
        }
    }
    for (key, value) in args {
        let schema = properties
            .get(key)
            .ok_or_else(|| format!("unknown argument {key}"))?;
        let valid = match schema["type"].as_str() {
            Some("string") => value.is_string(),
            Some("boolean") => value.is_boolean(),
            Some("integer") => value.as_u64().is_some_and(|v| v <= 600),
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
    let subcommand = match name {
        "list_sessions" => "list",
        "spawn_session" => "spawn",
        "send_prompt" => "send",
        "wait_session" => "wait",
        "read_session" => "read",
        "rename_session" => "rename",
        _ => return Err("unknown tool".into()),
    };
    let mut argv = vec!["forgectl".into(), "session".into(), subcommand.into()];
    for (key, value) in args {
        if key == "session" || key == "title" {
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
    if let Some(session) = args.get("session").and_then(Value::as_str) {
        argv.extend(["--".into(), session.into()]);
    }
    if let Some(title) = args.get("title").and_then(Value::as_str) {
        argv.push(title.into());
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
    }

    #[test]
    fn mcp_handshake_catalog_and_notifications_do_not_need_a_daemon() {
        let app = App {
            json: true,
            socket: None,
            timeout: Duration::from_secs(1),
            request_id: None,
            capture: None,
        };
        let initialize = handle(&app, json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05"}})).unwrap();
        assert_eq!(initialize["result"]["protocolVersion"], "2024-11-05");
        let catalog = handle(&app, json!({"jsonrpc":"2.0","id":2,"method":"tools/list"})).unwrap();
        assert_eq!(catalog["result"]["tools"].as_array().unwrap().len(), 6);
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
