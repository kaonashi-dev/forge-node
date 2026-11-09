//! The palette's subsequence ranking, as this client:
//! a match at a word start or in a run scores higher, which is what puts
//! "New Terminal" ahead of "Agent Settings" for `nt`. File paths are ranked by
//! the daemon (`SearchFiles`), not here.

const WORD_START: i32 = 8;
const RUN: i32 = 4;
/// Characters of label length that cost one point.
const LENGTH_STEP: usize = 8;

/// The query as it is matched: trimmed, without a leading `./`, lowercased
/// and with whitespace removed, as `foldQuery`.
#[must_use]
pub fn needle_of(query: &str) -> Vec<char> {
    let trimmed = query.trim().to_lowercase();
    let relative = trimmed.strip_prefix("./").unwrap_or(&trimmed);
    relative.chars().filter(|ch| !ch.is_whitespace()).collect()
}

fn lower(ch: char) -> char {
    ch.to_lowercase().next().unwrap_or(ch)
}

fn is_upper(ch: char) -> bool {
    ch.is_uppercase() && ch.to_lowercase().next() != Some(ch)
}

/// `candidate`'s score against `needle`, or `None` when the needle's
/// characters do not all appear in it in order.
#[must_use]
pub fn score_with(needle: &[char], candidate: &str) -> Option<i32> {
    if needle.is_empty() {
        return Some(0);
    }
    let mut score = 0i32;
    let mut at = 0usize;
    let mut previous: Option<usize> = None;
    let mut before: Option<char> = None;
    let mut length = 0usize;
    for (index, ch) in candidate.chars().enumerate() {
        length += 1;
        if at < needle.len() && lower(ch) == needle[at] {
            score += 1;
            let starts_word = match before {
                None => true,
                Some(prev) => matches!(prev, ' ' | '-' | '/') || (is_upper(ch) && !is_upper(prev)),
            };
            if starts_word {
                score += WORD_START;
            }
            if previous.is_some_and(|last| last + 1 == index) {
                score += RUN;
            }
            previous = Some(index);
            at += 1;
        }
        before = Some(ch);
    }
    (at == needle.len()).then(|| score - (length / LENGTH_STEP) as i32)
}

/// The indices of `labels` that match `query`, best first. An empty query
/// keeps the presentation order (the palette opens on a menu, not on a
/// ranking), and ties keep it too, so a query matching a whole group does
/// not shuffle it on every keystroke.
#[must_use]
pub fn rank<'a>(labels: impl IntoIterator<Item = &'a str>, query: &str) -> Vec<usize> {
    let needle = needle_of(query);
    let mut scored: Vec<(i32, usize)> = labels
        .into_iter()
        .enumerate()
        .filter_map(|(index, label)| score_with(&needle, label).map(|value| (value, index)))
        .collect();
    if !needle.is_empty() {
        scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    }
    scored.into_iter().map(|(_, index)| index).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn score(query: &str, candidate: &str) -> Option<i32> {
        score_with(&needle_of(query), candidate)
    }

    #[test]
    fn every_query_character_must_appear_in_order() {
        assert!(score("nt", "New Terminal").is_some());
        assert_eq!(
            score("lt", "New Terminal"),
            None,
            "both letters, wrong order"
        );
        assert_eq!(score("xyz", "New Terminal"), None);
        assert_eq!(score("", "anything"), Some(0));
    }

    #[test]
    fn a_word_start_finds_new_terminal_ahead_of_agent_settings() {
        assert!(score("nt", "New Terminal") > score("nt", "Agent Settings"));
    }

    #[test]
    fn runs_and_short_labels_score_higher() {
        assert!(score("abcd", "abcd") > score("abcd", "axbxcxd"));
        assert!(score("git", "Git") > score("git", "Git Panel Configuration Screen"));
    }

    #[test]
    fn spaces_and_case_are_ignored_and_a_slash_starts_a_word() {
        assert_eq!(score("n t", "New Terminal"), score("NT", "New Terminal"));
        assert!(score("sf", "src/forge.ts") > score("sf", "srcforge.ts"));
        assert!(
            score("cp", "CommandPalette") > score("cp", "copyPath"),
            "camelCase initials"
        );
        assert_eq!(
            needle_of(" ./Src Main "),
            vec!['s', 'r', 'c', 'm', 'a', 'i', 'n']
        );
    }

    #[test]
    fn ranking_drops_misses_and_keeps_order_on_an_empty_query_and_ties() {
        let labels = ["New Terminal", "New Agent", "Toggle Sidebar", "Never Again"];
        assert_eq!(rank(labels, "  "), vec![0, 1, 2, 3]);
        assert_eq!(rank(labels, "sidebar"), vec![2]);
        assert_eq!(rank(["aa x", "aa y", "aa z"], "aa"), vec![0, 1, 2]);
        assert_eq!(rank(["Agent Settings", "New Terminal"], "nt"), vec![1, 0]);
    }
}
