//! A project's codebase graph: files, the functions, classes and module-level
//! variables they declare, and who calls, imports, uses and inherits what.
//! Pure: the daemon parses the files and hands each one's symbols in.

use std::borrow::Cow;
use std::cmp::Reverse;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, OnceLock};

pub use domain::{EdgeKind, GraphNode, NodeKind};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub type NodeId = [u8; 16];

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Node {
    pub id: NodeId,
    pub kind: NodeKind,
    pub path: String,
    pub name: String,
    /// Byte range in the file.
    pub span: (u32, u32),
    /// 1-based lines where the span starts and ends; `0` for a file or an external call.
    pub line: u32,
    pub end_line: u32,
    pub lang: String,
    pub signature: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Edge {
    pub src: NodeId,
    pub dst: NodeId,
    pub kind: EdgeKind,
    pub weight: u8,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Graph {
    pub git_oid: String,
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
}

/// See [`Graph::delta`]: rows to write, and edges outside the replaced paths to delete.
#[derive(Clone, Debug, Default)]
pub struct Delta {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub stale: Vec<Edge>,
}

/// A parsed file, as the daemon read it.
#[derive(Clone, Debug, Default)]
pub struct FileInput {
    pub path: String,
    pub lang: String,
    pub source: String,
    pub definitions: Vec<Definition>,
    /// `(called name, byte range of the name)`.
    pub calls: Vec<(String, (usize, usize))>,
    pub imports: Vec<String>,
    /// `(child, parent)`.
    pub inherits: Vec<(String, String)>,
}

#[derive(Clone, Debug)]
pub struct Definition {
    /// The tags kind: `function`, `method`, `class`, `interface`, `constant`, …
    pub kind: String,
    pub name: String,
    pub span: (usize, usize),
    pub signature: String,
}

const MAX_PATH_DEPTH: usize = 6;
const SIMILAR_THRESHOLD: f64 = 0.5;
const MAX_SIMILAR: usize = 3;

fn node_kind(tags_kind: &str) -> Option<NodeKind> {
    match tags_kind {
        "function" | "method" => Some(NodeKind::Function),
        "class" | "interface" | "enum" => Some(NodeKind::Class),
        "constant" | "variable" => Some(NodeKind::Variable),
        _ => None,
    }
}

fn node_id(kind: NodeKind, path: &str, name: &str, span: (u32, u32)) -> NodeId {
    let mut hash = Sha256::new();
    hash.update(format!("{kind:?}\0{path}\0{name}\0{}\0{}", span.0, span.1));
    let digest = hash.finalize();
    let mut id = [0u8; 16];
    id.copy_from_slice(&digest[..16]);
    id
}

fn external(name: &str) -> Node {
    Node {
        id: node_id(NodeKind::ExternalCall, "", name, (0, 0)),
        kind: NodeKind::ExternalCall,
        path: String::new(),
        name: name.to_owned(),
        span: (0, 0),
        line: 0,
        end_line: 0,
        lang: String::new(),
        signature: String::new(),
    }
}

fn span32(span: (usize, usize)) -> (u32, u32) {
    let clamp = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
    (clamp(span.0), clamp(span.1))
}

fn contains(outer: (u32, u32), inner: (u32, u32)) -> bool {
    outer.0 <= inner.0 && inner.1 <= outer.1 && outer != inner
}

/// What import text names, candidates first-to-try: the last segment, then the one before it.
fn import_stems(import: &str) -> Vec<String> {
    let text = import.trim_matches(|c| c == '"' || c == '\'');
    let text = text.replace("::", "/");
    let text = if text.contains('/') {
        text
    } else {
        text.replace('.', "/")
    };
    let mut segments: Vec<&str> = text
        .split('/')
        .filter(|s| !s.is_empty() && *s != "." && *s != ".." && *s != "crate" && *s != "super")
        .collect();
    let mut stems = Vec::new();
    for _ in 0..2 {
        let Some(last) = segments.pop() else { break };
        let stem = last.split('.').next().unwrap_or(last);
        stems.push(stem.to_owned());
    }
    stems
}

/// What an import of this file is matched by: its stem, or its directory's
/// name for `mod.rs`, `lib.rs`, `index.*` and `__init__.py`.
fn file_stem(path: &str) -> &str {
    let mut parts = path.rsplit('/');
    let name = parts.next().unwrap_or(path);
    let stem = name.split('.').next().unwrap_or(name);
    match stem {
        "mod" | "lib" | "index" | "__init__" => parts.next().unwrap_or(stem),
        _ => stem,
    }
}

fn dir(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(dir, _)| dir)
}

struct Lines<'s> {
    source: &'s str,
    newlines: Vec<usize>,
}

