use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use fs_service::definition::{Answer, Confidence, DefinitionQuery, Position, Target};

use super::*;

struct Repo {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

fn repo(files: &[(&str, &str)]) -> Repo {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().canonicalize().expect("canonical root");
    let status = Command::new("git")
        .args(["init", "-q"])
        .current_dir(&root)
        .status()
        .expect("git init");
    assert!(status.success());
    let repo = Repo { _dir: dir, root };
    for (path, text) in files {
        write(&repo.root, path, text);
    }
    repo
}

fn plain_dir(files: &[(&str, &str)]) -> Repo {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().canonicalize().expect("canonical root");
    for (path, text) in files {
        write(&root, path, text);
    }
    Repo { _dir: dir, root }
}

fn write(root: &Path, path: &str, text: &str) {
    let full = root.join(path);
    std::fs::create_dir_all(full.parent().expect("parent")).expect("mkdir");
    std::fs::write(full, text).expect("write fixture");
}

struct Ask<'a> {
    source: &'a str,
    symbol: &'a str,
    position: Option<Position>,
    buffer: Option<&'a str>,
    limit: usize,
}

fn ask(source: &str, symbol: &str) -> Ask<'static> {
    Ask {
        source: Box::leak(source.to_string().into_boxed_str()),
        symbol: Box::leak(symbol.to_string().into_boxed_str()),
        position: None,
        buffer: None,
        limit: 50,
    }
}

impl Ask<'_> {
    fn run(&self, provider: &SyntaxProvider, root: &Path) -> Answer {
        let cancel = AtomicBool::new(false);
        self.run_with(provider, root, &cancel)
    }

    fn run_with(&self, provider: &SyntaxProvider, root: &Path, cancel: &AtomicBool) -> Answer {
        provider
            .find(&DefinitionQuery {
                root,
                source: self.source,
                symbol: self.symbol,
                position: self.position,
                buffer: self.buffer,
                limit: self.limit,
                deadline: Instant::now() + Duration::from_secs(20),
                cancel,
            })
            .expect("lookup")
    }
}

/// The caret at the start of the `nth` (from 0) occurrence of `needle`.
fn caret(text: &str, needle: &str, nth: usize) -> Position {
    let byte = text
        .match_indices(needle)
        .nth(nth)
        .map(|(at, _)| at)
        .expect("needle in fixture");
    let line_start = text[..byte].rfind('\n').map_or(0, |at| at + 1);
    Position {
        line: u32::try_from(text[..byte].matches('\n').count() + 1).expect("line"),
        utf16_column: fs_service::definition::utf16_column(&text[line_start..], byte - line_start),
    }
}

fn places(answer: &Answer) -> Vec<(String, u32)> {
    answer
        .candidates
        .iter()
        .map(|candidate| match &candidate.target {
            Target::Workspace(path) => (path.clone(), candidate.line),
            Target::External(path) => (path.display().to_string(), candidate.line),
        })
        .collect()
}

fn line_of(text: &str, needle: &str) -> u32 {
    let byte = text.find(needle).expect("needle in fixture");
    u32::try_from(text[..byte].matches('\n').count() + 1).expect("line")
}

/// Every symbol is found at the line of its declaring needle, from `source`.
fn assert_declares(files: &[(&str, &str)], source: &str, expected: &[(&str, &str, &str)]) {
    let repo = repo(files);
    let provider = SyntaxProvider::new();
    for (symbol, path, needle) in expected {
        let text = files
            .iter()
            .find(|(p, _)| p == path)
            .map(|(_, t)| *t)
            .expect("fixture file");
        let answer = ask(source, symbol).run(&provider, &repo.root);
        assert_eq!(
            places(&answer),
            vec![((*path).to_string(), line_of(text, needle))],
            "{symbol} from {source}"
        );
        assert!(answer.authoritative, "{symbol}");
        assert_eq!(answer.candidates[0].confidence, Confidence::Syntactic);
    }
}

