//! # agents
//!
//! Provider registry, descriptors, detection, and launch-spec construction for
//! Forge's agent CLIs (§7.5, §7.6, §13, ADR-007). Per principle P2 this crate
//! owns *all* provider-specific knowledge; the daemon's `AgentService` (§9.3)
//! is thin orchestration over [`AgentRegistry`], and no other crate branches on
//! provider id.
//!
//! ADR-007 defines two levels:
//! - **Descriptors** ([`builtins`]) — static data: candidate binaries, version
//!   probe, capabilities. The four MVP built-ins resolve entirely here.
//! - **Adapters** ([`AgentAdapter`]) — behavioral hooks. The MVP ships zero
//!   real adapters; [`DescriptorAdapter`] wraps any descriptor with no special
//!   behavior.
//!
//! The domain types ([`domain::AgentDescriptor`], [`domain::DetectionResult`],
//! [`domain::SpawnSpec`], …) are reused as-is and re-exported by `domain`.

pub mod builtins;
pub mod descriptor;
pub mod detection;
pub mod registry;
pub mod usage;

#[cfg(test)]
pub(crate) mod test_support;

pub use builtins::{builtin, builtins};
pub use descriptor::{
    build_launch, build_launch_with_env_overlay, ensure_profile_dirs, AgentAdapter, AgentError,
    DescriptorAdapter,
};
pub use detection::detect;
pub use registry::AgentRegistry;
pub use usage::{collect as collect_usage, collect_analytics};

/// One line of a provider's headless event stream, in a form a person reads.
///
/// Provider-shaped knowledge, so it lives here rather than in the GUI: the
/// crate that knows how a provider spells things is the one place allowed to
/// (C4). Two stream dialects are understood, because they are the two the
/// harness runs on:
///
/// - **Claude Code** (`--output-format stream-json`): one object per turn,
///   with the text or tool call inside `message.content`.
/// - **Codex** (`codex exec --json`): an *item* envelope — `item.started` /
///   `item.completed` around `agent_message`, `reasoning`, `command_execution`,
///   `file_change`, and friends. Summarising only its `type` was why a Codex
///   step read as a wall of `item.completed` and said nothing about what the
///   agent was actually doing.
///
/// Anything neither dialect claims falls back to the shallow shape below, and
/// a line that is not JSON at all (a provider that streams plain text) comes
/// back untouched. Reaching deeper than this would be guessing at a schema
/// that is theirs to change.
#[must_use]
pub fn summarize_stream_line(line: &str) -> String {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
        return line.to_owned();
    };
    let kind = value
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("event")
        .to_owned();
    if let Some(summary) = codex_line(&kind, &value) {
        return summary;
    }
    let text = value
        .get("result")
        .and_then(|v| v.as_str())
        .or_else(|| value.get("text").and_then(|v| v.as_str()))
        .map(str::to_owned)
        .or_else(|| {
            value
                .get("message")
                .and_then(|m| m.get("content"))
                .and_then(|c| c.as_array())
                .and_then(|blocks| blocks.iter().find_map(content_block))
        });
    match text {
        Some(text) => {
            let short = clip(&text, TEXT_MAX);
            if short.is_empty() {
                kind
            } else {
                format!("{kind}  {short}")
            }
        }
        None => kind,
    }
}

/// How much of a message survives one line of the viewer.
const TEXT_MAX: usize = 220;

/// How much of a command, or of what it printed, survives one line.
const COMMAND_MAX: usize = 200;
const OUTPUT_MAX: usize = 140;

/// One line, trimmed and cut to `max` — a stream line is read at a glance.
fn clip(text: &str, max: usize) -> String {
    let flat = text.replace(['\n', '\r', '\t'], " ");
    let flat = flat.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        return flat;
    }
    flat.chars().take(max.saturating_sub(1)).collect::<String>() + "…"
}

/// A Claude content block, as the part of it worth showing.
fn content_block(block: &serde_json::Value) -> Option<String> {
    match block.get("type").and_then(|t| t.as_str()) {
        Some("text") => block
            .get("text")
            .and_then(|t| t.as_str())
            .map(str::to_owned),
        // The reasoning itself, when the provider streams it.
        Some("thinking") => block
            .get("thinking")
            .and_then(|t| t.as_str())
            .map(|text| format!("thinking: {text}")),
        // A tool call is the most informative thing a stream says about what
        // the agent is *doing*, and its first argument says which thing.
        Some("tool_use") => {
            let name = block.get("name").and_then(|n| n.as_str()).unwrap_or("tool");
            match block.get("input").and_then(tool_input_hint) {
                Some(hint) => Some(format!("→ {name}  {hint}")),
                None => Some(format!("→ {name}")),
            }
        }
        _ => None,
    }
}