impl<'s> Lines<'s> {
    fn new(source: &'s str) -> Self {
        let newlines = source
            .bytes()
            .enumerate()
            .filter(|(_, b)| *b == b'\n')
            .map(|(at, _)| at)
            .collect();
        Self { source, newlines }
    }

    /// Counts the whole source for a byte that is not a char boundary inside it.
    fn at(&self, byte: usize) -> u32 {
        let byte = if self.source.is_char_boundary(byte) {
            byte
        } else {
            self.source.len()
        };
        let before = self.newlines.partition_point(|&at| at < byte);
        u32::try_from(before + 1).unwrap_or(u32::MAX)
    }
}

/// The smallest span of a set that strictly contains another, found by walking
/// up the set's containment forest; a set whose spans cross or invert is scanned.
struct Enclosing {
    members: Vec<usize>,
    spans: Vec<(u32, u32)>,
    owner: Vec<usize>,
    parent: Vec<Option<usize>>,
    nested: bool,
}

impl Enclosing {
    fn new(declared: &[Node], kinds: &[NodeKind]) -> Self {
        let members: Vec<usize> = (0..declared.len())
            .filter(|&i| kinds.contains(&declared[i].kind))
            .collect();
        let mut sorted = members.clone();
        sorted.sort_by_key(|&i| (forest_order(declared[i].span), i));
        let mut spans: Vec<(u32, u32)> = Vec::new();
        let mut owner = Vec::new();
        for i in sorted {
            if spans.last() != Some(&declared[i].span) {
                spans.push(declared[i].span);
                owner.push(i);
            }
        }
        let mut nested = spans.iter().all(|span| span.0 <= span.1);
        let mut parent = vec![None; spans.len()];
        let mut stack: Vec<usize> = Vec::new();
        for (at, span) in spans.iter().enumerate() {
            while let Some(&top) = stack.last() {
                let outer = spans[top];
                if outer.1 <= span.0 {
                    stack.pop();
                } else if span.1 <= outer.1 {
                    parent[at] = Some(top);
                    break;
                } else {
                    nested = false;
                    break;
                }
            }
            stack.push(at);
        }
        Self {
            members,
            spans,
            owner,
            parent,
            nested,
        }
    }

    fn find(&self, declared: &[Node], inner: (u32, u32)) -> Option<usize> {
        // An empty span where two spans touch lies in both, which the forest cannot hold.
        if !self.nested || inner.0 >= inner.1 {
            return self
                .members
                .iter()
                .copied()
                .filter(|&i| contains(declared[i].span, inner))
                .min_by_key(|&i| declared[i].span.1 - declared[i].span.0);
        }
        let after = self
            .spans
            .partition_point(|&span| forest_order(span) <= forest_order(inner));
        let mut at = after.checked_sub(1);
        while let Some(i) = at {
            if contains(self.spans[i], inner) {
                return Some(self.owner[i]);
            }
            at = self.parent[i];
        }
        None
    }
}

fn forest_order(span: (u32, u32)) -> (u32, Reverse<u32>) {
    (span.0, Reverse(span.1))
}

fn identifiers(text: &str) -> HashSet<&str> {
    text.split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .filter(|token| !token.is_empty())
        .collect()
}

/// A file's references, kept until every file's nodes exist to resolve them.
struct Pending {
    path: String,
    /// `(caller, called name)`; the caller is the file node for a top-level call.
    calls: Vec<(NodeId, String)>,
    imports: Vec<String>,
    inherits: Vec<(String, String)>,
}

#[derive(Default)]
pub struct Update {
    nodes: Vec<Node>,
    edges: Vec<Edge>,
    pending: Vec<Pending>,
}