#[test]
fn every_embedded_query_compiles() {
    for grammar in GRAMMARS {
        let compiled =
            grammars::compile(grammar).unwrap_or_else(|error| panic!("{}: {error}", grammar.id));
        assert!(
            compiled.tags.query.capture_index_for_name("name").is_some(),
            "{}",
            grammar.id
        );
        assert!(compiled.locals.is_some(), "{}", grammar.id);
    }
    assert_eq!(
        grammar_ids(),
        ["rust", "typescript", "tsx", "javascript", "python", "go"]
    );
}

#[test]
fn every_grammar_the_language_table_names_is_compiled_in() {
    for language in Registry::builtin().languages() {
        if let Some(grammar) = &language.grammar {
            assert!(grammar_ids().contains(&grammar.as_str()), "{grammar}");
        }
    }
    assert!(supports("src/main.rs"));
    assert!(supports("web/app.tsx"));
    assert!(!supports("build.gradle.kts"));
    assert!(!supports("README.md"));
}

const RUST: &str = r#"pub struct Store { pub items: Vec<u8> }
pub enum Mode { Fast, Slow(u8) }
pub union Bits { a: u32 }
pub trait Render { type Output; fn render(&self); }
pub type Alias = Store;
pub const LIMIT: usize = 4;
static mut COUNTER: u32 = 0;
impl Store { pub fn open() -> Self { Store { items: vec![] } } }
pub fn helper() {}
mod inner {}
macro_rules! shout { () => {}; }
"#;

#[test]
fn rust_items_variants_fields_and_constants_are_definitions() {
    assert_declares(
        &[("src/lib.rs", RUST), ("src/main.rs", "fn main() {}\n")],
        "src/main.rs",
        &[
            ("Store", "src/lib.rs", "pub struct Store"),
            ("items", "src/lib.rs", "pub items"),
            ("Mode", "src/lib.rs", "pub enum Mode"),
            ("Fast", "src/lib.rs", "Fast"),
            ("Slow", "src/lib.rs", "Slow"),
            ("Bits", "src/lib.rs", "union Bits"),
            ("Render", "src/lib.rs", "trait Render"),
            ("Output", "src/lib.rs", "type Output"),
            ("render", "src/lib.rs", "fn render"),
            ("Alias", "src/lib.rs", "type Alias"),
            ("LIMIT", "src/lib.rs", "const LIMIT"),
            ("COUNTER", "src/lib.rs", "static mut COUNTER"),
            ("open", "src/lib.rs", "fn open"),
            ("helper", "src/lib.rs", "fn helper"),
            ("inner", "src/lib.rs", "mod inner"),
            ("shout", "src/lib.rs", "macro_rules! shout"),
        ],
    );
}

const TYPESCRIPT: &str = r#"export interface Shape { area(): number; label: string }
export type Id = string;
export enum Color { Red, Green = 2 }
export abstract class Base { abstract draw(): void; }
export class Circle extends Base { radius = 1; draw() {} }
export const LIMIT = 4;
export function make(): Circle { return new Circle(); }
export const build = (n: number) => n;
declare function external(x: number): void;
namespace Space { }
"#;

#[test]
fn typescript_interfaces_aliases_enums_classes_and_constants_are_definitions() {
    let expected = [
        ("Shape", "src/shapes.ts", "interface Shape"),
        ("area", "src/shapes.ts", "area()"),
        ("label", "src/shapes.ts", "label:"),
        ("Id", "src/shapes.ts", "type Id"),
        ("Color", "src/shapes.ts", "enum Color"),
        ("Red", "src/shapes.ts", "Red"),
        ("Green", "src/shapes.ts", "Green"),
        ("Base", "src/shapes.ts", "abstract class Base"),
        ("Circle", "src/shapes.ts", "class Circle"),
        ("radius", "src/shapes.ts", "radius"),
        ("LIMIT", "src/shapes.ts", "const LIMIT"),
        ("make", "src/shapes.ts", "function make"),
        ("build", "src/shapes.ts", "const build"),
        ("external", "src/shapes.ts", "function external"),
        ("Space", "src/shapes.ts", "namespace Space"),
    ];
    assert_declares(
        &[
            ("src/shapes.ts", TYPESCRIPT),
            ("src/view.tsx", "export const View = () => <div />;\n"),
        ],
        "src/view.tsx",
        &expected,
    );
    let mut draw = ask("src/view.tsx", "draw").run(
        &SyntaxProvider::new(),
        &repo(&[("src/shapes.ts", TYPESCRIPT)]).root,
    );
    draw.candidates.sort_by_key(|c| c.line);
    assert_eq!(draw.candidates.len(), 2, "abstract signature and override");
    assert_declares(
        &[
            (
                "src/view.tsx",
                "export function View() { return <div />; }\n",
            ),
            ("src/main.ts", "View();\n"),
        ],
        "src/main.ts",
        &[("View", "src/view.tsx", "function View")],
    );
}

