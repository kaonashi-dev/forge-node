//! Format and clamp context envelopes for SendContext (§8.3).
//!
//! Keeps the framed paste / initial_prompt shape out of `core.rs`. Budgets are
//! applied before allocation so a wire string cannot balloon under the lock.

use domain::{ContextArtifactKind, ContextArtifactRef, ContextEnvelope};

/// Cap on `summary` / `instructions` stored and shown.
pub const MAX_CONTEXT_FIELD_BYTES: usize = 8_192;

/// Clamp a free-text field before it is stored or rendered.
#[must_use]
pub fn clamp_field(value: Option<String>) -> Option<String> {
    let raw = value?.trim().to_owned();
    if raw.is_empty() {
        return None;
    }
    Some(truncate_utf8(&raw, MAX_CONTEXT_FIELD_BYTES))
}

/// Build the prompt / PTY paste for a delivery.
#[must_use]
pub fn format_delivery(
    envelope: &ContextEnvelope,
    source_label: &str,
    transcript: Option<&str>,
) -> String {
    let mut out = String::from("--- forge context ---\n");
    out.push_str(&format!(
        "From session {} ({})\n",
        envelope.source_session_id, source_label
    ));
    if let Some(summary) = envelope.summary.as_deref() {
        out.push_str("Summary: ");
        out.push_str(summary);
        out.push('\n');
    }
    if let Some(instructions) = envelope.instructions.as_deref() {
        out.push_str("Instructions:\n");
        out.push_str(instructions);
        out.push('\n');
    }
    if let Some(text) = transcript.filter(|t| !t.trim().is_empty()) {
        out.push_str("\nCited transcript:\n```\n");
        out.push_str(text);
        if !text.ends_with('\n') {
            out.push('\n');
        }
        out.push_str("```\n");
    }
    out.push_str("--- end forge context ---\n");
    out
}

/// Optional transcript artifact for the envelope.
#[must_use]
pub fn transcript_artifact(text: &str) -> ContextArtifactRef {
    ContextArtifactRef {
        kind: ContextArtifactKind::TerminalExcerpt,
        value: text.to_owned(),
    }
}

fn truncate_utf8(input: &str, max_bytes: usize) -> String {
    if input.len() <= max_bytes {
        return input.to_owned();
    }
    let mut end = max_bytes;
    while end > 0 && !input.is_char_boundary(end) {
        end -= 1;
    }
    let mut out = input[..end].to_owned();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::{ContextId, SessionId, Timestamp};

    fn envelope(summary: Option<&str>, instructions: Option<&str>) -> ContextEnvelope {
        ContextEnvelope {
            id: ContextId::new(),
            source_session_id: SessionId::new(),
            target_session_id: None,
            summary: summary.map(str::to_owned),
            instructions: instructions.map(str::to_owned),
            artifacts: vec![],
            git_context: None,
            created_at: Timestamp::now(),
        }
    }

    #[test]
    fn clamp_field_drops_blank_and_bounds_length() {
        assert_eq!(clamp_field(Some("  ".into())), None);
        assert_eq!(clamp_field(Some("ok".into())).as_deref(), Some("ok"));
        let long = "x".repeat(MAX_CONTEXT_FIELD_BYTES + 40);
        let clamped = clamp_field(Some(long)).unwrap();
        assert!(clamped.len() <= MAX_CONTEXT_FIELD_BYTES + "…".len());
        assert!(clamped.ends_with('…'));
    }

    #[test]
    fn format_delivery_names_source_and_sections() {
        let env = envelope(Some("done planner"), Some("implement the plan"));
        let text = format_delivery(&env, "Planner", Some("line one\nline two"));
        assert!(text.contains("From session"));
        assert!(text.contains("Planner"));
        assert!(text.contains("Summary: done planner"));
        assert!(text.contains("Instructions:\nimplement the plan"));
        assert!(text.contains("Cited transcript:"));
        assert!(text.contains("line one"));
        assert!(text.contains("--- end forge context ---"));
    }
}