impl Update {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Everything that needs the source is read here, so it can be dropped after.
    pub fn add(&mut self, file: FileInput) {
        let file_node = Node {
            id: node_id(NodeKind::File, &file.path, &file.path, (0, 0)),
            kind: NodeKind::File,
            path: file.path.clone(),
            name: file.path.clone(),
            span: (0, span32((file.source.len(), 0)).0),
            line: 0,
            end_line: 0,
            lang: file.lang.clone(),
            signature: String::new(),
        };
        let lines = Lines::new(&file.source);
        let mut declared: Vec<Node> = Vec::new();
        for definition in &file.definitions {
            let Some(kind) = node_kind(&definition.kind) else {
                continue;
            };
            let span = span32(definition.span);
            declared.push(Node {
                id: node_id(kind, &file.path, &definition.name, span),
                kind,
                path: file.path.clone(),
                name: definition.name.clone(),
                span,
                line: lines.at(definition.span.0),
                end_line: lines.at(definition.span.1),
                lang: file.lang.clone(),
                signature: definition.signature.clone(),
            });
        }
        let owners = Enclosing::new(&declared, &[NodeKind::Function, NodeKind::Class]);
        let mut kept = Vec::new();
        let mut variables = Vec::new();
        for node in &declared {
            match owners.find(&declared, node.span).map(|i| &declared[i]) {
                None => {
                    self.edge(file_node.id, node.id, EdgeKind::Defines);
                    if node.kind == NodeKind::Variable {
                        variables.push(node.clone());
                    }
                }
                // A function's locals live in its span, not in the graph.
                Some(owner)
                    if owner.kind == NodeKind::Function && node.kind == NodeKind::Variable =>
                {
                    continue;
                }
                Some(owner) => self.edge(owner.id, node.id, EdgeKind::Declares),
            }
            kept.push(node.clone());
        }
        for function in kept.iter().filter(|n| n.kind == NodeKind::Function) {
            let text = file
                .source
                .get(function.span.0 as usize..function.span.1 as usize)
                .unwrap_or_default();
            let tokens = identifiers(text);
            for variable in &variables {
                if tokens.contains(variable.name.as_str()) {
                    self.edge(function.id, variable.id, EdgeKind::Uses);
                }
            }
        }
        let functions = Enclosing::new(&declared, &[NodeKind::Function]);
        let calls = file
            .calls
            .iter()
            .map(|(name, span)| {
                let caller = functions
                    .find(&declared, span32(*span))
                    .map_or(file_node.id, |i| declared[i].id);
                (caller, name.clone())
            })
            .collect();
        self.pending.push(Pending {
            path: file.path.clone(),
            calls,
            imports: file.imports,
            inherits: file.inherits,
        });
        self.nodes.push(file_node);
        self.nodes.extend(kept);
    }

    fn edge(&mut self, src: NodeId, dst: NodeId, kind: EdgeKind) {
        self.edges.push(Edge {
            src,
            dst,
            kind,
            weight: 1,
        });
    }
}