const JAVASCRIPT: &str = r#"export class Widget { size = 2; paint() {} }
export function mount() {}
const handlers = { onClick: () => {} };
export const VERSION = "1";
var legacy = 1;
"#;

#[test]
fn javascript_classes_functions_fields_and_constants_are_definitions() {
    assert_declares(
        &[
            ("lib/widget.js", JAVASCRIPT),
            ("lib/main.mjs", "mount();\n"),
        ],
        "lib/main.mjs",
        &[
            ("Widget", "lib/widget.js", "class Widget"),
            ("size", "lib/widget.js", "size"),
            ("paint", "lib/widget.js", "paint"),
            ("mount", "lib/widget.js", "function mount"),
            ("onClick", "lib/widget.js", "onClick"),
            ("handlers", "lib/widget.js", "const handlers"),
            ("VERSION", "lib/widget.js", "const VERSION"),
            ("legacy", "lib/widget.js", "var legacy"),
        ],
    );
}

const PYTHON: &str = r#"LIMIT = 4
first, second = 1, 2

class Store:
    kind = "store"

    def open(self):
        return self

def helper():
    pass
"#;

#[test]
fn python_classes_functions_and_module_constants_are_definitions() {
    assert_declares(
        &[("pkg/store.py", PYTHON), ("pkg/main.py", "helper()\n")],
        "pkg/main.py",
        &[
            ("LIMIT", "pkg/store.py", "LIMIT"),
            ("second", "pkg/store.py", "first, second"),
            ("Store", "pkg/store.py", "class Store"),
            ("kind", "pkg/store.py", "kind"),
            ("open", "pkg/store.py", "def open"),
            ("helper", "pkg/store.py", "def helper"),
        ],
    );
}

const GO: &str = r#"package store

const Limit = 4
var Default = New()

type Store struct {
	Items []byte
}

type Opener interface {
	Open() error
}

type ID = string

func New() *Store { return &Store{} }

func (s *Store) Close() error { return nil }
"#;

#[test]
fn go_types_functions_methods_and_package_names_are_definitions() {
    assert_declares(
        &[("store/store.go", GO), ("cmd/main.go", "package main\n")],
        "cmd/main.go",
        &[
            ("Limit", "store/store.go", "Limit"),
            ("Default", "store/store.go", "Default"),
            ("Store", "store/store.go", "type Store"),
            ("Items", "store/store.go", "Items"),
            ("Opener", "store/store.go", "type Opener"),
            ("Open", "store/store.go", "Open()"),
            ("ID", "store/store.go", "type ID"),
            ("New", "store/store.go", "func New"),
            ("Close", "store/store.go", "Close"),
        ],
    );
}

#[test]
fn a_declaration_in_a_comment_or_a_string_is_not_one() {
    let repo = repo(&[
        (
            "src/a.rs",
            "// fn ghost() {}\nconst NOTE: &str = \"fn ghost() {}\";\n/* struct ghost; */\n",
        ),
        ("src/b.py", "# def ghost(): pass\nx = 'def ghost(): pass'\n"),
    ]);
    let answer = ask("src/a.rs", "ghost").run(&SyntaxProvider::new(), &repo.root);
    assert!(answer.candidates.is_empty(), "{answer:?}");
    assert!(!answer.authoritative);
}

