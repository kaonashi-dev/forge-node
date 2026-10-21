//! Which language a file is, and which language server answers for it.
//!
//! The registry is data (`languages.toml`, embedded) merged with the user's
//! `[lsp]` overrides. Every other crate asks here instead of matching on an
//! extension or a server id, the way provider facts live in `crates/agents`.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

const BUILTIN: &str = include_str!("../languages.toml");

/// One language: how to recognise its files and read its lines.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Language {
    pub id: String,
    #[serde(default)]
    pub extensions: Vec<String>,
    /// Whole file names, for files with no extension (`Rakefile`).
    #[serde(default)]
    pub filenames: Vec<String>,
    /// Languages that resolve each other's names share a family.
    #[serde(default)]
    family: Option<String>,
    /// The LSP `languageId`, when it differs from `id`.
    #[serde(default)]
    lsp_id: Option<String>,
    /// A grammar id `syntax-index` compiles in, if any.
    #[serde(default)]
    pub grammar: Option<String>,
    #[serde(default)]
    pub line_comments: Vec<String>,
    #[serde(default)]
    pub block_comments: Vec<(String, String)>,
    #[serde(default = "default_quotes")]
    pub quotes: Vec<String>,
    /// Declarations may start with a type (`static int helper(`), not a keyword.
    #[serde(default)]
    pub typed_declarations: bool,
    /// False for prose and data, whose lines are never declarations.
    #[serde(default = "yes")]
    pub declarations: bool,
    /// Server ids, best first.
    #[serde(default)]
    pub servers: Vec<String>,
}

fn default_quotes() -> Vec<String> {
    vec!["\"".into(), "'".into(), "`".into()]
}

fn yes() -> bool {
    true
}

impl Language {
    #[must_use]
    pub fn family(&self) -> &str {
        self.family.as_deref().unwrap_or(&self.id)
    }

    #[must_use]
    pub fn lsp_id(&self) -> &str {
        self.lsp_id.as_deref().unwrap_or(&self.id)
    }

    /// Whether `line` starts a line comment once leading blanks are skipped.
    #[must_use]
    pub fn is_comment_line(&self, line: &str) -> bool {
        let line = line.trim_start();
        self.line_comments
            .iter()
            .any(|marker| line.starts_with(marker.as_str()))
    }
}

/// Which marker directory wins when several ancestors have one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RootStrategy {
    /// The closest ancestor with a marker: a package in a monorepo.
    #[default]
    Nearest,
    /// The top-most ancestor with a marker: a Cargo or Go workspace.
    Outermost,
}

/// How a server's binary is checked before it is ever launched.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Probe {
    /// A sibling binary to ask instead of `command`, when the server has no
    /// version flag of its own.
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    /// Substring the probe's output must contain; empty accepts any success.
    #[serde(default)]
    pub expect: String,
}

/// A language server Forge knows how to launch.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerSpec {
    #[serde(skip)]
    pub id: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    /// Upward search, best level first; an inner list is one level.
    #[serde(default)]
    pub root_markers: Vec<Vec<String>>,
    #[serde(default)]
    pub root_strategy: RootStrategy,
    #[serde(default)]
    pub probe: Probe,
    #[serde(default = "empty_object", deserialize_with = "toml_to_json")]
    pub initialization_options: serde_json::Value,
    /// Answers `workspace/configuration`.
    #[serde(default = "empty_object", deserialize_with = "toml_to_json")]
    pub settings: serde_json::Value,
    #[serde(default = "yes")]
    pub enabled: bool,
}

fn empty_object() -> serde_json::Value {
    serde_json::Value::Object(serde_json::Map::new())
}

fn toml_to_json<'de, D: serde::Deserializer<'de>>(de: D) -> Result<serde_json::Value, D::Error> {
    let value = toml::Value::deserialize(de)?;
    serde_json::to_value(value).map_err(serde::de::Error::custom)
}

/// What the user's config may change about one server. Every field is
/// optional so an override names only what it changes.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerOverride {
    pub command: Option<String>,
    pub args: Option<Vec<String>>,
    pub root_markers: Option<Vec<Vec<String>>>,
    pub root_strategy: Option<RootStrategy>,
    pub probe: Option<Probe>,
    #[serde(default, deserialize_with = "optional_toml_to_json")]
    pub initialization_options: Option<serde_json::Value>,
    #[serde(default, deserialize_with = "optional_toml_to_json")]
    pub settings: Option<serde_json::Value>,
    pub enabled: Option<bool>,
}

fn optional_toml_to_json<'de, D: serde::Deserializer<'de>>(
    de: D,
) -> Result<Option<serde_json::Value>, D::Error> {
    toml_to_json(de).map(Some)
}

/// What the user's config may change about one language.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LanguageOverride {
    pub extensions: Option<Vec<String>>,
    pub servers: Option<Vec<String>>,
}

