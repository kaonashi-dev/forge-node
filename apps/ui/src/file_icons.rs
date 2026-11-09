//! Which language mark a file wears, by name alone, as Tauri's `langIcons.ts`:
//! the tree is painted from a listing of names, and reading a file to learn
//! what it is would cost a round trip per row. A name that says nothing gets
//! the plain sheet rather than a guess.
//!
//! The marks are JetBrains' multicolour file-type icons (`assets/lang`), so
//! they draw through Rune's colour-icon path rather than as a tinted mask:
//! their colour is the information.

use std::sync::LazyLock;

use rune::Icon;

macro_rules! lang_icons {
    ($($name:literal)*) => {
        /// Every vendored mark, sorted: a [`FileIcon`] is an index into it.
        const NAMES: &[&str] = &[$($name),*];
        const DARK: &[&str] = &[$(include_str!(concat!("../assets/lang/dark/", $name, ".svg"))),*];
        const LIGHT: &[&str] = &[$(include_str!(concat!("../assets/lang/light/", $name, ".svg"))),*];
    };
}

lang_icons! {
    "actionscript" "angular" "api" "archive" "astro" "c" "coffeescript" "cpp" "css" "csv"
    "cucumber" "dart" "default" "deno" "diagram" "docker" "docker_compose" "editorconfig"
    "elixir" "env" "eslint" "font" "gleam" "go" "graphql" "groovy" "h" "handlebars" "haskell"
    "hcl" "html" "idl" "ignored" "image" "java" "java_class" "javascript" "jest" "js_test"
    "jshint" "json" "jsp" "jspx" "jsx" "jsx_test" "karma" "kotlin" "m" "makefile" "manifest"
    "markdown" "nextjs" "nix" "npm" "nuxt" "open_api" "patch" "perl" "php" "php_unit" "prisma"
    "python" "redis" "ruby" "rust" "sass" "scala" "settings" "sql" "storage" "stylelint"
    "swift" "terminal" "terraform" "toml" "ts_test" "tsx" "tsx_test" "typescript" "vcs" "vue"
    "wsdl" "xhtml" "xml" "xsd" "yaml" "yarn"
}

/// A whole file name, case aside. Beats every rule below it.
const BY_NAME: &[(&str, &str)] = &[
    ("dockerfile", "docker"),
    ("docker-compose.yml", "docker_compose"),
    ("docker-compose.yaml", "docker_compose"),
    ("compose.yml", "docker_compose"),
    ("compose.yaml", "docker_compose"),
    ("makefile", "makefile"),
    ("gnumakefile", "makefile"),
    ("cargo.toml", "rust"),
    ("cargo.lock", "rust"),
    ("package.json", "npm"),
    ("package-lock.json", "npm"),
    ("pnpm-lock.yaml", "npm"),
    ("pnpm-workspace.yaml", "npm"),
    ("yarn.lock", "yarn"),
    (".npmrc", "npm"),
    (".nvmrc", "npm"),
    ("tsconfig.json", "typescript"),
    ("jsconfig.json", "javascript"),
    ("angular.json", "angular"),
    ("deno.json", "deno"),
    ("deno.jsonc", "deno"),
    ("deno.lock", "deno"),
    ("phpunit.xml", "php_unit"),
    ("composer.json", "php"),
    ("gemfile", "ruby"),
    ("gemfile.lock", "ruby"),
    ("rakefile", "ruby"),
    ("go.mod", "go"),
    ("go.sum", "go"),
    ("build.gradle", "groovy"),
    ("settings.gradle", "groovy"),
    ("pom.xml", "java"),
    ("manifest.mf", "manifest"),
    (".editorconfig", "editorconfig"),
    (".jshintrc", "jshint"),
    (".gitattributes", "vcs"),
    (".gitmodules", "vcs"),
    (".gitkeep", "vcs"),
    (".mailmap", "vcs"),
    ("openapi.json", "open_api"),
    ("openapi.yaml", "open_api"),
    ("swagger.json", "open_api"),
];

