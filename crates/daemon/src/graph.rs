//! A project's codebase graph, kept current from whichever of its checkouts
//! asks. Git, parsing, SQLite and `cache.bin` all happen off the core lock;
//! one build per project at a time, and one per checkout across daemons.

use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use codebase_graph::{FileInput, Graph, Update};
use domain::{GraphFormat, GraphQuery, GraphStats, ProjectId, WorkspaceId};
use protocol::{ProtocolError, Response};

use crate::core::{db_err, Daemon};

const MAX_GRAPH_FILES: usize = 20_000;
const MAX_GRAPH_BYTES: u64 = 64 * 1024 * 1024;
const MAX_LISTING_BYTES: usize = 8 * 1024 * 1024;
const MAX_EXPORT_NODES: usize = 2_000;
pub(crate) const GRAPH_DIR: &str = ".forge/review/graph";

#[derive(Default)]
pub(crate) struct Graphs {
    loaded: Mutex<HashMap<ProjectId, Arc<Graph>>>,
    building: Mutex<HashMap<ProjectId, Arc<Mutex<()>>>>,
    /// Paths read from a dirty working tree by the last build, re-read by the
    /// next one even if the tree is clean again by then.
    dirty: Mutex<HashMap<ProjectId, Vec<String>>>,
    /// When each project's graph was last brought up to date by this daemon.
    built: Mutex<HashMap<ProjectId, std::time::Instant>>,
    pub(crate) answered: AtomicU64,
    pub(crate) rebuilds: AtomicU64,
}

impl Graphs {
    fn build_lock(&self, project: ProjectId) -> Arc<Mutex<()>> {
        let mut building = self
            .building
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Arc::clone(building.entry(project).or_default())
    }

    fn cached(&self, project: ProjectId) -> Option<Arc<Graph>> {
        self.loaded
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&project)
            .cloned()
    }

    fn keep(&self, project: ProjectId, graph: Arc<Graph>) {
        self.loaded
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(project, graph);
    }
}

