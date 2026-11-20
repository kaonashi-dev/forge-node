//! The pack blocks after the diff: graph, knowledge base, cross-repo
//! signatures and runtime results. Each is budgeted by tier and escaped and
//! redacted like the diff; with none of them the pack and prompt are v1's.

use std::fmt::Write as _;

use domain::{Coverage, GraphNode, Tier};

use crate::pack::{attr, neutralise, redact, TierCaps};

const MAX_CALLERS: usize = 5;
const MAX_CALLEES: usize = 5;
const MAX_SIMILAR: usize = 3;

/// One changed function and what the graph says around it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GraphFunction {
    pub path: String,
    pub line: u32,
    pub name: String,
    pub callers: Vec<GraphNode>,
    pub callees: Vec<GraphNode>,
    pub similar: Vec<GraphNode>,
}

/// A target the sandbox ran, as the runtime block reports it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RuntimeResult {
    pub target: String,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Extras {
    pub graph: Vec<GraphFunction>,
    /// `(reference, text)`: `architecture.md#<section>` and the excerpt.
    pub kb: Vec<(String, String)>,
    /// `path name signature`, one per line.
    pub cross_repo: Vec<String>,
    pub runtime: Vec<RuntimeResult>,
}

fn line(node: &GraphNode) -> String {
    let kind = match node.kind {
        domain::NodeKind::File => "file",
        domain::NodeKind::Class => "class",
        domain::NodeKind::Variable => "variable",
        domain::NodeKind::ExternalCall => "external",
        _ => "function",
    };
    if node.path.is_empty() {
        format!("{kind} {}", node.name)
    } else {
        format!("{}:{} {kind} {}", node.path, node.line, node.name)
    }
}

/// Whole sections until `cap`; what does not fit is counted in a note.
struct Budget<'a> {
    cap: usize,
    used: usize,
    cut: usize,
    out: &'a mut String,
}

impl Budget<'_> {
    fn push(&mut self, section: &str) -> bool {
        if self.cut > 0 || self.used + section.len() > self.cap {
            self.cut += 1;
            return false;
        }
        self.used += section.len();
        self.out.push_str(section);
        true
    }
}

fn block(
    tag: &str,
    nonce: &str,
    cap: usize,
    sections: impl IntoIterator<Item = String>,
    coverage: &mut Coverage,
    pack: &mut String,
) -> bool {
    let mut body = String::new();
    let mut budget = Budget {
        cap,
        used: 0,
        cut: 0,
        out: &mut body,
    };
    for section in sections {
        let (section, redactions) = redact(&section);
        coverage.redactions += redactions;
        budget.push(&neutralise(&section));
    }
    let cut = budget.cut;
    if cut > 0 {
        coverage.notes.push(format!(
            "<{tag}>: {cut} section(s) past the tier's {} KiB budget were left out",
            cap / 1024
        ));
    }
    if body.is_empty() {
        return false;
    }
    let _ = write!(pack, "<{tag}-{nonce}>\n{body}</{tag}-{nonce}>\n\n");
    true
}

/// The blocks to append to the pack, and what the prompt adds about them.
#[must_use]
pub fn blocks(
    extras: &Extras,
    tier: Tier,
    nonce: &str,
    coverage: &mut Coverage,
) -> (String, String) {
    let caps = TierCaps::of(tier);
    let mut pack = String::new();
    let mut prompt = String::new();
    let graph = extras.graph.iter().map(|function| {
        let mut section = format!(
            "## {}:{} {}\n",
            attr(&function.path),
            function.line,
            function.name
        );
        for (label, nodes, max) in [
            ("callers", &function.callers, MAX_CALLERS),
            ("callees", &function.callees, MAX_CALLEES),
            ("similar", &function.similar, MAX_SIMILAR),
        ] {
            if nodes.is_empty() {
                continue;
            }
            let _ = writeln!(section, "{label}:");
            for node in nodes.iter().take(max) {
                let _ = writeln!(section, "- {}", line(node));
            }
        }
        section
    });
    if block("graph", nonce, caps.graph_bytes, graph, coverage, &mut pack) {
        let _ = write!(
            prompt,
            "\n\n<graph-{nonce}> lists each changed function's callers, callees and similar \
             functions from the project's codebase graph. Read the callers before judging a \
             contract change. `forgectl review graph query --callers <function>` (also \
             `--callees`, `--similar`) answers more from the same graph."
        );
    }
    if caps.kb_bytes > 0 {
        let kb = extras
            .kb
            .iter()
            .map(|(reference, text)| format!("## {reference}\n{text}\n"));
        if block("kb", nonce, caps.kb_bytes, kb, coverage, &mut pack) {
            let _ = write!(
                prompt,
                "\n\n<kb-{nonce}> holds the project's knowledge base: its architecture, \
                 patterns and learned rules. When a finding rests on one of its sections, set \
                 `kb_ref` to that section, as `\"architecture.md#<section>\"`."
            );
        }
    }
    if caps.cross_repo_bytes > 0 {
        let lines = extras.cross_repo.iter().map(|line| format!("{line}\n"));
        if block(
            "cross-repo",
            nonce,
            caps.cross_repo_bytes,
            lines,
            coverage,
            &mut pack,
        ) {
            let _ = write!(
                prompt,
                "\n\n<cross-repo-{nonce}> holds the signatures of functions the change calls \
                 that are defined in other repositories; their bodies are not available."
            );
        }
    }
    if caps.runtime_bytes > 0 {
        let runtime = extras.runtime.iter().map(|result| {
            let exit = result
                .exit_code
                .map_or_else(|| "killed".to_owned(), |code| code.to_string());
            let mut section = format!("## {} (exit {exit})\n", result.target);
            if result.exit_code != Some(0) {
                for (name, text) in [("stdout", &result.stdout), ("stderr", &result.stderr)] {
                    if !text.trim().is_empty() {
                        let _ = write!(section, "{name}:\n{}\n", text.trim_end());
                    }
                }
            }
            section
        });
        if block(
            "runtime",
            nonce,
            caps.runtime_bytes,
            runtime,
            coverage,
            &mut pack,
        ) {
            let _ = write!(
                prompt,
                "\n\n<runtime-{nonce}> is what the project's sandboxed targets printed when \
                 Forge ran them on the change: a failure there is evidence, not a guess."
            );
        }
    }
    (pack, prompt)
}