/// Config files carry a version, a stage or a module system in the middle
/// (`.env.local`, `jest.config.mjs`, `Dockerfile.ci`), so the extension is the
/// wrong end of the name to read.
const BY_PREFIX: &[(&str, &str)] = &[
    (".env", "env"),
    (".eslintrc", "eslint"),
    ("eslint.config.", "eslint"),
    (".stylelintrc", "stylelint"),
    ("stylelint.config.", "stylelint"),
    ("jest.config.", "jest"),
    ("karma.conf.", "karma"),
    ("next.config.", "nextjs"),
    ("nuxt.config.", "nuxt"),
    ("dockerfile", "docker"),
    ("docker-compose.", "docker_compose"),
];

const BY_SUFFIX: &[(&str, &str)] = &[
    (".test.ts", "ts_test"),
    (".spec.ts", "ts_test"),
    (".test.mts", "ts_test"),
    (".test.tsx", "tsx_test"),
    (".spec.tsx", "tsx_test"),
    (".test.js", "js_test"),
    (".spec.js", "js_test"),
    (".test.mjs", "js_test"),
    (".test.cjs", "js_test"),
    (".test.jsx", "jsx_test"),
    (".spec.jsx", "jsx_test"),
    (".d.ts", "typescript"),
    ("ignore", "ignored"),
];

const BY_EXTENSION: &[(&str, &str)] = &[
    ("ts", "typescript"),
    ("mts", "typescript"),
    ("cts", "typescript"),
    ("tsx", "tsx"),
    ("js", "javascript"),
    ("mjs", "javascript"),
    ("cjs", "javascript"),
    ("jsx", "jsx"),
    ("as", "actionscript"),
    ("coffee", "coffeescript"),
    ("vue", "vue"),
    ("astro", "astro"),
    ("rs", "rust"),
    ("go", "go"),
    ("py", "python"),
    ("pyi", "python"),
    ("pyw", "python"),
    ("rb", "ruby"),
    ("gemspec", "ruby"),
    ("erb", "ruby"),
    ("java", "java"),
    ("class", "java_class"),
    ("jar", "archive"),
    ("kt", "kotlin"),
    ("kts", "kotlin"),
    ("scala", "scala"),
    ("sc", "scala"),
    ("groovy", "groovy"),
    ("gradle", "groovy"),
    ("swift", "swift"),
    ("c", "c"),
    ("h", "h"),
    ("hh", "h"),
    ("cc", "cpp"),
    ("cpp", "cpp"),
    ("cxx", "cpp"),
    ("hpp", "cpp"),
    ("hxx", "cpp"),
    ("m", "m"),
    ("mm", "m"),
    ("php", "php"),
    ("pl", "perl"),
    ("pm", "perl"),
    ("ex", "elixir"),
    ("exs", "elixir"),
    ("hs", "haskell"),
    ("gleam", "gleam"),
    ("dart", "dart"),
    ("nix", "nix"),
    ("tf", "terraform"),
    ("tfvars", "terraform"),
    ("hcl", "hcl"),
    ("sh", "terminal"),
    ("bash", "terminal"),
    ("zsh", "terminal"),
    ("fish", "terminal"),
    ("mk", "makefile"),
    ("sql", "sql"),
    ("prisma", "prisma"),
    ("graphql", "graphql"),
    ("gql", "graphql"),
    ("html", "html"),
    ("htm", "html"),
    ("xhtml", "xhtml"),
    ("xml", "xml"),
    ("xsd", "xsd"),
    ("wsdl", "wsdl"),
    ("idl", "idl"),
    ("jsp", "jsp"),
    ("jspx", "jspx"),
    ("uml", "diagram"),
    ("css", "css"),
    ("scss", "sass"),
    ("sass", "sass"),
    ("json", "json"),
    ("jsonc", "json"),
    ("jsonl", "json"),
    ("json5", "json"),
    ("yaml", "yaml"),
    ("yml", "yaml"),
    ("toml", "toml"),
    ("properties", "settings"),
    ("ini", "settings"),
    ("cfg", "settings"),
    ("conf", "settings"),
    ("md", "markdown"),
    ("mdx", "markdown"),
    ("markdown", "markdown"),
    ("csv", "csv"),
    ("tsv", "csv"),
    ("feature", "cucumber"),
    ("http", "api"),
    ("rest", "api"),
    ("patch", "patch"),
    ("diff", "patch"),
    ("hbs", "handlebars"),
    ("handlebars", "handlebars"),
    ("mustache", "handlebars"),
    ("svg", "image"),
    ("png", "image"),
    ("jpg", "image"),
    ("jpeg", "image"),
    ("gif", "image"),
    ("webp", "image"),
    ("bmp", "image"),
    ("ico", "image"),
    ("icns", "image"),
    ("ttf", "font"),
    ("otf", "font"),
    ("woff", "font"),
    ("woff2", "font"),
    ("zip", "archive"),
    ("tar", "archive"),
    ("gz", "archive"),
    ("tgz", "archive"),
    ("bz2", "archive"),
    ("xz", "archive"),
    ("rar", "archive"),
    ("7z", "archive"),
    ("db", "storage"),
    ("sqlite", "storage"),
    ("sqlite3", "storage"),
    ("rdb", "redis"),
];