/// The one field of a tool call that says what it is about.
///
/// The names are the ones the tools themselves use; a tool whose input has
/// none of them is summarised by its name alone rather than by a guess.
fn tool_input_hint(input: &serde_json::Value) -> Option<String> {
    for key in [
        "command",
        "file_path",
        "path",
        "pattern",
        "query",
        "prompt",
        "description",
        "url",
    ] {
        if let Some(text) = input.get(key).and_then(|v| v.as_str()) {
            if !text.trim().is_empty() {
                return Some(clip(text, COMMAND_MAX));
            }
        }
    }
    None
}

/// A Codex event, or `None` when the line is not one.
fn codex_line(kind: &str, value: &serde_json::Value) -> Option<String> {
    match kind {
        "thread.started" => {
            let id = value
                .get("thread_id")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            Some(format!("thread started  {id}"))
        }
        "turn.started" => Some("turn started".to_owned()),
        "turn.completed" => {
            let usage = value.get("usage")?;
            let field = |key: &str| usage.get(key).and_then(serde_json::Value::as_u64);
            Some(format!(
                "turn completed  in {} · out {} · reasoning {}",
                field("input_tokens").unwrap_or(0),
                field("output_tokens").unwrap_or(0),
                field("reasoning_output_tokens").unwrap_or(0),
            ))
        }
        "turn.failed" | "error" => {
            let message = value
                .get("error")
                .and_then(|e| {
                    e.get("message")
                        .and_then(|m| m.as_str())
                        .or_else(|| e.as_str())
                })
                .or_else(|| value.get("message").and_then(|m| m.as_str()))
                .unwrap_or("");
            Some(format!("error  {}", clip(message, TEXT_MAX)))
        }
        "item.started" | "item.updated" | "item.completed" => {
            let item = value.get("item")?;
            Some(codex_item(kind == "item.completed", item))
        }
        _ => None,
    }
}

/// One Codex item, told as what the agent did rather than as its envelope.
///
/// `started` and `completed` say different things on purpose: a command
/// announces itself when it starts and reports its exit code and last output
/// when it ends, so the pair reads like a shell transcript instead of like the
/// same line twice.
fn codex_item(completed: bool, item: &serde_json::Value) -> String {
    let item_type = item.get("type").and_then(|t| t.as_str()).unwrap_or("item");
    let text = |key: &str| item.get(key).and_then(|v| v.as_str()).unwrap_or("");
    match item_type {
        "agent_message" => format!("assistant  {}", clip(text("text"), TEXT_MAX)),
        // The chain of thought, when the model is configured to summarise it.
        "reasoning" => {
            let body = if text("text").is_empty() {
                text("summary")
            } else {
                text("text")
            };
            format!("thinking  {}", clip(body, TEXT_MAX))
        }
        "command_execution" => {
            let command = clip(&strip_shell_wrapper(text("command")), COMMAND_MAX);
            if !completed {
                return format!("$ {command}");
            }
            let code = item
                .get("exit_code")
                .and_then(serde_json::Value::as_i64)
                .unwrap_or(-1);
            let tail = last_meaningful_line(text("aggregated_output"));
            let mark = if code == 0 { "ok" } else { "FAILED" };
            if tail.is_empty() {
                format!("  ⤷ {mark} ({code})  {command}")
            } else {
                format!("  ⤷ {mark} ({code})  {}", clip(&tail, OUTPUT_MAX))
            }
        }
        "file_change" => {
            let changes = item
                .get("changes")
                .and_then(|c| c.as_array())
                .map(Vec::as_slice)
                .unwrap_or_default();
            let named: Vec<String> = changes
                .iter()
                .take(3)
                .map(|change| {
                    let path = change.get("path").and_then(|p| p.as_str()).unwrap_or("?");
                    let kind = change
                        .get("kind")
                        .and_then(|k| k.as_str())
                        .unwrap_or("edit");
                    format!("{} ({kind})", file_name(path))
                })
                .collect();
            let more = changes.len().saturating_sub(named.len());
            let verb = if completed { "edited" } else { "editing" };
            if more > 0 {
                format!("✎ {verb}  {} +{more} more", named.join(", "))
            } else {
                format!("✎ {verb}  {}", named.join(", "))
            }
        }
        "todo_list" => {
            let items = item
                .get("items")
                .and_then(|i| i.as_array())
                .map(Vec::as_slice)
                .unwrap_or_default();
            let next = items
                .iter()
                .find(|todo| {
                    todo.get("completed").and_then(serde_json::Value::as_bool) != Some(true)
                })
                .and_then(|todo| todo.get("text").and_then(|t| t.as_str()))
                .unwrap_or("");
            format!("todo  {} item(s)  {}", items.len(), clip(next, OUTPUT_MAX))
        }
        "web_search" => format!("⌕ search  {}", clip(text("query"), OUTPUT_MAX)),
        "mcp_tool_call" => {
            let server = text("server");
            let tool = text("tool");
            format!("→ {server}/{tool}")
        }
        "error" => format!("error  {}", clip(text("message"), TEXT_MAX)),
        other => other.to_owned(),
    }
}

