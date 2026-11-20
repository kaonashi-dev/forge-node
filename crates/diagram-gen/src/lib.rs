//! Diagrams of a change, from its diff and the project's codebase graph.
//! Deterministic: the same diff and graph give the same diagrams, and no
//! model is asked. At most two, preferring sequence, ER, class, then flow.

use std::collections::BTreeSet;

use codebase_graph::{EdgeKind, Graph, Index, Node, NodeKind};
use domain::{Diagram, DiagramEdge, DiagramKind, DiagramNode};

const MAX_DIAGRAMS: usize = 2;
const MAX_PARTICIPANTS: usize = 8;
const MAX_MESSAGES: usize = 20;
const MAX_FLOW_BRANCHES: usize = 12;
const MIN_FLOW_BRANCHES: usize = 4;
const MAX_LABEL_CHARS: usize = 60;
const TABLE_SUFFIXES: [&str; 6] = ["Row", "Record", "Model", "Entity", "Table", "Schema"];

/// A changed file: its patch, and its new text when it could be read.
#[derive(Clone, Debug, Default)]
pub struct ChangedFile {
    pub path: String,
    pub patch: String,
    pub text: Option<String>,
}

#[must_use]
pub fn generate(files: &[ChangedFile], graph: &Graph) -> Vec<Diagram> {
    let index = graph.index();
    let change = Change::new(files, &index);
    if change.trivial() {
        return Vec::new();
    }
    if change.tests_only() {
        return change.flow().into_iter().collect();
    }
    [
        DiagramKind::Sequence,
        DiagramKind::Er,
        DiagramKind::Class,
        DiagramKind::Flow,
    ]
    .into_iter()
    .filter_map(|kind| change.diagram(kind))
    .take(MAX_DIAGRAMS)
    .collect()
}

/// One kind, whether or not it would have been among the two chosen.
#[must_use]
pub fn generate_kind(files: &[ChangedFile], graph: &Graph, kind: DiagramKind) -> Option<Diagram> {
    let index = graph.index();
    let change = Change::new(files, &index);
    if change.trivial() {
        return None;
    }
    change.diagram(kind)
}

struct Change<'a, 'g> {
    files: &'a [ChangedFile],
    index: &'a Index<'g>,
    /// Per file, its new-side hunks.
    hunks: Vec<Vec<(u32, u32)>>,
}

fn hunks(patch: &str) -> Vec<(u32, u32)> {
    patch
        .lines()
        .filter_map(|line| {
            let rest = line.strip_prefix("@@ -")?;
            let new = rest.split_whitespace().find(|part| part.starts_with('+'))?;
            let mut parts = new[1..].splitn(2, ',');
            let start: u32 = parts.next()?.parse().ok()?;
            let count: u32 = match parts.next() {
                Some(count) => count.parse().ok()?,
                None => 1,
            };
            (count > 0).then(|| (start, start + count - 1))
        })
        .collect()
}

/// `(added, removed)` lines of a patch, without their markers.
fn changed_lines(patch: &str) -> (Vec<&str>, Vec<&str>) {
    let mut added = Vec::new();
    let mut removed = Vec::new();
    for line in patch.lines() {
        if line.starts_with("+++") || line.starts_with("---") {
            continue;
        }
        if let Some(rest) = line.strip_prefix('+') {
            added.push(rest);
        } else if let Some(rest) = line.strip_prefix('-') {
            removed.push(rest);
        }
    }
    (added, removed)
}

/// The new-side lines a patch shows, added and context, by line number.
fn new_lines(patch: &str) -> Vec<(u32, &str)> {
    let mut out = Vec::new();
    let mut at = 0u32;
    for line in patch.lines() {
        if line.starts_with("@@ ") {
            at = hunks(line).first().map_or(0, |(start, _)| *start);
            continue;
        }
        if at == 0 || line.starts_with("+++") || line.starts_with("---") {
            continue;
        }
        if let Some(rest) = line.strip_prefix('+').or_else(|| line.strip_prefix(' ')) {
            out.push((at, rest));
            at += 1;
        }
    }
    out
}

fn is_docs(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.starts_with("docs/")
        || [".md", ".txt", ".rst", ".adoc"]
            .iter()
            .any(|ext| lower.ends_with(ext))
}