/// One vendored mark.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileIcon(u8);

impl FileIcon {
    #[cfg(test)]
    fn name(self) -> &'static str {
        NAMES[usize::from(self.0)]
    }

    /// The mark drawn for a dark or a light ground: the artwork carries its
    /// own fills, so a pale theme needs the set drawn for it.
    #[must_use]
    pub fn icon(self, light: bool) -> &'static Icon {
        let set: &'static [Icon] = if light { &LIGHT_ICONS } else { &DARK_ICONS };
        &set[usize::from(self.0)]
    }

    fn named(name: &str) -> Self {
        let index = NAMES.binary_search(&name).unwrap_or_else(|_| {
            NAMES
                .binary_search(&"default")
                .expect("the vendored set has a default mark")
        });
        Self(u8::try_from(index).expect("fewer than 256 marks"))
    }
}

static DARK_ICONS: LazyLock<Vec<Icon>> = LazyLock::new(|| icons(&DARK_BODIES));
static LIGHT_ICONS: LazyLock<Vec<Icon>> = LazyLock::new(|| icons(&LIGHT_BODIES));
static DARK_BODIES: LazyLock<Vec<String>> =
    LazyLock::new(|| DARK.iter().map(|svg| body(svg)).collect());
static LIGHT_BODIES: LazyLock<Vec<String>> =
    LazyLock::new(|| LIGHT.iter().map(|svg| body(svg)).collect());

fn icons(bodies: &'static [String]) -> Vec<Icon> {
    NAMES
        .iter()
        .zip(bodies)
        .map(|(name, body)| Icon {
            name,
            view: 16.0,
            body: body.as_str(),
        })
        .collect()
}

