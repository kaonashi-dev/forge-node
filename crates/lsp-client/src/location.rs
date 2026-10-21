use serde_json::Value;

use crate::uri::uri_to_path;
use crate::Location;

/// Most locations one definition answer yields.
pub const MAX_LOCATIONS: usize = 64;

/// `Location | Location[] | LocationLink[] | null`, as the start of each
/// target; a link contributes its selection range, and a non-`file:` target
/// is skipped.
pub(crate) fn definition_locations(result: &Value) -> Vec<Location> {
    match result {
        Value::Null => Vec::new(),
        Value::Array(items) => items
            .iter()
            .filter_map(location)
            .take(MAX_LOCATIONS)
            .collect(),
        single => location(single).into_iter().collect(),
    }
}

fn location(value: &Value) -> Option<Location> {
    let (uri, range) = match value.get("targetUri") {
        Some(uri) => (
            uri,
            value
                .get("targetSelectionRange")
                .or_else(|| value.get("targetRange"))?,
        ),
        None => (value.get("uri")?, value.get("range")?),
    };
    let start = range.get("start")?;
    Some(Location {
        path: uri_to_path(uri.as_str()?)?,
        line: u32::try_from(start.get("line")?.as_u64()?).ok()?,
        character: u32::try_from(start.get("character")?.as_u64()?).ok()?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::PathBuf;

    fn at(path: &str, line: u32, character: u32) -> Location {
        Location {
            path: PathBuf::from(path),
            line,
            character,
        }
    }

    fn range(line: u32, character: u32) -> Value {
        json!({"start": {"line": line, "character": character}, "end": {"line": line, "character": character + 1}})
    }

    #[test]
    fn every_result_shape_normalises() {
        assert!(definition_locations(&Value::Null).is_empty());
        assert_eq!(
            definition_locations(&json!({"uri": "file:///a.rs", "range": range(1, 2)})),
            vec![at("/a.rs", 1, 2)]
        );
        assert_eq!(
            definition_locations(&json!([
                {"uri": "file:///a.rs", "range": range(1, 2)},
                {"uri": "jdt://contents/x.class", "range": range(0, 0)},
                {"uri": "file:///b%20c.rs", "range": range(3, 4)},
            ])),
            vec![at("/a.rs", 1, 2), at("/b c.rs", 3, 4)]
        );
        assert_eq!(
            definition_locations(&json!([{
                "originSelectionRange": range(9, 9),
                "targetUri": "file:///l.rs",
                "targetRange": range(10, 0),
                "targetSelectionRange": range(12, 5),
            }])),
            vec![at("/l.rs", 12, 5)]
        );
    }

    #[test]
    fn results_are_capped() {
        let many: Vec<Value> = (0..200)
            .map(|n| json!({"uri": "file:///a.rs", "range": range(n, 0)}))
            .collect();
        assert_eq!(
            definition_locations(&Value::Array(many)).len(),
            MAX_LOCATIONS
        );
    }

    #[test]
    fn malformed_entries_are_dropped() {
        assert!(definition_locations(&json!([
            {"uri": "file:///a.rs"},
            {"uri": 7, "range": range(0, 0)},
            {"uri": "file:///a.rs", "range": {"start": {"line": -1, "character": 0}}},
            "nonsense",
        ]))
        .is_empty());
    }
}
