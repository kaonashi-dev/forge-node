use std::fs;

use super::rank::{self, Dialect, Site};
use super::*;
use crate::tests::git_repo;
use crate::{search_definitions, search_files, SearchKind};

/// Every declaration of `symbol` in `text` read as `path`: (line, kind).
fn kinds(path: &str, text: &str, symbol: &str) -> Vec<(u32, Kind)> {
    let mut found = Vec::new();
    scan_text(path, text, symbol, &mut found);
    found.iter().map(|hit| (hit.line, hit.kind)).collect()
}

/// The column and kind of a one-line declaration.
fn one(path: &str, line: &str, symbol: &str) -> Option<(u32, Kind)> {
    let mut found = Vec::new();
    scan_text(path, line, symbol, &mut found);
    found.first().map(|hit| (hit.column, hit.kind))
}

fn lines(results: &SearchResults) -> Vec<(&str, u32)> {
    results
        .matches
        .iter()
        .map(|hit| (hit.path.as_str(), hit.line))
        .collect()
}

fn repo(files: &[(&str, &str)]) -> tempfile::TempDir {
    let tmp = git_repo();
    for (path, text) in files {
        let path = tmp.path().join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }
    tmp
}

#[test]
fn the_shapes_a_declaration_has() {
    use Kind::*;
    for (path, line, symbol, expected) in [
        (
            "a.rs",
            "pub fn open_file(p: &str) {",
            "open_file",
            (8, Keyword),
        ),
        (
            "a.ts",
            "export function openFile() {",
            "openFile",
            (17, Keyword),
        ),
        ("a.ts", "class OpenFile {", "OpenFile", (7, Keyword)),
        ("a.rs", "impl<T> OpenFile<T> {", "OpenFile", (9, Keyword)),
        (
            "a.rs",
            "macro_rules! open_file {",
            "open_file",
            (14, Keyword),
        ),
        // Go puts the receiver between the keyword and the name.
        (
            "a.go",
            "func (r *Repo) openFile(p string) {",
            "openFile",
            (16, Keyword),
        ),
        (
            "a.ts",
            "const openFile = (p) => {",
            "openFile",
            (7, Binding),
        ),
        (
            "a.ts",
            "  openFile(p: string): void {",
            "openFile",
            (3, Head),
        ),
    ] {
        assert_eq!(one(path, line, symbol), Some(expected), "{line}");
    }
}

/// A mention offered as a definition is worse than no answer.
#[test]
fn a_mention_is_refused() {
    for line in [
        "  openFile(path);",
        "  const x = openFile(path);",
        "import { openFile } from \"./api\";",
        "  if (openFile) {",
        "function outer() { return openFile(1) }",
        "fn reopenFile() {",
        "fn openFileTwice() {",
    ] {
        assert_eq!(one("a.ts", line, "openFile"), None, "{line}");
    }
}

#[test]
fn mutable_bindings_and_generic_methods() {
    for (path, line, symbol) in [
        ("a.rs", "let mut cache = Cache::new();", "cache"),
        ("a.rs", "static mut CACHE: usize = 0;", "CACHE"),
        ("a.ts", "  static find<T>(key: T): T {", "find"),
        ("a.ts", "  find<T extends Map<string, V>>(key: T) {", "find"),
        ("a.ts", "  find<T extends () => void>(key: T) {", "find"),
    ] {
        assert!(one(path, line, symbol).is_some(), "{line}");
    }
    assert_eq!(
        one("a.ts", "  static find<T>(key: T): T {", "find"),
        Some((10, Kind::Head)),
        "a static method is a method, not a static binding"
    );
    for line in [
        "find<T>(key);",
        "return find<T>(key);",
        "const result = find<T>(key);",
        "object.find<T>(key);",
        "find<T(key) {",
    ] {
        assert_eq!(one("a.ts", line, "find"), None, "{line}");
    }
}

