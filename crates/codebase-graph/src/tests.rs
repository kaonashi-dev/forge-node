use super::*;

/// A file whose definitions are found by their text: `("function", "fn open")`
/// spans from that text to the end of its line, or of the block it opens.
fn file(path: &str, source: &str, defs: &[(&str, &str, &str)], calls: &[&str]) -> FileInput {
    let definitions = defs
        .iter()
        .map(|(kind, name, start)| {
            let at = source.find(start).expect("definition text");
            let end = block_end(source, at);
            Definition {
                kind: (*kind).into(),
                name: (*name).into(),
                span: (at, end),
                signature: source[at..end].lines().next().unwrap_or_default().into(),
            }
        })
        .collect();
    let calls = calls
        .iter()
        .map(|name| {
            let needle = format!("{name}(");
            let at = source.find(&needle).expect("call text");
            ((*name).to_string(), (at, at + name.len()))
        })
        .collect();
    FileInput {
        path: path.into(),
        lang: "rust".into(),
        source: source.into(),
        definitions,
        calls,
        imports: Vec::new(),
        inherits: Vec::new(),
    }
}

/// The matching `}` of the first `{` on the line, else the first `;`, else the line's end.
fn block_end(source: &str, at: usize) -> usize {
    let line_end = source[at..].find('\n').map_or(source.len(), |n| at + n);
    let line_end = source[at..line_end]
        .find(';')
        .map_or(line_end, |n| at + n + 1);
    let Some(open) = source[at..line_end].find('{') else {
        return line_end;
    };
    let mut depth = 0;
    for (offset, ch) in source[at + open..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return at + open + offset + 1;
                }
            }
            _ => {}
        }
    }
    source.len()
}

fn build(files: Vec<FileInput>) -> Graph {
    let mut update = Update::new();
    for file in files {
        update.add(file);
    }
    let mut graph = Graph::default();
    graph.apply(update, &[], "oid1");
    graph
}

fn names(nodes: &[&Node]) -> Vec<String> {
    nodes
        .iter()
        .map(|n| format!("{}:{}", n.path, n.name))
        .collect()
}

fn has_edge(graph: &Graph, src: (&str, &str), dst: (&str, &str), kind: EdgeKind) -> bool {
    let find = |(path, name): (&str, &str)| {
        graph
            .nodes
            .iter()
            .find(|n| n.path == path && n.name == name)
            .map(|n| n.id)
    };
    let (Some(src), Some(dst)) = (find(src), find(dst)) else {
        return false;
    };
    graph
        .edges
        .iter()
        .any(|e| e.src == src && e.dst == dst && e.kind == kind)
}

const STORE: &str =
    "pub const LIMIT: usize = 4;\npub fn open() { check(LIMIT); }\nfn check(n: usize) {}\n";
const APP: &str = "use crate::store;\nfn main() { open(); log(); }\nfn log() { println(); }\n";

fn project() -> Vec<FileInput> {
    let mut app = file(
        "src/app.rs",
        APP,
        &[
            ("function", "main", "fn main"),
            ("function", "log", "fn log"),
        ],
        &["open", "log", "println"],
    );
    app.imports = vec!["crate::store".into()];
    vec![
        file(
            "src/store.rs",
            STORE,
            &[
                ("constant", "LIMIT", "pub const LIMIT"),
                ("function", "open", "pub fn open"),
                ("function", "check", "fn check"),
            ],
            &["check"],
        ),
        app,
    ]
}

#[test]
fn files_define_their_top_level_items() {
    let graph = build(project());
    assert!(has_edge(
        &graph,
        ("src/store.rs", "src/store.rs"),
        ("src/store.rs", "open"),
        EdgeKind::Defines
    ));
    assert!(has_edge(
        &graph,
        ("src/store.rs", "src/store.rs"),
        ("src/store.rs", "LIMIT"),
        EdgeKind::Defines
    ));
    let open = graph.nodes.iter().find(|n| n.name == "open").expect("open");
    assert_eq!(
        (open.kind, open.line, open.end_line),
        (NodeKind::Function, 2, 2)
    );
    assert_eq!(open.signature, "pub fn open() { check(LIMIT); }");
}

