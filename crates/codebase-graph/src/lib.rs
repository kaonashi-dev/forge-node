//! A project's codebase graph: files, the functions, classes and module-level
//! variables they declare, and who calls, imports, uses and inherits what.
//! Pure: the daemon parses the files and hands each one's symbols in.

use std::collections::{HashMap, HashSet, VecDeque};

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

fn line_at(source: &str, byte: usize) -> u32 {
    let before = source.get(..byte).unwrap_or(source);
    u32::try_from(before.bytes().filter(|b| *b == b'\n').count() + 1).unwrap_or(u32::MAX)
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
                line: line_at(&file.source, definition.span.0),
                end_line: line_at(&file.source, definition.span.1),
                lang: file.lang.clone(),
                signature: definition.signature.clone(),
            });
        }
        let enclosing = |inner: (u32, u32), kinds: &[NodeKind]| {
            declared
                .iter()
                .filter(|n| kinds.contains(&n.kind) && contains(n.span, inner))
                .min_by_key(|n| n.span.1 - n.span.0)
        };
        let mut kept = Vec::new();
        let mut variables = Vec::new();
        for node in &declared {
            match enclosing(node.span, &[NodeKind::Function, NodeKind::Class]) {
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
        let calls = file
            .calls
            .iter()
            .map(|(name, span)| {
                let caller = enclosing(span32(*span), &[NodeKind::Function])
                    .filter(|f| kept.iter().any(|k| k.id == f.id))
                    .map_or(file_node.id, |f| f.id);
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
    /// new files' references against the whole graph. An edge from an
    /// untouched file into a replaced one follows its target by kind and name,
    /// and its external calls resolve again against the functions `update` defines.
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
        let defined: HashMap<NodeId, String> = nodes
            .iter()
            .filter(|n| n.kind == NodeKind::Function)
            .map(|n| (external(&n.name).id, n.name.clone()))
            .collect();
        self.nodes.extend(nodes);
        self.edges.extend(edges);
        self.git_oid = git_oid.to_owned();

        let mut externals: HashMap<String, Node> = HashMap::new();
        let mut added = Vec::new();
        {
            let lookup = Lookup::new(&self.nodes);
            let place: HashMap<NodeId, &str> =
                self.nodes.iter().map(|n| (n.id, n.path.as_str())).collect();
            let mut imports: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
            for edge in self.edges.iter().filter(|e| e.kind == EdgeKind::Imports) {
                imports.entry(edge.src).or_default().push(edge.dst);
            }
            // As a full build resolves the call: from the caller's file and what it imports.
            let resolve_call = |caller: NodeId, name: &str| {
                let path = place.get(&caller).copied().unwrap_or_default();
                let file = node_id(NodeKind::File, path, path, (0, 0));
                lookup.function(name, path, imports.get(&file).map_or(&[], Vec::as_slice))
            };
            for file in &pending {
                let imported = lookup.imported_files(&file.path, &file.imports);
                let file_id = node_id(NodeKind::File, &file.path, &file.path, (0, 0));
                for target in &imported {
                    added.push((file_id, *target, EdgeKind::Imports));
                }
                for (caller, name) in &file.calls {
                    let dst = match lookup.function(name, &file.path, &imported) {
                        Some(id) => id,
                        None => {
                            externals
                                .entry(name.clone())
                                .or_insert_with(|| external(name))
                                .id
                        }
                    };
                    added.push((*caller, dst, EdgeKind::Calls));
                }
                for (child, parent) in &file.inherits {
                    let child = lookup.class(child, &file.path, &[]);
                    let parent = lookup.class(parent, &file.path, &imported);
                    if let (Some(child), Some(parent)) = (child, parent) {
                        added.push((child, parent, EdgeKind::Inherits));
                    }
                }
            }
            for (edge, target) in dangling {
                let same = lookup.same_place(&target);
                let dst = match (same, edge.kind) {
                    (Some(id), _) => Some(id),
                    (None, EdgeKind::Calls) => {
                        Some(resolve_call(edge.src, &target.name).unwrap_or_else(|| {
                            externals
                                .entry(target.name.clone())
                                .or_insert_with(|| external(&target.name))
                                .id
                        }))
                    }
                    (None, _) => None,
                };
                if let Some(dst) = dst {
                    added.push((edge.src, dst, edge.kind));
                }
            }
            // An untouched file's external call may now name a function this update defines.
            if !defined.is_empty() {
                for edge in &mut self.edges {
                    let Some(name) = defined
                        .get(&edge.dst)
                        .filter(|_| edge.kind == EdgeKind::Calls)
                    else {
                        continue;
                    };
                    if let Some(dst) = resolve_call(edge.src, name) {
                        edge.dst = dst;
                    }
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
    files: Vec<&'g Node>,
}

impl<'g> Lookup<'g> {
    fn new(nodes: &'g [Node]) -> Self {
        let mut by_name: HashMap<&str, Vec<&Node>> = HashMap::new();
        let mut files = Vec::new();
        for node in nodes {
            match node.kind {
                NodeKind::File => files.push(node),
                NodeKind::ExternalCall => {}
                _ => by_name.entry(node.name.as_str()).or_default().push(node),
            }
        }
        Self { by_name, files }
    }

    fn imported_files(&self, from: &str, imports: &[String]) -> Vec<NodeId> {
        let mut found = Vec::new();
        for import in imports {
            for stem in import_stems(import) {
                let matching: Vec<&Node> = self
                    .files
                    .iter()
                    .copied()
                    .filter(|f| f.path != from && file_stem(&f.path) == stem)
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
        imported: &[NodeId],
    ) -> Option<NodeId> {
        let candidates: Vec<&Node> = self
            .by_name
            .get(name)
            .map(|nodes| nodes.iter().copied().filter(|n| n.kind == kind).collect())
            .unwrap_or_default();
        if let Some(local) = candidates.iter().find(|n| n.path == path) {
            return Some(local.id);
        }
        let imported_paths: Vec<&str> = self
            .files
            .iter()
            .filter(|f| imported.contains(&f.id))
            .map(|f| f.path.as_str())
            .collect();
        if let Some(from_import) = candidates
            .iter()
            .find(|n| imported_paths.contains(&n.path.as_str()))
        {
            return Some(from_import.id);
        }
        match candidates.as_slice() {
            [one] => Some(one.id),
            _ => None,
        }
    }

    fn function(&self, name: &str, path: &str, imported: &[NodeId]) -> Option<NodeId> {
        self.resolve(name, NodeKind::Function, path, imported)
    }

    fn class(&self, name: &str, path: &str, imported: &[NodeId]) -> Option<NodeId> {
        self.resolve(name, NodeKind::Class, path, imported)
    }

    fn same_place(&self, old: &Node) -> Option<NodeId> {
        if old.kind == NodeKind::File {
            return self.files.iter().find(|f| f.path == old.path).map(|f| f.id);
        }
        self.by_name
            .get(old.name.as_str())?
            .iter()
            .find(|n| n.kind == old.kind && n.path == old.path)
            .map(|n| n.id)
    }
}

/// Adjacency over a graph, built once for a batch of queries.
pub struct Index<'g> {
    graph: &'g Graph,
    at: HashMap<NodeId, usize>,
    by_name: HashMap<&'g str, Vec<usize>>,
    out: Vec<Vec<(usize, EdgeKind)>>,
    incoming: Vec<Vec<(usize, EdgeKind)>>,
}

impl<'g> Index<'g> {
    fn new(graph: &'g Graph) -> Self {
        let at: HashMap<NodeId, usize> = graph
            .nodes
            .iter()
            .enumerate()
            .map(|(i, n)| (n.id, i))
            .collect();
        let mut by_name: HashMap<&str, Vec<usize>> = HashMap::new();
        for (i, node) in graph.nodes.iter().enumerate() {
            by_name.entry(node.name.as_str()).or_default().push(i);
        }
        let mut out = vec![Vec::new(); graph.nodes.len()];
        let mut incoming = vec![Vec::new(); graph.nodes.len()];
        for edge in &graph.edges {
            if let (Some(&src), Some(&dst)) = (at.get(&edge.src), at.get(&edge.dst)) {
                out[src].push((dst, edge.kind));
                incoming[dst].push((src, edge.kind));
            }
        }
        Self {
            graph,
            at,
            by_name,
            out,
            incoming,
        }
    }

    fn node(&self, at: usize) -> &'g Node {
        &self.graph.nodes[at]
    }

    fn functions(&self, name: &str) -> Vec<usize> {
        self.by_name
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
        self.at.get(id).map(|&i| self.node(i))
    }

    /// The functions (or files, for a top-level call) that call a function named `name`.
    #[must_use]
    pub fn callers(&self, name: &str) -> Vec<&'g Node> {
        self.collect(self.functions(name).into_iter().flat_map(|f| {
            self.incoming[f]
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
            self.out[f]
                .iter()
                .filter(|(_, k)| *k == EdgeKind::Calls)
                .map(|(dst, _)| *dst)
                .filter(move |dst| *dst != f)
        }));
        found.sort_by_key(|n| n.kind == NodeKind::ExternalCall);
        found
    }

    fn callee_set(&self, at: usize) -> HashSet<usize> {
        self.out[at]
            .iter()
            .filter(|(_, k)| *k == EdgeKind::Calls)
            .map(|(dst, _)| *dst)
            .collect()
    }

    /// Up to three other functions whose callees overlap `name`'s by a Jaccard index of at least 0.5.
    #[must_use]
    pub fn similar(&self, name: &str) -> Vec<&'g Node> {
        let mine = self.functions(name);
        let mut scored: Vec<(f64, usize)> = Vec::new();
        for &f in &mine {
            let calls = self.callee_set(f);
            if calls.is_empty() {
                continue;
            }
            for (other, node) in self.graph.nodes.iter().enumerate() {
                if node.kind != NodeKind::Function || mine.contains(&other) {
                    continue;
                }
                let theirs = self.callee_set(other);
                if theirs.is_empty() {
                    continue;
                }
                let shared = calls.intersection(&theirs).count();
                let union = calls.union(&theirs).count();
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
            for (next, kind) in &self.out[at] {
                if *kind == EdgeKind::Calls && !previous.contains_key(next) {
                    previous.insert(*next, Some(at));
                    queue.push_back((*next, depth + 1));
                }
            }
        }
        Vec::new()
    }

    fn file(&self, path: &str) -> Option<usize> {
        self.by_name
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
            self.out[file]
                .iter()
                .filter(|(_, k)| *k == EdgeKind::Imports)
                .map(|(dst, _)| *dst),
        )
    }

    /// The external calls made from `path`.
    #[must_use]
    pub fn external_calls(&self, path: &str) -> Vec<&'g Node> {
        let callers = self
            .graph
            .nodes
            .iter()
            .enumerate()
            .filter(|(_, n)| n.path == path)
            .map(|(i, _)| i);
        let mut found = self.collect(callers.flat_map(|caller| {
            self.out[caller]
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
        let Some(&at) = self.at.get(node) else {
            return (Vec::new(), Vec::new());
        };
        let pick = |list: &[(usize, EdgeKind)]| {
            self.collect(list.iter().filter(|(_, k)| *k == kind).map(|(i, _)| *i))
        };
        (pick(&self.out[at]), pick(&self.incoming[at]))
    }

    /// Answers a wire query.
    #[must_use]
    pub fn query(&self, query: &domain::GraphQuery) -> Vec<&'g Node> {
        use domain::GraphQuery as Q;
        match query {
            Q::Callers(name) => self.callers(name),
            Q::Callees(name) => self.callees(name),
            Q::Similar(name) => self.similar(name),
            Q::Path(from, to) => self.path(from, to),
            Q::Imports(path) => self.imports(path),
            Q::ExternalCalls(path) => self.external_calls(path),
            _ => Vec::new(),
        }
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
            .graph
            .nodes
            .iter()
            .filter(|n| n.kind == kind && n.path == path)
            .filter(|n| n.line <= lines.1 && n.end_line >= lines.0)
            .collect();
        found.sort_by_key(|n| n.line);
        found
    }
}

#[cfg(test)]
mod tests;
