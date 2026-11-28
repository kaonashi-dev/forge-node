//! A Forge-launched agent's transcript as chat turns, for a polled read.

use std::collections::{HashSet, VecDeque};
use std::hash::{Hash, Hasher};
use std::io::BufReader;
use std::path::Path;

use domain::{ConversationTurn, SessionConversation, SessionId, TurnRole};
use serde_json::Value;

use super::{bounded, cap, is_typed_prompt, user_text, CLAUDE_PROVIDER};

/// Turns a conversation read returns when the caller names no number.
pub const DEFAULT_CONVERSATION_TURNS: u32 = 60;
/// Hard cap on one read's turns.
pub const MAX_CONVERSATION_TURNS: u32 = 200;
/// Bytes a conversation read returns when the caller names no number.
pub const DEFAULT_CONVERSATION_BYTES: u32 = 64 * 1024;
/// Hard cap on one read's bytes.
pub const MAX_CONVERSATION_BYTES: u32 = 256 * 1024;

/// Longest tool line, in characters.
const TOOL_LINE_CHARS: usize = 100;
/// Tool lines one turn keeps, the newest; a turn that ran more is `truncated`.
const MAX_TURN_TOOLS: usize = 32;
/// Inputs that name what a tool call acted on, most telling first.
const TOOL_SUBJECTS: [&str; 6] = [
    "file_path",
    "command",
    "pattern",
    "url",
    "path",
    "description",
];

/// The session's conversation off `transcript`, the newest turns kept first.
///
/// `available: false` for every provider but Claude and for a transcript that
/// is not a readable file. The file is streamed through a sliding window of
/// turns and bytes, so the text held stays near `max_bytes` however long the
/// run; only an 8-byte hash per record already taken grows with it.
#[must_use]
pub fn read_conversation(
    session_id: SessionId,
    provider: &str,
    transcript: &Path,
    max_turns: u32,
    max_bytes: u32,
) -> SessionConversation {
    // The store is agent-writable: a FIFO in its place would block the open.
    if provider != CLAUDE_PROVIDER || !transcript.is_file() {
        return SessionConversation::unavailable(session_id);
    }
    let Ok(file) = std::fs::File::open(transcript) else {
        return SessionConversation::unavailable(session_id);
    };
    let mut window = Window::new(
        max_turns.min(MAX_CONVERSATION_TURNS) as usize,
        max_bytes.min(MAX_CONVERSATION_BYTES) as usize,
    );
    let unread = crate::jsonl::for_each(BufReader::new(file), bounded::MAX_RECORD_BYTES, |line| {
        window.record(line);
    });
    let (turns, truncated) = window.finish();
    SessionConversation {
        session_id,
        available: true,
        turns,
        truncated: truncated || unread.any(),
    }
}

struct Pending {
    role: TurnRole,
    text: String,
    tools: VecDeque<String>,
    tool_bytes: usize,
    /// Text or tool lines were left out of this turn.
    cut: bool,
}

impl Pending {
    fn new(role: TurnRole) -> Self {
        Self {
            role,
            text: String::new(),
            tools: VecDeque::new(),
            tool_bytes: 0,
            cut: false,
        }
    }

    fn size(&self) -> usize {
        self.text.len() + self.tool_bytes
    }

    /// Text past `budget` could never be returned: only the newest turn may
    /// be cut, and it keeps its beginning.
    fn push_text(&mut self, text: &str, budget: usize) {
        let separator = if self.text.is_empty() { "" } else { "\n\n" };
        let room = budget.saturating_sub(self.text.len());
        if separator.len() + text.len() <= room {
            self.text.push_str(separator);
            self.text.push_str(text);
            return;
        }
        self.cut = true;
        if room > separator.len() {
            self.text.push_str(separator);
            self.text
                .push_str(&text[..floor_boundary(text, room - separator.len())]);
        }
    }

    fn push_tool(&mut self, line: String) {
        self.tool_bytes += line.len();
        self.tools.push_back(line);
        if self.tools.len() > MAX_TURN_TOOLS {
            self.drop_oldest_tool();
        }
    }

