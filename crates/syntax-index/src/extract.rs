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
    }
}

fn clamp(text: &str) -> &str {
    if text.len() <= MAX_TEXT_BYTES {
        return text;
    }
    let mut end = MAX_TEXT_BYTES;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
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
