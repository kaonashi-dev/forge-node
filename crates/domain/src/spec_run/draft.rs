//! What a drafted spec must declare in its `forge.spec/v1` block, and the warnings G1 shows
//! beside it. [`check_spec`] is for a spec being saved; an approved one is re-read with
//! [`super::schema::parse_spec`] alone, so a run approved before the block existed still reads.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::model::CriteriaDiff;
use super::schema::{find_block, parse_spec, Criterion, SpecDoc};
use crate::Hypothesis;

pub const SPEC_SCHEMA: &str = "forge.spec/v1";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpecKind {
    Bug,
    Feature,
    Change,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Rating {
    Clear,
    Partial,
    Missing,
}

impl Rating {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Clear => "Clear",
            Self::Partial => "Partial",
            Self::Missing => "Missing",
        }
    }
}

/// Every field is required: a category left out is refused like one rated Missing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Readiness {
    pub scope: Rating,
    pub data: Rating,
    pub user_flow: Rating,
    pub performance_security: Rating,
    pub integrations: Rating,
    pub edge_cases: Rating,
    pub terminology: Rating,
    pub done_signal: Rating,
}

impl Readiness {
    #[must_use]
    pub fn categories(&self) -> [(&'static str, Rating); 8] {
        [
            ("scope", self.scope),
            ("data", self.data),
            ("user flow", self.user_flow),
            ("performance and security", self.performance_security),
            ("integrations", self.integrations),
            ("edge cases", self.edge_cases),
            ("terminology", self.terminology),
            ("done signal", self.done_signal),
        ]
    }
}

/// How a bug is reached: by a command or test alone, by a sequence of user steps, or only with
/// crafted input (a hostile repository, a malformed frame).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reachability {
    Automatic,
    Sequence,
    Adversarial,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reproduction {
    #[serde(default)]
    pub reachability: Option<Reachability>,
    #[serde(default)]
    pub trigger: String,
    #[serde(default)]
    pub command: String,
}

/// A review hypothesis family the drafter weighed: covered by criteria, or out of scope.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Risk {
    pub hypothesis: String,
    #[serde(default)]
    pub criteria: Vec<String>,
    #[serde(default)]
    pub out_of_scope: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpecBlock {
    pub kind: SpecKind,
    pub readiness: Readiness,
    #[serde(default)]
    pub reproduction: Option<Reproduction>,
    #[serde(default)]
    pub risks: Vec<Risk>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckedSpec {
    pub doc: SpecDoc,
    pub block: SpecBlock,
    pub warnings: Vec<String>,
}

/// The spec without its fenced `forge.spec*` JSON blocks: the block follows the last section, so a
/// section cut from the text would otherwise carry it.
#[must_use]
pub fn prose(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut fence: Vec<&str> = Vec::new();
    let mut open = false;
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            if open {
                fence.push(line);
                if !fence.iter().any(|l| l.contains("\"forge.spec")) {
                    fence.iter().for_each(|l| push_line(&mut out, l));
                }
                fence.clear();
            } else {
                fence.push(line);
            }
            open = !open;
            continue;
        }
        if open {
            fence.push(line);
        } else {
            push_line(&mut out, line);
        }
    }
    fence.iter().for_each(|l| push_line(&mut out, l));
    out
}

fn push_line(out: &mut String, line: &str) {
    out.push_str(line);
    out.push('\n');
}

/// The `forge.spec/v1` block of a spec, for display; it was checked when the spec was saved.
#[must_use]
pub fn spec_block(text: &str) -> Option<SpecBlock> {
    serde_json::from_value(find_block(text, SPEC_SCHEMA)?).ok()
}

/// # Errors
/// Every reason [`parse_spec`] refuses, plus: no `forge.spec/v1` block, a readiness category left
/// out or rated Missing, a bug without a reproduction that names its reachability.
pub fn check_spec(text: &str) -> Result<CheckedSpec, Vec<String>> {
    let parsed = parse_spec(text);
    let block = block_of(text);
    let (doc, block) = match (parsed, block) {
        (Ok(doc), Ok(block)) => (doc, block),
        (parsed, block) => {
            let mut errors = parsed.err().unwrap_or_default();
            errors.extend(block.err().unwrap_or_default());
            return Err(errors);
        }
    };
    let mut warnings = hedges(&doc.criteria);
    if let Some(reproduction) = &block.reproduction {
        if reproduction.trigger.trim().is_empty() {
            warnings.push("the reproduction names no trigger".to_owned());
        }
        if reproduction.command.trim().is_empty() {
            warnings.push("the reproduction names no command".to_owned());
        }
    }
    warnings.extend(risk_warnings(&block.risks, &doc));
    Ok(CheckedSpec {
        doc,
        block,
        warnings,
    })
}

