//! One parse, and what the tags and locals queries read off it.

use std::collections::HashMap;
use std::ops::ControlFlow;

use fs_service::definition::utf16_column;
use tree_sitter::{
    Node, ParseOptions, Parser, QueryCursor, QueryCursorOptions, StreamingIterator, Tree,
};

use crate::cache::Definition;
use crate::grammars::{Compiled, LocalRole};

/// Longest declaring line a candidate carries, in bytes.
pub(crate) const MAX_TEXT_BYTES: usize = 512;
/// A generated file can declare far more than anyone navigates to; past this
/// the rest of the file is not offered.
pub(crate) const MAX_DEFINITIONS_PER_FILE: usize = 16_384;

fn progress<'a, S>(expired: &'a dyn Fn() -> bool) -> impl FnMut(&S) -> ControlFlow<()> + 'a {
    move |_| {
        if expired() {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    }
}

/// `None` when the grammar cannot be loaded or `expired` stopped the parse.
pub(crate) fn parse(
    parser: &mut Parser,
    compiled: &Compiled,
    text: &str,
    expired: &dyn Fn() -> bool,
) -> Option<Tree> {
    parser.set_language(&compiled.language).ok()?;
    let bytes = text.as_bytes();
    let mut on_progress = progress(expired);
    let tree = parser.parse_with_options(
        &mut |offset, _| bytes.get(offset..).unwrap_or_default(),
        None,
        Some(ParseOptions::new().progress_callback(&mut on_progress)),
    );
    if tree.is_none() {
        parser.reset();
    }
    tree
}

/// Every `@definition.*` with its `@name`, in source order; `None` when
/// `expired` cut the query short.
pub(crate) fn definitions(
    compiled: &Compiled,
    tree: &Tree,
    text: &str,
    expired: &dyn Fn() -> bool,
) -> Option<Vec<Definition>> {
    let tags = &compiled.tags;
    let mut cursor = QueryCursor::new();
    let mut on_progress = progress(expired);
    let mut matches = cursor.matches_with_options(
        &tags.query,
        tree.root_node(),
        text.as_bytes(),
        QueryCursorOptions::new().progress_callback(&mut on_progress),
    );
    let mut found: Vec<Definition> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    while let Some(matched) = matches.next() {
        let is_definition = matched.captures.iter().any(|capture| {
            tags.definition
                .get(capture.index as usize)
                .copied()
                .unwrap_or(false)
        });
        if !is_definition {
            continue;
        }
        for capture in matched.captures.iter().filter(|c| c.index == tags.name) {
            if found.len() >= MAX_DEFINITIONS_PER_FILE {
                break;
            }
            if seen.insert(capture.node.byte_range()) {
                found.push(definition_at(text, capture.node));
            }
        }
    }
    drop(matches);
    if expired() {
        return None;
    }
    found.sort_by_key(|d| (d.line, d.column));
    Some(found)
}

fn definition_at(text: &str, name: Node<'_>) -> Definition {
    let range = name.byte_range();
    let start = text.get(..range.start).map_or(0, |before| {
        before.rfind('\n').map_or(0, |newline| newline + 1)
    });
    let end = text
        .get(range.start..)
        .and_then(|after| after.find('\n'))
        .map_or(text.len(), |newline| range.start + newline);
    let line = text.get(start..end).unwrap_or_default();
    Definition {
        name: text.get(range).unwrap_or_default().into(),
        line: u32::try_from(name.start_position().row + 1).unwrap_or(u32::MAX),
        column: utf16_column(line, name.start_byte() - start),
        text: clamp(line.trim()).into(),
        container: container(text, name).map(Into::into),
        annotation: name
            .parent()
            .and_then(|parent| parent.child_by_field_name("type"))
            .and_then(|annotated| type_name(text, annotated))
            .map(Into::into),
    }
}

/// Where a declaration lives, by the field that names it: Rust's `impl`
/// names its type, the rest their own name. An object literal's key counts,
/// so `UI.errors.title` lives in `errors`.
const CONTAINERS: [(&str, &str); 13] = [
    ("interface_declaration", "name"),
    ("type_alias_declaration", "name"),
    ("class_declaration", "name"),
    ("abstract_class_declaration", "name"),
    ("enum_declaration", "name"),
    ("variable_declarator", "name"),
    ("pair", "key"),
    ("struct_item", "name"),
    ("enum_item", "name"),
    ("trait_item", "name"),
    ("impl_item", "type"),
    ("class_definition", "name"),
    ("type_spec", "name"),
];

/// Ancestors walked looking for a container; a member sits a few levels below.
const MAX_CONTAINER_DEPTH: usize = 12;

