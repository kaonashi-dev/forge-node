//! The editor palette: the calm-dark values of the editor roles in
//! `apps/tauri/src/theme/{tokens,editorTheme,syntax}.ts`, copied by hand until
//! both apps read one token source.

use std::sync::OnceLock;

use domain::EditorScope;
use rune::Color;
use rune::color::hex;

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
    pub rev: u64,
}

static CALM_DARK: OnceLock<EditorPalette> = OnceLock::new();

pub fn calm_dark() -> EditorPalette {
    *CALM_DARK.get_or_init(build_calm_dark)
}

fn build_calm_dark() -> EditorPalette {
    let c = |value: &str| hex(value).unwrap_or(Color::TRANSPARENT);
    EditorPalette {
        background: c("#111112"),
        foreground: c("#ededef"),
        caret: c("#ededef"),
        selection: c("#434345"),
        active_line: c("#1c1c1d"),
        gutter_foreground: c("#7e7e87"),
        gutter_active_foreground: c("#ededef"),
        matching_bracket: c("#515154"),
        search_match: c("#4c401f"),
        search_match_selected: c("#6c4929"),
        git_added: c("#4ade80"),
        git_modified: c("#e3b341"),
        git_deleted: c("#f87171"),
        comment: c("#7e7e87"),
        keyword: c("#fca5a5"),
        control_keyword: c("#c4b5fd"),
        string: c("#86efac"),
        number: c("#c4b5fd"),
        ty: c("#8fe0ee"),
        function: c("#f0c96a"),
        property: c("#a5c0fa"),
        constant: c("#c4b5fd"),
        fold: c("#a0a0a8"),
        rev: 1,
    }
}

impl EditorPalette {
    pub fn scope(self, scope: EditorScope) -> Color {
        match scope {
            EditorScope::Plain => self.foreground,
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