/// The vendored file as a body for a 16-unit square: its root `<svg>` nested
/// at that size, so a 40-unit, a 2105×2016 or a size-only drawing scales and
/// centres as a browser's `background-image` would. The wrapping group's
/// black fill is SVG's default, a no-op, and is there because Rune draws a
/// body through its colour path only when it names a colour (`fill="#`), which
/// a drawing that relies on black, `style` or a class does not.
fn body(svg: &str) -> String {
    let start = svg.find("<svg").unwrap_or(0);
    let lead = &svg[..start];
    let lead = match (lead.find("<?xml"), lead.find("?>")) {
        (Some(a), Some(b)) if a < b => format!("{}{}", &lead[..a], &lead[b + 2..]),
        _ => lead.to_string(),
    };
    let Some(close) = svg[start..].find('>').map(|at| start + at) else {
        return String::new();
    };
    let mut attrs = String::new();
    let (mut view_box, mut width, mut height) = (None, None, None);
    for (name, value) in attributes(svg[start + 4..close].trim_end_matches('/')) {
        match name {
            "width" => width = Some(value),
            "height" => height = Some(value),
            "viewBox" => view_box = Some(value.to_string()),
            _ => attrs.push_str(&format!(r#" {name}="{value}""#)),
        }
    }
    let view_box = view_box.unwrap_or_else(|| {
        format!(
            "0 0 {} {}",
            width.unwrap_or("16").trim_end_matches("px"),
            height.unwrap_or("16").trim_end_matches("px")
        )
    });
    format!(
        r##"<g fill="#000">{lead}<svg width="16" height="16" viewBox="{view_box}"{attrs}>{}</g>"##,
        &svg[close + 1..]
    )
}

/// `name="value"` pairs of an open tag's attribute text, either quote.
fn attributes(tag: &str) -> impl Iterator<Item = (&str, &str)> {
    let mut rest = tag;
    std::iter::from_fn(move || {
        rest = rest.trim_start();
        let (name, after) = rest.split_once('=')?;
        let after = after.trim_start();
        let quote = after.chars().next()?;
        let (value, tail) = after[1..].split_once(quote)?;
        rest = tail;
        Some((name.trim(), value))
    })
}

fn find(table: &[(&str, &'static str)], hit: impl Fn(&str) -> bool) -> Option<&'static str> {
    table
        .iter()
        .find(|(key, _)| hit(key))
        .map(|(_, mark)| *mark)
}

fn starts_with(name: &str, prefix: &str) -> bool {
    name.len() >= prefix.len()
        && name.as_bytes()[..prefix.len()].eq_ignore_ascii_case(prefix.as_bytes())
}

fn ends_with(name: &str, suffix: &str) -> bool {
    name.len() >= suffix.len()
        && name.as_bytes()[name.len() - suffix.len()..].eq_ignore_ascii_case(suffix.as_bytes())
}

/// The mark for one file, from a name or a whole path. Whole name first, then
/// the prefixes and the compound suffixes, then the extension:
/// `next.config.js` is Next and not JavaScript, and `app.test.ts` is a test
/// and not TypeScript, and only the longer rule can say so.
#[must_use]
pub fn for_path(path: &str) -> FileIcon {
    let name = path.rsplit('/').next().unwrap_or(path);
    let mark = find(BY_NAME, |key| name.eq_ignore_ascii_case(key))
        .or_else(|| find(BY_PREFIX, |prefix| starts_with(name, prefix)))
        .or_else(|| find(BY_SUFFIX, |suffix| ends_with(name, suffix)))
        .or_else(|| {
            // A leading dot is part of the name: `.gitignore` is a dotfile,
            // and reading `gitignore` as its extension would tint it as source.
            let dot = name.rfind('.').filter(|&dot| dot > 0)?;
            let ext = &name[dot + 1..];
            find(BY_EXTENSION, |key| ext.eq_ignore_ascii_case(key))
        });
    FileIcon::named(mark.unwrap_or("default"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mark(path: &str) -> &'static str {
        for_path(path).name()
    }

    fn on_disk(variant: &str) -> Vec<String> {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("assets/lang")
            .join(variant);
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .expect("vendored set")
            .filter_map(|entry| {
                let name = entry.ok()?.file_name().into_string().ok()?;
                name.strip_suffix(".svg").map(str::to_string)
            })
            .collect();
        names.sort();
        names
    }

    /// The guard the table rests on: a name with no file fails to build, and
    /// a file no name reaches is dead weight in the binary.
    #[test]
    fn has_one_file_per_name_in_both_variants_and_names_each_once() {
        let names: Vec<String> = NAMES.iter().map(|name| (*name).to_string()).collect();
        assert!(
            NAMES.windows(2).all(|pair| pair[0] < pair[1]),
            "sorted, no repeats"
        );
        assert_eq!(on_disk("dark"), names);
        assert_eq!(on_disk("light"), names);
    }

    #[test]
    fn every_rule_names_a_vendored_mark() {
        for table in [BY_NAME, BY_PREFIX, BY_SUFFIX, BY_EXTENSION] {
            for (key, mark) in table {
                assert!(NAMES.binary_search(mark).is_ok(), "{key} names {mark}");
            }
        }
    }

    #[test]
    fn every_mark_is_a_sized_colour_body() {
        for light in [false, true] {
            for (index, name) in NAMES.iter().enumerate() {
                let body = FileIcon(index as u8).icon(light).body;
                assert!(
                    body.contains(r##"fill="#"##),
                    "{name} draws in its own colours"
                );
                assert!(!body.contains("<?xml"), "{name} keeps no prolog");
                assert_eq!(
                    body.matches(r#"width="16" height="16" viewBox=""#).count(),
                    1,
                    "{name}"
                );
            }
        }
        assert!(DARK_BODIES[FileIcon::named("hcl").0 as usize].contains(r#"viewBox="0 0 64 64""#));
        assert!(LIGHT_BODIES[FileIcon::named("default").0 as usize].contains("JetBrains"));
    }

    #[test]
    fn reads_the_extension() {
        assert_eq!(mark("main.rs"), "rust");
        assert_eq!(mark("store.ts"), "typescript");
        assert_eq!(mark("App.tsx"), "tsx");
        assert_eq!(mark("setup.py"), "python");
        assert_eq!(mark("style.scss"), "sass");
        assert_eq!(mark("run.fish"), "terminal");
    }

    #[test]
    fn takes_the_last_segment_of_a_path() {
        assert_eq!(mark("crates/daemon/src/core.rs"), "rust");
        assert_eq!(mark("./deep/name.with.dots.json"), "json");
    }

    #[test]
    fn is_case_insensitive() {
        assert_eq!(mark("Makefile"), "makefile");
        assert_eq!(mark("README.MD"), "markdown");
        assert_eq!(mark("Dockerfile.ci"), "docker");
    }

    #[test]
    fn lets_a_whole_name_beat_the_extension() {
        assert_eq!(mark("Cargo.toml"), "rust");
        assert_eq!(mark("other.toml"), "toml");
        assert_eq!(mark("package.json"), "npm");
        assert_eq!(mark("tsconfig.json"), "typescript");
        assert_eq!(mark("openapi.yaml"), "open_api");
    }

    #[test]
    fn lets_a_compound_suffix_beat_the_extension() {
        assert_eq!(mark("prFilters.test.ts"), "ts_test");
        assert_eq!(mark("App.spec.tsx"), "tsx_test");
        assert_eq!(mark("types.d.ts"), "typescript");
        assert_eq!(mark("plain.ts"), "typescript");
    }

    #[test]
    fn reads_a_config_file_from_its_front_where_the_name_is() {
        assert_eq!(mark(".env.local"), "env");
        assert_eq!(mark("jest.config.mjs"), "jest");
        assert_eq!(mark("next.config.ts"), "nextjs");
        assert_eq!(mark(".eslintrc.json"), "eslint");
    }

    #[test]
    fn treats_a_dotfile_as_a_name_and_not_as_an_extension() {
        assert_eq!(mark(".gitignore"), "ignored");
        assert_eq!(mark(".dockerignore"), "ignored");
        assert_eq!(mark(".gitattributes"), "vcs");
        assert_eq!(mark(".hushlogin"), "default");
    }

    #[test]
    fn falls_back_rather_than_guessing() {
        assert_eq!(mark("LICENSE"), "default");
        assert_eq!(mark("data.msgpack"), "default");
        assert_eq!(mark(""), "default");
    }

    #[test]
    fn marks_the_languages_this_checkout_is_written_in() {
        assert_eq!(mark("main.js"), "javascript");
        assert_eq!(mark("bus.ts"), "typescript");
        assert_eq!(mark("core.rs"), "rust");
        assert_eq!(mark("Main.kt"), "kotlin");
        assert_eq!(mark("deny.json"), "json");
        assert_eq!(mark("schema.prisma"), "prisma");
        assert_eq!(mark("0001_init.sql"), "sql");
        assert_eq!(mark("AppShell.tsx"), "tsx");
    }

    #[test]
    fn draws_one_file_per_mark_and_ground() {
        let rust = for_path("crates/daemon/src/core.rs");
        assert!(!std::ptr::eq(rust.icon(false), rust.icon(true)));
        assert!(std::ptr::eq(
            rust.icon(true),
            for_path("main.rs").icon(true)
        ));
    }
}