#[test]
fn the_buffer_overrides_the_saved_source() {
    let saved = "fn main() {}\n";
    let edited = "fn main() {}\n\nfn fresh() {}\n";
    let repo = repo(&[("src/main.rs", saved), ("src/other.rs", "fn other() {}\n")]);
    let provider = SyntaxProvider::new();
    let mut query = ask("src/main.rs", "fresh");
    query.buffer = Some(edited);
    let answer = query.run(&provider, &repo.root);
    assert_eq!(places(&answer), vec![("src/main.rs".to_string(), 3)]);

    let mut query = ask("src/other.rs", "main");
    query.buffer = Some("fn other() {}\n");
    assert_eq!(
        places(&query.run(&provider, &repo.root)),
        vec![("src/main.rs".to_string(), 1)]
    );

    let mut query = ask("src/main.rs", "main");
    query.buffer = Some("\n\nfn main() {}\n");
    assert_eq!(
        places(&query.run(&provider, &repo.root)),
        vec![("src/main.rs".to_string(), 3)],
        "the stale saved copy is not offered beside the buffer"
    );
}

fn local(files: &[(&str, &str)], source: &str, symbol: &str, at: (&str, usize)) -> Answer {
    let repo = repo(files);
    let text = files
        .iter()
        .find(|(path, _)| *path == source)
        .map(|(_, text)| *text)
        .expect("source fixture");
    let mut query = ask(source, symbol);
    query.position = Some(caret(text, at.0, at.1));
    query.run(&SyntaxProvider::new(), &repo.root)
}

fn assert_local(answer: &Answer, expected: &[(&str, u32)]) {
    let expected: Vec<(String, u32)> = expected
        .iter()
        .map(|(path, line)| ((*path).to_string(), *line))
        .collect();
    assert_eq!(places(answer), expected, "{answer:?}");
    assert!(answer.authoritative);
    assert!(answer
        .candidates
        .iter()
        .all(|c| c.confidence == Confidence::Local));
}

#[test]
fn a_rust_parameter_or_let_shadows_the_global_item() {
    let text = "fn value() -> u8 { 1 }\n\nfn first(value: u8) -> u8 {\n    value + 1\n}\n\nfn second() -> u8 {\n    let value = 2;\n    let value = value + 1;\n    value\n}\n\nfn third() -> u8 {\n    value()\n}\n";
    let files = [("src/lib.rs", text)];
    assert_local(
        &local(&files, "src/lib.rs", "value", ("value + 1", 0)),
        &[("src/lib.rs", 3)],
    );
    assert_local(
        &local(&files, "src/lib.rs", "value", ("value + 1", 1)),
        &[("src/lib.rs", 8)],
    );
    assert_local(
        &local(&files, "src/lib.rs", "value", ("value\n}", 0)),
        &[("src/lib.rs", 9)],
    );
    assert_local(
        &local(&files, "src/lib.rs", "value", ("value()", 1)),
        &[("src/lib.rs", 1)],
    );
}

#[test]
fn rust_patterns_closures_and_match_arms_bind_locals_but_variants_do_not() {
    let text = "fn run(input: Option<u8>) {\n    if let Some(inner) = input {\n        drop(inner);\n    }\n    let f = |item: u8| item;\n    match input {\n        None => {}\n        Some(other) => drop(other),\n    }\n}\n";
    let files = [("src/lib.rs", text)];
    assert_local(
        &local(&files, "src/lib.rs", "inner", ("inner)", 1)),
        &[("src/lib.rs", 2)],
    );
    assert_local(
        &local(&files, "src/lib.rs", "item", ("item;", 0)),
        &[("src/lib.rs", 5)],
    );
    assert_local(
        &local(&files, "src/lib.rs", "other", ("other)", 1)),
        &[("src/lib.rs", 8)],
    );
    let none = local(&files, "src/lib.rs", "None", ("None", 0));
    assert!(none
        .candidates
        .iter()
        .all(|c| c.confidence != Confidence::Local));
}

