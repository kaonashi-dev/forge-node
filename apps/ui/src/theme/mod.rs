//! Colours from `apps/tauri/src/theme/tokens.ts`, the one source, resolved by
//! `bun run theme:rune` into `themes.json`. A Tauri test fails when the two
//! drift. A built-in base is read, never derived; only a custom theme (JSON
//! stored in `ui.theme_base`, as Tauri stores it) is derived, by `custom`.

pub mod custom;
pub mod preview;
pub mod roles;

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Arc, Mutex, OnceLock};

use serde::Deserialize;

pub use custom::Palette;
pub use roles::Roles;

const THEMES_JSON: &str = include_str!("themes.json");
/// Where the choice lives in the daemon's `app_state`, shared with Tauri.
pub const THEME_KEY: &str = "ui.theme_base";
/// The last custom theme, kept so choosing a base does not lose it.
pub const CUSTOM_KEY: &str = "ui.custom_theme";
/// The stored value that follows the system's light or dark.
pub const SYSTEM: &str = "system";
/// What a person who never chose sees, as in Tauri.
pub const DEFAULT_BASE: &str = "calm-dark";
/// Previews of a custom theme are distinct keys; past this many, caches restart.
pub const CACHE_LIMIT: usize = 24;

#[derive(Deserialize)]
struct ThemeFile {
    bases: Vec<String>,
    themes: HashMap<String, Theme>,
    accents: Accents,
    #[cfg(test)]
    variants: Vec<VariantCase>,
}

#[derive(Debug, Deserialize)]
pub struct Theme {
    pub label: String,
    pub light: bool,
    pub palette: Palette,
    pub editor: EditorColors,
    pub chrome: roles::ChromeColors,
}

#[derive(Debug, Deserialize)]
pub struct Swatch {
    pub name: String,
    pub color: String,
}

#[derive(Debug, Deserialize)]
struct Accents {
    dark: Vec<Swatch>,
    light: Vec<Swatch>,
}

#[cfg(test)]
#[derive(Debug, Deserialize)]
pub struct VariantColor {
    pub key: String,
    pub value: String,
}

#[cfg(test)]
#[derive(Debug, Deserialize)]
pub struct VariantCase {
    pub base: String,
    pub color: Option<VariantColor>,
    pub contrast: u8,
    pub expected: custom::CustomTheme,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
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

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
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

fn built_in() -> &'static HashMap<String, Arc<Theme>> {
    static THEMES: OnceLock<HashMap<String, Arc<Theme>>> = OnceLock::new();
    THEMES.get_or_init(|| {
        let raw: ThemeFile =
            serde_json::from_str(THEMES_JSON).expect("themes.json is generated and tested");
        raw.themes
            .into_iter()
            .map(|(id, theme)| (id, Arc::new(theme)))
            .collect()
    })
}

/// The bases a person can pick, in `tokens.ts` order.
pub fn bases() -> &'static [String] {
    &file().bases
}

/// Tauri's accent swatches for a light or dark theme.
pub fn accents(light: bool) -> &'static [Swatch] {
    if light {
        &file().accents.light
    } else {
        &file().accents.dark
    }
}

#[cfg(test)]
pub fn variant_fixture() -> Vec<custom_fixture::Case> {
    custom_fixture::cases(&file().variants)
}

#[cfg(test)]
mod custom_fixture {
    use super::custom::{CustomTheme, ThemeColor};

    pub struct Case {
        pub label: String,
        pub base: String,
        pub name: String,
        pub color: Option<(ThemeColor, String)>,
        pub contrast: u8,
        pub expected: CustomTheme,
    }

    pub fn cases(raw: &[super::VariantCase]) -> Vec<Case> {
        raw.iter()
            .map(|case| Case {
                label: format!(
                    "{} {:?} {}",
                    case.base,
                    case.color.as_ref().map(|c| (&c.key, &c.value)),
                    case.contrast
                ),
                base: case.base.clone(),
                name: case.expected.name.clone(),
                color: case.color.as_ref().map(|color| {
                    let key = match color.key.as_str() {
                        "accent" => ThemeColor::Accent,
                        "bg" => ThemeColor::Bg,
                        _ => ThemeColor::Text,
                    };
                    (key, color.value.clone())
                }),
                contrast: case.contrast,
                expected: case.expected.clone(),
            })
            .collect()
    }
}

/// Whether a stored preference is a custom theme rather than a base id.
#[must_use]
pub fn is_custom(value: &str) -> bool {
    value.starts_with('{')
}