/// `/bin/zsh -lc "…"` is how Codex runs everything; the quoted part is the
/// command a person means.
fn strip_shell_wrapper(command: &str) -> String {
    for marker in [" -lc ", " -c "] {
        if let Some(rest) = command.split_once(marker).map(|(_, rest)| rest) {
            let rest = rest.trim();
            let unquoted = rest
                .strip_prefix('"')
                .and_then(|r| r.strip_suffix('"'))
                .or_else(|| rest.strip_prefix('\'').and_then(|r| r.strip_suffix('\'')))
                .unwrap_or(rest);
            return unquoted.to_owned();
        }
    }
    command.to_owned()
}

/// The last line a command printed that says anything.
fn last_meaningful_line(output: &str) -> String {
    output
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("")
        .to_owned()
}

/// The tail of a path, which is the part that identifies a file on one line.
fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

#[cfg(test)]
mod summary_tests {
    use super::summarize_stream_line;

    #[test]
    fn an_assistant_turn_shows_its_text_and_a_tool_call_shows_its_name() {
        assert_eq!(
            summarize_stream_line(
                r#"{"type":"assistant","message":{"content":[{"type":"text","text":"reading the spec"}]}}"#
            ),
            "assistant  reading the spec"
        );
        assert_eq!(
            summarize_stream_line(
                r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Read"}]}}"#
            ),
            "assistant  → Read"
        );
        assert_eq!(
            summarize_stream_line(r#"{"type":"result","result":"done"}"#),
            "result  done"
        );
    }

    /// The two shapes that must not be swallowed: a line with nothing to say,
    /// and a line that is not JSON at all.
    /// The Codex dialect: the item envelope has to become what the agent did,
    /// because summarising its `type` alone is what made a Codex step read as
    /// a column of `item.completed`.
    #[test]
    fn a_codex_item_reads_as_the_thing_the_agent_did() {
        assert_eq!(
            summarize_stream_line(
                r#"{"type":"item.completed","item":{"id":"i0","type":"agent_message","text":"I'll write the spec"}}"#
            ),
            "assistant  I'll write the spec"
        );
        assert_eq!(
            summarize_stream_line(
                r#"{"type":"item.completed","item":{"type":"reasoning","text":"checking the theme tokens"}}"#
            ),
            "thinking  checking the theme tokens"
        );
        assert_eq!(
            summarize_stream_line(
                r#"{"type":"item.started","item":{"type":"command_execution","command":"/bin/zsh -lc \"rg theme crates\"","status":"in_progress"}}"#
            ),
            "$ rg theme crates"
        );
        assert_eq!(
            summarize_stream_line(
                r#"{"type":"item.completed","item":{"type":"command_execution","command":"/bin/zsh -lc \"ls\"","aggregated_output":"a\nb\n","exit_code":0,"status":"completed"}}"#
            ),
            "  ⤷ ok (0)  b"
        );
        assert_eq!(
            summarize_stream_line(
                r#"{"type":"item.completed","item":{"type":"file_change","changes":[{"path":"/w/harness/features.json","kind":"update"}],"status":"completed"}}"#
            ),
            "✎ edited  features.json (update)"
        );
        assert_eq!(
            summarize_stream_line(
                r#"{"type":"turn.completed","usage":{"input_tokens":10,"output_tokens":2,"reasoning_output_tokens":1}}"#
            ),
            "turn completed  in 10 · out 2 · reasoning 1"
        );
    }

    /// A tool call says which file or command it is about, not just its name.
    #[test]
    fn a_claude_tool_call_carries_its_first_argument() {
        assert_eq!(
            summarize_stream_line(
                r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash","input":{"command":"cargo test"}}]}}"#
            ),
            "assistant  → Bash cargo test"
        );
        assert_eq!(
            summarize_stream_line(
                r#"{"type":"assistant","message":{"content":[{"type":"thinking","thinking":"weighing the options"}]}}"#
            ),
            "assistant  thinking: weighing the options"
        );
    }

    #[test]
    fn a_line_with_no_text_keeps_its_kind_and_plain_text_survives_whole() {
        assert_eq!(
            summarize_stream_line(r#"{"type":"system","subtype":"init"}"#),
            "system"
        );
        assert_eq!(summarize_stream_line("thinking…"), "thinking…");
        assert_eq!(summarize_stream_line(""), "");
    }
}
