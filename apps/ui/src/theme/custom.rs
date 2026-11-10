//! A custom theme stored as JSON in `ui.theme_base`. The built-in bases never
//! pass through here: their colours come resolved in `themes.json`, and
//! `editor_colors_match_every_built_in_base` holds this derivation to that
//! file.

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

/// The keys every palette must carry: forge-dark's.
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

/// Readable against one ground: keep the hue while it reads, else step toward
/// `toward`.
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

/// Validates a custom theme; the error is the message to show.
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

/// The editor's colours for a palette.
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

/// A new accent, ground or foreground, carrying the roles
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

/// Contrast 60 is the palette as drawn; lower melts panels and
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

/// The edited theme, its mode read off the new ground, with
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

/// Contrast as a continuous control: a 0–1 track position and the 0–100
/// level `with_contrast` takes.
// Settings › Appearance moves to a slider separately; until then only tests call these.
#[cfg_attr(not(test), allow(dead_code, reason = "only tests call the slider yet"))]
pub mod slider {
    use super::{CustomTheme, DEFAULT_CONTRAST, variant};

    /// The level at a track position; out-of-range or NaN positions clamp.
    #[must_use]
    pub fn level(position: f32) -> u8 {
        if position.is_nan() {
            return DEFAULT_CONTRAST;
        }
        (position.clamp(0.0, 1.0) * 100.0).round() as u8
    }

    #[must_use]
    pub fn position(level: u8) -> f32 {
        f32::from(level.min(100)) / 100.0
    }