fn unpoisoned<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl Daemon {
    /// The project's graph as of `checkout`'s `HEAD` and working tree:
    /// incremental from the stored one unless `full`, which re-parses everything.
    pub(crate) fn refresh_graph(
        &self,
        project: ProjectId,
        checkout: &Path,
        ignore: &[String],
        full: bool,
    ) -> Result<(Arc<Graph>, GraphStats), String> {
        let build = self.graphs.build_lock(project);
        let _building = unpoisoned(&build);
        let head = git_service::head_commit(checkout)
            .ok_or_else(|| "the checkout has no commit to index".to_owned())?;
        let stored = match self.graphs.cached(project) {
            Some(graph) => Some(graph),
            None => self
                .review_store
                .load_graph(project)
                .map_err(|error| format!("could not read the stored graph: {error}"))?
                .map(Arc::new),
        };
        let dir = graph_dir(checkout)?;
        let _lock = lock_index(&dir)?;
        let was_dirty = unpoisoned(&self.graphs.dirty)
            .remove(&project)
            .unwrap_or_default();
        let (base, listed, incremental) = match stored.filter(|_| !full) {
            Some(stored) => match changed_paths(checkout, &stored.git_oid, &head) {
                Some(mut paths) => {
                    for path in &was_dirty {
                        if !paths.contains(path) {
                            paths.push(path.clone());
                        }
                    }
                    (stored, paths, true)
                }
                None => (Arc::new(Graph::default()), list_files(checkout)?, false),
            },
            None => (Arc::new(Graph::default()), list_files(checkout)?, false),
        };
        let dirty = dirty_paths(checkout);
        let mut stats = GraphStats {
            git_oid: head.clone(),
            ..GraphStats::default()
        };
        if incremental && listed.is_empty() && base.git_oid == head {
            stats.nodes = count(base.nodes.len());
            stats.edges = count(base.edges.len());
            return Ok((base, stats));
        }
        let (update, read, removed) = parse_files(checkout, &listed, ignore, &mut stats);
        let mut graph = Graph::clone(&base);
        graph.apply(update, &removed, &head);
        let stored = if incremental {
            let touched: Vec<String> = read.iter().chain(&removed).cloned().collect();
            let touched_set: HashSet<&str> = touched.iter().map(String::as_str).collect();
            let ids: HashSet<codebase_graph::NodeId> = graph
                .nodes
                .iter()
                .filter(|n| touched_set.contains(n.path.as_str()))
                .map(|n| n.id)
                .collect();
            let edges: Vec<codebase_graph::Edge> = graph
                .edges
                .iter()
                .filter(|e| ids.contains(&e.src) || ids.contains(&e.dst))
                .copied()
                .collect();
            let reached: HashSet<codebase_graph::NodeId> =
                edges.iter().flat_map(|e| [e.src, e.dst]).collect();
            let nodes: Vec<codebase_graph::Node> = graph
                .nodes
                .iter()
                .filter(|n| {
                    ids.contains(&n.id)
                        || (n.kind == domain::NodeKind::ExternalCall && reached.contains(&n.id))
                })
                .cloned()
                .collect();
            self.review_store
                .replace_graph_paths(project, &head, &touched, &nodes, &edges)
        } else {
            self.review_store.replace_graph(project, &graph)
        };
        stored.map_err(|error| format!("could not store the graph: {error}"))?;
        write_cache(&dir, &graph)?;
        if !incremental || base.git_oid != head {
            self.refresh_architecture(project, &graph);
        }
        unpoisoned(&self.graphs.dirty).insert(project, dirty);
        self.graphs.rebuilds.fetch_add(1, Ordering::Relaxed);
        stats.nodes = count(graph.nodes.len());
        stats.edges = count(graph.edges.len());
        let graph = Arc::new(graph);
        self.graphs.keep(project, Arc::clone(&graph));
        unpoisoned(&self.graphs.built).insert(project, std::time::Instant::now());
        Ok((graph, stats))
    }

    /// The signatures, in the listed projects, of what the change's functions
    /// call that this project does not define: `<project> path name signature`
    /// lines, and a note per repository that could not be read.
    pub(crate) fn cross_repo_signatures(
        &self,
        workspace_root: &Path,
        repos: &[String],
        graph: &Graph,
        functions: &[review::GraphFunction],
        notes: &mut Vec<String>,
    ) -> Vec<String> {
        let projects: Vec<domain::Project> = self.lock().projects.values().cloned().collect();
        let mut others = Vec::new();
        for repo in repos {
            let by_path = std::fs::canonicalize(workspace_root.join(repo)).ok();
            let found = projects.iter().find(|project| {
                project.name == *repo
                    || by_path.as_ref().is_some_and(|path| {
                        std::fs::canonicalize(&project.root_path).ok().as_ref() == Some(path)
                    })
            });
            let Some(project) = found else {
                notes.push(format!("cross-repo repo '{repo}' not found"));
                continue;
            };
            match self.cross_repo_graph(project, notes) {
                Some(other) => others.push((project.name.clone(), other)),
                None => notes.push(format!("cross-repo repo '{repo}' has no graph")),
            }
        }
        let index = graph.index();
        let mut lines = Vec::new();
        let mut seen = HashSet::new();
        for function in functions {
            for callee in index.callees(&function.name) {
                if callee.kind != domain::NodeKind::ExternalCall
                    || !seen.insert(callee.name.clone())
                {
                    continue;
                }
                for (name, other) in &others {
                    for defined in other.nodes.iter().filter(|node| {
                        node.kind == domain::NodeKind::Function && node.name == callee.name
                    }) {
                        lines.push(format!(
                            "{name} {} {} {}",
                            defined.path, defined.name, defined.signature
                        ));
                    }
                }
            }
        }
        lines
    }

    /// Another project's graph for cross-repo use: rebuilt when this daemon
    /// has not brought it up to date in a day, the stored one otherwise or on failure.
    fn cross_repo_graph(
        &self,
        project: &domain::Project,
        notes: &mut Vec<String>,
    ) -> Option<Arc<Graph>> {
        const STALE: std::time::Duration = std::time::Duration::from_secs(24 * 3600);
        let fresh = unpoisoned(&self.graphs.built)
            .get(&project.id)
            .is_some_and(|at| at.elapsed() < STALE);
        let stored = self.graphs.cached(project.id).or_else(|| {
            self.review_store
                .load_graph(project.id)
                .ok()
                .flatten()
                .map(Arc::new)
        });
        if fresh && stored.is_some() {
            return stored;
        }
        let ignore = crate::review::repo_config(&project.root_path).ignore;
        match self.refresh_graph(project.id, &project.root_path, &ignore, false) {
            Ok((graph, _)) => Some(graph),
            Err(error) => {
                if stored.is_some() {
                    notes.push(format!(
                        "cross-repo repo '{}' could not be re-indexed ({error}); its older graph was used",
                        project.name
                    ));
                }
                stored
            }
        }
    }

    fn graph_target(
        &self,
        workspace_id: WorkspaceId,
    ) -> Result<(ProjectId, PathBuf, Vec<String>), ProtocolError> {
        let path = self.workspace_path(workspace_id)?;
        let project = self
            .lock()
            .workspaces
            .get(&workspace_id)
            .map(|ws| ws.project_id)
            .ok_or_else(|| ProtocolError::not_found("workspace"))?;
        let ignore = crate::review::repo_config(&path).ignore;
        Ok((project, path, ignore))
    }

    pub(crate) fn rebuild_graph(
        &self,
        workspace_id: WorkspaceId,
        full: bool,
    ) -> Result<Response, ProtocolError> {
        let (project, path, ignore) = self.graph_target(workspace_id)?;
        let (_, stats) = self
            .refresh_graph(project, &path, &ignore, full)
            .map_err(ProtocolError::invalid_request)?;
        Ok(Response::GraphRebuilt(stats))
    }

    /// The stored graph, built first when the project has none.
    fn stored_graph(&self, workspace_id: WorkspaceId) -> Result<Arc<Graph>, ProtocolError> {
        let (project, path, ignore) = self.graph_target(workspace_id)?;
        if let Some(graph) = self.graphs.cached(project) {
            return Ok(graph);
        }
        if let Some(graph) = self.review_store.load_graph(project).map_err(db_err)? {
            let graph = Arc::new(graph);
            self.graphs.keep(project, Arc::clone(&graph));
            return Ok(graph);
        }
        self.refresh_graph(project, &path, &ignore, false)
            .map(|(graph, _)| graph)
            .map_err(ProtocolError::invalid_request)
    }

    pub(crate) fn query_graph(
        &self,
        workspace_id: WorkspaceId,
        query: &GraphQuery,
    ) -> Result<Response, ProtocolError> {
        let graph = self.stored_graph(workspace_id)?;
        self.graphs.answered.fetch_add(1, Ordering::Relaxed);
        let nodes = graph
            .index()
            .query(query)
            .into_iter()
            .map(codebase_graph::Node::to_wire)
            .collect();
        Ok(Response::GraphNodes(nodes))
    }

    pub(crate) fn export_graph(
        &self,
        workspace_id: WorkspaceId,
        format: GraphFormat,
    ) -> Result<Response, ProtocolError> {
        let graph = self.stored_graph(workspace_id)?;
        Ok(Response::GraphExport(match format {
            GraphFormat::Mermaid => graph.to_mermaid(MAX_EXPORT_NODES),
            _ => graph.to_dot(MAX_EXPORT_NODES),
        }))
    }
}

fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// `<checkout>/.forge/review/graph`, created with a `.gitignore` of `*` so the
/// checkout stays clean; refused when it resolves outside the checkout.
fn graph_dir(checkout: &Path) -> Result<PathBuf, String> {
    let dir = checkout.join(GRAPH_DIR);
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&dir)
        .map_err(|error| format!("could not create {GRAPH_DIR}: {error}"))?;
    let root = std::fs::canonicalize(checkout).map_err(|error| error.to_string())?;
    let real = std::fs::canonicalize(&dir).map_err(|error| error.to_string())?;
    if !real.starts_with(&root) {
        return Err(format!("{GRAPH_DIR} resolves outside the checkout"));
    }
    let ignore = real.join(".gitignore");
    if !ignore.exists() {
        std::fs::write(&ignore, "*\n")
            .map_err(|error| format!("could not write {GRAPH_DIR}/.gitignore: {error}"))?;
    }
    Ok(real)
}

/// Held for the build: another daemon indexing the same checkout waits.
fn lock_index(dir: &Path) -> Result<nix::fcntl::Flock<std::fs::File>, String> {
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(0o600)
        .open(dir.join("index.lock"))
        .map_err(|error| format!("could not open the graph lock: {error}"))?;
    nix::fcntl::Flock::lock(file, nix::fcntl::FlockArg::LockExclusive)
        .map_err(|(_, error)| format!("could not lock the graph: {error}"))
}