    fn drop_oldest_tool(&mut self) {
        if let Some(old) = self.tools.pop_front() {
            self.tool_bytes -= old.len();
            self.cut = true;
        }
    }

    /// Fit the newest turn alone into `budget`: tool lines go oldest first,
    /// then the text is cut on a character boundary.
    fn fit(&mut self, budget: usize) {
        while self.tool_bytes > budget {
            self.drop_oldest_tool();
        }
        let room = budget - self.tool_bytes;
        if self.text.len() > room {
            self.text.truncate(floor_boundary(&self.text, room));
            self.cut = true;
        }
    }
}

struct Window {
    turns: VecDeque<Pending>,
    size: usize,
    limit: usize,
    budget: usize,
    /// Whole turns left out.
    dropped: bool,
    /// Hashes of the user records and assistant blocks already taken. Resuming
    /// a Claude session copies earlier records into the new transcript.
    seen: HashSet<u64>,
}

impl Window {
    fn new(limit: usize, budget: usize) -> Self {
        Self {
            turns: VecDeque::new(),
            size: 0,
            limit,
            budget,
            dropped: false,
            seen: HashSet::new(),
        }
    }

    fn record(&mut self, line: &[u8]) {
        let Ok(line) = std::str::from_utf8(line) else {
            return;
        };
        let line = line.trim();
        let assistant = line.contains(r#""type":"assistant""#);
        if !assistant && !line.contains(r#""type":"user""#) {
            return;
        }
        // Tool results are most of a transcript's bytes and never a turn.
        if !assistant && line.contains(r#""toolUseResult""#) {
            return;
        }
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            return;
        };
        if value.get("isSidechain").and_then(Value::as_bool) == Some(true) {
            return;
        }
        match value.get("type").and_then(Value::as_str) {
            Some("user") => self.user(&value),
            Some("assistant") => self.assistant(&value),
            _ => {}
        }
    }

    fn user(&mut self, value: &Value) {
        if !is_typed_prompt(value) {
            return;
        }
        let Some(text) = user_text(value) else {
            return;
        };
        let uuid = value.get("uuid").and_then(Value::as_str);
        if !self.fresh(uuid, "user", &[]) {
            return;
        }
        let mut turn = Pending::new(TurnRole::User);
        turn.push_text(&text, self.budget);
        self.size += turn.size();
        self.turns.push_back(turn);
        self.evict();
    }

