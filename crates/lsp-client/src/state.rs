use serde_json::Value;

use crate::Status;

/// Open progress tokens kept; a server that begins more is not tracked further.
const MAX_PROGRESS_TOKENS: usize = 64;
const MAX_TITLE_BYTES: usize = 200;

#[derive(Debug, Default)]
pub(crate) struct State {
    pub(crate) initialized: bool,
    pub(crate) exited: Option<String>,
    progress: Vec<Progress>,
    server_status: Option<ServerStatus>,
}

#[derive(Debug)]
struct Progress {
    token: String,
    title: String,
    percentage: Option<u32>,
}

#[derive(Debug)]
struct ServerStatus {
    quiescent: bool,
    message: Option<String>,
}

impl State {
    pub(crate) fn status(&self) -> Status {
        if let Some(reason) = &self.exited {
            return Status::Exited {
                reason: reason.clone(),
            };
        }
        if !self.initialized {
            return Status::Starting;
        }
        let latest = self.progress.last();
        let indexing = |fallback: Option<&str>| Status::Indexing {
            title: latest
                .map(|p| p.title.as_str())
                .or(fallback)
                .unwrap_or("Indexing")
                .to_owned(),
            percentage: latest.and_then(|p| p.percentage),
        };
        // rust-analyzer's own status outranks token bookkeeping: it stays
        // non-quiescent between progress tokens, and quiescent while a
        // long-lived token is still open.
        match &self.server_status {
            Some(status) if status.quiescent => Status::Ready,
            Some(status) => indexing(status.message.as_deref()),
            None if latest.is_some() => indexing(None),
            None => Status::Ready,
        }
    }

    /// `$/progress` with a work-done payload; partial-result progress has no
    /// `kind` and is ignored.
    pub(crate) fn apply_progress(&mut self, params: &Value) {
        let Some(token) = params.get("token") else {
            return;
        };
        let token = match token {
            Value::String(text) => text.clone(),
            other => other.to_string(),
        };
        let Some(value) = params.get("value") else {
            return;
        };
        let percentage = value
            .get("percentage")
            .and_then(Value::as_u64)
            .map(|p| u32::try_from(p.min(100)).unwrap_or(100));
        let existing = self.progress.iter().position(|p| p.token == token);
        match value.get("kind").and_then(Value::as_str) {
            Some("begin") => {
                if let Some(index) = existing {
                    self.progress.remove(index);
                }
                if self.progress.len() < MAX_PROGRESS_TOKENS {
                    let title = value.get("title").and_then(Value::as_str).unwrap_or("");
                    self.progress.push(Progress {
                        token,
                        title: clip(title, MAX_TITLE_BYTES).to_owned(),
                        percentage,
                    });
                }
            }
            Some("report") => {
                if let (Some(index), Some(_)) = (existing, percentage) {
                    self.progress[index].percentage = percentage;
                }
            }
            Some("end") => {
                if let Some(index) = existing {
                    self.progress.remove(index);
                }
            }
            _ => {}
        }
    }

    /// rust-analyzer's `experimental/serverStatus`.
    pub(crate) fn apply_server_status(&mut self, params: &Value) {
        let Some(quiescent) = params.get("quiescent").and_then(Value::as_bool) else {
            return;
        };
        let message = params
            .get("message")
            .and_then(Value::as_str)
            .map(|m| clip(m, MAX_TITLE_BYTES).to_owned());
        self.server_status = Some(ServerStatus { quiescent, message });
    }
}

/// The longest prefix of `text` within `max` bytes that ends on a char boundary.
pub(crate) fn clip(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ready() -> State {
        State {
            initialized: true,
            ..State::default()
        }
    }

    fn progress(token: Value, kind: &str, extra: Value) -> Value {
        let mut value = json!({"kind": kind});
        if let (Some(target), Value::Object(fields)) = (value.as_object_mut(), extra) {
            target.extend(fields);
        }
        json!({"token": token, "value": value})
    }

    #[test]
    fn starting_until_initialized_and_exited_wins() {
        let mut state = State::default();
        assert_eq!(state.status(), Status::Starting);
        state.apply_progress(&progress(json!("t"), "begin", json!({"title": "x"})));
        assert_eq!(state.status(), Status::Starting);
        state.initialized = true;
        state.exited = Some("gone".into());
        assert_eq!(
            state.status(),
            Status::Exited {
                reason: "gone".into()
            }
        );
    }

    #[test]
    fn progress_tokens_drive_indexing() {
        let mut state = ready();
        assert_eq!(state.status(), Status::Ready);
        state.apply_progress(&progress(json!(1), "begin", json!({"title": "Loading"})));
        state.apply_progress(&progress(
            json!("b"),
            "begin",
            json!({"title": "Indexing", "percentage": 5}),
        ));
        state.apply_progress(&progress(json!("b"), "report", json!({"percentage": 250})));
        assert_eq!(
            state.status(),
            Status::Indexing {
                title: "Indexing".into(),
                percentage: Some(100)
            }
        );
        state.apply_progress(&progress(json!("b"), "end", json!({})));
        assert_eq!(
            state.status(),
            Status::Indexing {
                title: "Loading".into(),
                percentage: None
            }
        );
        state.apply_progress(&progress(json!(1), "end", json!({})));
        assert_eq!(state.status(), Status::Ready);
    }

    #[test]
    fn partial_result_progress_is_ignored() {
        let mut state = ready();
        state.apply_progress(&json!({"token": "p", "value": [1, 2]}));
        assert_eq!(state.status(), Status::Ready);
    }

    #[test]
    fn server_status_outranks_tokens() {
        let mut state = ready();
        state.apply_server_status(&json!({"health": "ok", "quiescent": false}));
        assert_eq!(
            state.status(),
            Status::Indexing {
                title: "Indexing".into(),
                percentage: None
            }
        );
        state.apply_progress(&progress(json!("t"), "begin", json!({"title": "Building"})));
        state.apply_server_status(&json!({"health": "ok", "quiescent": true}));
        assert_eq!(state.status(), Status::Ready);
    }

    #[test]
    fn tokens_are_bounded() {
        let mut state = ready();
        for n in 0..MAX_PROGRESS_TOKENS * 2 {
            state.apply_progress(&progress(
                json!(n),
                "begin",
                json!({"title": n.to_string()}),
            ));
        }
        assert_eq!(state.progress.len(), MAX_PROGRESS_TOKENS);
    }

    #[test]
    fn clip_respects_char_boundaries() {
        assert_eq!(clip("héllo", 2), "h");
        assert_eq!(clip("héllo", 3), "hé");
        assert_eq!(clip("hi", 10), "hi");
    }
}