fn container<'t>(text: &'t str, name: Node<'_>) -> Option<&'t str> {
    let mut node = name.parent()?;
    for _ in 0..MAX_CONTAINER_DEPTH {
        if let Some((_, field)) = CONTAINERS.iter().find(|(kind, _)| *kind == node.kind()) {
            if let Some(named) = node.child_by_field_name(field) {
                // A declaration is not its own container, and a destructuring
                // pattern names nothing.
                let names = named.kind().ends_with("identifier") || named.kind() == "generic_type";
                if named.id() != name.id() && names {
                    return type_name(text, named).or_else(|| text.get(named.byte_range()));
                }
            }
        }
        node = node.parent()?;
    }
    None
}

/// The plain type a type annotation names: `Money` for `: Money`,
/// `Box<Money>`'s `Box`. `None` for a union, a literal or an array.
fn type_name<'t>(text: &'t str, node: Node<'_>) -> Option<&'t str> {
    let mut node = node;
    for _ in 0..4 {
        match node.kind() {
            "type_identifier" => return text.get(node.byte_range()),
            "type_annotation" => node = node.named_child(0)?,
            "generic_type" => {
                node = node
                    .child_by_field_name("name")
                    .or_else(|| node.child_by_field_name("type"))?;
            }
            _ => return None,
        }
    }
    None
}

/// What a member access at the caret is looked up on.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Receiver {
    /// `UI_STRINGS` in `UI_STRINGS.title`; `queue` in `this.queue.add`.
    pub(crate) name: String,
    /// Types a binding of `name` in this file is annotated with.
    pub(crate) types: Vec<String>,
    /// Object types `name` is destructured from (`{ data }: Props`), whose
    /// field `name` says the type.
    pub(crate) destructured_from: Vec<String>,
}

/// `(member kind, object field, property field)` across the grammars.
const MEMBER_ACCESS: [(&str, &str, &str); 4] = [
    ("member_expression", "object", "property"),
    ("field_expression", "value", "field"),
    ("attribute", "object", "attribute"),
    ("selector_expression", "operand", "field"),
];

/// Receiver bindings searched past this many nodes are not searched.
const MAX_RECEIVER_NODES: usize = 200_000;
const MAX_RECEIVER_TYPES: usize = 8;

/// The receiver of `symbol` at `caret` when the caret is on the property of
/// a member access, with what this file says about its type.
pub(crate) fn receiver(
    tree: &Tree,
    text: &str,
    symbol: &str,
    caret: usize,
    expired: &dyn Fn() -> bool,
) -> Option<Receiver> {
    let bytes = text.as_bytes();
    let root = tree.root_node();
    let property = [caret, caret.saturating_sub(1)]
        .into_iter()
        .filter_map(|at| root.named_descendant_for_byte_range(at, at))
        .find(|node| bytes.get(node.byte_range()) == Some(symbol.as_bytes()))?;
    let access = property.parent()?;
    let (_, object_field, property_field) = MEMBER_ACCESS
        .iter()
        .find(|(kind, _, _)| *kind == access.kind())?;
    if access.child_by_field_name(property_field)?.id() != property.id() {
        return None;
    }
    let object = access.child_by_field_name(object_field)?;
    let name_node = if object.named_child_count() == 0 {
        object
    } else {
        // `this.queue.add` and `self.cache.get`: the field is the receiver.
        let (_, _, inner_property) = MEMBER_ACCESS
            .iter()
            .find(|(kind, _, _)| *kind == object.kind())?;
        object.child_by_field_name(inner_property)?
    };
    let name = text.get(name_node.byte_range())?;
    if matches!(name, "this" | "self" | "super") {
        return None;
    }
    let mut found = Receiver {
        name: name.to_string(),
        ..Receiver::default()
    };
    let mut cursor = root.walk();
    let mut visited = 0usize;
    'walk: loop {
        let node = cursor.node();
        visited += 1;
        if visited > MAX_RECEIVER_NODES || (visited % 4096 == 0 && expired()) {
            break;
        }
        if node.child_count() == 0 && bytes.get(node.byte_range()) == Some(name.as_bytes()) {
            annotate(text, node, &mut found);
        }
        if cursor.goto_first_child() || cursor.goto_next_sibling() {
            continue;
        }
        loop {
            if !cursor.goto_parent() {
                break 'walk;
            }
            if cursor.goto_next_sibling() {
                break;
            }
        }
    }
    Some(found)
}