/// The `[lsp.languages]` / `[lsp.servers]` tables of the user's config.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Overrides {
    #[serde(default)]
    pub languages: BTreeMap<String, LanguageOverride>,
    /// A server id not in the built-ins must name a `command`.
    #[serde(default)]
    pub servers: BTreeMap<String, ServerOverride>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    language: Vec<Language>,
    #[serde(default)]
    server: BTreeMap<String, ServerSpec>,
}

/// Languages and servers, after overrides.
#[derive(Clone, Debug, Default)]
pub struct Registry {
    languages: Vec<Language>,
    servers: BTreeMap<String, ServerSpec>,
}

impl Registry {
    /// The embedded table. Its parse is pinned by a unit test, so an empty
    /// registry here means a build that skipped that test, and every caller
    /// already treats "unknown language" as "no index, no server".
    #[must_use]
    pub fn builtin() -> &'static Registry {
        static BUILT: OnceLock<Registry> = OnceLock::new();
        BUILT.get_or_init(|| {
            Self::parse(BUILTIN).unwrap_or_else(|error| {
                tracing::error!(%error, "built-in language table does not parse");
                Registry::default()
            })
        })
    }

    fn parse(text: &str) -> Result<Registry, toml::de::Error> {
        let file: File = toml::from_str(text)?;
        let servers = file
            .server
            .into_iter()
            .map(|(id, mut spec)| {
                spec.id.clone_from(&id);
                (id, spec)
            })
            .collect();
        Ok(Registry {
            languages: file.language,
            servers,
        })
    }

    /// The built-ins with the user's overrides applied.
    ///
    /// A new server without a `command` is skipped and reported, rather than
    /// refusing the whole config over one entry.
    #[must_use]
    pub fn with_overrides(overrides: &Overrides) -> Registry {
        let mut registry = Self::builtin().clone();
        for (id, change) in &overrides.servers {
            match registry.servers.get_mut(id) {
                Some(spec) => apply_server(spec, change),
                None => {
                    let Some(command) = change.command.clone() else {
                        tracing::warn!(server = %id, "[lsp.servers] entry has no command; ignored");
                        continue;
                    };
                    let mut spec = ServerSpec {
                        id: id.clone(),
                        command,
                        args: Vec::new(),
                        root_markers: Vec::new(),
                        root_strategy: RootStrategy::Nearest,
                        probe: Probe::default(),
                        initialization_options: empty_object(),
                        settings: empty_object(),
                        enabled: true,
                    };
                    apply_server(&mut spec, change);
                    registry.servers.insert(id.clone(), spec);
                }
            }
        }
        for (id, change) in &overrides.languages {
            let Some(language) = registry.languages.iter_mut().find(|l| &l.id == id) else {
                tracing::warn!(language = %id, "[lsp.languages] names an unknown language; ignored");
                continue;
            };
            if let Some(extensions) = &change.extensions {
                language.extensions.clone_from(extensions);
            }
            if let Some(servers) = &change.servers {
                language.servers.clone_from(servers);
            }
        }
        registry
    }

    #[must_use]
    pub fn language(&self, id: &str) -> Option<&Language> {
        self.languages.iter().find(|language| language.id == id)
    }

    /// The language a path's extension (or whole file name) names.
    #[must_use]
    pub fn language_for_path(&self, path: &str) -> Option<&Language> {
        let path = Path::new(path);
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if let Some(found) = self
            .languages
            .iter()
            .find(|language| language.filenames.iter().any(|f| f == name))
        {
            return Some(found);
        }
        let extension = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if extension.is_empty() {
            return None;
        }
        self.languages
            .iter()
            .find(|language| language.extensions.contains(&extension))
    }

    pub fn languages(&self) -> impl Iterator<Item = &Language> {
        self.languages.iter()
    }

    #[must_use]
    pub fn server(&self, id: &str) -> Option<&ServerSpec> {
        self.servers.get(id)
    }

    pub fn servers(&self) -> impl Iterator<Item = &ServerSpec> {
        self.servers.values()
    }

    /// The enabled servers for a path's language, best first.
    pub fn servers_for_path<'a>(&'a self, path: &str) -> impl Iterator<Item = &'a ServerSpec> {
        self.language_for_path(path)
            .into_iter()
            .flat_map(|language| language.servers.iter())
            .filter_map(|id| self.servers.get(id))
            .filter(|spec| spec.enabled)
    }
}

fn apply_server(spec: &mut ServerSpec, change: &ServerOverride) {
    if let Some(command) = &change.command {
        spec.command.clone_from(command);
    }
    if let Some(args) = &change.args {
        spec.args.clone_from(args);
    }
    if let Some(markers) = &change.root_markers {
        spec.root_markers.clone_from(markers);
    }
    if let Some(strategy) = change.root_strategy {
        spec.root_strategy = strategy;
    }
    if let Some(probe) = &change.probe {
        spec.probe = probe.clone();
    }
    if let Some(options) = &change.initialization_options {
        spec.initialization_options = options.clone();
    }
    if let Some(settings) = &change.settings {
        spec.settings = settings.clone();
    }
    if let Some(enabled) = change.enabled {
        spec.enabled = enabled;
    }
}

