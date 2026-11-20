use domain::Severity;
use serde_json::{Map, Value};

use crate::clip;
use crate::validate::{MAX_FINDINGS, MAX_SUGGESTION_BYTES, MAX_TEXT_BYTES};

pub const SCHEMA: &str = "forge.review/v1";
pub const SCHEMA_V2: &str = "forge.review/v2";

const MAX_SUMMARY_BYTES: usize = 16 * 1024;
const MAX_PATTERNS: usize = 10;
const MAX_PATTERN_BYTES: usize = 200;
const MAX_COVERAGE_NOTES: usize = 20;
const MAX_NOTE_BYTES: usize = 500;
const MAX_KB_REF_BYTES: usize = 200;

#[derive(Clone, Debug, PartialEq)]
pub struct ReviewOutput {
    pub schema: domain::SchemaVersion,
    pub summary: String,
    pub findings: Vec<RawFinding>,
    /// A v1 answer's mermaid diagram, fenced or not; a v2 answer's `diagrams` are ignored.
    pub diagram: Option<String>,
    /// Conventions the reviewer confirmed, for the knowledge base.
    pub patterns: Vec<String>,
    pub coverage_notes: Vec<String>,
}

/// A finding as the model wrote it, strings already capped.
#[derive(Clone, Debug, PartialEq)]
pub struct RawFinding {
    pub path: String,
    pub line: Option<u32>,
    pub end_line: Option<u32>,
    pub severity: Severity,
    pub category: String,
    pub rule_id: Option<String>,
    pub confidence: f32,
    pub title: String,
    pub body: String,
    pub evidence: String,
    pub suggestion: Option<String>,
    /// The hypothesis the model says it hunted; the daemon keeps its agent's own.
    pub hypothesis: Option<String>,
    pub kb_ref: Option<String>,
}

/// The first JSON object in `text` that carries a `schema` key, fences and prose ignored.
///
/// # Errors
/// No such object, or one whose schema is neither [`SCHEMA`] nor [`SCHEMA_V2`].
pub fn parse_output(text: &str) -> Result<ReviewOutput, String> {
    let object = text
        .match_indices('{')
        .find_map(|(at, _)| {
            let mut stream = serde_json::Deserializer::from_str(&text[at..]).into_iter::<Value>();
            match stream.next() {
                Some(Ok(Value::Object(map))) if map.contains_key("schema") => Some(map),
                _ => None,
            }
        })
        .ok_or_else(|| "no JSON object with a `schema` key in the reviewer's answer".to_owned())?;
    let schema = match object.get("schema").and_then(Value::as_str).unwrap_or("") {
        SCHEMA => domain::SchemaVersion::V1,
        SCHEMA_V2 => domain::SchemaVersion::V2,
        other => {
            return Err(format!(
                "unexpected schema `{other}`, expected `{SCHEMA}` or `{SCHEMA_V2}`"
            ))
        }
    };
    let strings = |key: &str, count: usize, max: usize| -> Vec<String> {
        object
            .get(key)
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| {
                        item.as_str()
                            .or_else(|| item.get("note").and_then(Value::as_str))
                    })
                    .map(str::trim)
                    .filter(|text| !text.is_empty())
                    .map(|text| clip(text, max).to_owned())
                    .take(count)
                    .collect()
            })
            .unwrap_or_default()
    };
    let v2 = schema == domain::SchemaVersion::V2;
    let findings = object
        .get("findings")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_object)
                .filter_map(raw_finding)
                .take(MAX_FINDINGS)
                .collect()
        })
        .unwrap_or_default();
    Ok(ReviewOutput {
        schema,
        summary: text_field(&object, "summary", MAX_SUMMARY_BYTES).unwrap_or_default(),
        findings,
        diagram: if v2 {
            None
        } else {
            text_field(&object, "diagram", MAX_SUGGESTION_BYTES)
        },
        patterns: if v2 {
            strings("patterns", MAX_PATTERNS, MAX_PATTERN_BYTES)
        } else {
            Vec::new()
        },
        coverage_notes: if v2 {
            strings("coverage_notes", MAX_COVERAGE_NOTES, MAX_NOTE_BYTES)
        } else {
            Vec::new()
        },
    })
}

/// `None` for a finding without a path or a recognisable severity.
fn raw_finding(map: &Map<String, Value>) -> Option<RawFinding> {
    let severity = match map.get("severity")?.as_str()?.trim() {
        "P0" | "p0" => Severity::P0,
        "P1" | "p1" => Severity::P1,
        "P2" | "p2" => Severity::P2,
        _ => return None,
    };
    let number = |key| {
        map.get(key)
            .and_then(Value::as_u64)
            .and_then(|n| u32::try_from(n).ok())
            .filter(|n| *n > 0)
    };
    Some(RawFinding {
        path: text_field(map, "path", MAX_TEXT_BYTES)?,
        line: number("line"),
        end_line: number("end_line"),
        severity,
        category: text_field(map, "category", 64).unwrap_or_else(|| "general".to_owned()),
        rule_id: text_field(map, "rule_id", 64),
        #[allow(
            clippy::cast_possible_truncation,
            reason = "a confidence is clamped to 0..=1 first"
        )]
        confidence: map
            .get("confidence")
            .and_then(Value::as_f64)
            .unwrap_or(0.5)
            .clamp(0.0, 1.0) as f32,
        title: text_field(map, "title", MAX_TEXT_BYTES).unwrap_or_default(),
        body: text_field(map, "body", MAX_TEXT_BYTES).unwrap_or_default(),
        evidence: text_field(map, "evidence", MAX_TEXT_BYTES).unwrap_or_default(),
        suggestion: text_field(map, "suggestion", MAX_SUGGESTION_BYTES),
        hypothesis: text_field(map, "hypothesis", 64),
        kb_ref: text_field(map, "kb_ref", MAX_KB_REF_BYTES),
    })
}

fn text_field(map: &Map<String, Value>, key: &str, max: usize) -> Option<String> {
    let text = map.get(key)?.as_str()?.trim();
    (!text.is_empty()).then(|| clip(text, max).to_owned())
}