#[test]
fn typescript_declaration_matrix() {
    for line in [
        "export async function lookup<T>(value: T): Promise<T> {",
        "export function* lookup() { yield 1; }",
        "export const lookup = <T>(value: T) => value;",
        "export type lookup<T> = { value: T };",
        "export interface lookup<T> { value: T; }",
        "  lookup(value: string): void;",
        "  lookup?(value: string): void;",
        "  abstract lookup<T>(value: T): T;",
        "  lookup(value: string) { return value; }",
        "  public async lookup<T>(value: T): Promise<T> { return value; }",
        "  get lookup(): string { return this.value; }",
        "  private async lookup(value: string = ')'): Promise<void> {",
        "  lookup<T extends '>('>(value: T): T {",
        "#[inline] pub fn lookup() {}",
    ] {
        assert!(one("a.ts", line, "lookup").is_some(), "{line}");
    }
    for line in [
        "lookup(value);",
        "await lookup(value);",
        "return lookup(value);",
        "object.lookup(value);",
        "const value = lookup(value);",
        "import { lookup } from './service';",
        "export { lookup };",
        "const value = condition ? lookup(value) : other;",
        "// export function lookup() {}",
        "/* const lookup = () => {}; */",
        "const example = 'function lookup() {}';",
        "const example = `class lookup {}`;",
    ] {
        assert_eq!(one("a.ts", line, "lookup"), None, "{line}");
    }
}

#[test]
fn python_and_kotlin_declaration_matrix() {
    for (path, line) in [
        ("service.py", "async def release(self, payin):"),
        ("service.pyi", "def release(payin: Payin) -> None: ..."),
        ("service.py", "release = lambda payin: payin"),
        ("service.py", "release: Callable[[Payin], None]"),
        ("service.py", "  self.release = handler"),
        ("service.kt", "private suspend fun release(payin: Payin) {}"),
        ("service.kt", "private fun <T> release(payin: T): T = payin"),
        ("service.kt", "fun Payin.release(): Unit {}"),
        ("service.kt", "inline fun <T> List<T>.release() = size"),
        ("service.kts", "val Payin.release: Boolean get() = settled"),
        ("service.kt", "typealias release = (Payin) -> Unit"),
    ] {
        assert!(one(path, line, "release").is_some(), "{path}: {line}");
    }
    for (path, line) in [
        ("service.py", "await release(payin)"),
        ("service.py", "from other import release"),
        ("service.py", "if release == handler:"),
        ("service.py", "release == handler"),
        ("service.py", "# def release(payin):"),
        ("service.py", "example = 'def release(payin):'"),
        ("service.kt", "payin.release()"),
        ("service.kt", "val result = payin.release()"),
        ("service.kt", "import api.release"),
        ("service.kt", "// fun Payin.release() {}"),
    ] {
        assert_eq!(one(path, line, "release"), None, "{path}: {line}");
    }
    assert_eq!(
        kinds("call.py", "send(\n    release=True,\n)\n", "release"),
        [],
        "a keyword argument is not an assignment"
    );
}