/// Records the type a binding at `node` is annotated with, if it is one.
fn annotate(text: &str, node: Node<'_>, found: &mut Receiver) {
    let Some(parent) = node.parent() else {
        return;
    };
    let push = |list: &mut Vec<String>, ty: Option<&str>| {
        if let Some(ty) = ty {
            if list.len() < MAX_RECEIVER_TYPES && !list.iter().any(|seen| seen == ty) {
                list.push(ty.to_string());
            }
        }
    };
    let binds = ["name", "pattern"].iter().any(|field| {
        parent
            .child_by_field_name(field)
            .is_some_and(|n| n.id() == node.id())
    });
    if binds {
        if let Some(annotated) = parent.child_by_field_name("type") {
            push(&mut found.types, type_name(text, annotated));
        }
        return;
    }
    if node.kind() == "shorthand_property_identifier_pattern" && parent.kind() == "object_pattern" {
        if let Some(annotated) = parent.parent().and_then(|p| p.child_by_field_name("type")) {
            push(&mut found.destructured_from, type_name(text, annotated));
        }
    }
}

fn clamp(text: &str) -> &str {
    clip(text, MAX_TEXT_BYTES)
}

/// Byte offset of a 1-based line and UTF-16 column, clamped to that line.
pub(crate) fn caret_byte(text: &str, line: u32, utf16_column: u32) -> Option<usize> {
    let mut start = 0;
    for _ in 1..line {
        start += text.get(start..)?.find('\n')? + 1;
    }
    let rest = text.get(start..)?;
    let line = rest.find('\n').map_or(rest, |end| &rest[..end]);
    Some(start + fs_service::definition::byte_for_utf16(line, utf16_column))
}

#[derive(Clone, Copy)]
struct Binding<'tree> {
    node: Node<'tree>,
    hoisted: bool,
}