#[test]
fn a_member_access_is_never_a_local() {
    let text = "struct S { len: u8 }\nfn f(s: S, len: u8) -> u8 {\n    s.len + len\n}\n";
    let answer = local(&[("src/lib.rs", text)], "src/lib.rs", "len", ("len +", 0));
    assert_eq!(places(&answer), vec![("src/lib.rs".to_string(), 1)]);
    assert_eq!(answer.candidates[0].confidence, Confidence::Syntactic);
    let answer = local(&[("src/lib.rs", text)], "src/lib.rs", "len", ("len\n", 0));
    assert_local(&answer, &[("src/lib.rs", 2)]);
}

#[test]
fn ecma_parameters_and_block_bindings_are_locals() {
    let ts = "export function size(): number { return 1; }\nexport function area(size: number) {\n  return size * 2;\n}\nexport function other() {\n  const { size } = { size: 3 };\n  return size;\n}\n";
    assert_local(
        &local(&[("src/a.ts", ts)], "src/a.ts", "size", ("size * 2", 0)),
        &[("src/a.ts", 2)],
    );
    assert_local(
        &local(&[("src/a.ts", ts)], "src/a.ts", "size", ("size;", 0)),
        &[("src/a.ts", 6)],
    );
    let js = "function size() {}\nfunction area(size) {\n  return size * 2;\n}\nconst run = (size) => size;\n";
    assert_local(
        &local(&[("src/a.js", js)], "src/a.js", "size", ("size * 2", 0)),
        &[("src/a.js", 2)],
    );
    assert_local(
        &local(&[("src/a.js", js)], "src/a.js", "size", ("size;", 0)),
        &[("src/a.js", 5)],
    );
    let tsx = "export const App = (props: { name: string }) => {\n  const name = props.name;\n  return <b>{name}</b>;\n};\n";
    assert_local(
        &local(&[("src/a.tsx", tsx)], "src/a.tsx", "name", ("name}", 0)),
        &[("src/a.tsx", 2)],
    );
}

#[test]
fn python_function_bindings_are_locals_and_class_bodies_do_not_leak() {
    let text = "count = 0\n\nclass Store:\n    count = 1\n\n    def total(self, count):\n        return count\n\n    def size(self):\n        return count\n\ndef run():\n    for count in range(3):\n        pass\n    return count\n";
    let files = [("pkg/a.py", text)];
    assert_local(
        &local(&files, "pkg/a.py", "count", ("count\n", 0)),
        &[("pkg/a.py", 6)],
    );
    assert_local(
        &local(&files, "pkg/a.py", "count", ("count\n", 1)),
        &[("pkg/a.py", 1)],
    );
    assert_local(
        &local(&files, "pkg/a.py", "count", ("count\n", 2)),
        &[("pkg/a.py", 13)],
    );
}

#[test]
fn go_parameters_and_short_declarations_are_locals() {
    let text = "package main\n\nfunc value() int { return 1 }\n\nfunc use(value int) int {\n\treturn value\n}\n\nfunc other() int {\n\tvalue := 2\n\tvalue := value + 1\n\treturn value\n}\n";
    let files = [("main.go", text)];
    assert_local(
        &local(&files, "main.go", "value", ("value\n}", 0)),
        &[("main.go", 5)],
    );
    assert_local(
        &local(&files, "main.go", "value", ("value + 1", 0)),
        &[("main.go", 10)],
    );
    assert_local(
        &local(&files, "main.go", "value", ("value\n}", 1)),
        &[("main.go", 11)],
    );
}

