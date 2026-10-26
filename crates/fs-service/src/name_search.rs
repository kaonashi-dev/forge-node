//! Name ranking for the navigation index.
//!
//! `neo_frizbee` is a SIMD fuzzy matcher. Basename and depth bonuses stay ours so
//! a query without `/` still prefers `entries.ts` over a hit in a folder named
//! `entries`.

use neo_frizbee::{CaseMatching, Config, Matcher, SortStrategy};

use crate::{EntryKind, FileEntry, FileTree, SearchMatch, SearchResults, MAX_SEARCH_RESULTS};

/// Enough to lift a basename hit over a longer path that also matched.
const BASENAME_BONUS: u32 = 1_000;
const EXACT_BONUS: u32 = 500;

/// Rank files in `tree` for `query`, best first, at most `limit` hits.
#[must_use]
pub fn search_names(tree: &FileTree, query: &str, limit: usize) -> SearchResults {
    let limit = limit.clamp(1, MAX_SEARCH_RESULTS);
    let needle = needle_of(query);
    let needle = needle.as_str();
    if needle.is_empty() {
        return SearchResults {
            matches: Vec::new(),
            truncated: tree.truncated,
        };
    }

    let files: Vec<&FileEntry> = tree
        .entries
        .iter()
        .filter(|entry| !entry.ignored && entry.kind == EntryKind::File)
        .collect();
    let haystacks: Vec<&str> = files.iter().map(|entry| entry.path.as_str()).collect();
    // `Smart` would make one capital in the query demand an exact case; the
    // matching-case bonus still ranks the exact spelling first under `Ignore`.
    let config = Config::default()
        .max_typos(Some(0))
        .casing(CaseMatching::Ignore)
        .sort(SortStrategy::IndexAsc);
    let hits = Matcher::new(needle, &config).match_list(&haystacks);

    let path_query = needle.contains('/');
    let mut scored: Vec<(u32, u32, &FileEntry)> = hits
        .into_iter()
        .filter_map(|hit| {
            let entry = *files.get(hit.index as usize)?;
            Some((
                name_score(
                    entry.path.as_str(),
                    needle,
                    hit.score,
                    hit.exact,
                    path_query,
                ),
                hit.index,
                entry,
            ))
        })
        .collect();
    let rank =
        |a: &(u32, u32, &FileEntry), b: &(u32, u32, &FileEntry)| b.0.cmp(&a.0).then(a.1.cmp(&b.1));
    let truncated = tree.truncated || scored.len() > limit;
    if scored.len() > limit {
        scored.select_nth_unstable_by(limit, rank);
        scored.truncate(limit);
    }
    scored.sort_unstable_by(rank);
    SearchResults {
        matches: scored
            .into_iter()
            .map(|(_, _, entry)| SearchMatch {
                text: entry.path.clone(),
                path: entry.path.clone(),
                line: 0,
                column: 0,
                before: Vec::new(),
                after: Vec::new(),
            })
            .collect(),
        truncated,
    }
}

/// The query as the matcher sees it: spaces dropped, as the palette's own
/// fuzzy does, so `app shell` is `appshell`; a pasted `./src/x` is `src/x`.
fn needle_of(query: &str) -> String {
    let trimmed = query.trim();
    let relative = trimmed.strip_prefix("./").unwrap_or(trimmed);
    relative.chars().filter(|c| !c.is_whitespace()).collect()
}

fn name_score(path: &str, needle: &str, match_score: u16, exact: bool, path_query: bool) -> u32 {
    let mut score = u32::from(match_score);
    if exact {
        score += EXACT_BONUS;
    }
    if path_query {
        return score;
    }
    let base = path.rsplit('/').next().unwrap_or(path);
    if same_without_spaces(base, needle) {
        score += BASENAME_BONUS + EXACT_BONUS;
    } else if subsequence(base, needle) {
        score += BASENAME_BONUS;
    }
    score.saturating_sub(slash_count(path))
}

/// `needle` has had its whitespace dropped, so `Release Notes.md` must too.
fn same_without_spaces(base: &str, needle: &str) -> bool {
    base.chars()
        .filter(|c| !c.is_whitespace())
        .map(|c| c.to_ascii_lowercase())
        .eq(needle.chars().map(|c| c.to_ascii_lowercase()))
}