#[test]
fn typed_languages_declare_functions_by_return_type() {
    use Kind::*;
    let java = "public class Calc {\n    public static int compute(int x) {\n        return compute(x - 1);\n    }\n    private String label() {\n        int total = compute(1);\n        return \"\";\n    }\n    int count;\n    public Calc(int seed) {\n    }\n    abstract List<String> names();\n}\n";
    assert_eq!(kinds("Calc.java", java, "compute"), [(2, Keyword)]);
    assert_eq!(kinds("Calc.java", java, "label"), [(5, Keyword)]);
    assert_eq!(kinds("Calc.java", java, "total"), [(6, Binding)]);
    assert_eq!(kinds("Calc.java", java, "count"), [(9, Member)]);
    assert_eq!(kinds("Calc.java", java, "Calc"), [(1, Keyword), (10, Head)]);
    assert_eq!(kinds("Calc.java", java, "names"), [(12, Head)]);

    let c = "static int helper(int x) {\n    return helper(x);\n}\n#define LIMIT 10\nint use(void) { return LIMIT; }\ntypedef struct {\n    int width;\n} Point;\ntypedef unsigned long size;\nchar *\ndup(const char *s)\n{\n}\nint prototype(int);\n";
    assert_eq!(kinds("calc.c", c, "helper"), [(1, Keyword)]);
    assert_eq!(kinds("calc.c", c, "LIMIT"), [(4, Keyword)]);
    assert_eq!(kinds("calc.c", c, "Point"), [(8, Keyword)]);
    assert_eq!(kinds("calc.c", c, "size"), [(9, Keyword)]);
    assert_eq!(kinds("calc.c", c, "width"), [(7, Member)]);
    assert_eq!(kinds("calc.c", c, "dup"), [(11, Head)]);
    assert_eq!(kinds("calc.c", c, "prototype"), [(14, Head)]);

    let csharp = "namespace App\n{\n  class Calc\n  {\n    public async Task<int> Run(int x)\n    {\n      lock (gate) { Run(x); }\n      return x;\n    }\n  }\n}\n";
    assert_eq!(kinds("Calc.cs", csharp, "Run"), [(5, Keyword)]);
    assert_eq!(kinds("Calc.cs", csharp, "lock"), []);

    let cpp = "std::string Widget::name() const {\n  return name_;\n}\nWidget::Widget(int x) : x_(x) {}\n";
    assert_eq!(kinds("w.cpp", cpp, "name"), [(1, Keyword)]);
    assert_eq!(kinds("w.cpp", cpp, "Widget"), [(4, Head)]);
    assert_eq!(
        kinds(
            "main.dart",
            "Future<void> run() async {\n  run();\n}\nint twice(int x) => x * 2;\n",
            "twice"
        ),
        [(4, Keyword)]
    );
}

#[test]
fn bodies_declare_variants_fields_and_members() {
    use Kind::*;
    let rust = "pub enum Shape {\n    Circle(f32),\n    #[default]\n    Square,\n    Named { side: f32 },\n}\npub struct P {\n    pub width: u32,\n    pub(crate) height: u32,\n}\nfn f(p: P) -> u32 { let s = Shape::Square; p.width }\nenum Tiny { Up, Down }\n";
    assert_eq!(kinds("lib.rs", rust, "Circle"), [(2, Keyword)]);
    assert_eq!(kinds("lib.rs", rust, "Square"), [(4, Keyword)]);
    assert_eq!(kinds("lib.rs", rust, "Named"), [(5, Keyword)]);
    assert_eq!(kinds("lib.rs", rust, "width"), [(8, Member)]);
    assert_eq!(kinds("lib.rs", rust, "height"), [(9, Member)]);
    assert_eq!(kinds("lib.rs", rust, "Down"), [(12, Keyword)]);

    let ts = "export class P {\n  #secret() { return 1; }\n  handler = () => 1;\n  name: string = '';\n  private readonly maybe?: number;\n  run() {\n    handler = 2;\n    const local = { name: 1 };\n  }\n}\nconst o = { handler: 1 };\nenum Color { Red = 1, Green }\n";
    assert_eq!(kinds("p.ts", ts, "secret"), [(2, Head)]);
    assert_eq!(kinds("p.ts", ts, "handler"), [(3, Member)]);
    assert_eq!(kinds("p.ts", ts, "name"), [(4, Member)]);
    assert_eq!(kinds("p.ts", ts, "maybe"), [(5, Member)]);
    assert_eq!(kinds("p.ts", ts, "Green"), [(12, Keyword)]);

    let go = "package main\ntype T struct {\n\tCount int\n\tName, Alias string\n}\nfunc main() {\n\tcount := 1\n\ta, total := f()\n\tfor i, v := range xs {\n\t}\n\t_ = count == total\n}\n";
    assert_eq!(kinds("main.go", go, "count"), [(7, Binding)]);
    assert_eq!(kinds("main.go", go, "total"), [(8, Binding)]);
    assert_eq!(kinds("main.go", go, "v"), [(9, Binding)]);
    assert_eq!(kinds("main.go", go, "Count"), [(3, Member)]);
    assert_eq!(kinds("main.go", go, "Name"), [(4, Member)]);
}