impl Graph {
    /// Replaces the files in `update` and drops `removed`, then resolves the
    /// new files' references against the whole graph. An untouched file's import
    /// or inheritance into a replaced file follows its target by kind and path; its
    /// calls to a name `update` defines or drops resolve again, as a full build would.
    pub fn apply(&mut self, update: Update, removed: &[String], git_oid: &str) {
        let Update {
            nodes,
            edges,
            pending,
        } = update;
        let gone_paths: HashSet<&str> = pending
            .iter()
            .map(|p| p.path.as_str())
            .chain(removed.iter().map(String::as_str))
            .collect();
        let gone: HashMap<NodeId, Node> = self
            .nodes
            .iter()
            .filter(|n| gone_paths.contains(n.path.as_str()))
            .map(|n| (n.id, n.clone()))
            .collect();
        self.nodes.retain(|n| !gone.contains_key(&n.id));
        let mut dangling = Vec::new();
        let mut kept_edges = Vec::new();
        for edge in std::mem::take(&mut self.edges) {
            if gone.contains_key(&edge.src) {
                continue;
            }
            match gone.get(&edge.dst) {
                Some(target) => dangling.push((edge, target.clone())),
                None => kept_edges.push(edge),
            }
        }
        self.edges = kept_edges;
        let changed_names: HashSet<String> = nodes
            .iter()
            .chain(gone.values())
            .filter(|n| n.kind == NodeKind::Function)
            .map(|n| n.name.clone())
            .collect();
        self.nodes.extend(nodes);
        self.edges.extend(edges);
        self.git_oid = git_oid.to_owned();

        let mut externals: HashMap<String, Node> = HashMap::new();
        let mut external_id = |name: &str| {
            externals
                .entry(name.to_owned())
                .or_insert_with(|| external(name))
                .id
        };
        let mut added = Vec::new();
        {
            let lookup = Lookup::new(&self.nodes);
            let place: HashMap<NodeId, &Node> = self.nodes.iter().map(|n| (n.id, n)).collect();
            let replaced_imports = dangling.iter().filter_map(|(edge, target)| {
                Some((edge.src, lookup.same_place(target)?, edge.kind))
            });
            let mut imports: HashMap<&str, HashSet<&str>> = HashMap::new();
            for (src, dst, kind) in self
                .edges
                .iter()
                .map(|e| (e.src, e.dst, e.kind))
                .chain(replaced_imports)
            {
                if kind != EdgeKind::Imports {
                    continue;
                }
                if let (Some(from), Some(to)) =
                    (lookup.file_paths.get(&src), lookup.file_paths.get(&dst))
                {
                    imports.entry(*from).or_default().insert(*to);
                }
            }
            let nothing = HashSet::new();
            // As a full build resolves the call: from the caller's file and what it imports.
            let resolve_call = |caller: NodeId, name: &str| {
                let path = place.get(&caller).map_or("", |n| n.path.as_str());
                lookup.function(name, path, imports.get(path).unwrap_or(&nothing))
            };
            for file in &pending {
                let imported = lookup.imported_files(&file.path, &file.imports);
                let imported_paths = lookup.paths(&imported);
                let file_id = node_id(NodeKind::File, &file.path, &file.path, (0, 0));
                for target in &imported {
                    added.push((file_id, *target, EdgeKind::Imports));
                }
                for (caller, name) in &file.calls {
                    let dst = lookup
                        .function(name, &file.path, &imported_paths)
                        .unwrap_or_else(|| external_id(name));
                    added.push((*caller, dst, EdgeKind::Calls));
                }
                for (child, parent) in &file.inherits {
                    let child = lookup.class(child, &file.path, &nothing);
                    let parent = lookup.class(parent, &file.path, &imported_paths);
                    if let (Some(child), Some(parent)) = (child, parent) {
                        added.push((child, parent, EdgeKind::Inherits));
                    }
                }
            }
            for (edge, target) in &dangling {
                let dst = match edge.kind {
                    EdgeKind::Calls => Some(
                        resolve_call(edge.src, &target.name)
                            .unwrap_or_else(|| external_id(&target.name)),
                    ),
                    _ => lookup.same_place(target),
                };
                if let Some(dst) = dst {
                    added.push((edge.src, dst, edge.kind));
                }
            }
            // A function added or removed under a name changes how an untouched file's call to it resolves.
            if !changed_names.is_empty() {
                for edge in self.edges.iter_mut().filter(|e| e.kind == EdgeKind::Calls) {
                    let Some(name) = place
                        .get(&edge.dst)
                        .map(|n| n.name.as_str())
                        .filter(|name| changed_names.contains(*name))
                    else {
                        continue;
                    };
                    edge.dst = resolve_call(edge.src, name).unwrap_or_else(|| external_id(name));
                }
            }
        }
        let known: HashSet<NodeId> = self.nodes.iter().map(|n| n.id).collect();
        self.nodes
            .extend(externals.into_values().filter(|n| !known.contains(&n.id)));
        for (src, dst, kind) in added {
            self.edges.push(Edge {
                src,
                dst,
                kind,
                weight: 1,
            });
        }
        let mut seen = HashSet::new();
        self.edges
            .retain(|e| e.src != e.dst || e.kind == EdgeKind::Calls);
        self.edges.retain(|e| seen.insert((e.src, e.dst, e.kind)));
        let called: HashSet<NodeId> = self.edges.iter().map(|e| e.dst).collect();
        self.nodes
            .retain(|n| n.kind != NodeKind::ExternalCall || called.contains(&n.id));
        self.nodes
            .sort_by(|a, b| (&a.path, a.span, &a.name).cmp(&(&b.path, b.span, &b.name)));
        self.edges.sort_by_key(|e| (e.src, e.dst, e.kind));
    }

    /// What a store holding `base` needs to hold `self`, once it has dropped
    /// the nodes of `paths` (the files an update replaced or removed) and every
    /// edge touching them.
    #[must_use]
    pub fn delta(&self, base: &Graph, paths: &[String]) -> Delta {
        let touched: HashSet<&str> = paths.iter().map(String::as_str).collect();
        let ids: HashSet<NodeId> = self
            .nodes
            .iter()
            .filter(|n| touched.contains(n.path.as_str()))
            .map(|n| n.id)
            .collect();
        let before: HashSet<&Edge> = base.edges.iter().collect();
        let after: HashSet<&Edge> = self.edges.iter().collect();
        let edges: Vec<Edge> = self
            .edges
            .iter()
            .filter(|e| ids.contains(&e.src) || ids.contains(&e.dst) || !before.contains(e))
            .copied()
            .collect();
        let stale = base
            .edges
            .iter()
            .filter(|e| !after.contains(e))
            .copied()
            .collect();
        let reached: HashSet<NodeId> = edges.iter().flat_map(|e| [e.src, e.dst]).collect();
        let nodes = self
            .nodes
            .iter()
            .filter(|n| {
                ids.contains(&n.id) || (n.kind == NodeKind::ExternalCall && reached.contains(&n.id))
            })
            .cloned()
            .collect();
        Delta {
            nodes,
            edges,
            stale,
        }
    }

