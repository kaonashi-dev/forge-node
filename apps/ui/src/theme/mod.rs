//! Colours from `apps/tauri/src/theme/tokens.ts`, the one source, resolved by
//! `bun run theme:rune` into `themes.json`. A Tauri test fails when the two
//! drift, so nothing here mixes, lifts or invents a colour.

use std::collections::HashMap;
use std::sync::OnceLock;

use serde::Deserialize;

const THEMES_JSON: &str = include_str!("themes.json");
/// Where the choice lives in the daemon's `app_state`, shared with Tauri.
pub const THEME_KEY: &str = "ui.theme_base";
/// The stored value that follows the system's light or dark.
pub const SYSTEM: &str = "system";
/// What a person who never chose sees, as in Tauri.
pub const DEFAULT_BASE: &str = "calm-dark";

#[derive(Deserialize)]
struct ThemeFile {
    bases: Vec<String>,
    themes: HashMap<String, Theme>,
}

#[derive(Debug, Deserialize)]
pub struct Theme {
    pub label: String,
    pub light: bool,
    pub palette: TermColors,
    pub editor: EditorColors,
}

/// The palette's ground, text and accent for the chrome, and the terminal's
/// ground, text and sixteen named colours.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TermColors {
    pub bg: String,
    pub text: String,
    pub accent: String,
    pub term_bg: String,
    pub term_fg: String,
    pub ansi: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EditorColors {
    pub background: String,
    pub foreground: String,
    pub caret: String,
    pub selection: String,
    pub active_line: String,
    pub gutter_foreground: String,
    pub gutter_active_foreground: String,
    pub matching_bracket: String,
    pub search_match: String,
    pub search_match_selected: String,
    pub git_added: String,
    pub git_modified: String,
    pub git_deleted: String,
    pub scopes: Scopes,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Scopes {
    pub comment: String,
    pub keyword: String,
    pub control_keyword: String,
    pub string: String,
    pub number: String,
    #[serde(rename = "type")]
    pub ty: String,
    pub function: String,
    pub property: String,
    pub constant: String,
    pub meta: String,
}

fn file() -> &'static ThemeFile {
    static FILE: OnceLock<ThemeFile> = OnceLock::new();
    FILE.get_or_init(|| {
        // Embedded and checked by `themes_json_has_every_base`: a parse
        // failure is a build that never passed its own tests.
        serde_json::from_str(THEMES_JSON).expect("themes.json is generated and tested")
    })
}

/// The bases a person can pick, in `tokens.ts` order.
pub fn bases() -> &'static [String] {
    &file().bases
}

pub fn theme(base: &str) -> Option<&'static Theme> {
    file().themes.get(base)
}

/// The base shown when none is chosen: the calm pair, following the system.
#[must_use]
pub fn default_base(dark: bool) -> &'static str {
    if dark { "calm-dark" } else { "calm-light" }
}

/// The chrome's kit for `base`, derived by `rune-kit` from the palette's
/// ground, text and accent; `fallback_dark` decides when the base is unknown.
pub fn kit(base: &str, fallback_dark: bool) -> rune_kit::Kit {
    thread_local! {
        static KITS: std::cell::RefCell<HashMap<String, rune_kit::Kit>> =
            std::cell::RefCell::new(HashMap::new());
    }
    KITS.with(|kits| {
        if let Some(kit) = kits.borrow().get(base) {
            return kit.clone();
        }
        let parse = |value: &str| rune::color::hex(value);
        let kit = match theme(base) {
            Some(found) => match (
                parse(&found.palette.bg),
                parse(&found.palette.text),
                parse(&found.palette.accent),
            ) {
                (Some(bg), Some(fg), Some(accent)) => {
                    rune_kit::Kit::from_palette(rune_kit::PaletteSeed { bg, fg, accent })
                }
                _ => rune_kit::Kit::neutral(!found.light),
            },
            None => rune_kit::Kit::neutral(fallback_dark),
        };
        kits.borrow_mut().insert(base.to_string(), kit.clone());
        kit
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn themes_json_has_every_base() {
        assert!(bases().len() >= 2);
        for base in bases() {
            let theme = theme(base).expect("each listed base has a theme");
            assert!(theme.editor.background.starts_with('#'), "{base}");
        }
        assert!(!theme(default_base(true)).expect("calm-dark").light);
        assert!(theme(default_base(false)).expect("calm-light").light);
    }
}