#[test]
fn other_languages_declare_in_their_own_forms() {
    use Kind::*;
    assert_eq!(
        kinds(
            "b.rb",
            "class Builder\n  def self.build\n  end\nend\nBuilder.build\n",
            "build"
        ),
        [(2, Keyword)]
    );
    assert_eq!(
        kinds(
            "init.el",
            "(defun my-helper ()\n  (helper))\n(defun helper ()\n  nil)\n",
            "helper"
        ),
        [(3, Keyword)]
    );
    assert_eq!(
        kinds("core.clj", "(defn- parse [x])\n(parse 1)\n", "parse"),
        [(1, Keyword)]
    );
    assert_eq!(
        kinds(
            "m.lua",
            "local M = {}\nfunction M.build(x)\nend\nlocal count = 1\n",
            "build"
        ),
        [(2, Keyword)]
    );
    assert_eq!(
        kinds(
            "run.sh",
            "LIMIT=10\nexport PATH_X=1\nrun() {\n  echo $LIMIT\n}\nrun\n",
            "run"
        ),
        [(3, Head)]
    );
    assert_eq!(
        kinds("run.sh", "LIMIT=10\n[ \"$LIMIT\" ]\n", "LIMIT"),
        [(1, Binding)]
    );
    assert_eq!(
        kinds(
            "q.sql",
            "CREATE TABLE IF NOT EXISTS public.users (\n  id int\n);\nSELECT * FROM users;\n",
            "users"
        ),
        [(1, Keyword)]
    );
    assert_eq!(
        kinds("m.hs", "parse :: String -> Int\nparse = length\n", "parse"),
        [(1, Keyword)]
    );
    assert_eq!(
        kinds("schema.prisma", "model User {\n  id Int @id\n}\n", "User"),
        [(1, Keyword)]
    );
}

#[test]
fn lifetimes_are_not_character_literals() {
    let rust = "impl<'a> Parser<'a> {\n    fn parse(&self) -> char { '{' }\n}\nstruct Holder<'a, 'b> { parser: Parser<'a> }\n";
    assert_eq!(kinds("p.rs", rust, "Parser"), [(1, Kind::Keyword)]);
    assert_eq!(kinds("p.rs", rust, "parse"), [(2, Kind::Keyword)]);
    assert_eq!(kinds("p.rs", rust, "parser"), [(4, Kind::Member)]);
}

#[test]
fn comments_and_docstrings_hide_declarations_across_lines() {
    assert_eq!(
        kinds(
            "old.rs",
            "/*\nfn retired() {}\n/* nested */\nfn still_retired() {}\n*/\nfn live() {}\n",
            "retired"
        ),
        []
    );
    assert_eq!(
        kinds(
            "old.rs",
            "/*\n/* nested */\nfn still_retired() {}\n*/\n",
            "still_retired"
        ),
        []
    );
    assert_eq!(
        kinds(
            "doc.py",
            "def real():\n    \"\"\"\n    def ghost():\n    \"\"\"\n    ghost = 1\n",
            "ghost"
        ),
        [(5, Kind::Binding)]
    );
    assert_eq!(
        kinds(
            "doc.ts",
            "/**\n * export function ghost() {}\n */\nconst s = `\nfunction ghost() {}\n`;\n",
            "ghost"
        ),
        []
    );
}

#[test]
fn prose_and_data_files_declare_nothing() {
    for path in [
        "README.md",
        "config.yaml",
        "package.json",
        "Cargo.toml",
        "notes.txt",
        "a.css",
    ] {
        assert_eq!(
            kinds(
                path,
                "```rust\nfn openFile() {}\n```\nopenFile: 1\n",
                "openFile"
            ),
            [],
            "{path}"
        );
    }
}