    #[must_use]
    pub fn index(&self) -> Index<'_> {
        Index::new(self)
    }

    /// The `cache.bin` format.
    ///
    /// # Errors
    /// Never for a graph this crate built; the encoder's error otherwise.
    pub fn encode(&self) -> Result<Vec<u8>, String> {
        rmp_serde::to_vec(self).map_err(|e| e.to_string())
    }

    /// # Errors
    /// Bytes that are not an encoded graph.
    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        rmp_serde::from_slice(bytes).map_err(|e| e.to_string())
    }
}

impl Graph {
    /// Graphviz text of the first `cap` nodes and the edges between them,
    /// with a comment line saying how many nodes were left out.
    #[must_use]
    pub fn to_dot(&self, cap: usize) -> String {
        let (kept, left) = self.capped(cap);
        let mut out = String::from("digraph codebase {\n");
        for (at, node) in &kept {
            out.push_str(&format!("  n{at} [label=\"{}\"];\n", quoted(&label(node))));
        }
        for (src, dst, kind) in self.capped_edges(&kept) {
            out.push_str(&format!("  n{src} -> n{dst} [label=\"{kind:?}\"];\n"));
        }
        if left > 0 {
            out.push_str(&format!("  // {left} node(s) left out past {cap}\n"));
        }
        out.push_str("}\n");
        out
    }

    /// A mermaid `flowchart` of the first `cap` nodes, like [`Graph::to_dot`].
    #[must_use]
    pub fn to_mermaid(&self, cap: usize) -> String {
        let (kept, left) = self.capped(cap);
        let mut out = String::from("flowchart LR\n");
        for (at, node) in &kept {
            out.push_str(&format!(
                "  n{at}[\"{}\"]\n",
                label(node).replace('"', "#quot;")
            ));
        }
        for (src, dst, kind) in self.capped_edges(&kept) {
            out.push_str(&format!("  n{src} -->|{kind:?}| n{dst}\n"));
        }
        if left > 0 {
            out.push_str(&format!("  %% {left} node(s) left out past {cap}\n"));
        }
        out
    }

    fn capped(&self, cap: usize) -> (Vec<(usize, &Node)>, usize) {
        let kept: Vec<(usize, &Node)> = self.nodes.iter().enumerate().take(cap).collect();
        (kept, self.nodes.len().saturating_sub(cap))
    }

    fn capped_edges(&self, kept: &[(usize, &Node)]) -> Vec<(usize, usize, EdgeKind)> {
        let at: HashMap<NodeId, usize> = kept.iter().map(|(i, n)| (n.id, *i)).collect();
        self.edges
            .iter()
            .filter_map(|e| Some((*at.get(&e.src)?, *at.get(&e.dst)?, e.kind)))
            .collect()
    }
}

fn label(node: &Node) -> String {
    match node.kind {
        NodeKind::File => node.path.clone(),
        NodeKind::ExternalCall => format!("{} (external)", node.name),
        _ => format!("{}:{} {}", node.path, node.line, node.name),
    }
}

fn quoted(text: &str) -> String {
    text.replace('\\', "\\\\").replace('"', "\\\"")
}

impl Node {
    #[must_use]
    pub fn to_wire(&self) -> GraphNode {
        GraphNode {
            kind: self.kind,
            path: self.path.clone(),
            name: self.name.clone(),
            line: self.line,
            signature: self.signature.clone(),
        }
    }
}

/// Name resolution over a node list.
struct Lookup<'g> {
    by_name: HashMap<&'g str, Vec<&'g Node>>,
    by_stem: HashMap<&'g str, Vec<&'g Node>>,
    file_at: HashMap<&'g str, &'g Node>,
    file_paths: HashMap<NodeId, &'g str>,
}

impl<'g> Lookup<'g> {
    fn new(nodes: &'g [Node]) -> Self {
        let mut by_name: HashMap<&str, Vec<&Node>> = HashMap::new();
        let mut by_stem: HashMap<&str, Vec<&Node>> = HashMap::new();
        let mut file_at = HashMap::new();
        let mut file_paths = HashMap::new();
        for node in nodes {
            match node.kind {
                NodeKind::File => {
                    by_stem.entry(file_stem(&node.path)).or_default().push(node);
                    file_at.entry(node.path.as_str()).or_insert(node);
                    file_paths.insert(node.id, node.path.as_str());
                }
                NodeKind::ExternalCall => {}
                _ => by_name.entry(node.name.as_str()).or_default().push(node),
            }
        }
        Self {
            by_name,
            by_stem,
            file_at,
            file_paths,
        }
    }

