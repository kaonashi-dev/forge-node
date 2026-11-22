use super::*;

fn graph_of(files: &[(&str, &str)]) -> Graph {
    let mut update = codebase_graph::Update::new();
    for (path, source) in files {
        let symbols = syntax_index::symbols(path, source).unwrap_or_default();
        update.add(codebase_graph::FileInput {
            path: (*path).into(),
            lang: String::new(),
            source: (*source).into(),
            definitions: symbols
                .definitions
                .into_iter()
                .map(|d| codebase_graph::Definition {
                    kind: d.kind,
                    name: d.name,
                    span: d.span,
                    signature: d.signature,
                })
                .collect(),
            calls: symbols
                .calls
                .into_iter()
                .map(|c| (c.name, c.span))
                .collect(),
            imports: symbols.imports,
            inherits: symbols.inherits,
        });
    }
    let mut graph = Graph::default();
    graph.apply(update, &[], "oid");
    graph
}

/// The file as changed: every line from `from` to `to` (1-based) shown as added.
fn changed(path: &str, text: &str, from: u32, to: u32) -> ChangedFile {
    let lines: Vec<&str> = text.lines().collect();
    let mut patch = format!(
        "--- a/{path}\n+++ b/{path}\n@@ -{from},0 +{from},{} @@\n",
        to - from + 1
    );
    for line in &lines[(from - 1) as usize..to as usize] {
        patch.push('+');
        patch.push_str(line);
        patch.push('\n');
    }
    ChangedFile {
        path: path.into(),
        patch,
        text: Some(text.into()),
    }
}

const LIB: &str = "pub fn total(a: u32, b: u32) -> u32 {\n    a + b\n}\n";
const MAIN: &str = "mod lib;\n\nfn main() {\n    let sum = lib::total(2, 3);\n    show(sum);\n}\n\nfn show(n: u32) {}\n";

fn kinds(diagrams: &[Diagram]) -> Vec<DiagramKind> {
    diagrams.iter().map(|d| d.kind).collect()
}

#[test]
fn a_cross_file_call_is_a_sequence_between_modules() {
    let graph = graph_of(&[("lib.rs", LIB), ("main.rs", MAIN)]);
    let diagrams = generate(&[changed("lib.rs", LIB, 1, 3)], &graph);
    assert_eq!(kinds(&diagrams), [DiagramKind::Sequence]);
    let sequence = &diagrams[0];
    assert_eq!(
        sequence
            .nodes
            .iter()
            .map(|n| n.id.as_str())
            .collect::<Vec<_>>(),
        ["main.rs", "lib.rs"]
    );
    assert_eq!(sequence.edges, [edge("main.rs", "lib.rs", "total")]);
    assert!(sequence.to_mermaid().starts_with("sequenceDiagram\n"));
}

#[test]
fn a_migration_is_an_er_diagram_with_its_columns_and_references() {
    let sql = "CREATE TABLE users (\n  id INTEGER PRIMARY KEY,\n  email TEXT\n);\nCREATE TABLE orders (\n  id INTEGER,\n  user_id INTEGER REFERENCES users(id),\n  PRIMARY KEY (id)\n);\n";
    let diagrams = generate(
        &[changed("migrations/001.sql", sql, 1, 9)],
        &Graph::default(),
    );
    assert_eq!(kinds(&diagrams), [DiagramKind::Er]);
    let er = &diagrams[0];
    assert_eq!(er.nodes[0].label, "users\nid\nemail");
    assert_eq!(er.nodes[1].label, "orders\nid\nuser_id");
    assert_eq!(er.edges, [edge("orders", "users", "user_id")]);
}

#[test]
fn a_table_like_struct_is_an_entity() {
    let text = "pub struct UserRecord {\n    pub id: u64,\n    pub email: String,\n}\n";
    let graph = graph_of(&[("src/db.rs", text)]);
    let er =
        generate_kind(&[changed("src/db.rs", text, 1, 4)], &graph, DiagramKind::Er).expect("er");
    assert_eq!(er.nodes[0].label, "UserRecord\nid\nemail");
}

#[test]
fn a_changed_type_with_a_parent_is_a_class_diagram() {
    let text =
        "class Base { run() { return 1; } }\nclass Disk extends Base { flush() { return 2; } }\n";
    let graph = graph_of(&[("web/disk.ts", text)]);
    let diagrams = generate(&[changed("web/disk.ts", text, 2, 2)], &graph);
    assert_eq!(kinds(&diagrams), [DiagramKind::Class]);
    assert_eq!(diagrams[0].edges, [edge("Disk", "Base", "inherits")]);
    assert!(diagrams[0].to_mermaid().contains("Base <|-- Disk"));
}