#[test]
fn a_call_resolves_in_its_file_then_through_imports_then_project_wide() {
    let graph = build(project());
    assert!(has_edge(
        &graph,
        ("src/store.rs", "open"),
        ("src/store.rs", "check"),
        EdgeKind::Calls
    ));
    assert!(has_edge(
        &graph,
        ("src/app.rs", "main"),
        ("src/store.rs", "open"),
        EdgeKind::Calls
    ));
    assert!(has_edge(
        &graph,
        ("src/app.rs", "main"),
        ("src/app.rs", "log"),
        EdgeKind::Calls
    ));
    assert!(has_edge(
        &graph,
        ("src/app.rs", "log"),
        ("", "println"),
        EdgeKind::Calls
    ));
    let external = graph
        .nodes
        .iter()
        .filter(|n| n.kind == NodeKind::ExternalCall)
        .count();
    assert_eq!(external, 1);
}

#[test]
fn an_import_names_a_file_by_stem() {
    let graph = build(project());
    assert!(has_edge(
        &graph,
        ("src/app.rs", "src/app.rs"),
        ("src/store.rs", "src/store.rs"),
        EdgeKind::Imports
    ));
    assert_eq!(
        names(&graph.index().imports("src/app.rs")),
        ["src/store.rs:src/store.rs"]
    );
}

#[test]
fn a_function_uses_the_module_variables_it_names() {
    let graph = build(project());
    assert!(has_edge(
        &graph,
        ("src/store.rs", "open"),
        ("src/store.rs", "LIMIT"),
        EdgeKind::Uses
    ));
    assert!(!has_edge(
        &graph,
        ("src/store.rs", "check"),
        ("src/store.rs", "LIMIT"),
        EdgeKind::Uses
    ));
}

#[test]
fn a_class_declares_its_members_and_inherits_its_parent() {
    let source =
        "class Base { run() {} }\nclass Disk extends Base { flush() { const tmp = 1; } }\n";
    let mut input = file(
        "web/disk.ts",
        source,
        &[
            ("class", "Base", "class Base"),
            ("method", "run", "run()"),
            ("class", "Disk", "class Disk"),
            ("method", "flush", "flush()"),
            ("variable", "tmp", "const tmp"),
        ],
        &[],
    );
    input.inherits = vec![("Disk".into(), "Base".into())];
    let graph = build(vec![input]);
    assert!(has_edge(
        &graph,
        ("web/disk.ts", "Disk"),
        ("web/disk.ts", "flush"),
        EdgeKind::Declares
    ));
    assert!(has_edge(
        &graph,
        ("web/disk.ts", "Disk"),
        ("web/disk.ts", "Base"),
        EdgeKind::Inherits
    ));
    assert!(
        !graph.nodes.iter().any(|n| n.name == "tmp"),
        "a function's local is not a node"
    );
}

#[test]
fn callers_callees_and_external_calls_answer_from_the_edges() {
    let graph = build(project());
    let index = graph.index();
    assert_eq!(names(&index.callers("open")), ["src/app.rs:main"]);
    assert_eq!(
        names(&index.callees("main")),
        ["src/app.rs:log", "src/store.rs:open"]
    );
    assert_eq!(names(&index.external_calls("src/app.rs")), [":println"]);
    assert!(index.callers("nobody").is_empty());
}