#[test]
fn closer_files_come_first_and_another_family_is_dropped_once_one_answers() {
    let repo = repo(&[
        ("src/a/main.rs", "fn main() { thing(); }\n"),
        ("src/a/near.rs", "pub fn thing() {}\n"),
        ("src/a/b/deep.rs", "pub fn thing() {}\n"),
        ("aaa/far.rs", "pub fn thing() {}\n"),
        ("src/a/script.py", "def thing():\n    pass\n"),
    ]);
    let answer = ask("src/a/main.rs", "thing").run(&SyntaxProvider::new(), &repo.root);
    let paths: Vec<String> = places(&answer).into_iter().map(|(path, _)| path).collect();
    assert_eq!(paths, ["src/a/near.rs", "src/a/b/deep.rs", "aaa/far.rs"]);
    let answer = ask("src/a/script.py", "main").run(&SyntaxProvider::new(), &repo.root);
    assert_eq!(
        places(&answer),
        vec![("src/a/main.rs".to_string(), 1)],
        "another family answers when the source's own has nothing"
    );
}

#[test]
fn a_parse_budget_truncates_and_the_caller_limit_does_not_cost_authority() {
    let mut files: Vec<(String, String)> = (0..MAX_FILES_PARSED + 10)
        .map(|n| (format!("src/m{n:03}.rs"), "pub fn thing() {}\n".to_string()))
        .collect();
    files.push(("src/main.rs".into(), "fn main() {}\n".into()));
    let borrowed: Vec<(&str, &str)> = files
        .iter()
        .map(|(p, t)| (p.as_str(), t.as_str()))
        .collect();
    let repo = repo(&borrowed);
    let provider = SyntaxProvider::new();
    let mut query = ask("src/main.rs", "thing");
    query.limit = 10_000;
    let answer = query.run(&provider, &repo.root);
    assert_eq!(answer.candidates.len(), MAX_FILES_PARSED);
    assert!(answer.truncated);
    assert!(!answer.authoritative);
    let answer = query.run(&provider, &repo.root);
    assert_eq!(
        answer.candidates.len(),
        MAX_FILES_PARSED + 10,
        "the cache answers past the parse budget"
    );
    assert!(!answer.truncated);
    assert!(answer.authoritative);

    query.limit = 3;
    let answer = query.run(&SyntaxProvider::new(), &repo.root);
    assert_eq!(answer.candidates.len(), 3);
    assert!(answer.truncated);
    assert!(answer.authoritative);
}

#[test]
fn the_cache_spares_a_second_parse_until_the_file_changes_or_is_invalidated() {
    let repo = repo(&[
        ("src/main.rs", "fn main() {}\n"),
        ("src/lib.rs", "pub fn thing() {}\n"),
    ]);
    let provider = SyntaxProvider::new();
    let parses = || provider.parses.load(Ordering::Relaxed);
    let query = ask("src/main.rs", "thing");
    assert_eq!(query.run(&provider, &repo.root).candidates.len(), 1);
    assert_eq!(parses(), 1);
    assert_eq!(query.run(&provider, &repo.root).candidates.len(), 1);
    assert_eq!(parses(), 1, "warm");

    write(&repo.root, "src/lib.rs", "\npub fn thing() {}\n");
    assert_eq!(
        places(&query.run(&provider, &repo.root)),
        vec![("src/lib.rs".to_string(), 2)]
    );
    assert_eq!(parses(), 2, "a new length is a new stamp");

    provider.invalidate(&repo.root, Some("src/lib.rs"));
    query.run(&provider, &repo.root);
    assert_eq!(parses(), 3);
    provider.invalidate(Path::new("/elsewhere"), None);
    query.run(&provider, &repo.root);
    assert_eq!(parses(), 3, "another root's invalidation leaves this one");
    provider.invalidate(&repo.root, None);
    query.run(&provider, &repo.root);
    assert_eq!(parses(), 4);
}

#[test]
fn a_cancelled_lookup_stops_and_says_so() {
    let repo = repo(&[("src/lib.rs", "pub fn thing() {}\n")]);
    let cancel = AtomicBool::new(true);
    let answer = ask("src/main.rs", "thing").run_with(&SyntaxProvider::new(), &repo.root, &cancel);
    assert!(answer.candidates.is_empty());
    assert!(answer.truncated);
    assert!(!answer.authoritative);
}