    /// Claude records one content block per assistant record, so `message.id`
    /// alone would keep only a message's first block: the block is the key.
    fn assistant(&mut self, value: &Value) {
        let Some(message) = value.get("message") else {
            return;
        };
        let id = message.get("id").and_then(Value::as_str);
        let cwd = value.get("cwd").and_then(Value::as_str);
        match message.get("content") {
            Some(Value::String(text)) => {
                if self.fresh(id, "text", &[text]) {
                    self.agent_text(text);
                }
            }
            Some(Value::Array(blocks)) => {
                for block in blocks {
                    match block.get("type").and_then(Value::as_str) {
                        Some("text") => {
                            let Some(text) = block.get("text").and_then(Value::as_str) else {
                                continue;
                            };
                            if self.fresh(id, "text", &[text]) {
                                self.agent_text(text);
                            }
                        }
                        Some("tool_use") => {
                            let Some(line) = tool_line(block, cwd) else {
                                continue;
                            };
                            let call = block.get("id").and_then(Value::as_str).unwrap_or(&line);
                            if self.fresh(id, "tool", &[call]) {
                                self.agent_tool(line);
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }

    /// `false` for a record already taken; a record with no id is always new.
    fn fresh(&mut self, id: Option<&str>, kind: &str, parts: &[&str]) -> bool {
        let Some(id) = id else {
            return true;
        };
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        (id, kind, parts).hash(&mut hasher);
        self.seen.insert(hasher.finish())
    }

    fn agent_text(&mut self, text: &str) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        let budget = self.budget;
        self.grow_agent(|turn| turn.push_text(text, budget));
    }

    fn agent_tool(&mut self, line: String) {
        self.grow_agent(|turn| turn.push_tool(line));
    }

    /// An agent turn starts only with something to show, so a run of
    /// thinking-only records never becomes an empty turn.
    fn grow_agent(&mut self, grow: impl FnOnce(&mut Pending)) {
        if self
            .turns
            .back()
            .is_none_or(|turn| turn.role != TurnRole::Agent)
        {
            self.turns.push_back(Pending::new(TurnRole::Agent));
        }
        let Some(turn) = self.turns.back_mut() else {
            return;
        };
        let before = turn.size();
        grow(turn);
        self.size = self.size - before + turn.size();
        self.evict();
    }

    /// Sizes only grow, so a turn whose newer turns already fill the budget
    /// can never be returned and is dropped while streaming.
    fn evict(&mut self) {
        while let Some(front) = self.turns.front() {
            let over_count = self.turns.len() > self.limit;
            let over_budget = self.turns.len() > 1 && self.size - front.size() >= self.budget;
            if !over_count && !over_budget {
                break;
            }
            self.size -= front.size();
            self.turns.pop_front();
            self.dropped = true;
        }
    }

    fn finish(self) -> (Vec<ConversationTurn>, bool) {
        let mut truncated = self.dropped;
        let mut kept = Vec::new();
        let mut size = 0usize;
        for mut turn in self.turns.into_iter().rev() {
            if kept.is_empty() {
                turn.fit(self.budget);
            } else if size + turn.size() > self.budget {
                truncated = true;
                break;
            }
            size += turn.size();
            truncated |= turn.cut;
            kept.push(ConversationTurn {
                role: turn.role,
                text: turn.text,
                tools: turn.tools.into(),
            });
        }
        kept.reverse();
        (kept, truncated)
    }
}

/// `Read src/lib.rs`, `Bash: cargo test`: the tool and what it acted on.
fn tool_line(block: &Value, cwd: Option<&str>) -> Option<String> {
    let name = block.get("name").and_then(Value::as_str)?.trim();
    if name.is_empty() {
        return None;
    }
    let input = block.get("input");
    let subject = TOOL_SUBJECTS.iter().find_map(|key| {
        let value = input?.get(*key)?.as_str()?;
        (!value.trim().is_empty()).then_some((*key, value))
    });
    let line = match subject {
        Some(("command", command)) => format!("{name}: {}", one_line(command)),
        Some(("file_path" | "path", path)) => {
            format!("{name} {}", one_line(relative(path, cwd)))
        }
        Some((_, value)) => format!("{name} {}", one_line(value)),
        None => name.to_owned(),
    };
    Some(cap(&line, TOOL_LINE_CHARS))
}

/// Whitespace folded to single spaces, stopping once past the tool-line cap
/// so a megabyte heredoc is not folded whole.
fn one_line(text: &str) -> String {
    let mut line = String::new();
    let mut chars = 0usize;
    for word in text.split_whitespace() {
        if chars > TOOL_LINE_CHARS {
            break;
        }
        if !line.is_empty() {
            line.push(' ');
            chars += 1;
        }
        line.push_str(word);
        chars += word.chars().count();
    }
    line
}

fn relative<'a>(path: &'a str, cwd: Option<&str>) -> &'a str {
    cwd.and_then(|cwd| path.strip_prefix(cwd)?.strip_prefix('/'))
        .filter(|rest| !rest.is_empty())
        .unwrap_or(path)
}

fn floor_boundary(text: &str, index: usize) -> usize {
    let mut end = index.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    end
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt::Write as _;

    fn user(uuid: &str, text: &str) -> String {
        serde_json::json!({
            "type": "user", "uuid": uuid, "cwd": "/repo",
            "message": {"role": "user", "content": text},
        })
        .to_string()
    }

    fn tool_result(uuid: &str) -> String {
        serde_json::json!({
            "type": "user", "uuid": uuid, "toolUseResult": {"stdout": "ok"},
            "message": {"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "toolu_1", "content": "ok"}
            ]},
        })
        .to_string()
    }

    fn assistant(id: &str, block: &Value) -> String {
        serde_json::json!({
            "type": "assistant", "uuid": format!("{id}-{block}"), "cwd": "/repo",
            "message": {"id": id, "role": "assistant", "content": [block]},
        })
        .to_string()
    }

    fn text(text: &str) -> Value {
        serde_json::json!({"type": "text", "text": text})
    }

    fn tool(id: &str, name: &str, input: &Value) -> Value {
        serde_json::json!({"type": "tool_use", "id": id, "name": name, "input": input})
    }

    fn read(lines: &[String], max_turns: u32, max_bytes: u32) -> SessionConversation {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("run.jsonl");
        let mut body = String::new();
        for line in lines {
            writeln!(body, "{line}").unwrap();
        }
        std::fs::write(&path, body).unwrap();
        read_conversation(SessionId::new(), "claude", &path, max_turns, max_bytes)
    }

    fn roles_and_text(conversation: &SessionConversation) -> Vec<(TurnRole, &str)> {
        conversation
            .turns
            .iter()
            .map(|turn| (turn.role, turn.text.as_str()))
            .collect()
    }

    #[test]
    fn assistant_records_merge_into_one_turn_with_their_tools() {
        let conversation = read(
            &[
                user("u1", "fix the test"),
                assistant(
                    "m1",
                    &serde_json::json!({"type": "thinking", "thinking": "hmm"}),
                ),
                assistant("m1", &text("Looking.")),
                assistant(
                    "m1",
                    &tool(
                        "t1",
                        "Read",
                        &serde_json::json!({"file_path": "/repo/src/lib.rs"}),
                    ),
                ),
                tool_result("r1"),
                assistant(
                    "m2",
                    &tool(
                        "t2",
                        "Bash",
                        &serde_json::json!({"command": "cargo test\n  --workspace", "description": "run"}),
                    ),
                ),
                tool_result("r2"),
                assistant("m3", &text("Fixed.")),
                user("u2", "thanks"),
            ],
            DEFAULT_CONVERSATION_TURNS,
            DEFAULT_CONVERSATION_BYTES,
        );
        assert!(conversation.available);
        assert!(!conversation.truncated);
        assert_eq!(
            roles_and_text(&conversation),
            [
                (TurnRole::User, "fix the test"),
                (TurnRole::Agent, "Looking.\n\nFixed."),
                (TurnRole::User, "thanks"),
            ]
        );
        assert_eq!(
            conversation.turns[1].tools,
            ["Read src/lib.rs", "Bash: cargo test --workspace"]
        );
    }

    #[test]
    fn a_resumed_transcript_does_not_repeat_its_copied_records() {
        let turn = [
            user("u1", "hello"),
            assistant("m1", &text("Hi.")),
            assistant(
                "m1",
                &tool("t1", "Grep", &serde_json::json!({"pattern": "fn main"})),
            ),
        ];
        let mut lines = turn.to_vec();
        lines.extend(turn);
        lines.push(user("u2", "again"));
        lines.push(assistant("m2", &text("Hi again.")));
        let conversation = read(
            &lines,
            DEFAULT_CONVERSATION_TURNS,
            DEFAULT_CONVERSATION_BYTES,
        );
        assert_eq!(
            roles_and_text(&conversation),
            [
                (TurnRole::User, "hello"),
                (TurnRole::Agent, "Hi."),
                (TurnRole::User, "again"),
                (TurnRole::Agent, "Hi again."),
            ]
        );
        assert_eq!(conversation.turns[1].tools, ["Grep fn main"]);
    }

    #[test]
    fn a_thinking_only_reply_is_not_a_turn() {
        let conversation = read(
            &[
                user("u1", "think"),
                assistant(
                    "m1",
                    &serde_json::json!({"type": "thinking", "thinking": "..."}),
                ),
            ],
            DEFAULT_CONVERSATION_TURNS,
            DEFAULT_CONVERSATION_BYTES,
        );
        assert_eq!(roles_and_text(&conversation), [(TurnRole::User, "think")]);
    }

    #[test]
    fn the_turn_limit_keeps_the_newest_and_says_so() {
        let lines: Vec<String> = (0..5)
            .flat_map(|n| {
                [
                    user(&format!("u{n}"), &format!("q{n}")),
                    assistant(&format!("m{n}"), &text(&format!("a{n}"))),
                ]
            })
            .collect();
        let exact = read(&lines, 10, DEFAULT_CONVERSATION_BYTES);
        assert_eq!(exact.turns.len(), 10);
        assert!(
            !exact.truncated,
            "a conversation that fits is not truncated"
        );

        let tail = read(&lines, 3, DEFAULT_CONVERSATION_BYTES);
        assert!(tail.truncated);
        assert_eq!(
            roles_and_text(&tail),
            [
                (TurnRole::Agent, "a3"),
                (TurnRole::User, "q4"),
                (TurnRole::Agent, "a4"),
            ]
        );
    }

    #[test]
    fn the_byte_budget_stops_at_a_whole_turn_and_cuts_only_the_newest() {
        let lines = [
            user("u1", "first question"),
            assistant("m1", &text("an older answer")),
            user("u2", "é".repeat(20).as_str()),
        ];
        let conversation = read(&lines, DEFAULT_CONVERSATION_TURNS, 25);
        assert!(conversation.truncated);
        assert_eq!(
            conversation.turns.len(),
            1,
            "the older turns do not fit beside it"
        );
        assert_eq!(
            conversation.turns[0].text,
            "é".repeat(12),
            "cut on a character boundary"
        );

        let roomy = read(&lines, DEFAULT_CONVERSATION_TURNS, 55);
        assert!(roomy.truncated);
        assert_eq!(
            roles_and_text(&roomy),
            [
                (TurnRole::Agent, "an older answer"),
                (TurnRole::User, "é".repeat(20).as_str()),
            ]
        );
    }

    #[test]
    fn tool_lines_are_one_capped_line_relative_to_the_checkout() {
        let long = "x ".repeat(200);
        let conversation = read(
            &[
                assistant(
                    "m1",
                    &tool("t1", "Bash", &serde_json::json!({"command": long})),
                ),
                assistant(
                    "m1",
                    &tool("t2", "TodoWrite", &serde_json::json!({"todos": []})),
                ),
                assistant(
                    "m1",
                    &tool(
                        "t3",
                        "Edit",
                        &serde_json::json!({"file_path": "/elsewhere/a.rs"}),
                    ),
                ),
            ],
            DEFAULT_CONVERSATION_TURNS,
            DEFAULT_CONVERSATION_BYTES,
        );
        let tools = &conversation.turns[0].tools;
        assert_eq!(tools[0].chars().count(), TOOL_LINE_CHARS + 1);
        assert!(tools[0].starts_with("Bash: x x") && tools[0].ends_with('…'));
        assert_eq!(tools[1], "TodoWrite");
        assert_eq!(tools[2], "Edit /elsewhere/a.rs");
    }

    #[test]
    fn an_oversized_record_marks_the_read_truncated() {
        let huge = "y".repeat(bounded::MAX_RECORD_BYTES as usize + 1);
        let conversation = read(
            &[user("u1", "hi"), assistant("m1", &text(&huge))],
            DEFAULT_CONVERSATION_TURNS,
            DEFAULT_CONVERSATION_BYTES,
        );
        assert!(conversation.truncated);
        assert_eq!(roles_and_text(&conversation), [(TurnRole::User, "hi")]);
    }

    #[test]
    fn other_providers_and_missing_files_are_unavailable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("run.jsonl");
        std::fs::write(&path, user("u1", "hi")).unwrap();
        let id = SessionId::new();
        assert!(!read_conversation(id, "opencode", &path, 10, 1024).available);
        assert!(!read_conversation(id, "claude", &dir.path().join("gone"), 10, 1024).available);
        assert!(!read_conversation(id, "claude", dir.path(), 10, 1024).available);
        assert!(read_conversation(id, "claude", &path, 10, 1024).available);
    }
}