const BRANCHY: &str = "fn pick(n: u32) -> u32 {\n    if n == 0 {\n        return 0;\n    } else if n == 1 {\n        return 1;\n    }\n    match n {\n        2 => 4,\n        3 => 9,\n        _ => n,\n    }\n}\n";

#[test]
fn a_function_with_more_than_three_branches_is_a_flow() {
    let graph = graph_of(&[("src/pick.rs", BRANCHY)]);
    let diagrams = generate(&[changed("src/pick.rs", BRANCHY, 2, 3)], &graph);
    assert_eq!(kinds(&diagrams), [DiagramKind::Flow]);
    let flow = &diagrams[0];
    assert_eq!(flow.nodes[0], node("start", "pick"));
    assert_eq!(
        flow.nodes[1..]
            .iter()
            .map(|n| n.id.as_str())
            .collect::<Vec<_>>(),
        ["l2", "l4", "l8", "l9", "l10"]
    );
    assert_eq!(flow.edges.len(), flow.nodes.len() - 1);
    assert!(flow.to_mermaid().starts_with("flowchart TD\n"));
    let simple = "fn one(n: u32) -> u32 {\n    if n > 1 { n } else { 0 }\n}\n";
    let one = graph_of(&[("src/one.rs", simple)]);
    assert!(generate(&[changed("src/one.rs", simple, 1, 3)], &one).is_empty());
}

#[test]
fn docs_and_whitespace_only_changes_draw_nothing() {
    let graph = graph_of(&[("lib.rs", LIB), ("main.rs", MAIN)]);
    assert!(generate(
        &[changed("docs/guide.md", "# a\nif if if if\n", 1, 2)],
        &graph
    )
    .is_empty());
    let restyled = ChangedFile {
        path: "lib.rs".into(),
        patch: "@@ -1,3 +1,3 @@\n-pub fn total(a: u32, b: u32) -> u32 {\n+pub fn total(a: u32,  b: u32) -> u32 {\n     a + b\n }\n".into(),
        text: Some(LIB.into()),
    };
    assert!(generate(&[restyled], &graph).is_empty());
}

#[test]
fn a_tests_only_change_draws_at_most_one_flow() {
    let graph = graph_of(&[("tests/pick.rs", BRANCHY), ("lib.rs", LIB)]);
    let diagrams = generate(&[changed("tests/pick.rs", BRANCHY, 1, 12)], &graph);
    assert_eq!(kinds(&diagrams), [DiagramKind::Flow]);
}

#[test]
fn a_file_without_a_parser_draws_only_a_flow_from_its_hunk() {
    let script = "case \"$1\" in\n  a) if true; then echo; fi ;;\n  b) if false; then echo; fi ;;\nesac\nif x; then y; fi\n";
    let graph = graph_of(&[("run.sh", script)]);
    let diagrams = generate(&[changed("run.sh", script, 1, 5)], &graph);
    assert_eq!(kinds(&diagrams), [DiagramKind::Flow]);
    assert_eq!(diagrams[0].title, "Branches in run.sh");
}

#[test]
fn at_most_two_diagrams_in_priority_order() {
    let sql = "CREATE TABLE users (\n  id INTEGER\n);\n";
    let main_branchy = format!("{MAIN}{BRANCHY}");
    let graph = graph_of(&[("lib.rs", LIB), ("main.rs", &main_branchy)]);
    let files = [
        changed("lib.rs", LIB, 1, 3),
        changed("migrations/1.sql", sql, 1, 3),
        changed("main.rs", &main_branchy, 9, 20),
    ];
    assert_eq!(
        kinds(&generate(&files, &graph)),
        [DiagramKind::Sequence, DiagramKind::Er]
    );
    assert!(generate_kind(&files, &graph, DiagramKind::Flow).is_some());
}

#[test]
fn the_same_input_draws_the_same_diagrams() {
    let graph = graph_of(&[("lib.rs", LIB), ("main.rs", MAIN)]);
    let files = [changed("lib.rs", LIB, 1, 3), changed("main.rs", MAIN, 3, 6)];
    assert_eq!(generate(&files, &graph), generate(&files, &graph));
}