#[test]
fn outside_git_or_outside_its_grammars_the_answer_is_empty() {
    let dir = plain_dir(&[("src/lib.rs", "pub fn thing() {}\n")]);
    let answer = ask("src/main.rs", "thing").run(&SyntaxProvider::new(), &dir.root);
    assert_eq!(answer, Answer::default());

    let repo = repo(&[
        ("src/lib.rs", "pub fn thing() {}\n"),
        ("app/Main.kt", "fun thing() {}\n"),
    ]);
    let answer = ask("app/Main.kt", "thing").run(&SyntaxProvider::new(), &repo.root);
    assert_eq!(answer, Answer::default());
}

#[test]
fn a_source_path_that_climbs_out_is_refused() {
    let repo = repo(&[("src/lib.rs", "pub fn thing() {}\n")]);
    let cancel = AtomicBool::new(false);
    let result = SyntaxProvider::new().find(&DefinitionQuery {
        root: &repo.root,
        source: "../x.rs",
        symbol: "thing",
        position: None,
        buffer: None,
        limit: 5,
        deadline: Instant::now() + Duration::from_secs(5),
        cancel: &cancel,
    });
    assert!(matches!(result, Err(FsError::EscapesWorkspace(_))));
}

#[test]
fn columns_count_utf16_and_long_lines_are_clamped_on_a_char_boundary() {
    let long = format!("pub fn thing() {{}} // {}\n", "é".repeat(400));
    let text = format!("/* 😀 */ pub fn wide() {{}}\n{long}");
    let repo = repo(&[("src/lib.rs", &text), ("src/main.rs", "fn main() {}\n")]);
    let provider = SyntaxProvider::new();
    let answer = ask("src/main.rs", "wide").run(&provider, &repo.root);
    assert_eq!(answer.candidates[0].column, 17);
    let answer = ask("src/main.rs", "thing").run(&provider, &repo.root);
    let shown = &answer.candidates[0].text;
    assert!(shown.len() <= extract::MAX_TEXT_BYTES && shown.starts_with("pub fn thing"));
}

/// `cargo test -p syntax-index --release -- --ignored --nocapture lookup_latency`
#[test]
#[ignore = "measures this checkout; run by hand"]
fn lookup_latency_on_this_repository() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("workspace root");
    for grammar in GRAMMARS {
        let started = Instant::now();
        grammars::compile(grammar).expect("compiles");
        println!(
            "{:>10} queries compile in {:.1} ms",
            grammar.id,
            started.elapsed().as_secs_f64() * 1000.0
        );
    }
    let provider = SyntaxProvider::new();
    for (source, symbol) in [
        ("crates/daemon/src/core.rs", "Daemon"),
        ("crates/daemon/src/core.rs", "search_files"),
        ("crates/daemon/src/core.rs", "new"),
        ("apps/tauri/src/main.tsx", "createSignal"),
        ("apps/tauri/src/main.tsx", "Store"),
    ] {
        for round in ["cold", "warm"] {
            let mut query = ask(source, symbol);
            query.limit = 200;
            let started = Instant::now();
            let answer = query.run(&provider, &root);
            println!(
                "{symbol:>14} {round}: {:>7.1} ms, {} candidates, truncated {}, authoritative {}, parses so far {}",
                started.elapsed().as_secs_f64() * 1000.0,
                answer.candidates.len(),
                answer.truncated,
                answer.authoritative,
                provider.parses.load(Ordering::Relaxed),
            );
        }
    }
    let source = "crates/daemon/src/core.rs";
    let text = std::fs::read_to_string(root.join(source)).expect("core.rs");
    let mut query = ask(source, "Daemon");
    query.position = Some(caret(&text, "Daemon", 3));
    for round in ["first", "second"] {
        let started = Instant::now();
        let answer = query.run(&provider, &root);
        println!(
            "Daemon at a caret in core.rs ({} KiB), {round}: {:.1} ms, {:?}",
            text.len() / 1024,
            started.elapsed().as_secs_f64() * 1000.0,
            answer.candidates.first().map(|c| (c.line, c.confidence)),
        );
    }
}