/// Whether two paths belong to languages that resolve each other's names.
#[must_use]
pub fn same_family(registry: &Registry, a: &str, b: &str) -> bool {
    match (registry.language_for_path(a), registry.language_for_path(b)) {
        (Some(a), Some(b)) => a.family() == b.family(),
        _ => false,
    }
}

/// The directory a server should treat as its workspace for `file`.
///
/// Searched upward from the file's directory and never above `checkout`; the
/// checkout itself is the answer when no level matches. `exists` is the only
/// filesystem access, so the policy is testable without a disk.
#[must_use]
pub fn find_root(
    spec: &ServerSpec,
    checkout: &Path,
    file: &Path,
    exists: impl Fn(&Path) -> bool,
) -> PathBuf {
    let start = file.parent().unwrap_or(checkout);
    if !start.starts_with(checkout) {
        return checkout.to_path_buf();
    }
    let ancestors: Vec<&Path> = start
        .ancestors()
        .take_while(|dir| dir.starts_with(checkout))
        .collect();
    for level in &spec.root_markers {
        let has = |dir: &Path| level.iter().any(|marker| exists(&dir.join(marker)));
        let found = match spec.root_strategy {
            RootStrategy::Nearest => ancestors.iter().find(|dir| has(dir)),
            RootStrategy::Outermost => ancestors.iter().rev().find(|dir| has(dir)),
        };
        if let Some(dir) = found {
            return dir.to_path_buf();
        }
    }
    checkout.to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn the_builtin_table_parses_and_every_named_server_exists() {
        let registry = Registry::parse(BUILTIN).expect("languages.toml parses");
        assert!(registry.language("rust").is_some());
        for language in registry.languages() {
            for server in &language.servers {
                assert!(
                    registry.server(server).is_some(),
                    "{} names unknown server {server}",
                    language.id
                );
            }
        }
        let mut seen = HashSet::new();
        for language in registry.languages() {
            for extension in &language.extensions {
                assert!(
                    seen.insert(extension.clone()),
                    "{extension} is claimed twice"
                );
            }
        }
    }

    #[test]
    fn paths_resolve_by_extension_and_by_file_name() {
        let registry = Registry::builtin();
        assert_eq!(registry.language_for_path("src/a.TSX").unwrap().id, "tsx");
        assert_eq!(
            registry.language_for_path("x/types.d.ts").unwrap().id,
            "typescript"
        );
        assert_eq!(registry.language_for_path("Rakefile").unwrap().id, "ruby");
        assert!(registry.language_for_path("Makefile").is_none());
        assert!(same_family(registry, "a.tsx", "b.js"));
        assert!(!same_family(registry, "a.rs", "b.js"));
        assert_eq!(
            registry.language("tsx").unwrap().lsp_id(),
            "typescriptreact"
        );
    }

    #[test]
    fn overrides_change_one_field_and_can_add_a_server() {
        let overrides: Overrides = toml::from_str(
            r#"
            [servers.rust-analyzer]
            command = "/opt/ra"
            [servers.ruby-lsp]
            command = "ruby-lsp"
            root_markers = [["Gemfile"]]
            [servers.nameless]
            args = ["x"]
            [languages.ruby]
            servers = ["ruby-lsp"]
            "#,
        )
        .unwrap();
        let registry = Registry::with_overrides(&overrides);
        let ra = registry.server("rust-analyzer").unwrap();
        assert_eq!(ra.command, "/opt/ra");
        assert_eq!(ra.root_strategy, RootStrategy::Outermost);
        assert_eq!(ra.initialization_options["cargo"]["targetDir"], true);
        assert!(registry.server("nameless").is_none());
        let ruby: Vec<_> = registry.servers_for_path("app.rb").map(|s| &s.id).collect();
        assert_eq!(ruby, ["ruby-lsp"]);
    }

    #[test]
    fn roots_stop_at_the_checkout_and_follow_the_strategy() {
        let registry = Registry::builtin();
        let checkout = Path::new("/w");
        let markers: HashSet<&Path> = [
            Path::new("/w/Cargo.toml"),
            Path::new("/w/crates/a/Cargo.toml"),
            Path::new("/Cargo.lock"),
        ]
        .into_iter()
        .collect();
        let exists = |p: &Path| markers.contains(p);
        let ra = registry.server("rust-analyzer").unwrap();
        let file = Path::new("/w/crates/a/src/lib.rs");
        assert_eq!(find_root(ra, checkout, file, exists), Path::new("/w"));
        let mut nearest = ra.clone();
        nearest.root_strategy = RootStrategy::Nearest;
        assert_eq!(
            find_root(&nearest, checkout, file, exists),
            Path::new("/w/crates/a")
        );
        assert_eq!(
            find_root(ra, checkout, Path::new("/elsewhere/x.rs"), exists),
            Path::new("/w")
        );
    }
}