fn block_of(text: &str) -> Result<SpecBlock, Vec<String>> {
    let value = find_block(text, SPEC_SCHEMA).ok_or_else(|| {
        vec![format!(
            "the spec has no JSON block with \"schema\": \"{SPEC_SCHEMA}\" (kind and readiness)"
        )]
    })?;
    let block: SpecBlock =
        serde_json::from_value(value).map_err(|e| vec![format!("{SPEC_SCHEMA}: {e}")])?;
    let mut errors: Vec<String> = block
        .readiness
        .categories()
        .into_iter()
        .filter(|(_, rating)| *rating == Rating::Missing)
        .map(|(name, _)| format!("readiness: {name} is Missing; resolve it before saving"))
        .collect();
    if block.kind == SpecKind::Bug
        && block
            .reproduction
            .as_ref()
            .is_none_or(|r| r.reachability.is_none())
    {
        errors.push(
            "a bug needs a reproduction with its reachability: automatic, sequence or adversarial"
                .to_owned(),
        );
    }
    if errors.is_empty() {
        Ok(block)
    } else {
        Err(errors)
    }
}

const HEDGES: [&str; 16] = [
    "might",
    "could",
    "should",
    "may",
    "maybe",
    "perhaps",
    "possibly",
    "ideally",
    "if possible",
    "where appropriate",
    "as needed",
    "podría",
    "debería",
    "quizás",
    "tal vez",
    "si es posible",
];

/// A criterion that hedges cannot fail a check; one without SHALL is not in the EARS form.
fn hedges(criteria: &[Criterion]) -> Vec<String> {
    let mut warnings = Vec::new();
    for criterion in criteria {
        let lower = criterion.text.to_lowercase();
        let words: Vec<&str> = lower
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| !w.is_empty())
            .collect();
        let shall = words.iter().position(|w| matches!(*w, "shall" | "deberá"));
        // "WHEN the config could not be parsed" is a condition, not a hedge.
        let behaviour = &words[shall.map_or(0, |at| at + 1)..];
        let found: Vec<&str> = HEDGES
            .iter()
            .copied()
            .filter(|hedge| {
                let parts: Vec<&str> = hedge.split(' ').collect();
                behaviour.windows(parts.len()).any(|window| window == parts)
            })
            .collect();
        if !found.is_empty() {
            warnings.push(format!(
                "{} hedges ({}): state what THE SYSTEM SHALL do",
                criterion.id,
                found.join(", ")
            ));
        }
        if shall.is_none() {
            warnings.push(format!(
                "{} has no SHALL: write it as WHEN <event> THE SYSTEM SHALL <behaviour>",
                criterion.id
            ));
        }
    }
    warnings
}

fn risk_warnings(risks: &[Risk], doc: &SpecDoc) -> Vec<String> {
    let mut warnings = Vec::new();
    for risk in risks {
        let name = risk.hypothesis.as_str();
        if Hypothesis::from_name(name).is_none() {
            warnings.push(format!("risk {name} is not a review hypothesis"));
        }
        for id in &risk.criteria {
            if !doc.has(id) {
                warnings.push(format!(
                    "risk {name} names {id}, which the spec does not have"
                ));
            }
        }
        let excused = risk
            .out_of_scope
            .as_deref()
            .is_some_and(|why| !why.trim().is_empty());
        if risk.criteria.is_empty() && !excused {
            warnings.push(format!(
                "risk {name} is neither covered by a criterion nor declared out of scope"
            ));
        }
    }
    warnings
}

/// References past this many in "Code context" are not checked.
pub const MAX_CODE_REFS: usize = 64;

/// `path:line` or `path:start-end` as the spec cites it; `line` is the last line it needs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodeRef {
    pub path: String,
    pub line: u32,
}

/// The `path:line` references of the "Code context" section, deduplicated, at most
/// [`MAX_CODE_REFS`].
#[must_use]
pub fn code_context_refs(text: &str) -> Vec<CodeRef> {
    let mut refs: Vec<CodeRef> = Vec::new();
    let mut inside = false;
    for line in text.lines() {
        if let Some(heading) = line.strip_prefix("## ") {
            let heading = heading.trim().to_lowercase();
            inside =
                heading.starts_with("code context") || heading.starts_with("contexto de código");
            continue;
        }
        if line.starts_with("# ") {
            inside = false;
        }
        if !inside {
            continue;
        }
        let tokens = line.split(|c: char| c.is_whitespace() || "`()[],;\"'".contains(c));
        for token in tokens {
            let Some(found) = code_ref(token.trim_end_matches(['.', ':'])) else {
                continue;
            };
            if !refs.contains(&found) {
                refs.push(found);
            }
            if refs.len() == MAX_CODE_REFS {
                return refs;
            }
        }
    }
    refs
}