    fn paths(&self, files: &[NodeId]) -> HashSet<&'g str> {
        files
            .iter()
            .filter_map(|id| self.file_paths.get(id).copied())
            .collect()
    }

    fn imported_files(&self, from: &str, imports: &[String]) -> Vec<NodeId> {
        let mut found = Vec::new();
        for import in imports {
            for stem in import_stems(import) {
                let matching: Vec<&Node> = self
                    .by_stem
                    .get(stem.as_str())
                    .into_iter()
                    .flatten()
                    .copied()
                    .filter(|f| f.path != from)
                    .collect();
                let near: Vec<&Node> = matching
                    .iter()
                    .copied()
                    .filter(|f| dir(&f.path) == dir(from) || dir(dir(&f.path)) == dir(from))
                    .collect();
                let pick = match (near.as_slice(), matching.as_slice()) {
                    ([one], _) | ([], [one]) => Some(one.id),
                    _ => None,
                };
                if let Some(id) = pick {
                    if !found.contains(&id) {
                        found.push(id);
                    }
                    break;
                }
            }
        }
        found
    }

    fn resolve(
        &self,
        name: &str,
        kind: NodeKind,
        path: &str,
        imported: &HashSet<&str>,
    ) -> Option<NodeId> {
        let candidates: Vec<&Node> = self
            .by_name
            .get(name)?
            .iter()
            .copied()
            .filter(|n| n.kind == kind)
            .collect();
        if let Some(local) = candidates.iter().find(|n| n.path == path) {
            return Some(local.id);
        }
        if let Some(from_import) = candidates
            .iter()
            .find(|n| imported.contains(n.path.as_str()))
        {
            return Some(from_import.id);
        }
        match candidates.as_slice() {
            [one] => Some(one.id),
            _ => None,
        }
    }

    fn function(&self, name: &str, path: &str, imported: &HashSet<&str>) -> Option<NodeId> {
        self.resolve(name, NodeKind::Function, path, imported)
    }

    fn class(&self, name: &str, path: &str, imported: &HashSet<&str>) -> Option<NodeId> {
        self.resolve(name, NodeKind::Class, path, imported)
    }

    fn same_place(&self, old: &Node) -> Option<NodeId> {
        if old.kind == NodeKind::File {
            return self.file_at.get(old.path.as_str()).map(|f| f.id);
        }
        self.by_name
            .get(old.name.as_str())?
            .iter()
            .find(|n| n.kind == old.kind && n.path == old.path)
            .map(|n| n.id)
    }
}

#[derive(Clone)]
struct Adjacency {
    at: HashMap<NodeId, usize>,
    by_name: HashMap<String, Vec<usize>>,
    by_path: HashMap<String, Vec<usize>>,
    out: Vec<Vec<(usize, EdgeKind)>>,
    incoming: Vec<Vec<(usize, EdgeKind)>>,
    /// Distinct `Calls` targets; `calling_functions` is the inverse, from functions only.
    callees: Vec<Vec<usize>>,
    calling_functions: Vec<Vec<usize>>,
}

impl Adjacency {
    fn new(graph: &Graph) -> Self {
        let at: HashMap<NodeId, usize> = graph
            .nodes
            .iter()
            .enumerate()
            .map(|(i, n)| (n.id, i))
            .collect();
        let mut by_name: HashMap<String, Vec<usize>> = HashMap::new();
        let mut by_path: HashMap<String, Vec<usize>> = HashMap::new();
        for (i, node) in graph.nodes.iter().enumerate() {
            by_name.entry(node.name.clone()).or_default().push(i);
            match by_path.get_mut(&node.path) {
                Some(found) => found.push(i),
                None => {
                    by_path.insert(node.path.clone(), vec![i]);
                }
            }
        }
        let mut out = vec![Vec::new(); graph.nodes.len()];
        let mut incoming = vec![Vec::new(); graph.nodes.len()];
        let mut callees = vec![Vec::new(); graph.nodes.len()];
        let mut calling_functions = vec![Vec::new(); graph.nodes.len()];
        for edge in &graph.edges {
            if let (Some(&src), Some(&dst)) = (at.get(&edge.src), at.get(&edge.dst)) {
                out[src].push((dst, edge.kind));
                incoming[dst].push((src, edge.kind));
                if edge.kind == EdgeKind::Calls {
                    callees[src].push(dst);
                    if graph.nodes[src].kind == NodeKind::Function {
                        calling_functions[dst].push(src);
                    }
                }
            }
        }
        for list in callees.iter_mut().chain(calling_functions.iter_mut()) {
            list.sort_unstable();
            list.dedup();
        }
        Self {
            at,
            by_name,
            by_path,
            out,
            incoming,
            callees,
            calling_functions,
        }
    }
}

