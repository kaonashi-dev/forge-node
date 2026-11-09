//! The editor's colours as Rune values, from the shared theme file.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use domain::EditorScope;
use rune::Color;
use rune::color::hex;

use crate::theme;

#[derive(Clone, Copy, Debug)]
pub struct EditorPalette {
    pub background: Color,
    pub foreground: Color,
    pub caret: Color,
    pub selection: Color,
    pub active_line: Color,
    pub gutter_foreground: Color,
    pub gutter_active_foreground: Color,
    pub matching_bracket: Color,
    pub search_match: Color,
    pub search_match_selected: Color,
    pub git_added: Color,
    pub git_modified: Color,
    pub git_deleted: Color,
    pub comment: Color,
    pub keyword: Color,
    pub control_keyword: Color,
    pub string: Color,
    pub number: Color,
    pub ty: Color,
    pub function: Color,
    pub property: Color,
    pub constant: Color,
    pub fold: Color,
    /// Changes with the base, so memoised rows repaint when the theme does.
    pub rev: u64,
}

/// The palette for `base`, parsed once; an unknown base falls back to the
/// default for its brightness rather than painting nothing.
pub fn for_base(base: &str, dark: bool) -> EditorPalette {
    static CACHE: OnceLock<Mutex<HashMap<String, EditorPalette>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut cache = cache
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(palette) = cache.get(base) {
        return *palette;
    }
    let (id, colors) = match theme::theme(base) {
        Some(found) => (base, found),
        None => {
            let id = theme::default_base(dark);
            match theme::theme(id) {
                Some(found) => (id, found),
                None => return EditorPalette::blank(),
            }
        }
    };
    let rev = theme::bases().iter().position(|b| b == id).map_or_else(
        || {
            use std::hash::{Hash, Hasher};
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            id.hash(&mut hasher);
            hasher.finish() | 1 << 63
        },
        |i| i as u64 + 1,
    );
    let palette = EditorPalette::from_colors(&colors.editor, rev);
    if cache.len() >= theme::CACHE_LIMIT {
        cache.clear();
    }
    cache.insert(base.to_string(), palette);
    palette
}

impl EditorPalette {
    fn from_colors(e: &theme::EditorColors, rev: u64) -> Self {
        let c = |value: &str| hex(value).unwrap_or(Color::TRANSPARENT);
        Self {
            background: c(&e.background),
            foreground: c(&e.foreground),
            caret: c(&e.caret),
            selection: c(&e.selection),
            active_line: c(&e.active_line),
            gutter_foreground: c(&e.gutter_foreground),
            gutter_active_foreground: c(&e.gutter_active_foreground),
            matching_bracket: c(&e.matching_bracket),
            search_match: c(&e.search_match),
            search_match_selected: c(&e.search_match_selected),
            git_added: c(&e.git_added),
            git_modified: c(&e.git_modified),
            git_deleted: c(&e.git_deleted),
            comment: c(&e.scopes.comment),
            keyword: c(&e.scopes.keyword),
            control_keyword: c(&e.scopes.control_keyword),
            string: c(&e.scopes.string),
            number: c(&e.scopes.number),
            ty: c(&e.scopes.ty),
            function: c(&e.scopes.function),
            property: c(&e.scopes.property),
            constant: c(&e.scopes.constant),
            fold: c(&e.scopes.meta),
            rev,
        }
    }

    fn blank() -> Self {
        let t = Color::TRANSPARENT;
        Self {
            background: t,
            foreground: t,
            caret: t,
            selection: t,
            active_line: t,
            gutter_foreground: t,
            gutter_active_foreground: t,
            matching_bracket: t,
            search_match: t,
            search_match_selected: t,
            git_added: t,
            git_modified: t,
            git_deleted: t,
            comment: t,
            keyword: t,
            control_keyword: t,
            string: t,
            number: t,
            ty: t,
            function: t,
            property: t,
            constant: t,
            fold: t,
            rev: 0,
        }
    }

    pub fn scope(self, scope: EditorScope) -> Color {
        match scope {
            EditorScope::Comment => self.comment,
            EditorScope::Keyword => self.keyword,
            EditorScope::ControlKeyword => self.control_keyword,
            EditorScope::String => self.string,
            EditorScope::Number => self.number,
            EditorScope::Type => self.ty,
            EditorScope::Function => self.function,
            EditorScope::Property => self.property,
            EditorScope::Constant => self.constant,
            _ => self.foreground,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_light_and_dark_editors_differ_and_an_unknown_base_falls_back() {
        let dark = for_base(theme::default_base(true), true);
        let light = for_base(theme::default_base(false), false);
        assert_ne!(dark.background, light.background);
        assert_ne!(dark.rev, light.rev);
        let fallback = for_base("no-such-base", true);
        assert_eq!(fallback.background, dark.background);
    }
}