#[test]
fn similar_functions_share_most_callees() {
    let source = "fn a() { x(); y(); z(); }\nfn b() { x(); y(); }\nfn c() { w(); }\nfn x() {}\nfn y() {}\nfn z() {}\nfn w() {}\n";
    let mut input = file(
        "src/lib.rs",
        source,
        &[
            ("function", "a", "fn a"),
            ("function", "b", "fn b"),
            ("function", "c", "fn c"),
            ("function", "x", "fn x"),
            ("function", "y", "fn y"),
            ("function", "z", "fn z"),
            ("function", "w", "fn w"),
        ],
        &[],
    );
    for (name, nth) in [("x", 0), ("y", 0), ("z", 0), ("x", 1), ("y", 1), ("w", 0)] {
        let at = source
            .match_indices(&format!("{name}()"))
            .nth(nth)
            .expect("call")
            .0;
        input.calls.push((name.into(), (at, at + 1)));
    }
    let graph = build(vec![input]);
    assert_eq!(names(&graph.index().similar("a")), ["src/lib.rs:b"]);
    assert!(graph.index().similar("c").is_empty());
}

#[test]
fn a_path_follows_calls_within_six_hops() {
    let graph = build(project());
    assert_eq!(
        names(&graph.index().path("main", "check")),
        ["src/app.rs:main", "src/store.rs:open", "src/store.rs:check"]
    );
    assert!(graph.index().path("check", "main").is_empty());
}

#[test]
fn an_update_replaces_one_file_and_keeps_calls_into_it() {
    let mut graph = build(project());
    let shifted = format!("\n\n{STORE}");
    let mut update = Update::new();
    update.add(file(
        "src/store.rs",
        &shifted,
        &[
            ("constant", "LIMIT", "pub const LIMIT"),
            ("function", "open", "pub fn open"),
            ("function", "check", "fn check"),
        ],
        &["check"],
    ));
    graph.apply(update, &[], "oid2");
    assert_eq!(graph.git_oid, "oid2");
    assert!(has_edge(
        &graph,
        ("src/app.rs", "main"),
        ("src/store.rs", "open"),
        EdgeKind::Calls
    ));
    let open = graph.nodes.iter().find(|n| n.name == "open").expect("open");
    assert_eq!(open.line, 4);
    assert_eq!(graph.nodes.iter().filter(|n| n.name == "open").count(), 1);
}

#[test]
fn removing_a_file_turns_calls_into_it_external() {
    let mut graph = build(project());
    graph.apply(Update::new(), &["src/store.rs".into()], "oid3");
    assert!(!graph.nodes.iter().any(|n| n.path == "src/store.rs"));
    assert!(has_edge(
        &graph,
        ("src/app.rs", "main"),
        ("", "open"),
        EdgeKind::Calls
    ));
    assert!(!graph
        .index()
        .imports("src/app.rs")
        .iter()
        .any(|n| n.path == "src/store.rs"));
}

#[test]
fn the_cache_format_round_trips() {
    let graph = build(project());
    let bytes = graph.encode().expect("encode");
    assert_eq!(Graph::decode(&bytes).expect("decode"), graph);
    assert!(Graph::decode(b"not a graph").is_err());
}

#[test]
fn functions_in_finds_what_a_hunk_touches() {
    let graph = build(project());
    assert_eq!(
        names(&graph.index().functions_in("src/store.rs", (2, 3))),
        ["src/store.rs:open", "src/store.rs:check"]
    );
    assert!(graph
        .index()
        .functions_in("src/store.rs", (1, 1))
        .is_empty());
}

#[test]
fn a_build_is_deterministic() {
    assert_eq!(build(project()), build(project()));
}

#[test]
fn exports_name_nodes_and_edges_and_say_what_the_cap_left_out() {
    let graph = build(project());
    let dot = graph.to_dot(2_000);
    assert!(dot.starts_with("digraph codebase {"));
    assert!(dot.contains("src/store.rs:2 open"));
    assert!(dot.contains("[label=\"Calls\"]"));
    assert!(!dot.contains("left out"));
    let mermaid = graph.to_mermaid(2);
    assert!(mermaid.starts_with("flowchart LR\n"));
    assert!(mermaid.contains(&format!(
        "%% {} node(s) left out past 2",
        graph.nodes.len() - 2
    )));
}