/// A base by id, or a custom theme by its JSON; `None` for anything unknown
/// or invalid, so the caller falls back rather than painting nothing.
pub fn theme(base: &str) -> Option<Arc<Theme>> {
    if !is_custom(base) {
        return built_in().get(base).cloned();
    }
    static CUSTOM: OnceLock<Mutex<HashMap<String, Arc<Theme>>>> = OnceLock::new();
    let cache = CUSTOM.get_or_init(|| Mutex::new(HashMap::new()));
    let mut cache = cache
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(found) = cache.get(base) {
        return Some(Arc::clone(found));
    }
    let parsed = custom::parse(base).ok()?;
    let theme = Arc::new(Theme {
        label: parsed.name.clone(),
        light: parsed.mode == "light",
        editor: custom::editor_colors(&parsed.palette),
        chrome: roles::chrome_colors(&parsed.palette),
        palette: parsed.palette,
    });
    if cache.len() >= CACHE_LIMIT {
        cache.clear();
    }
    cache.insert(base.to_string(), Arc::clone(&theme));
    Some(theme)
}

/// The base shown when none is chosen: the calm pair, following the system.
#[must_use]
pub fn default_base(dark: bool) -> &'static str {
    if dark { "calm-dark" } else { "calm-light" }
}

struct Resolved {
    kit: rune_kit::Kit,
    roles: Arc<Roles>,
    tokens: Fingerprint,
}

/// Tokens every base and every contrast level moves. Views ask per row, and
/// hashing the whole token set cost more than the rows (the switch bench).
type Fingerprint = [rune::Color; 10];

fn fingerprint(kit: &rune_kit::Kit) -> Fingerprint {
    let c = &kit.c;
    [
        c.page,
        c.bg_base,
        c.bg_accent,
        c.text_base,
        c.text_muted,
        c.text_faint,
        c.border_muted,
        c.layer_02,
        c.layer_03,
        c.danger.icon,
    ]
}

thread_local! {
    static RESOLVED: std::cell::RefCell<HashMap<String, Rc<Resolved>>> =
        std::cell::RefCell::new(HashMap::new());
}

fn cached(base: &str) -> Option<Rc<Resolved>> {
    RESOLVED.with(|resolved| {
        if let Some(found) = resolved.borrow().get(base) {
            return Some(Rc::clone(found));
        }
        let found = theme(base)?;
        let roles = Arc::new(Roles::new(&found));
        let kit = roles.kit();
        let entry = Rc::new(Resolved {
            tokens: fingerprint(&kit),
            kit,
            roles,
        });
        let mut resolved = resolved.borrow_mut();
        if resolved.len() >= CACHE_LIMIT {
            resolved.clear();
        }
        resolved.insert(base.to_string(), Rc::clone(&entry));
        Some(entry)
    })
}

fn resolve(base: &str, fallback_dark: bool) -> Rc<Resolved> {
    cached(base)
        .or_else(|| cached(default_base(fallback_dark)))
        .expect("the default bases are built in and tested")
}

/// The chrome's kit for `base`: rune-kit's tokens carrying Tauri's roles. An
/// unknown base paints as the default one for `fallback_dark`.
pub fn kit(base: &str, fallback_dark: bool) -> rune_kit::Kit {
    resolve(base, fallback_dark).kit.clone()
}

/// Every chrome role for `base`, for what rune-kit has no token for: project
/// hues, needs-you, git status, the strip and the active tab.
pub fn roles_for(base: &str, fallback_dark: bool) -> Arc<Roles> {
    Arc::clone(&resolve(base, fallback_dark).roles)
}

/// The roles `k` was built from, so a view holding only the kit reads the
/// same theme; a kit `kit` did not build gets the default base's.
pub fn roles(k: &rune_kit::Kit) -> Arc<Roles> {
    let tokens = fingerprint(k);
    let known = RESOLVED.with(|resolved| {
        resolved
            .borrow()
            .values()
            .find(|found| found.tokens == tokens)
            .map(|found| Arc::clone(&found.roles))
    });
    known.unwrap_or_else(|| roles_for(default_base(k.dark), k.dark))
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
        assert_eq!(accents(false).len(), accents(true).len());
    }

    #[test]
    fn a_custom_theme_resolves_like_a_base() {
        let source = theme("gruvbox").expect("gruvbox").palette.clone();
        let json = custom::variant("Mine", source, 70).to_json();
        let found = theme(&json).expect("a valid custom theme resolves");
        assert_eq!(found.label, "Mine");
        assert!(!found.light);
        assert!(theme("{not json").is_none());
    }
}