    /// `theme` at `level`, always from its unadjusted palette, so a drag
    /// through many levels never compounds.
    #[must_use]
    pub fn at(theme: &CustomTheme, level: u8) -> CustomTheme {
        variant(&theme.name, theme.origin().clone(), level)
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
    fn variants_match_the_recorded_fixture() {
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

    fn fixture() -> serde_json::Value {
        serde_json::json!({ "name": "My theme", "mode": "dark", "palette": base("neutral") })
    }

    fn with(mut value: serde_json::Value, path: &[&str], field: serde_json::Value) -> String {
        let mut at = &mut value;
        for key in &path[..path.len() - 1] {
            at = &mut at[*key];
        }
        at[path[path.len() - 1]] = field;
        value.to_string()
    }

    fn rejects(source: &str, needle: &str) {
        let error = parse(source).expect_err(source);
        assert!(error.contains(needle), "{error:?} should name {needle}");
    }

    #[test]
    fn import_accepts_light_and_trims_names_as_custom_theme_ts() {
        let parsed = parse(&with(
            with(fixture(), &["name"], " Day ".into())
                .parse()
                .expect("json"),
            &["mode"],
            "light".into(),
        ))
        .expect("valid");
        assert_eq!(parsed.name, "Day");
        assert_eq!(parsed.mode, "light");
        assert_eq!(parsed.palette, base("neutral"));
    }

    #[test]
    fn import_rejects_malformed_documents_and_injection() {
        for source in ["{", "null", "[]", "{}"] {
            assert!(parse(source).is_err(), "{source}");
        }
        for bg in [
            serde_json::Value::Null,
            "red".into(),
            "#fff".into(),
            "url(https://example.com)".into(),
            123.into(),
        ] {
            rejects(&with(fixture(), &["palette", "bg"], bg), "palette.bg");
        }
        for ansi in [
            serde_json::json!([]),
            serde_json::json!(vec!["#ffffff"; 17]),
            serde_json::json!(vec!["bad"; 16]),
        ] {
            rejects(&with(fixture(), &["palette", "ansi"], ansi), "palette.ansi");
        }
    }

    #[test]
    fn import_rejects_oversized_input_before_parsing() {
        rejects(&" ".repeat(MAX_THEME_BYTES + 1), "16 KB");
        rejects(&"é".repeat(MAX_THEME_BYTES), "16 KB");
    }

    #[test]
    fn import_rejects_unsupported_modes_empty_names_and_bad_contrast() {
        rejects(&with(fixture(), &["mode"], "system".into()), "Mode");
        rejects(&with(fixture(), &["name"], " ".into()), "name");
        for contrast in [
            serde_json::json!(-1),
            serde_json::json!(101),
            serde_json::json!(60.5),
            serde_json::json!("60"),
            serde_json::Value::Null,
        ] {
            let adjustments =
                serde_json::json!({ "palette": base("neutral"), "contrast": contrast });
            rejects(&with(fixture(), &["adjustments"], adjustments), "contrast");
        }
        let empty = serde_json::json!({ "palette": {}, "contrast": 60 });
        rejects(&with(fixture(), &["adjustments"], empty), "palette.bg");
    }

    #[test]
    fn a_finish_is_carried_derived_or_refused_as_custom_theme_ts() {
        let theme = |palette: serde_json::Value| {
            serde_json::json!({ "name": "Ink", "mode": "dark", "palette": palette }).to_string()
        };
        let calm = base("calm-dark");
        let calm_json = serde_json::to_value(&calm).expect("json");
        assert_eq!(
            parse(&theme(calm_json.clone()))
                .expect("calm")
                .palette
                .finish,
            calm.finish
        );
        let forge = serde_json::to_value(base("forge-dark")).expect("json");
        assert_eq!(parse(&theme(forge)).expect("forge").palette.finish, None);
        let mut no_hover = calm_json.clone();
        no_hover["finish"] = serde_json::json!({ "tab": "#1c1c1f", "rule": "#1f1f22" });
        assert_eq!(
            parse(&theme(no_hover))
                .expect("hover is optional")
                .palette
                .finish,
            Some(Finish {
                tab: "#1c1c1f".into(),
                rule: "#1f1f22".into(),
                hover: None
            })
        );
        for finish in [
            serde_json::json!("#1c1c1f"),
            serde_json::json!({ "tab": "#1c1c1f" }),
            serde_json::json!({ "tab": "url(x)", "rule": "#1f1f22" }),
            serde_json::json!({ "tab": "#1c1c1f", "rule": "#1f1f22", "hover": "red" }),
        ] {
            let mut palette = calm_json.clone();
            palette["finish"] = finish;
            rejects(&theme(palette), "finish");
        }
    }

    #[test]
    fn colors_are_complete_hex_or_nothing() {
        assert_eq!(normalize_color(" #AABBCC ").as_deref(), Some("#aabbcc"));
        for value in ["#abc", "red", "#12345g", "url(test)", "#12345678"] {
            assert_eq!(normalize_color(value), None, "{value}");
        }
    }

    #[test]
    fn a_new_ground_moves_the_surfaces_and_keeps_the_hues() {
        let source = base("gruvbox-hard");
        let edited = with_color(&source, ThemeColor::Bg, "#383838").expect("valid");
        assert_eq!(edited.bg, "#383838");
        assert_eq!(edited.editor, edited.bg);
        assert_eq!(edited.term_bg, edited.bg);
        assert_eq!(edited.surface, "#42403f");
        assert_eq!(edited.ansi[0], edited.bg);
        assert_eq!(edited.ansi[1..], source.ansi[1..]);
        assert_eq!(source, base("gruvbox-hard"), "the base is not mutated");
        for value in ["#ffffff", "#000000"] {
            let surface = with_color(&source, ThemeColor::Bg, value)
                .expect("valid")
                .surface;
            assert!(normalize_color(&surface).is_some(), "{surface}");
        }
    }

    #[test]
    fn a_new_foreground_carries_terminal_text_and_pinned_rows() {
        let source = base("gruvbox-hard");
        let edited = with_color(&source, ThemeColor::Text, "#D5C4A1").expect("valid");
        assert_eq!(edited.text, "#d5c4a1");
        assert_eq!(edited.term_fg, edited.text);
        assert_eq!(edited.ansi[15], edited.text);
        assert_eq!(edited.ansi[7], source.ansi[7]);
        for (id, text) in [("calm-dark", "#f0e6d2"), ("calm-light", "#20183a")] {
            let palette = base(id);
            let edited = with_color(&palette, ThemeColor::Text, text).expect("valid");
            assert_ne!(edited.fg_default, palette.fg_default, "{id}");
            let ratio = |p: &Palette| contrast(c(p.fg_default.as_deref().unwrap_or("")), c(&p.bg));
            assert!((ratio(&edited) - ratio(&palette)).abs() < 2.0, "{id}");
            let same = with_color(&palette, ThemeColor::Text, &palette.text).expect("valid");
            assert_eq!(same.fg_default, palette.fg_default, "{id}");
        }
    }

    #[test]
    fn contrast_scales_secondary_text_and_leaves_chosen_colours() {
        for id in ["gruvbox-hard", "gruvbox-light"] {
            let palette = base(id);
            let variants: Vec<Palette> = [0, DEFAULT_CONTRAST, 100]
                .iter()
                .map(|level| with_contrast(&palette, *level))
                .collect();
            let ratios: Vec<f64> = variants
                .iter()
                .map(|p| contrast(c(&p.muted), c(&p.bg)))
                .collect();
            assert!(
                ratios[0] < ratios[1] && ratios[1] < ratios[2],
                "{id}: {ratios:?}"
            );
            for variant in &variants {
                assert_eq!(
                    (&variant.bg, &variant.text, &variant.accent, &variant.ansi),
                    (&palette.bg, &palette.text, &palette.accent, &palette.ansi),
                    "{id}"
                );
            }
        }
    }

    #[test]
    fn adjustments_round_trip_and_sixty_restores_the_exact_colours() {
        let color =
            with_color(&base("gruvbox-hard"), ThemeColor::Accent, "#83a598").expect("valid");
        let saved = parse(&variant("Gruvbox", color.clone(), 93).to_json()).expect("saved");
        assert_eq!(saved.contrast(), 93);
        let restored = variant(&saved.name, saved.origin().clone(), DEFAULT_CONTRAST);
        assert_eq!(restored.palette, color);
        assert_eq!(restored.name, "Gruvbox");
        assert_eq!(variant("x", base("gruvbox-hard"), 120).contrast(), 100);
        let white = with_color(&base("gruvbox-hard"), ThemeColor::Bg, "#ffffff").expect("valid");
        assert_eq!(variant("x", white, 60).mode, "light");
    }

    #[test]
    fn mix_and_contrast_agree_with_mix_ts() {
        assert_eq!(mix(0x0011_2233, 0x00aa_bbcc, 0), 0x0011_2233);
        assert_eq!(mix(0x0011_2233, 0x00aa_bbcc, 255), 0x00aa_bbcc);
        let grey = mix(0x0020_2020, 0x00f0_f0f0, pct(37.0));
        assert!(grey >> 16 == (grey >> 8) & 0xff && grey & 0xff == (grey >> 8) & 0xff);
        assert_eq!((pct(6.0), pct(7.0), pct(14.0)), (15, 18, 36));
        assert!((contrast(0, 0x00ff_ffff) - 21.0).abs() < 1e-9);
        assert!((contrast(0x00ff_ffff, 0) - 21.0).abs() < 1e-9);
    }

    #[test]
    fn every_base_reads_its_syntax_and_terminal_colours() {
        for id in super::super::bases() {
            let theme = super::super::theme(id).expect("listed");
            let ground = c(&theme.editor.background);
            let scopes = &theme.editor.scopes;
            for (name, color) in [
                ("comment", &scopes.comment),
                ("keyword", &scopes.keyword),
                ("string", &scopes.string),
                ("number", &scopes.number),
                ("type", &scopes.ty),
                ("function", &scopes.function),
                ("property", &scopes.property),
                ("constant", &scopes.constant),
            ] {
                let ratio = contrast(c(color), ground);
                assert!(ratio >= SCOPE_FLOOR - 1e-9, "{id} {name} {ratio:.2}");
            }
            let term = c(&theme.palette.term_bg);
            assert!(
                contrast(c(&theme.palette.term_fg), term) >= 4.5,
                "{id} termFg"
            );
        }
    }
}