fn is_test(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.split('/').any(|part| {
        part == "tests"
            || part == "test"
            || part == "__tests__"
            || part.contains("_test.")
            || part.contains(".test.")
            || part.contains(".spec.")
            || part.starts_with("test_")
    })
}

fn squeezed(line: &str) -> String {
    line.chars().filter(|c| !c.is_whitespace()).collect()
}

fn clip(text: &str) -> String {
    let text = text.trim();
    if text.chars().count() <= MAX_LABEL_CHARS {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(MAX_LABEL_CHARS - 1).collect();
    out.push('…');
    out
}

fn node(id: impl Into<String>, label: impl Into<String>) -> DiagramNode {
    DiagramNode {
        id: id.into(),
        label: label.into(),
    }
}

fn edge(from: &str, to: &str, label: impl Into<String>) -> DiagramEdge {
    DiagramEdge {
        from: from.to_owned(),
        to: to.to_owned(),
        label: label.into(),
    }
}

/// Branch tokens on one line: `if`, `elif`, `case`, a Rust match arm, a `?`.
fn branches(line: &str, rust: bool) -> usize {
    let code = line.split("//").next().unwrap_or(line);
    let words = code
        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .filter(|word| matches!(*word, "if" | "elif" | "case"))
        .count();
    let arms = if rust { code.matches("=>").count() } else { 0 };
    let questions = code
        .char_indices()
        .filter(|(at, c)| {
            *c == '?'
                && !matches!(code[at + 1..].chars().next(), Some('.' | '?'))
                && !code[..*at].ends_with('?')
        })
        .count();
    words + arms + questions
}

impl<'a, 'g> Change<'a, 'g> {
    fn new(files: &'a [ChangedFile], index: &'a Index<'g>) -> Self {
        let hunks = files.iter().map(|file| hunks(&file.patch)).collect();
        Self {
            files,
            index,
            hunks,
        }
    }

    /// Docs only, or only whitespace changed.
    fn trivial(&self) -> bool {
        if self.files.iter().all(|file| is_docs(&file.path)) {
            return true;
        }
        let mut added = Vec::new();
        let mut removed = Vec::new();
        for file in self.files {
            let (plus, minus) = changed_lines(&file.patch);
            added.extend(plus.into_iter().map(squeezed).filter(|l| !l.is_empty()));
            removed.extend(minus.into_iter().map(squeezed).filter(|l| !l.is_empty()));
        }
        added.sort();
        removed.sort();
        added == removed
    }

    fn tests_only(&self) -> bool {
        self.files.iter().all(|file| is_test(&file.path))
    }

    fn diagram(&self, kind: DiagramKind) -> Option<Diagram> {
        match kind {
            DiagramKind::Sequence => self.sequence(),
            DiagramKind::Er => self.er(),
            DiagramKind::Class => self.class(),
            _ => self.flow(),
        }
    }

    fn touched(&self, kind: NodeKind) -> Vec<&'g Node> {
        let mut seen = BTreeSet::new();
        let mut found = Vec::new();
        for (file, hunks) in self.files.iter().zip(&self.hunks) {
            for hunk in hunks {
                for node in self.index.declared_in(&file.path, *hunk, kind) {
                    if seen.insert(node.id) {
                        found.push(node);
                    }
                }
            }
        }
        found
    }

    fn sequence(&self) -> Option<Diagram> {
        let mut participants: Vec<String> = Vec::new();
        let mut messages: Vec<(String, String, String)> = Vec::new();
        let mut add = |from: &str, to: &str, label: &str| {
            for path in [from, to] {
                if !participants.iter().any(|p| p == path) {
                    if participants.len() >= MAX_PARTICIPANTS {
                        return;
                    }
                    participants.push(path.to_owned());
                }
            }
            let message = (from.to_owned(), to.to_owned(), label.to_owned());
            if messages.len() < MAX_MESSAGES && !messages.contains(&message) {
                messages.push(message);
            }
        };
        for function in self.touched(NodeKind::Function) {
            for callee in self.index.callees(&function.name) {
                if callee.kind == NodeKind::Function && callee.path != function.path {
                    add(&function.path, &callee.path, &callee.name);
                }
            }
            for caller in self.index.callers(&function.name) {
                if caller.kind == NodeKind::Function && caller.path != function.path {
                    add(&caller.path, &function.path, &function.name);
                }
            }
        }
        if messages.is_empty() {
            return None;
        }
        Some(Diagram {
            kind: DiagramKind::Sequence,
            title: "Calls across modules".into(),
            nodes: participants
                .iter()
                .map(|path| node(path.clone(), path.clone()))
                .collect(),
            edges: messages
                .iter()
                .map(|(from, to, label)| edge(from, to, label.clone()))
                .collect(),
        })
    }

    fn er(&self) -> Option<Diagram> {
        let mut entities: Vec<(String, Vec<String>, Vec<String>)> = Vec::new();
        for file in self.files {
            let lower = file.path.to_ascii_lowercase();
            if lower.ends_with(".sql") || lower.ends_with(".prisma") || lower.contains("migration")
            {
                entities.extend(schema_entities(&new_lines(&file.patch)));
            }
        }
        for class in self.touched(NodeKind::Class) {
            if !TABLE_SUFFIXES
                .iter()
                .any(|suffix| class.name.ends_with(suffix))
            {
                continue;
            }
            let fields = self
                .files
                .iter()
                .find(|file| file.path == class.path)
                .and_then(|file| file.text.as_deref())
                .map(|text| struct_fields(text, class.line, class.end_line))
                .unwrap_or_default();
            entities.push((class.name.clone(), fields, Vec::new()));
        }
        if entities.is_empty() {
            return None;
        }
        let names: Vec<String> = entities.iter().map(|(name, _, _)| name.clone()).collect();
        let mut edges = Vec::new();
        for (name, fields, references) in &entities {
            for other in &names {
                if other == name {
                    continue;
                }
                let stem = other.to_ascii_lowercase();
                let stem = stem.trim_end_matches('s');
                let by_field = fields.iter().find(|field| {
                    let field = field.to_ascii_lowercase();
                    field
                        .strip_suffix("_id")
                        .is_some_and(|f| f.trim_end_matches('s') == stem)
                });
                let by_reference = references.iter().any(|r| r.eq_ignore_ascii_case(other));
                if let Some(field) = by_field {
                    edges.push(edge(name, other, field.clone()));
                } else if by_reference {
                    edges.push(edge(name, other, "references"));
                }
            }
        }
        Some(Diagram {
            kind: DiagramKind::Er,
            title: "Changed schema".into(),
            nodes: entities
                .iter()
                .map(|(name, fields, _)| {
                    let mut label = name.clone();
                    for field in fields {
                        label.push('\n');
                        label.push_str(field);
                    }
                    node(name.clone(), label)
                })
                .collect(),
            edges,
        })
    }

    fn class(&self) -> Option<Diagram> {
        let mut nodes: Vec<&Node> = Vec::new();
        let mut edges = Vec::new();
        let keep = |node: &'g Node, nodes: &mut Vec<&'g Node>| {
            if !nodes.iter().any(|n| n.id == node.id) {
                nodes.push(node);
            }
        };
        for class in self.touched(NodeKind::Class) {
            let (parents, children) = self.index.edges_of(&class.id, EdgeKind::Inherits);
            if parents.is_empty() && children.is_empty() {
                continue;
            }
            keep(class, &mut nodes);
            for parent in parents {
                keep(parent, &mut nodes);
                edges.push(edge(&class.name, &parent.name, "inherits"));
            }
            for child in children {
                keep(child, &mut nodes);
                edges.push(edge(&child.name, &class.name, "inherits"));
            }
        }
        if edges.is_empty() {
            return None;
        }
        edges.dedup();
        Some(Diagram {
            kind: DiagramKind::Class,
            title: "Changed type hierarchy".into(),
            nodes: nodes
                .iter()
                .map(|n| node(n.name.clone(), format!("{} ({})", n.name, n.path)))
                .collect(),
            edges,
        })
    }

    fn flow(&self) -> Option<Diagram> {
        for function in self.touched(NodeKind::Function) {
            let rust = function.path.ends_with(".rs");
            let Some(text) = self
                .files
                .iter()
                .find(|file| file.path == function.path)
                .and_then(|file| file.text.as_deref())
            else {
                continue;
            };
            let lines: Vec<(u32, &str)> = text
                .lines()
                .zip(1u32..)
                .map(|(line, at)| (at, line))
                .filter(|(at, _)| *at >= function.line && *at <= function.end_line)
                .collect();
            if let Some(diagram) = flow_of(&function.name, &lines, rust) {
                return Some(diagram);
            }
        }
        for (file, hunks) in self.files.iter().zip(&self.hunks) {
            let parsed = hunks.iter().any(|hunk| {
                !self
                    .index
                    .declared_in(&file.path, *hunk, NodeKind::Function)
                    .is_empty()
            });
            if parsed || is_docs(&file.path) {
                continue;
            }
            let lines = new_lines(&file.patch);
            if let Some(diagram) = flow_of(&file.path, &lines, file.path.ends_with(".rs")) {
                return Some(diagram);
            }
        }
        None
    }
}