fn code_ref(token: &str) -> Option<CodeRef> {
    let (path, lines) = token.rsplit_once(':')?;
    let (path, lines) = match path.rsplit_once(':') {
        Some((file, line)) if line.parse::<u32>().is_ok() => (file, line),
        _ => (path, lines),
    };
    let end = lines.rsplit('-').next()?;
    let line: u32 = end.parse().ok()?;
    let first = lines.split('-').next()?;
    let looks_like_path = path.contains('/') || path.contains('.') || is_root_file_name(path);
    (looks_like_path && line > 0 && first.parse::<u32>().is_ok() && !path.contains("://")).then(
        || CodeRef {
            path: path.to_owned(),
            line,
        },
    )
}

/// `Makefile`, `README`, `Dockerfile`: capitalized so that prose such as `ratio 3:2` or
/// `step:1` is not taken for a file.
fn is_root_file_name(name: &str) -> bool {
    name.as_bytes().first().is_some_and(u8::is_ascii_uppercase)
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// Words `/bin/sh` runs itself, which no `PATH` lookup finds.
const SHELL_BUILTINS: [&str; 24] = [
    "test", "[", "[[", "true", "false", "cd", "echo", "printf", "exit", "export", "set", "command",
    "!", ":", ".", "if", "for", "while", "until", "case", "exec", "eval", "source", "time",
];

/// Each automatic validation's program: the first word after any `NAME=value`, `None` for a
/// shell builtin. A command that opens with a subshell or a quote is left unchecked.
#[must_use]
pub fn validation_programs(doc: &SpecDoc) -> Vec<(String, String)> {
    doc.criteria
        .iter()
        .filter_map(|criterion| {
            let command = criterion.automatic.as_deref()?;
            let program = shell_words(command).find(|word| !is_assignment(word))?;
            let plain = !program.starts_with(['(', '{', '\'', '"', '$', '~']);
            (plain && !SHELL_BUILTINS.contains(&program))
                .then(|| (criterion.id.clone(), program.to_owned()))
        })
        .collect()
}

/// Words split on whitespace outside quotes, the quotes kept: `LABEL="a b" cargo` is two words.
fn shell_words(command: &str) -> impl Iterator<Item = &str> {
    let mut rest = command.trim_start();
    std::iter::from_fn(move || {
        if rest.is_empty() {
            return None;
        }
        let mut quote: Option<char> = None;
        let end = rest
            .char_indices()
            .find(|&(_, c)| match quote {
                Some(open) if c == open => {
                    quote = None;
                    false
                }
                Some(_) => false,
                None if c == '\'' || c == '"' => {
                    quote = Some(c);
                    false
                }
                None => c.is_whitespace(),
            })
            .map_or(rest.len(), |(at, _)| at);
        let word = &rest[..end];
        rest = rest[end..].trim_start();
        Some(word)
    })
}

fn is_assignment(word: &str) -> bool {
    word.split_once('=').is_some_and(|(name, _)| {
        !name.is_empty()
            && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
            && !name.as_bytes()[0].is_ascii_digit()
    })
}

/// The criteria `new` adds, removes or rewords against `old`, by id.
#[must_use]
pub fn criteria_diff(old: &SpecDoc, new: &SpecDoc) -> CriteriaDiff {
    let before: BTreeMap<&str, &Criterion> =
        old.criteria.iter().map(|c| (c.id.as_str(), c)).collect();
    let after: BTreeMap<&str, &Criterion> =
        new.criteria.iter().map(|c| (c.id.as_str(), c)).collect();
    CriteriaDiff {
        added: after
            .keys()
            .filter(|id| !before.contains_key(*id))
            .map(|id| (*id).to_owned())
            .collect(),
        removed: before
            .keys()
            .filter(|id| !after.contains_key(*id))
            .map(|id| (*id).to_owned())
            .collect(),
        changed: after
            .iter()
            .filter(|(id, c)| before.get(*id).is_some_and(|old| old != *c))
            .map(|(id, _)| (*id).to_owned())
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const READY: &str =
        "\"readiness\": {\"scope\": \"clear\", \"data\": \"clear\", \"user_flow\": \"partial\", \
\"performance_security\": \"clear\", \"integrations\": \"clear\", \"edge_cases\": \"partial\", \
\"terminology\": \"clear\", \"done_signal\": \"clear\"}";

    fn spec(criteria: &str, block: &str) -> String {
        format!(
            "# Loader cache\nRoute: standard\n\n## Acceptance criteria\n{criteria}\n\n```json\n\
{{\"schema\": \"forge.spec/v1\", {block}}}\n```\n"
        )
    }

    const AC: &str = "- AC-1: WHEN the loader runs twice THE SYSTEM SHALL read the disk once\n  \
- Automatic validation: `cargo test -p loader cache_hit -- --exact` → exit 0";

    #[test]
    fn a_feature_with_every_category_rated_is_accepted_without_warnings() {
        let text = spec(AC, &format!("\"kind\": \"feature\", {READY}"));
        let checked = check_spec(&text).unwrap();
        assert_eq!(checked.block.kind, SpecKind::Feature);
        assert_eq!(checked.block.readiness.user_flow, Rating::Partial);
        assert!(checked.warnings.is_empty(), "{:?}", checked.warnings);
        assert_eq!(spec_block(&text), Some(checked.block));
    }

    #[test]
    fn a_missing_category_or_block_is_refused_with_the_spec_errors() {
        let missing = READY.replace("\"data\": \"clear\"", "\"data\": \"missing\"");
        let errors =
            check_spec(&spec(AC, &format!("\"kind\": \"change\", {missing}"))).unwrap_err();
        assert_eq!(
            errors,
            ["readiness: data is Missing; resolve it before saving"]
        );

        let left_out = READY.replace("\"terminology\": \"clear\", ", "");
        let errors =
            check_spec(&spec(AC, &format!("\"kind\": \"change\", {left_out}"))).unwrap_err();
        assert!(
            errors[0].contains("missing field `terminology`"),
            "{errors:?}"
        );

        let errors = check_spec("# T\nJust prose.").unwrap_err();
        assert!(errors[0].contains("no acceptance criterion"), "{errors:?}");
        assert!(errors[1].contains("no JSON block"), "{errors:?}");
    }

    #[test]
    fn a_bug_needs_a_reproduction_that_names_its_reachability() {
        let bare = spec(AC, &format!("\"kind\": \"bug\", {READY}"));
        assert!(check_spec(&bare).unwrap_err()[0].contains("reachability"));
        let untyped = spec(
            AC,
            &format!("\"kind\": \"bug\", {READY}, \"reproduction\": {{\"trigger\": \"twice\"}}"),
        );
        assert!(check_spec(&untyped).is_err());

        let typed = spec(
            AC,
            &format!(
                "\"kind\": \"bug\", {READY}, \"reproduction\": {{\"reachability\": \"sequence\", \"trigger\": \"open twice\"}}"
            ),
        );
        let checked = check_spec(&typed).unwrap();
        assert_eq!(
            checked.block.reproduction.unwrap().reachability,
            Some(Reachability::Sequence)
        );
        assert_eq!(checked.warnings, ["the reproduction names no command"]);
    }

    #[test]
    fn hedged_criteria_and_loose_risks_are_warnings_not_errors() {
        let criteria = "- AC-1: The loader might cache reads; we should consider a TTL\n  \
- Manual validation: look\n- AC-2: WHEN asked THE SYSTEM SHALL answer, si es posible\n  \
- Manual validation: ask\n- AC-3: WHEN the config could not be parsed THE SYSTEM SHALL name the line\n  \
- Manual validation: break it";
        let risks = "\"risks\": [{\"hypothesis\": \"RaceCondition\", \"criteria\": [\"AC-9\"]}, \
{\"hypothesis\": \"MigrationGap\"}, {\"hypothesis\": \"Gremlins\", \"out_of_scope\": \"none\"}, \
{\"hypothesis\": \"MemoryDiskDrift\", \"out_of_scope\": \"no cache survives a restart\"}]";
        let checked = check_spec(&spec(
            criteria,
            &format!("\"kind\": \"feature\", {READY}, {risks}"),
        ))
        .unwrap();
        assert_eq!(
            checked.warnings,
            [
                "AC-1 hedges (might, should): state what THE SYSTEM SHALL do",
                "AC-1 has no SHALL: write it as WHEN <event> THE SYSTEM SHALL <behaviour>",
                "AC-2 hedges (si es posible): state what THE SYSTEM SHALL do",
                "risk RaceCondition names AC-9, which the spec does not have",
                "risk MigrationGap is neither covered by a criterion nor declared out of scope",
                "risk Gremlins is not a review hypothesis",
            ]
        );
    }

    #[test]
    fn prose_drops_only_the_forge_blocks() {
        let text = "## Out of scope\n- Windows\n\n```sh\ncargo test\n```\n\n```json\n\
{\"schema\":\"forge.spec/v1\", \"kind\": \"bug\"}\n```\n";
        assert_eq!(
            prose(text),
            "## Out of scope\n- Windows\n\n```sh\ncargo test\n```\n\n"
        );
        assert_eq!(prose("```\nunclosed"), "```\nunclosed\n");
    }

    #[test]
    fn code_context_refs_come_only_from_their_section() {
        let text = "# T\n\n## Problem\nSee src/elsewhere.rs:9.\n\n## Code context\n\
- `crates/daemon/src/spec_runs.rs:687` saves the spec (also crates/daemon/src/spec_runs.rs:687).\n\
- docs/spec-runs.md:46-60, the checks; https://example.com:443 is not a path; ratio 3:2\n\
- src/lib.rs:10:5 builds it\n\
- Makefile:200 runs the gate; step:1 is prose\n\
\n## Assumptions\n- src/after.rs:1\n";
        assert_eq!(
            code_context_refs(text),
            [
                CodeRef {
                    path: "crates/daemon/src/spec_runs.rs".into(),
                    line: 687
                },
                CodeRef {
                    path: "docs/spec-runs.md".into(),
                    line: 60
                },
                CodeRef {
                    path: "src/lib.rs".into(),
                    line: 10
                },
                CodeRef {
                    path: "Makefile".into(),
                    line: 200
                },
            ]
        );
    }

    #[test]
    fn a_validations_program_skips_assignments_and_builtins() {
        let doc = parse_spec(
            "- AC-1: WHEN a THE SYSTEM SHALL b\n  - Automatic validation: `RUST_LOG=debug cargo test -p x` → 0\n\
- AC-2: WHEN a THE SYSTEM SHALL b\n  - Automatic validation: `test -f done.txt` → 0\n\
- AC-3: WHEN a THE SYSTEM SHALL b\n  - Automatic validation: `scripts/dev check` → 0\n\
- AC-4: WHEN a THE SYSTEM SHALL b\n  - Automatic validation: `(cd x && make)` → 0\n\
- AC-5: WHEN a THE SYSTEM SHALL b\n  - Manual validation: look\n\
- AC-6: WHEN a THE SYSTEM SHALL b\n  - Automatic validation: `if cargo test; then exit 0; fi` → 0\n\
- AC-7: WHEN a THE SYSTEM SHALL b\n  - Automatic validation: `~/bin/tool --check` → 0\n\
- AC-8: WHEN a THE SYSTEM SHALL b\n  - Automatic validation: `LABEL=\"cache test\" X='a b' bun test` → 0",
        )
        .unwrap();
        assert_eq!(
            validation_programs(&doc),
            [
                ("AC-1".to_owned(), "cargo".to_owned()),
                ("AC-3".to_owned(), "scripts/dev".to_owned()),
                ("AC-8".to_owned(), "bun".to_owned()),
            ]
        );
    }

    #[test]
    fn the_criteria_diff_names_added_removed_and_reworded_ids() {
        let old = parse_spec(&format!(
            "{AC}\n- AC-2: WHEN x THE SYSTEM SHALL y\n  - Manual validation: look\n\
- AC-3: WHEN z THE SYSTEM SHALL w\n  - Manual validation: look"
        ))
        .unwrap();
        let new = parse_spec(&format!(
            "{AC}\n- AC-2: WHEN x THE SYSTEM SHALL y and z\n  - Manual validation: look\n\
- AC-4: WHEN v THE SYSTEM SHALL u\n  - Manual validation: look"
        ))
        .unwrap();
        assert_eq!(
            criteria_diff(&old, &new),
            CriteriaDiff {
                added: vec!["AC-4".into()],
                removed: vec!["AC-3".into()],
                changed: vec!["AC-2".into()],
            }
        );
        assert!(criteria_diff(&new, &new).is_empty());

        let steps = |manual: &str, expected: &str| {
            parse_spec(&format!(
                "- AC-1: WHEN a THE SYSTEM SHALL b\n  - Automatic validation: `true` → {expected}\n\
- AC-2: WHEN c THE SYSTEM SHALL d\n  - Manual validation:\n    1. {manual}\n"
            ))
            .unwrap()
        };
        assert_eq!(
            criteria_diff(
                &steps("open it", "exit 0"),
                &steps("open it twice", "exit 1")
            )
            .changed,
            ["AC-1", "AC-2"]
        );
    }
}