#[test]
fn type_only_imports_and_reexports_are_not_declarations() {
    for text in [
        "import { sessionIsActive, type Session } from \"../contracts/runtime\";\n",
        "import type { Session } from './runtime';\n",
        "export type { Session } from './runtime';\n",
        "export { type Session } from './runtime';\n",
        "import {\n  sessionIsActive,\n  type Session,\n} from './runtime';\n",
        "use crate::session::{\n    Session,\n};\n",
    ] {
        let path = if text.starts_with("use") {
            "a.rs"
        } else {
            "a.ts"
        };
        assert_eq!(kinds(path, text, "Session"), [], "{text}");
    }
    assert_eq!(
        kinds(
            "a.ts",
            "import type { A } from 'a';\nexport type Session = { id: string };\n",
            "Session"
        ),
        [(2, Kind::Keyword)]
    );
}

#[test]
fn trailing_lambdas_are_calls_where_a_keyword_is_required() {
    assert_eq!(
        kinds(
            "build.gradle.kts",
            "repeat(3) {\n    println(it)\n}\n",
            "repeat"
        ),
        []
    );
    assert_eq!(
        kinds(
            "app.swift",
            "withAnimation(.easeIn) {\n  x = 1\n}\n",
            "withAnimation"
        ),
        []
    );
    assert_eq!(
        kinds(
            "app.swift",
            "func withAnimation(_ a: Animation, body: () -> Void) {\n}\n",
            "withAnimation"
        ),
        [(1, Kind::Keyword)]
    );
}

#[test]
fn keywords_and_non_identifiers_never_reach_git() {
    assert!(is_identifier("openFile"));
    assert!(is_identifier("_private"));
    assert!(is_identifier("$el"));
    assert!(!is_identifier(""));
    assert!(!is_identifier("open file"));
    assert!(!is_identifier(".*"));
    assert!(!is_identifier("2fast"));
    for keyword in ["if", "while", "return", "import", "self", "fn"] {
        assert!(!is_searchable(keyword), "{keyword}");
    }
    for name in ["new", "from", "default", "then", "match", "delete", "type"] {
        assert!(is_searchable(name), "{name}");
    }
    let tmp = repo(&[(
        "a.js",
        "function f(x) {\n  if (x) {\n    return 1;\n  }\n  while (x) {\n  }\n}\n",
    )]);
    for keyword in ["if", "while"] {
        let found = search_definitions(tmp.path(), "a.js", keyword, 64).unwrap();
        assert!(found.matches.is_empty() && !found.truncated, "{keyword}");
    }
}

#[test]
fn a_mention_reads_no_following_lines() {
    for line in [
        "this.release(payin, tx);",
        "await release(payin);",
        "release(payin);",
    ] {
        let at = line.find("release").unwrap();
        let site = Site {
            code: line,
            at,
            len: "release".len(),
            container: None,
            in_brackets: false,
            closed_typedef: false,
        };
        let following =
            std::iter::from_fn(|| -> Option<&str> { panic!("a mention read following lines") });
        assert_eq!(
            rank::rank(Dialect::Script, &site, following),
            None,
            "{line}"
        );
    }
}

#[test]
fn a_declaration_outranks_a_closer_binding() {
    let tmp = repo(&[
        (
            "src/a.rs",
            "fn caller() {\n    let config = load();\n    config.go();\n}\n",
        ),
        ("src/config.rs", "pub struct config;\n"),
        ("src/consts.rs", "pub const config: u8 = 1;\n"),
    ]);
    let found = search_definitions(tmp.path(), "src/a.rs", "config", 64).unwrap();
    assert_eq!(
        lines(&found),
        [("src/config.rs", 1), ("src/consts.rs", 1), ("src/a.rs", 2)],
        "struct and a module-level const before a local, whatever the file"
    );
    assert!(!found.truncated);
}

