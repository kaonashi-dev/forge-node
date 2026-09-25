//! The command contract `forgectl schema` prints. Agents read this, not the manual.

use serde_json::{json, Value};

pub fn schema_value() -> Value {
    json!({
        "program": "forgectl",
        "json": {
            "flag": "--json",
            "env": "FORGECTL_JSON=1",
            "ok": {"ok": true, "result": {}},
            "error": {"ok": false, "error": {"code": "", "message": "", "details": {}, "next": [["forgectl"]]}}
        },
        "exit_codes": [
            {"code": 0, "meaning": "ok or wait condition met"},
            {"code": 1, "meaning": "other daemon refusal"},
            {"code": 2, "meaning": "usage"},
            {"code": 3, "meaning": "unreachable, protocol mismatch, or sandbox_denied"},
            {"code": 4, "meaning": "not found"},
            {"code": 5, "meaning": "conflict or precondition failed"},
            {"code": 6, "meaning": "policy rail"},
            {"code": 124, "meaning": "wait timed out"}
        ],
        "identity_env": ["FORGE_SESSION_ID", "FORGE_RUN_ID", "FORGE_TASK_ID", "FORGE_ATTEMPT_ID"],
        "commands": [
            "status", "guide", "schema", "providers",
            "run start", "run list", "run show", "run brief", "run wait", "run close", "run resume",
            "task add", "task list", "task show", "task start", "task review", "task accept",
            "task reject", "task cancel", "task integrate", "task cleanup",
            "context", "report", "report show", "ask", "inbox", "send",
            "state list", "state get", "state set", "state del", "state watch",
            "session list", "session show", "session read", "session kill",
            "hook"
        ],
        "rules": [
            "state set requires --if-version; 0 is create-only",
            "run wait is level-triggered and returns the triggering items",
            "only an explicit report settles an attempt",
            "a repeated --request-id returns the stored result"
        ]
    })
}
