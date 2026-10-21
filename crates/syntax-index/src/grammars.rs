//! The compiled-in grammars: adding a language is one dependency, one row in
//! [`GRAMMARS`], its query files and a fixture test.

use std::sync::OnceLock;

use tree_sitter::{Language, Query};
use tree_sitter_language::LanguageFn;

pub(crate) struct Grammar {
    pub(crate) id: &'static str,
    language: LanguageFn,
    tags: &'static str,
    locals: &'static str,
}

macro_rules! queries {
    ($($file:literal),+) => {
        concat!($(include_str!(concat!("../queries/", $file))),+)
    };
}

pub(crate) const GRAMMARS: &[Grammar] = &[
    Grammar {
        id: "rust",
        language: tree_sitter_rust::LANGUAGE,
        tags: queries!("rust/tags.scm"),
        locals: queries!("rust/locals.scm"),
    },
    Grammar {
        id: "typescript",
        language: tree_sitter_typescript::LANGUAGE_TYPESCRIPT,
        tags: queries!("ecma/tags.scm", "typescript/tags.scm"),
        locals: queries!("ecma/locals.scm", "typescript/locals.scm"),
    },
    Grammar {
        id: "tsx",
        language: tree_sitter_typescript::LANGUAGE_TSX,
        tags: queries!("ecma/tags.scm", "typescript/tags.scm"),
        locals: queries!("ecma/locals.scm", "typescript/locals.scm"),
    },
    Grammar {
        id: "javascript",
        language: tree_sitter_javascript::LANGUAGE,
        tags: queries!("ecma/tags.scm", "javascript/tags.scm"),
        locals: queries!("ecma/locals.scm", "javascript/locals.scm"),
    },
    Grammar {
        id: "python",
        language: tree_sitter_python::LANGUAGE,
        tags: queries!("python/tags.scm"),
        locals: queries!("python/locals.scm"),
    },
    Grammar {
        id: "go",
        language: tree_sitter_go::LANGUAGE,
        tags: queries!("go/tags.scm"),
        locals: queries!("go/locals.scm"),
    },
];

/// Capture indices of a tags query.
pub(crate) struct Tags {
    pub(crate) query: Query,
    pub(crate) name: u32,
    pub(crate) definition: Vec<bool>,
}

/// What each capture of a locals query means.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum LocalRole {
    Scope,
    /// Names bound directly in it are invisible to the scopes nested in it.
    IsolatedScope,
    /// Visible from its own position onward.
    Definition,
    /// Visible throughout its scope.
    HoistedDefinition,
    /// A [`LocalRole::Definition`] inside it is visible only once it ends,
    /// so `let x = x + 1` resolves the right-hand `x` to an earlier binding.
    Declaration,
    Reference,
    /// A name looked up on something else (`a.b`, `a::b`), never a local.
    Member,
    Other,
}

pub(crate) struct Locals {
    pub(crate) query: Query,
    pub(crate) roles: Vec<LocalRole>,
}

pub(crate) struct Compiled {
    pub(crate) language: Language,
    pub(crate) tags: Tags,
    pub(crate) locals: Option<Locals>,
}

static COMPILED: [OnceLock<Option<Compiled>>; GRAMMARS.len()] =
    [const { OnceLock::new() }; GRAMMARS.len()];

/// The compiled queries of a grammar id, built once on first use.
///
/// `None` for an unknown id, and for a query that does not compile, which the
/// `every_embedded_query_compiles` test rules out for the built-ins.
pub(crate) fn compiled(id: &str) -> Option<&'static Compiled> {
    let index = GRAMMARS.iter().position(|grammar| grammar.id == id)?;
    COMPILED[index]
        .get_or_init(|| match compile(&GRAMMARS[index]) {
            Ok(compiled) => Some(compiled),
            Err(error) => {
                tracing::error!(grammar = id, %error, "embedded syntax query does not compile");
                None
            }
        })
        .as_ref()
}

pub(crate) fn compile(grammar: &Grammar) -> Result<Compiled, tree_sitter::QueryError> {
    let language = Language::new(grammar.language);
    let tags = Query::new(&language, grammar.tags)?;
    let names = tags.capture_names();
    let definition = names
        .iter()
        .map(|name| name.starts_with("definition."))
        .collect();
    let name = tags
        .capture_index_for_name("name")
        .unwrap_or(u32::try_from(names.len()).unwrap_or(u32::MAX));
    let locals = if grammar.locals.trim().is_empty() {
        None
    } else {
        let query = Query::new(&language, grammar.locals)?;
        let roles = query
            .capture_names()
            .iter()
            .map(|name| match *name {
                "local.scope" => LocalRole::Scope,
                "local.scope.isolated" => LocalRole::IsolatedScope,
                "local.definition" => LocalRole::Definition,
                "local.definition.hoisted" => LocalRole::HoistedDefinition,
                "local.declaration" => LocalRole::Declaration,
                "local.reference" => LocalRole::Reference,
                "local.member" => LocalRole::Member,
                _ => LocalRole::Other,
            })
            .collect();
        Some(Locals { query, roles })
    };
    Ok(Compiled {
        language,
        tags: Tags {
            query: tags,
            name,
            definition,
        },
        locals,
    })
}