#[test]
fn closeness_orders_declarations_of_the_same_kind() {
    let tmp = repo(&[
        ("lib/b.ts", "export function thing() {}\n"),
        ("src/a.ts", "thing();\n"),
        ("src/c.ts", "export function thing() {}\n"),
        ("src/d.py", "def thing():\n    pass\n"),
        ("src/a_test.ts", "  thing(): void {}\n"),
    ]);
    let found = search_definitions(tmp.path(), "src/a.ts", "thing", 64).unwrap();
    assert_eq!(
        lines(&found),
        [
            ("src/c.ts", 1),
            ("src/a_test.ts", 1),
            ("lib/b.ts", 1),
            ("src/d.py", 1)
        ]
    );
}

#[test]
fn many_mentions_cannot_starve_the_declaration() {
    let usages = "  useThing();\n".repeat(6000);
    let tmp = repo(&[
        ("a_usages.ts", usages.as_str()),
        ("z_decl.ts", "export function useThing() { return 1; }\n"),
        ("other.ts", "useThing();\n"),
    ]);
    for source in ["z_decl.ts", "other.ts"] {
        let found = search_definitions(tmp.path(), source, "useThing", 64).unwrap();
        assert_eq!(lines(&found), [("z_decl.ts", 1)], "{source}");
        assert!(!found.truncated, "{source}");
    }
}

#[test]
fn minified_lines_and_bundles_do_not_make_a_search_partial() {
    let long = format!("var a=1;{}function useThing(){{}}\n", "x=1;".repeat(2000));
    let tmp = repo(&[
        ("bundle.js", long.as_str()),
        ("vendor.min.js", "function useThing(){}\n"),
        (
            "docs/guide.md",
            "```ts\nexport function useThing() {}\n```\n",
        ),
        ("src/thing.ts", "export function useThing() {}\n"),
    ]);
    let found = search_definitions(tmp.path(), "src/thing.ts", "useThing", 64).unwrap();
    assert_eq!(lines(&found), [("src/thing.ts", 1)]);
    assert!(!found.truncated);
}

#[test]
fn an_oversized_candidate_marks_the_search_partial() {
    let mut big = "export function useThing() {}\n".to_string();
    big.push_str(&"// filler\n".repeat(crate::MAX_FILE_BYTES / 10 + 1));
    let tmp = repo(&[
        ("big.ts", big.as_str()),
        ("small.ts", "export function useThing() {}\n"),
    ]);
    let found = search_definitions(tmp.path(), "small.ts", "useThing", 64).unwrap();
    assert_eq!(lines(&found), [("small.ts", 1)]);
    assert!(found.truncated);
}

#[test]
fn enough_close_declarations_stop_the_reads() {
    let tmp = repo(&[
        ("src/a.rs", "fn thing() {}\nfn other() { thing() }\n"),
        ("src/b.rs", "fn thing() {}\n"),
        ("zz/c.py", "def thing(): pass\n"),
    ]);
    let root = fs::canonicalize(tmp.path()).unwrap();
    let mut budget = Budget {
        files: MAX_FILES,
        bytes: MAX_READ_BYTES,
    };
    let (found, _) = search_within(&root, "thing", 1, Some("src/a.rs"), &mut budget).unwrap();
    assert_eq!(lines(&found), [("src/a.rs", 1)]);
    assert!(found.truncated, "files were left unread");
    assert_eq!(MAX_FILES - budget.files, 1);
}

#[test]
fn a_plain_directory_is_walked_instead() {
    let tmp = tempfile::tempdir().unwrap();
    fs::write(
        tmp.path().join("a.rs"),
        "pub fn open_file() {}\nfn main() { open_file() }\n",
    )
    .unwrap();
    fs::write(tmp.path().join("README.md"), "fn open_file() {}\n").unwrap();
    let found = search_files(tmp.path(), "open_file", SearchKind::Definition, 64).unwrap();
    assert_eq!(lines(&found), [("a.rs", 1)]);
}