/// Where `symbol` at `caret` is bound in this file, if the caret is on a
/// plain name and some binding of it is in scope there.
///
/// The innermost scope with a visible binding wins. In it, the latest
/// ordinary binding before the caret shadows the rest; failing one, every
/// hoisted binding is offered.
pub(crate) fn locals(
    compiled: &Compiled,
    tree: &Tree,
    text: &str,
    symbol: &str,
    caret: usize,
    expired: &dyn Fn() -> bool,
) -> Option<Vec<Definition>> {
    let locals = compiled.locals.as_ref()?;
    let mut scopes: Vec<(Node<'_>, bool)> = Vec::new();
    let mut declarations: Vec<Node<'_>> = Vec::new();
    let mut bindings: HashMap<usize, Binding<'_>> = HashMap::new();
    let mut on_name = false;
    let mut on_member = false;
    let mut cursor = QueryCursor::new();
    let mut on_progress = progress(expired);
    let bytes = text.as_bytes();
    let mut matches = cursor.matches_with_options(
        &locals.query,
        tree.root_node(),
        bytes,
        QueryCursorOptions::new().progress_callback(&mut on_progress),
    );
    while let Some(matched) = matches.next() {
        for capture in matched.captures {
            let node = capture.node;
            let role = locals
                .roles
                .get(capture.index as usize)
                .copied()
                .unwrap_or(LocalRole::Other);
            match role {
                LocalRole::Scope => scopes.push((node, false)),
                LocalRole::IsolatedScope => scopes.push((node, true)),
                LocalRole::Declaration => declarations.push(node),
                LocalRole::Other => {}
                LocalRole::Definition
                | LocalRole::HoistedDefinition
                | LocalRole::Reference
                | LocalRole::Member => {
                    if bytes.get(node.byte_range()) != Some(symbol.as_bytes()) {
                        continue;
                    }
                    let at_caret = node.start_byte() <= caret && caret <= node.end_byte();
                    match role {
                        LocalRole::Member => on_member |= at_caret,
                        LocalRole::Reference => on_name |= at_caret,
                        _ => {
                            on_name |= at_caret;
                            let hoisted = role == LocalRole::HoistedDefinition;
                            bindings
                                .entry(node.id())
                                .and_modify(|b| b.hoisted |= hoisted)
                                .or_insert(Binding { node, hoisted });
                        }
                    }
                }
            }
        }
    }
    drop(matches);
    if expired() || !on_name || on_member || bindings.is_empty() {
        return None;
    }

    let root = tree.root_node();
    let caret_scope = innermost(&scopes, caret, caret, None);
    let mut best: Option<(usize, Vec<Binding<'_>>, Option<usize>)> = None;
    for binding in bindings.into_values() {
        let node = binding.node;
        let scope = innermost(&scopes, node.start_byte(), node.end_byte(), Some(node));
        let (range, isolated) = scope.map_or((root.byte_range(), false), |(scope, isolated)| {
            (scope.byte_range(), isolated)
        });
        if caret < range.start || caret > range.end {
            continue;
        }
        if isolated && caret_scope.map(|(s, _)| s.id()) != scope.map(|(s, _)| s.id()) {
            continue;
        }
        let on_itself = node.start_byte() <= caret && caret <= node.end_byte();
        let visible_from = if binding.hoisted || on_itself {
            None
        } else {
            let from = declarations
                .iter()
                .filter(|d| d.start_byte() <= node.start_byte() && node.end_byte() <= d.end_byte())
                .map(Node::end_byte)
                .min()
                .unwrap_or(node.start_byte());
            if from > caret {
                continue;
            }
            Some(from)
        };
        let width = range.len();
        let replace = match &best {
            None => true,
            Some((best_width, _, _)) => width < *best_width,
        };
        if replace {
            best = Some((width, vec![binding], visible_from));
            continue;
        }
        if let Some((best_width, group, best_from)) = &mut best {
            if width != *best_width {
                continue;
            }
            match (visible_from, *best_from) {
                (Some(from), Some(current)) if from > current => {
                    *group = vec![binding];
                    *best_from = Some(from);
                }
                (Some(_), Some(_)) => {}
                (Some(from), None) => {
                    *group = vec![binding];
                    *best_from = Some(from);
                }
                (None, Some(_)) => {}
                (None, None) => group.push(binding),
            }
        }
    }
    let (_, group, _) = best?;
    let mut found: Vec<Definition> = group
        .into_iter()
        .map(|binding| definition_at(text, binding.node))
        .collect();
    found.sort_by_key(|d| (d.line, d.column));
    Some(found)
}

/// The smallest scope around `start..end`, skipping a scope whose own
/// `name` is that range: a function's name is bound outside the function.
fn innermost<'tree>(
    scopes: &[(Node<'tree>, bool)],
    start: usize,
    end: usize,
    named: Option<Node<'tree>>,
) -> Option<(Node<'tree>, bool)> {
    scopes
        .iter()
        .copied()
        .filter(|(scope, _)| scope.start_byte() <= start && end <= scope.end_byte())
        .filter(|(scope, _)| {
            named.is_none_or(|named| {
                scope
                    .child_by_field_name("name")
                    .is_none_or(|name| name.id() != named.id())
            })
        })
        .min_by_key(|(scope, _)| scope.end_byte() - scope.start_byte())
}

/// Signature lines are cut here, in bytes.
const MAX_SIGNATURE_BYTES: usize = 200;
/// A generated file can call far more than any graph needs.
const MAX_REFERENCES_PER_FILE: usize = 65_536;

/// Definitions and the references the tags query captures, in source order.
pub(crate) fn symbols(compiled: &Compiled, tree: &Tree, text: &str) -> crate::Symbols {
    let tags = &compiled.tags;
    let names = tags.query.capture_names();
    let parent = tags.query.capture_index_for_name("parent");
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(&tags.query, tree.root_node(), text.as_bytes());
    let mut found = crate::Symbols::default();
    let mut seen = std::collections::HashSet::new();
    let slice = |node: Node<'_>| text.get(node.byte_range()).unwrap_or_default().to_string();
    while let Some(matched) = matches.next() {
        let name = matched.captures.iter().find(|c| c.index == tags.name);
        let parent = matched.captures.iter().find(|c| Some(c.index) == parent);
        let Some((role, outer)) = matched.captures.iter().find_map(|c| {
            let role = names.get(c.index as usize)?;
            (role.starts_with("definition.") || role.starts_with("reference."))
                .then_some((*role, c.node))
        }) else {
            continue;
        };
        let Some(name) = name.map(|c| c.node) else {
            continue;
        };
        if let Some(kind) = role.strip_prefix("definition.") {
            if found.definitions.len() < MAX_DEFINITIONS_PER_FILE && seen.insert(name.byte_range())
            {
                let signature = slice(outer);
                let signature = signature.lines().next().unwrap_or_default().trim();
                found.definitions.push(crate::Symbol {
                    kind: kind.to_string(),
                    name: slice(name),
                    span: (outer.start_byte(), outer.end_byte()),
                    signature: clip(signature, MAX_SIGNATURE_BYTES).to_string(),
                });
            }
            continue;
        }
        let references = found.calls.len() + found.imports.len() + found.inherits.len();
        if references >= MAX_REFERENCES_PER_FILE {
            continue;
        }
        match role {
            "reference.call" => found.calls.push(crate::Reference {
                name: slice(name),
                span: (name.start_byte(), name.end_byte()),
            }),
            "reference.import" => found.imports.push(slice(name)),
            "reference.implementation" => {
                if let Some(parent) = parent {
                    found.inherits.push((slice(name), slice(parent.node)));
                }
            }
            _ => {}
        }
    }
    found
        .definitions
        .sort_by_key(|definition| definition.span.0);
    found
}

fn clip(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}