fn slash_count(path: &str) -> u32 {
    path.bytes().filter(|byte| *byte == b'/').count() as u32
}

fn subsequence(haystack: &str, needle: &str) -> bool {
    let mut hay = haystack.chars().flat_map(char::to_lowercase);
    for wanted in needle.chars().flat_map(char::to_lowercase) {
        loop {
            match hay.next() {
                Some(got) if got == wanted => break,
                Some(_) => {}
                None => return false,
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FileEntry;

    fn tree(paths: &[&str]) -> FileTree {
        FileTree {
            entries: paths
                .iter()
                .map(|path| FileEntry {
                    path: (*path).to_owned(),
                    kind: EntryKind::File,
                    ignored: false,
                    symlink: None,
                })
                .collect(),
            truncated: false,
        }
    }

    #[test]
    fn prefers_an_exact_basename_over_a_longer_path_that_contains_it() {
        let hits = search_names(
            &tree(&[
                "src/palette/entries.test.ts",
                "src/palette/entries.ts",
                "docs/entries/overview.md",
            ]),
            "entries.ts",
            10,
        );
        assert_eq!(hits.matches[0].path, "src/palette/entries.ts");
    }

    #[test]
    fn still_matches_a_camel_hump_and_a_missing_underscore() {
        let hits = search_names(&tree(&["apps/tauri/src/app_shell.rs"]), "appshell", 10);
        assert_eq!(hits.matches.len(), 1);
    }

    #[test]
    fn prefers_camel_humps_and_consecutive_runs() {
        let camel = search_names(&tree(&["FooBar.rs", "offbeat.rs"]), "fb", 10);
        assert_eq!(camel.matches[0].path, "FooBar.rs");
        let run = search_names(&tree(&["zshell.rs", "zsxhxe.rs"]), "she", 10);
        assert_eq!(run.matches[0].path, "zshell.rs");
    }

    #[test]
    fn ignores_case_and_spaces_but_ranks_the_exact_spelling_first() {
        let paths = tree(&["src/appShell.ts", "src/AppShell.tsx", "src/other.ts"]);
        let hits = search_names(&paths, "AppShell", 10);
        let found: Vec<_> = hits.matches.iter().map(|m| m.path.as_str()).collect();
        assert_eq!(found, ["src/AppShell.tsx", "src/appShell.ts"]);

        let spaced = search_names(&paths, "app shell", 10);
        assert_eq!(spaced.matches.len(), 2);
        let dotted = search_names(&paths, "./SRC/appshell", 10);
        assert_eq!(dotted.matches.len(), 2);
    }

    #[test]
    fn a_basename_with_spaces_typed_exactly_earns_the_exact_bonus() {
        let needle = needle_of("Release Notes.md");
        assert_eq!(
            name_score("docs/Release Notes.md", &needle, 0, false, false),
            BASENAME_BONUS + EXACT_BONUS - 1
        );
    }

    #[test]
    fn an_empty_query_does_not_invent_hits() {
        let hits = search_names(&tree(&["a.rs"]), "  ", 10);
        assert!(hits.matches.is_empty());
    }

    #[test]
    fn limited_results_preserve_full_ranking_and_tie_order() {
        let paths: Vec<String> = (0..150)
            .map(|i| match i % 3 {
                0 => format!("src/{i:03}/entries.ts"),
                1 => format!("src/entries/{i:03}.ts"),
                _ => format!("entries/{i:03}/entries.test.ts"),
            })
            .collect();
        let mut tree = tree(&paths.iter().map(String::as_str).collect::<Vec<_>>());
        tree.entries[0].ignored = true;
        tree.entries[1].kind = EntryKind::Directory;

        for query in ["entries", "entries.ts", "src/entries", "missing"] {
            let full = search_names(&tree, query, MAX_SEARCH_RESULTS);
            assert!(!full.truncated);
            for limit in [1, 2, 17, 100, 149, MAX_SEARCH_RESULTS] {
                let limited = search_names(&tree, query, limit);
                assert_eq!(
                    limited.matches,
                    full.matches[..limit.min(full.matches.len())],
                    "{query}, limit={limit}"
                );
                assert_eq!(limited.truncated, full.matches.len() > limit);
            }
        }
        tree.truncated = true;
        assert!(search_names(&tree, "missing", MAX_SEARCH_RESULTS).truncated);
    }
}