fn write_cache(dir: &Path, graph: &Graph) -> Result<(), String> {
    let bytes = graph.encode()?;
    let temp = dir.join("cache.bin.tmp");
    std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&temp)
        .and_then(|mut file| file.write_all(&bytes))
        .and_then(|()| std::fs::rename(&temp, dir.join("cache.bin")))
        .map_err(|error| format!("could not write cache.bin: {error}"))
}

fn nul_paths(stdout: &str) -> Vec<String> {
    stdout
        .split('\0')
        .filter(|path| !path.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Tracked files plus untracked ones git does not ignore.
fn list_files(checkout: &Path) -> Result<Vec<String>, String> {
    let out = git_service::run_git_bounded(
        Some(checkout),
        &[
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ],
        MAX_LISTING_BYTES,
    )
    .map_err(|error| format!("git ls-files failed: {error}"))?;
    if !out.success() {
        return Err(format!("git ls-files failed: {}", out.stderr.trim()));
    }
    let mut paths = nul_paths(&out.stdout);
    paths.sort();
    paths.dedup();
    Ok(paths)
}

/// What changed between `old` and `head`, plus what the working tree changes;
/// `None` when `old` is not a commit this repository still has.
fn changed_paths(checkout: &Path, old: &str, head: &str) -> Option<Vec<String>> {
    let mut paths = if old == head {
        Vec::new()
    } else {
        let out = git_service::run_git_bounded(
            Some(checkout),
            &["diff", "--name-only", "-z", "--no-renames", old, head, "--"],
            MAX_LISTING_BYTES,
        )
        .ok()?;
        if !out.success() {
            return None;
        }
        nul_paths(&out.stdout)
    };
    for path in dirty_paths(checkout) {
        if !paths.contains(&path) {
            paths.push(path);
        }
    }
    Some(paths)
}

/// `git status` paths, renames as both sides.
fn dirty_paths(checkout: &Path) -> Vec<String> {
    let Ok(out) = git_service::run_git_bounded(
        Some(checkout),
        &["status", "--porcelain", "-z", "--untracked-files=all"],
        MAX_LISTING_BYTES,
    ) else {
        return Vec::new();
    };
    let mut paths = Vec::new();
    let mut records = out.stdout.split('\0');
    while let Some(record) = records.next() {
        let Some(path) = record.get(3..) else {
            continue;
        };
        paths.push(path.to_owned());
        if record.starts_with('R') || record.starts_with('C') {
            if let Some(from) = records.next() {
                paths.push(from.to_owned());
            }
        }
    }
    paths
}

/// Parsed files (as an update), the paths it read, and the paths that are gone.
fn parse_files(
    checkout: &Path,
    paths: &[String],
    ignore: &[String],
    stats: &mut GraphStats,
) -> (Update, Vec<String>, Vec<String>) {
    let mut update = Update::new();
    let mut read = Vec::new();
    let mut removed = Vec::new();
    let mut bytes = 0u64;
    for path in paths {
        if path.starts_with(".forge/review/graph/") || review::excluded_path(path, ignore) {
            removed.push(redacted(path));
            continue;
        }
        let full = checkout.join(path);
        let meta = match std::fs::symlink_metadata(&full) {
            Ok(meta) if meta.is_file() => meta,
            Ok(_) | Err(_) => {
                removed.push(redacted(path));
                continue;
            }
        };
        let cap = u64::try_from(fs_service::MAX_FILE_BYTES).unwrap_or(u64::MAX);
        if read.len() >= MAX_GRAPH_FILES
            || meta.len() > cap
            || bytes.saturating_add(meta.len()) > MAX_GRAPH_BYTES
        {
            stats.skipped.push(path.clone());
            continue;
        }
        let mut text = String::new();
        let readable = std::fs::File::open(&full)
            .and_then(|file| file.take(cap).read_to_string(&mut text))
            .is_ok();
        if !readable {
            stats.skipped.push(path.clone());
            continue;
        }
        bytes += meta.len();
        update.add(file_input(path, text));
        read.push(redacted(path));
    }
    stats.parsed = count(read.len());
    (update, read, removed)
}

fn redacted(text: &str) -> String {
    review::redact(text).0
}

fn file_input(path: &str, source: String) -> FileInput {
    let lang = languages::Registry::builtin()
        .language_for_path(path)
        .map(|language| language.id.clone())
        .unwrap_or_default();
    let symbols = syntax_index::symbols(path, &source).unwrap_or_default();
    FileInput {
        path: redacted(path),
        lang,
        definitions: symbols
            .definitions
            .into_iter()
            .map(|definition| codebase_graph::Definition {
                kind: definition.kind,
                name: redacted(&definition.name),
                span: definition.span,
                signature: redacted(&definition.signature),
            })
            .collect(),
        calls: symbols
            .calls
            .into_iter()
            .map(|call| (redacted(&call.name), call.span))
            .collect(),
        imports: symbols
            .imports
            .iter()
            .map(|import| redacted(import))
            .collect(),
        inherits: symbols
            .inherits
            .iter()
            .map(|(child, parent)| (redacted(child), redacted(parent)))
            .collect(),
        source,
    }
}

/// Each function a new-side hunk of an included file touches, with its
/// callers, callees and similar functions.
pub(crate) fn changed_functions(
    graph: &Graph,
    files: &[domain::DiffFile],
    included: &[String],
) -> Vec<review::GraphFunction> {
    let index = graph.index();
    let wire = |nodes: Vec<&codebase_graph::Node>| {
        nodes
            .into_iter()
            .map(codebase_graph::Node::to_wire)
            .collect()
    };
    let mut seen = HashSet::new();
    let mut found = Vec::new();
    for file in files.iter().filter(|file| included.contains(&file.path)) {
        let path = redacted(&file.path);
        for hunk in review::hunks(&file.patch) {
            for function in index.functions_in(&path, hunk) {
                if !seen.insert(function.id) {
                    continue;
                }
                found.push(review::GraphFunction {
                    path: function.path.clone(),
                    line: function.line,
                    name: function.name.clone(),
                    callers: wire(index.callers(&function.name)),
                    callees: wire(index.callees(&function.name)),
                    similar: wire(index.similar(&function.name)),
                });
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indexed_names_paths_and_signatures_are_redacted() {
        let token = "ghp_0123456789abcdefghijklmnopqrstuvwxyz";
        let source = format!("pub fn {token}() {{}}\nfn main() {{ {token}(); }}\n");
        let input = file_input(&format!("src/{token}.rs"), source);
        assert_eq!(input.path, "src/«redacted».rs");
        assert!(input
            .definitions
            .iter()
            .all(|d| !d.name.contains("ghp_") && !d.signature.contains("ghp_")));
        assert!(input.definitions.iter().any(|d| d.name == "«redacted»"));
        assert!(input.calls.iter().all(|(name, _)| name == "«redacted»"));
    }
}