/// A shared graph that builds its [`Index`] on the first query and keeps it.
pub struct Indexed {
    graph: Arc<Graph>,
    adjacency: OnceLock<Adjacency>,
}

impl Indexed {
    #[must_use]
    pub fn new(graph: Arc<Graph>) -> Self {
        Self {
            graph,
            adjacency: OnceLock::new(),
        }
    }

    #[must_use]
    pub fn graph(&self) -> &Arc<Graph> {
        &self.graph
    }

    #[must_use]
    pub fn index(&self) -> Index<'_> {
        Index {
            graph: &self.graph,
            adjacency: Cow::Borrowed(self.adjacency.get_or_init(|| Adjacency::new(&self.graph))),
        }
    }

    #[must_use]
    pub fn is_indexed(&self) -> bool {
        self.adjacency.get().is_some()
    }
}

/// Adjacency over a graph, built once for a batch of queries.
pub struct Index<'g> {
    graph: &'g Graph,
    adjacency: Cow<'g, Adjacency>,
}

impl<'g> Index<'g> {
    fn new(graph: &'g Graph) -> Self {
        Self {
            graph,
            adjacency: Cow::Owned(Adjacency::new(graph)),
        }
    }

    fn node(&self, at: usize) -> &'g Node {
        &self.graph.nodes[at]
    }

    fn functions(&self, name: &str) -> Vec<usize> {
        self.adjacency
            .by_name
            .get(name)
            .map(|found| {
                found
                    .iter()
                    .copied()
                    .filter(|&i| self.node(i).kind == NodeKind::Function)
                    .collect()
            })
            .unwrap_or_default()
    }

    fn collect(&self, found: impl IntoIterator<Item = usize>) -> Vec<&'g Node> {
        let mut seen = HashSet::new();
        found
            .into_iter()
            .filter(|i| seen.insert(*i))
            .map(|i| self.node(i))
            .collect()
    }

    #[must_use]
    pub fn get(&self, id: &NodeId) -> Option<&'g Node> {
        self.adjacency.at.get(id).map(|&i| self.node(i))
    }

    /// The functions (or files, for a top-level call) that call a function named `name`.
    #[must_use]
    pub fn callers(&self, name: &str) -> Vec<&'g Node> {
        self.collect(self.functions(name).into_iter().flat_map(|f| {
            self.adjacency.incoming[f]
                .iter()
                .filter(|(_, k)| *k == EdgeKind::Calls)
                .map(|(src, _)| *src)
                .filter(move |src| *src != f)
        }))
    }

    /// What functions named `name` call, project functions before external calls.
    #[must_use]
    pub fn callees(&self, name: &str) -> Vec<&'g Node> {
        let mut found = self.collect(self.functions(name).into_iter().flat_map(|f| {
            self.adjacency.out[f]
                .iter()
                .filter(|(_, k)| *k == EdgeKind::Calls)
                .map(|(dst, _)| *dst)
                .filter(move |dst| *dst != f)
        }));
        found.sort_by_key(|n| n.kind == NodeKind::ExternalCall);
        found
    }

    /// Up to three other functions whose callees overlap `name`'s by a Jaccard index of at least 0.5.
    #[must_use]
    pub fn similar(&self, name: &str) -> Vec<&'g Node> {
        let mine = self.functions(name);
        let mut scored: Vec<(f64, usize)> = Vec::new();
        for &f in &mine {
            let calls = &self.adjacency.callees[f];
            let mut shared_with: HashMap<usize, usize> = HashMap::new();
            for &callee in calls {
                for &other in &self.adjacency.calling_functions[callee] {
                    if !mine.contains(&other) {
                        *shared_with.entry(other).or_default() += 1;
                    }
                }
            }
            for (other, shared) in shared_with {
                let union = calls.len() + self.adjacency.callees[other].len() - shared;
                #[allow(clippy::cast_precision_loss, reason = "callee counts are small")]
                let jaccard = shared as f64 / union as f64;
                if jaccard >= SIMILAR_THRESHOLD {
                    scored.push((jaccard, other));
                }
            }
        }
        scored.sort_by(|a, b| {
            b.0.total_cmp(&a.0)
                .then_with(|| (&self.node(a.1).path, a.1).cmp(&(&self.node(b.1).path, b.1)))
        });
        self.collect(scored.into_iter().map(|(_, i)| i))
            .into_iter()
            .take(MAX_SIMILAR)
            .collect()
    }

    /// The shortest call chain from a function named `from` to one named `to`,
    /// both ends included; empty when none is within six calls.
    #[must_use]
    pub fn path(&self, from: &str, to: &str) -> Vec<&'g Node> {
        let targets: HashSet<usize> = self.functions(to).into_iter().collect();
        let mut previous: HashMap<usize, Option<usize>> = HashMap::new();
        let mut queue = VecDeque::new();
        for start in self.functions(from) {
            previous.insert(start, None);
            queue.push_back((start, 0usize));
        }
        while let Some((at, depth)) = queue.pop_front() {
            if targets.contains(&at) {
                let mut chain = vec![at];
                let mut cursor = at;
                while let Some(Some(before)) = previous.get(&cursor) {
                    chain.push(*before);
                    cursor = *before;
                }
                chain.reverse();
                return chain.into_iter().map(|i| self.node(i)).collect();
            }
            if depth >= MAX_PATH_DEPTH {
                continue;
            }
            for (next, kind) in &self.adjacency.out[at] {
                if *kind == EdgeKind::Calls && !previous.contains_key(next) {
                    previous.insert(*next, Some(at));
                    queue.push_back((*next, depth + 1));
                }
            }
        }
        Vec::new()
    }

    fn file(&self, path: &str) -> Option<usize> {
        self.adjacency
            .by_name
            .get(path)?
            .iter()
            .copied()
            .find(|&i| self.node(i).kind == NodeKind::File)
    }

    /// The files `path` imports.
    #[must_use]
    pub fn imports(&self, path: &str) -> Vec<&'g Node> {
        let Some(file) = self.file(path) else {
            return Vec::new();
        };
        self.collect(
            self.adjacency.out[file]
                .iter()
                .filter(|(_, k)| *k == EdgeKind::Imports)
                .map(|(dst, _)| *dst),
        )
    }

    /// The external calls made from `path`.
    #[must_use]
    pub fn external_calls(&self, path: &str) -> Vec<&'g Node> {
        let mut found = self.collect(self.at_path(path).flat_map(|caller| {
            self.adjacency.out[caller]
                .iter()
                .filter(|(dst, k)| {
                    *k == EdgeKind::Calls && self.node(*dst).kind == NodeKind::ExternalCall
                })
                .map(|(dst, _)| *dst)
        }));
        found.sort_by(|a, b| a.name.cmp(&b.name));
        found
    }

    /// What `node` reaches by `kind` edges, and what reaches it.
    #[must_use]
    pub fn edges_of(&self, node: &NodeId, kind: EdgeKind) -> (Vec<&'g Node>, Vec<&'g Node>) {
        let Some(&at) = self.adjacency.at.get(node) else {
            return (Vec::new(), Vec::new());
        };
        let pick = |list: &[(usize, EdgeKind)]| {
            self.collect(list.iter().filter(|(_, k)| *k == kind).map(|(i, _)| *i))
        };
        (
            pick(&self.adjacency.out[at]),
            pick(&self.adjacency.incoming[at]),
        )
    }

    /// Answers a wire query; `None` for a question this build does not know.
    #[must_use]
    pub fn query(&self, query: &domain::GraphQuery) -> Option<Vec<&'g Node>> {
        use domain::GraphQuery as Q;
        Some(match query {
            Q::Callers(name) => self.callers(name),
            Q::Callees(name) => self.callees(name),
            Q::Similar(name) => self.similar(name),
            Q::Path(from, to) => self.path(from, to),
            Q::Imports(path) => self.imports(path),
            Q::ExternalCalls(path) => self.external_calls(path),
            _ => return None,
        })
    }

    /// The functions of `path` whose lines overlap `lines`, inclusive.
    #[must_use]
    pub fn functions_in(&self, path: &str, lines: (u32, u32)) -> Vec<&'g Node> {
        self.declared_in(path, lines, NodeKind::Function)
    }

    /// The `kind` nodes of `path` whose lines overlap `lines`, inclusive.
    #[must_use]
    pub fn declared_in(&self, path: &str, lines: (u32, u32), kind: NodeKind) -> Vec<&'g Node> {
        let mut found: Vec<&Node> = self
            .at_path(path)
            .map(|i| self.node(i))
            .filter(|n| n.kind == kind)
            .filter(|n| n.line <= lines.1 && n.end_line >= lines.0)
            .collect();
        found.sort_by_key(|n| n.line);
        found
    }

    fn at_path(&self, path: &str) -> impl Iterator<Item = usize> + '_ {
        self.adjacency
            .by_path
            .get(path)
            .into_iter()
            .flatten()
            .copied()
    }
}

#[cfg(test)]
mod tests;
