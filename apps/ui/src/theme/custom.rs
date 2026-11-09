//! A custom theme stored as JSON in `ui.theme_base`, read and edited as
//! Tauri's `customTheme.ts`, `themeVariants.ts` and `editorTheme.ts` do. The
//! built-in bases never pass through here: their colours come resolved in
//! `themes.json`, and `editor_colors_match_every_built_in_base` holds this
//! derivation to that file.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::{EditorColors, Scopes};

pub const MAX_THEME_BYTES: usize = 16 * 1024;
pub const DEFAULT_CONTRAST: u8 = 60;
/// Syntax keeps 4:1 against the editor ground, as `SCOPE_CONTRAST_FLOOR`.
const SCOPE_FLOOR: f64 = 4.0;
const GUTTER_FLOOR: f64 = 3.0;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Finish {
    pub tab: String,
    pub rule: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hover: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Palette {
    pub bg: String,
    pub rail: String,
    pub sidebar: String,
    pub strip: String,
    pub surface: String,
    pub surface_hi: String,
    pub editor: String,
    pub step: String,
    pub text: String,
    pub muted: String,
    pub faint: String,
    pub accent: String,
    pub needs_you: String,
    pub needs_you_deep: String,
    pub green: String,
    pub red: String,
    pub amber: String,
    pub blue: String,
    pub term_bg: String,
    pub term_fg: String,
    pub git_added: String,
    pub git_modified: String,
    pub git_deleted: String,
    pub git_untracked: String,
    pub git_conflict: String,
    pub git_ignored: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fg_default: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fg_subtle: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub border: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub needs_you_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finish: Option<Finish>,
    pub ansi: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Adjustments {
    pub palette: Palette,
    pub contrast: u8,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CustomTheme {
    pub name: String,
    /// `"light"` or `"dark"`.
    pub mode: String,
    pub palette: Palette,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adjustments: Option<Adjustments>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThemeColor {
    Accent,
    Bg,
    Text,
}

/// The keys every palette must carry, as `parsePalette` walks forge-dark's.
const REQUIRED: [&str; 26] = [
    "bg",
    "rail",
    "sidebar",
    "surface",
    "surfaceHi",
    "editor",
    "step",
    "text",
    "muted",
    "faint",
    "accent",
    "needsYou",
    "needsYouDeep",
    "green",
    "red",
    "amber",
    "blue",
    "termBg",
    "termFg",
    "gitAdded",
    "gitModified",
    "gitDeleted",
    "gitUntracked",
    "gitConflict",
    "gitIgnored",
    "strip",
];
const OVERRIDES: [&str; 4] = ["fgDefault", "fgSubtle", "border", "needsYouText"];

pub fn mix(base: u32, over: u32, amount: u32) -> u32 {
    let over_weight = amount & 0xff;
    let base_weight = 255 - over_weight;
    let channel = |shift: u32| {
        (((base >> shift) & 0xff) * base_weight + ((over >> shift) & 0xff) * over_weight + 127)
            / 255
    };
    (channel(16) << 16) | (channel(8) << 8) | channel(0)
}

/// A percentage as the 0–255 weight `mix` takes, truncated as JavaScript's `| 0`.
pub fn pct(percent: f64) -> u32 {
    (((percent * 255.0 + 50.0) / 100.0) as i64 & 0xff) as u32
}

fn srgb(byte: u32) -> f64 {
    let value = f64::from(byte) / 255.0;
    if value <= 0.04045 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

pub fn luminance(color: u32) -> f64 {
    0.2126 * srgb((color >> 16) & 0xff)
        + 0.7152 * srgb((color >> 8) & 0xff)
        + 0.0722 * srgb(color & 0xff)
}

pub fn contrast(a: u32, b: u32) -> f64 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// `readableColor` against one ground: keep the hue while it reads, else step toward `toward`.
fn lift(color: u32, ground: u32, toward: u32, floor: f64) -> u32 {
    let score = |candidate: u32| contrast(candidate, ground);
    if score(color) >= floor {
        return color;
    }
    let mut target = toward;
    if score(target) < floor {
        for candidate in [0, 0x00ff_ffff] {
            if score(candidate) > score(target) {
                target = candidate;
            }
        }
    }
    for step in 1..=25 {
        let candidate = mix(color, target, pct(f64::from(step) * 4.0));
        if score(candidate) >= floor {
            return candidate;
        }
    }
    if score(target) > score(color) {
        target
    } else {
        color
    }
}

pub fn hex(color: u32) -> String {
    format!("#{color:06x}")
}

pub fn parse_hex(value: &str) -> Option<u32> {
    let raw = value.strip_prefix('#').unwrap_or(value);
    (raw.len() == 6 && raw.chars().all(|ch| ch.is_ascii_hexdigit()))
        .then(|| u32::from_str_radix(raw, 16).ok())
        .flatten()
}

fn is_color(value: Option<&Value>) -> bool {
    value
        .and_then(Value::as_str)
        .is_some_and(|text| text.len() == 7 && text.starts_with('#') && parse_hex(text).is_some())
}

/// `#RRGGBB`, lowercased, or `None` for anything else.
#[must_use]
pub fn normalize_color(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (trimmed.len() == 7 && trimmed.starts_with('#') && parse_hex(trimmed).is_some())
        .then(|| trimmed.to_ascii_lowercase())
}

/// Validates as `parseCustomTheme` does, with its messages.
pub fn parse(source: &str) -> Result<CustomTheme, String> {
    if source.len() > MAX_THEME_BYTES {
        return Err("Theme JSON must be 16 KB or smaller.".into());
    }
    let value: Value = serde_json::from_str(source)
        .map_err(|_| "Invalid JSON. Check commas and quotation marks.".to_string())?;
    let Some(object) = value.as_object() else {
        return Err("Theme name must contain 1–60 characters.".into());
    };
    let name = object.get("name").and_then(Value::as_str).unwrap_or("");
    if name.trim().is_empty() || name.encode_utf16().count() > 60 {
        return Err("Theme name must contain 1–60 characters.".into());
    }
    let mode = match object.get("mode").and_then(Value::as_str) {
        Some(mode @ ("light" | "dark")) => mode.to_string(),
        _ => return Err("Mode must be \"light\" or \"dark\".".into()),
    };
    let palette = parse_palette(object.get("palette"))?;
    let adjustments = match object.get("adjustments") {
        None => None,
        Some(adjustments) => {
            let contrast = adjustments
                .get("contrast")
                .and_then(Value::as_u64)
                .filter(|level| *level <= 100);
            let Some(contrast) = contrast.filter(|_| adjustments.is_object()) else {
                return Err("Theme contrast must be an integer between 0 and 100.".into());
            };
            Some(Adjustments {
                palette: parse_palette(adjustments.get("palette"))?,
                contrast: contrast as u8,
            })
        }
    };
    Ok(CustomTheme {
        name: name.trim().to_string(),
        mode,
        palette,
        adjustments,
    })
}

fn parse_palette(value: Option<&Value>) -> Result<Palette, String> {
    let Some(object) = value.and_then(Value::as_object) else {
        return Err("A palette object is required. Download a theme for all fields.".into());
    };
    let mut out = Map::new();
    for key in REQUIRED {
        let color = object.get(key);
        if key == "strip" && color.is_none() {
            continue;
        }
        if !is_color(color) {
            return Err(format!("palette.{key} must be a #RRGGBB color."));
        }
        out.insert(key.into(), color.cloned().unwrap_or(Value::Null));
    }
    let ansi = object.get("ansi").and_then(Value::as_array);
    if !ansi.is_some_and(|ansi| ansi.len() == 16 && ansi.iter().all(|c| is_color(Some(c)))) {
        return Err("palette.ansi must contain exactly 16 #RRGGBB colors.".into());
    }
    out.insert("ansi".into(), object["ansi"].clone());
    if !out.contains_key("strip") {
        let strip = derive_strip(
            out["rail"].as_str().unwrap_or_default(),
            out["bg"].as_str().unwrap_or_default(),
        );
        out.insert("strip".into(), Value::String(strip));
    }
    for key in OVERRIDES {
        let color = object.get(key);
        if color.is_none() {
            continue;
        }
        if !is_color(color) {
            return Err(format!("palette.{key} must be a #RRGGBB color."));
        }
        out.insert(key.into(), color.cloned().unwrap_or(Value::Null));
    }
    if let Some(finish) = object.get("finish") {
        let ok = finish.is_object()
            && is_color(finish.get("tab"))
            && is_color(finish.get("rule"))
            && finish
                .get("hover")
                .is_none_or(|hover| is_color(Some(hover)));
        if !ok {
            return Err(
                if finish.get("hover").is_some() && !is_color(finish.get("hover")) {
                    "palette.finish.hover must be a #RRGGBB color.".into()
                } else {
                    "palette.finish needs #RRGGBB tab and rule colors.".into()
                },
            );
        }
        out.insert("finish".into(), finish.clone());
    }
    serde_json::from_value(Value::Object(out)).map_err(|error| error.to_string())
}

fn derive_strip(rail: &str, bg: &str) -> String {
    hex(mix(
        parse_hex(rail).unwrap_or(0),
        parse_hex(bg).unwrap_or(0),
        pct(50.0),
    ))
}

fn c(value: &str) -> u32 {
    parse_hex(value).unwrap_or(0)
}

/// The editor's colours for a palette, as `editorPalette` derives them.
#[must_use]
pub fn editor_colors(p: &Palette) -> EditorColors {
    let ground = c(&p.editor);
    let text = c(&p.text);
    let ansi: Vec<u32> = p.ansi.iter().map(|color| c(color)).collect();
    let at = |index: usize| ansi.get(index).copied().unwrap_or(text);
    let scope = |color: u32| hex(lift(color, ground, text, SCOPE_FLOOR));
    EditorColors {
        background: p.editor.clone(),
        foreground: p.text.clone(),
        caret: p.text.clone(),
        selection: hex(mix(ground, c(&p.accent), pct(26.0))),
        active_line: hex(mix(ground, text, pct(5.0))),
        gutter_foreground: hex(lift(c(&p.faint), ground, text, GUTTER_FLOOR)),
        gutter_active_foreground: p.text.clone(),
        matching_bracket: hex(mix(ground, c(&p.accent), pct(30.0))),
        search_match: hex(mix(ground, c(&p.amber), pct(28.0))),
        search_match_selected: hex(mix(ground, c(&p.needs_you), pct(40.0))),
        git_added: p.git_added.clone(),
        git_modified: p.git_modified.clone(),
        git_deleted: p.git_deleted.clone(),
        scopes: Scopes {
            comment: scope(c(&p.faint)),
            keyword: scope(at(9)),
            control_keyword: scope(at(13)),
            string: scope(at(10)),
            number: scope(at(13)),
            ty: scope(at(14)),
            function: scope(at(11)),
            property: scope(at(12)),
            constant: scope(at(13)),
            meta: scope(c(&p.muted)),
        },
    }
}

/// `withThemeColor`: a new accent, ground or foreground, carrying the roles
/// that hang off it so the palette stays one theme.
pub fn with_color(source: &Palette, key: ThemeColor, color: &str) -> Result<Palette, String> {
    let Some(value) = normalize_color(color) else {
        return Err("Use a six-digit hex color, such as #458588.".into());
    };
    let mut result = source.clone();
    match key {
        ThemeColor::Accent => result.accent = value,
        ThemeColor::Bg => {
            let original = c(&source.bg);
            let next = c(&value);
            let shift = |color: &str| {
                let from = c(color);
                let mut changed = 0u32;
                for bits in [16, 8, 0] {
                    let channel = i64::from((from >> bits) & 255) + i64::from((next >> bits) & 255)
                        - i64::from((original >> bits) & 255);
                    changed |= (channel.clamp(0, 255) as u32) << bits;
                }
                hex(changed)
            };
            result.bg = value;
            result.rail = shift(&source.rail);
            result.sidebar = shift(&source.sidebar);
            result.strip = shift(&source.strip);
            result.surface = shift(&source.surface);
            result.surface_hi = shift(&source.surface_hi);
            result.editor = shift(&source.editor);
            result.term_bg = shift(&source.term_bg);
            result.step = shift(&source.step);
            result.border = source.border.as_deref().map(shift);
            result.finish = source.finish.as_ref().map(|finish| Finish {
                tab: shift(&finish.tab),
                rule: shift(&finish.rule),
                hover: finish.hover.as_deref().map(shift),
            });
            if let Some(first) = result.ansi.first_mut() {
                *first = result.term_bg.clone();
            }
        }
        ThemeColor::Text => {
            result.text = value.clone();
            result.term_fg = value.clone();
            if source.fg_default.is_some() && value != source.text.to_ascii_lowercase() {
                result.fg_default = Some(toward_ground(source, &value));
            }
            for index in [7, 15] {
                if source
                    .ansi
                    .get(index)
                    .is_some_and(|color| color.eq_ignore_ascii_case(&source.text))
                {
                    result.ansi[index] = value.clone();
                }
            }
        }
    }
    Ok(result)
}

/// `fgDefault` sits a step below the foreground; a new foreground keeps that step.
fn toward_ground(source: &Palette, text: &str) -> String {
    let from = c(&source.text);
    let ground = c(&source.bg);
    let pinned = c(source.fg_default.as_deref().unwrap_or(&source.text));
    let (mut widest, mut step) = (0i64, 0f64);
    for bits in [16, 8, 0] {
        let span = i64::from((ground >> bits) & 255) - i64::from((from >> bits) & 255);
        if span.abs() > widest.abs() {
            widest = span;
            step = (i64::from((pinned >> bits) & 255) - i64::from((from >> bits) & 255)) as f64
                / span as f64;
        }
    }
    let amount = if widest == 0 {
        0.0
    } else {
        step.clamp(0.0, 1.0)
    };
    hex(mix(c(text), ground, pct(amount * 100.0)))
}

/// `withThemeContrast`: 60 is the palette as drawn; lower melts panels and
/// secondary text into the ground, higher pulls them toward the foreground.
#[must_use]
pub fn with_contrast(source: &Palette, contrast: u8) -> Palette {
    let level = contrast.min(100);
    let mut result = source.clone();
    if level == DEFAULT_CONTRAST {
        return result;
    }
    let (level, base) = (f64::from(level), f64::from(DEFAULT_CONTRAST));
    let amount = if level < base {
        (base - level) / base * 35.0
    } else {
        (level - base) / (100.0 - base) * 20.0
    };
    let toward = c(if level < base {
        &source.bg
    } else {
        &source.text
    });
    let shift = |color: &str| hex(mix(c(color), toward, pct(amount)));
    result.rail = shift(&source.rail);
    result.sidebar = shift(&source.sidebar);
    result.strip = shift(&source.strip);
    result.surface = shift(&source.surface);
    result.surface_hi = shift(&source.surface_hi);
    result.step = shift(&source.step);
    result.muted = shift(&source.muted);
    result.faint = shift(&source.faint);
    result.border = source.border.as_deref().map(shift);
    result.fg_subtle = source.fg_subtle.as_deref().map(shift);
    result
}

/// `themeVariant`: the edited theme, its mode read off the new ground, with
/// the unadjusted palette kept so contrast is recomputed rather than compounded.
#[must_use]
pub fn variant(name: &str, palette: Palette, contrast: u8) -> CustomTheme {
    let level = contrast.min(100);
    CustomTheme {
        name: name.to_string(),
        mode: if luminance(c(&palette.bg)) > 0.5 {
            "light"
        } else {
            "dark"
        }
        .into(),
        palette: with_contrast(&palette, level),
        adjustments: Some(Adjustments {
            palette,
            contrast: level,
        }),
    }
}

impl CustomTheme {
    /// The colours the editor starts from: before contrast, when adjusted.
    #[must_use]
    pub fn origin(&self) -> &Palette {
        self.adjustments
            .as_ref()
            .map_or(&self.palette, |adjustments| &adjustments.palette)
    }

    #[must_use]
    pub fn contrast(&self) -> u8 {
        self.adjustments
            .as_ref()
            .map_or(DEFAULT_CONTRAST, |adjustments| adjustments.contrast)
    }

    /// Compact JSON, the form stored in `ui.theme_base`.
    #[must_use]
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    #[must_use]
    pub fn to_pretty_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base(id: &str) -> Palette {
        super::super::theme(id)
            .expect("built-in base")
            .palette
            .clone()
    }

    #[test]
    fn editor_colors_match_every_built_in_base() {
        for id in super::super::bases() {
            let theme = super::super::theme(id).expect("listed");
            assert_eq!(
                editor_colors(&theme.palette),
                theme.editor,
                "{id}: editorPalette drifted from this port"
            );
        }
    }

    #[test]
    fn variants_match_the_fixture_tauri_emits() {
        for case in super::super::variant_fixture() {
            let source = base(&case.base);
            let palette = match case.color {
                Some((key, ref value)) => with_color(&source, key, value).expect("valid"),
                None => source,
            };
            let got = variant(&case.name, palette, case.contrast);
            assert_eq!(got, case.expected, "{}", case.label);
        }
    }

    #[test]
    fn a_theme_round_trips_and_bad_ones_say_why() {
        let theme = variant("Mine", base("gruvbox"), 70);
        assert_eq!(parse(&theme.to_json()).expect("round trip"), theme);
        assert_eq!(
            parse("{").unwrap_err(),
            "Invalid JSON. Check commas and quotation marks."
        );
        assert_eq!(
            parse(r#"{"name":"x","mode":"dim"}"#).unwrap_err(),
            "Mode must be \"light\" or \"dark\"."
        );
        let mut value = serde_json::to_value(&theme).expect("json");
        value["palette"]["ansi"] = serde_json::json!(["#000000"]);
        assert_eq!(
            parse(&value.to_string()).unwrap_err(),
            "palette.ansi must contain exactly 16 #RRGGBB colors."
        );
        assert!(
            with_color(&base("gruvbox"), ThemeColor::Accent, "teal").is_err(),
            "a word is not a colour"
        );
    }
}