fn flow_of(title: &str, lines: &[(u32, &str)], rust: bool) -> Option<Diagram> {
    let branching: Vec<(u32, &str, usize)> = lines
        .iter()
        .map(|(at, line)| (*at, *line, branches(line, rust)))
        .filter(|(_, _, count)| *count > 0)
        .collect();
    let total: usize = branching.iter().map(|(_, _, count)| count).sum();
    if total < MIN_FLOW_BRANCHES {
        return None;
    }
    let mut nodes = vec![node("start", title)];
    let mut edges = Vec::new();
    let mut previous = "start".to_owned();
    for (at, line, _) in branching.iter().take(MAX_FLOW_BRANCHES) {
        let id = format!("l{at}");
        nodes.push(node(id.clone(), format!("{at}: {}", clip(line))));
        edges.push(edge(&previous, &id, ""));
        previous = id;
    }
    Some(Diagram {
        kind: DiagramKind::Flow,
        title: format!("Branches in {title}"),
        nodes,
        edges,
    })
}

/// `CREATE TABLE` and Prisma `model` blocks: `(name, columns, referenced tables)`.
fn schema_entities(lines: &[(u32, &str)]) -> Vec<(String, Vec<String>, Vec<String>)> {
    let mut found = Vec::new();
    let mut open: Option<(String, Vec<String>, Vec<String>)> = None;
    for (_, line) in lines {
        let trimmed = line.trim();
        let upper = trimmed.to_ascii_uppercase();
        if let Some((name, fields, references)) = open.as_mut() {
            if trimmed.starts_with(')') || trimmed.starts_with('}') {
                found.push((
                    std::mem::take(name),
                    std::mem::take(fields),
                    std::mem::take(references),
                ));
                open = None;
                continue;
            }
            if let Some(at) = upper.find("REFERENCES ") {
                let target: String = trimmed[at + "REFERENCES ".len()..]
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect();
                if !target.is_empty() {
                    references.push(target);
                }
            }
            let first = trimmed
                .split(|c: char| c.is_whitespace() || c == ',')
                .next()
                .unwrap_or_default()
                .trim_matches(|c| c == '"' || c == '`');
            let constraint = [
                "PRIMARY",
                "FOREIGN",
                "CONSTRAINT",
                "UNIQUE",
                "CHECK",
                "INDEX",
                "KEY",
            ]
            .iter()
            .any(|word| first.eq_ignore_ascii_case(word));
            if !first.is_empty()
                && !constraint
                && !first.starts_with("--")
                && !first.starts_with("@@")
            {
                fields.push(first.to_owned());
            }
            continue;
        }
        let name = if upper.starts_with("CREATE TABLE") {
            let rest = trimmed["CREATE TABLE".len()..].trim_start();
            let rest = if rest.to_ascii_uppercase().starts_with("IF NOT EXISTS") {
                rest["IF NOT EXISTS".len()..].trim_start()
            } else {
                rest
            };
            Some(rest)
        } else {
            trimmed.strip_prefix("model ")
        };
        if let Some(rest) = name {
            let name: String = rest
                .trim_matches(|c| c == '"' || c == '`')
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            if !name.is_empty() {
                open = Some((name, Vec::new(), Vec::new()));
            }
        }
    }
    found.extend(open);
    found
}

/// `name: Type` fields between two lines of a struct or class.
fn struct_fields(text: &str, from: u32, to: u32) -> Vec<String> {
    text.lines()
        .zip(1u32..)
        .filter(|(_, at)| *at > from && *at <= to)
        .filter_map(|(line, _)| {
            let line = line
                .trim()
                .trim_start_matches("pub ")
                .trim_start_matches("readonly ");
            let (name, _) = line.split_once(':')?;
            let name = name.trim().trim_end_matches('?');
            (!name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_'))
                .then(|| name.to_owned())
        })
        .collect()
}

#[cfg(test)]
mod tests;
