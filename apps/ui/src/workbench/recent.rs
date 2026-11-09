//! Files opened lately, per checkout, in the `app_state` key Tauri writes, so
//! the two clients share one list. What ⌘P shows before anything is typed.

use std::collections::BTreeMap;

pub const RECENT_FILES_KEY: &str = "ui.files.recent";
/// Larger than what the palette opens on, so a file opened a while ago still
/// ranks once its name is typed.
pub const RECENT_MEMORY: usize = 50;
pub const RECENT_FILE_LIMIT: usize = 30;

/// Checkout id to its paths, most recent first.
pub type RecentFiles = BTreeMap<String, Vec<String>>;

/// Anything unexpected reads as nothing remembered: a preference written by
/// another build must degrade to an empty list, never an error.
#[must_use]
pub fn parse(raw: Option<&str>) -> RecentFiles {
    let Some(serde_json::Value::Object(map)) = raw.and_then(|raw| serde_json::from_str(raw).ok())
    else {
        return RecentFiles::new();
    };
    map.into_iter()
        .filter_map(|(workspace, paths)| {
            let serde_json::Value::Array(paths) = paths else {
                return None;
            };
            let clean: Vec<String> = paths
                .into_iter()
                .filter_map(|path| match path {
                    serde_json::Value::String(path) => Some(path),
                    _ => None,
                })
                .take(RECENT_MEMORY)
                .collect();
            (!clean.is_empty()).then_some((workspace, clean))
        })
        .collect()
}

/// The lists with `path` moved to the front of `workspace`'s, or `None` when
/// it already leads and there is nothing to write.
#[must_use]
pub fn with_opened(mut current: RecentFiles, workspace: &str, path: &str) -> Option<RecentFiles> {
    let list = current.entry(workspace.to_string()).or_default();
    if list.first().map(String::as_str) == Some(path) {
        return None;
    }
    list.retain(|item| item != path);
    list.insert(0, path.to_string());
    list.truncate(RECENT_MEMORY);
    Some(current)
}

/// Recently opened first, then what has uncommitted changes: in a checkout
/// just switched to, nothing has been opened yet but an agent has written.
#[must_use]
pub fn initial_files<'a>(
    recent: impl IntoIterator<Item = &'a str>,
    changed: impl IntoIterator<Item = &'a str>,
    limit: usize,
) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for path in recent.into_iter().chain(changed) {
        if out.len() >= limit {
            break;
        }
        if !out.iter().any(|seen| seen == path) {
            out.push(path.to_string());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_malformed_preference_reads_as_nothing() {
        assert!(parse(None).is_empty());
        assert!(parse(Some("not json")).is_empty());
        assert!(parse(Some("[1,2]")).is_empty());
        let parsed = parse(Some(r#"{"w1":["a.rs",3,"b.rs"],"w2":"x","w3":[]}"#));
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed["w1"], vec!["a.rs".to_string(), "b.rs".to_string()]);
    }

    #[test]
    fn opening_moves_a_path_to_the_front_once() {
        let first = with_opened(RecentFiles::new(), "w", "a.rs").expect("written");
        let second = with_opened(first, "w", "b.rs").expect("written");
        let again = with_opened(second.clone(), "w", "a.rs").expect("written");
        assert_eq!(again["w"], vec!["a.rs".to_string(), "b.rs".to_string()]);
        assert!(with_opened(again, "w", "a.rs").is_none(), "already first");
        let mut full = RecentFiles::new();
        full.insert(
            "w".into(),
            (0..RECENT_MEMORY).map(|i| i.to_string()).collect(),
        );
        let full = with_opened(full, "w", "new").expect("written");
        assert_eq!(full["w"].len(), RECENT_MEMORY);
    }

    #[test]
    fn the_opening_list_is_recent_then_changed_without_repeats() {
        assert_eq!(
            initial_files(["b.rs", "a.rs"], ["a.rs", "c.rs", "d.rs"], 3),
            vec!["b.rs", "a.rs", "c.rs"]
        );
    }
}
