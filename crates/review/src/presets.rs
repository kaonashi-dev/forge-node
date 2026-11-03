use domain::Tier;
use toml::Value;

use crate::config::{Categories, Config, ContextSettings, Strictness, CATEGORIES};
use crate::rules::Rule;

pub const DEFAULT_PRESET: &str = "estandar";
pub const MAX_INSTRUCTIONS_BYTES: usize = 4096;

/// A built-in or a `.forge/review/presets/<id>.toml`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Preset {
    pub id: String,
    pub name: String,
    pub tier: Option<Tier>,
    /// Rule ids, or `["*"]` for every enabled rule.
    pub rules: Vec<String>,
    /// A partial override of the config's categories.
    pub categories: Vec<(String, bool)>,
    pub instructions: String,
}

/// What one run is told and checked against, every layer already applied.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resolved {
    pub preset: String,
    pub tier: Tier,
    pub categories: Categories,
    /// Enabled and chosen by the preset; the diff's scope is applied later.
    pub rules: Vec<Rule>,
    pub instructions: String,
    pub language: String,
    pub strictness: Strictness,
    pub max_findings: usize,
    pub ignore: Vec<String>,
    pub context: ContextSettings,
    pub notes: Vec<String>,
}

fn preset(
    id: &str,
    name: &str,
    tier: Option<Tier>,
    categories: &[(&str, bool)],
    instructions: &str,
) -> Preset {
    Preset {
        id: id.to_owned(),
        name: name.to_owned(),
        tier,
        rules: vec!["*".to_owned()],
        categories: categories
            .iter()
            .map(|(name, on)| ((*name).to_owned(), *on))
            .collect(),
        instructions: instructions.to_owned(),
    }
}

#[must_use]
pub fn builtins() -> Vec<Preset> {
    vec![
        preset(DEFAULT_PRESET, "Estándar", None, &[], ""),
        preset(
            "seguridad",
            "Seguridad",
            None,
            &[
                ("security", true),
                ("logic", false),
                ("performance", false),
                ("style", false),
                ("docs", false),
            ],
            "Focus on security: injection, authentication and authorization, secrets, unsafe \
             input handling and data exposure.",
        ),
        preset(
            "negocio",
            "Reglas de negocio",
            Some(Tier::Plus),
            &[("performance", false), ("style", false), ("docs", false)],
            "Report only violations of the project's rules in <rules> and of the invariants \
             in <project_context>. Ignore style.",
        ),
        preset(
            "profundo",
            "Profundo",
            Some(Tier::Apex),
            &CATEGORIES.map(|name| (name, true)),
            "",
        ),
    ]
}

/// The built-ins followed by `(file name, text)` presets, sorted by name; a
/// repository preset replaces the built-in of the same id in its place.
/// Errors name the file; an invalid file is skipped, an unknown rule id ignored.
#[must_use]
pub fn parse_presets(files: &[(String, String)], rules: &[Rule]) -> (Vec<Preset>, Vec<String>) {
    let mut presets = builtins();
    let mut errors = Vec::new();
    for (name, text) in files {
        let id = name.strip_suffix(".toml").unwrap_or(name);
        match parse_preset(id, text) {
            Ok(mut preset) => {
                preset.rules.retain(|rule| {
                    let known = rule == "*" || rules.iter().any(|known| known.id == *rule);
                    if !known {
                        errors.push(format!("{name}: unknown rule id `{rule}`"));
                    }
                    known
                });
                match presets.iter_mut().find(|other| other.id == preset.id) {
                    Some(builtin) => *builtin = preset,
                    None => presets.push(preset),
                }
            }
            Err(problem) => errors.push(format!("{name}: {problem}")),
        }
    }
    (presets, errors)
}

fn parse_preset(id: &str, text: &str) -> Result<Preset, String> {
    let table = text
        .parse::<toml::Table>()
        .map_err(|error| format!("not valid TOML: {}", error.message()))?;
    let name = match table.get("name") {
        None => id.to_owned(),
        Some(Value::String(name)) if !name.trim().is_empty() => name.trim().to_owned(),
        Some(_) => return Err("`name` must be a non-empty string".into()),
    };
    let tier = match table.get("tier").map(|v| v.as_str()) {
        None => None,
        Some(Some("base")) => Some(Tier::Base),
        Some(Some("plus")) => Some(Tier::Plus),
        Some(Some("apex")) => Some(Tier::Apex),
        Some(_) => return Err("`tier` must be base, plus or apex".into()),
    };
    let rules = match table.get("rules") {
        None => vec!["*".to_owned()],
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| item.as_str().map(str::to_owned))
            .collect::<Option<Vec<_>>>()
            .ok_or("`rules` must be a list of rule ids")?,
        Some(_) => return Err("`rules` must be a list of rule ids".into()),
    };
    let mut categories = Vec::new();
    match table.get("categories") {
        None => {}
        Some(Value::Table(table)) => {
            for (category, value) in table {
                match value.as_bool() {
                    Some(on) if CATEGORIES.contains(&category.as_str()) => {
                        categories.push((category.clone(), on));
                    }
                    _ => {
                        return Err(format!(
                            "`categories.{category}` must be a known category set to true or false"
                        ))
                    }
                }
            }
        }
        Some(_) => return Err("`categories` must be a table".into()),
    }
    let instructions = match table.get("instructions") {
        None => String::new(),
        Some(Value::String(text)) if text.len() <= MAX_INSTRUCTIONS_BYTES => text.trim().to_owned(),
        Some(Value::String(_)) => {
            return Err(format!(
                "`instructions` is over {MAX_INSTRUCTIONS_BYTES} bytes"
            ))
        }
        Some(_) => return Err("`instructions` must be a string".into()),
    };
    Ok(Preset {
        id: id.to_owned(),
        name,
        tier,
        rules,
        categories,
        instructions,
    })
}

/// The config's `default_preset` when it names one of `presets`, else
/// [`DEFAULT_PRESET`] and a note saying so.
#[must_use]
pub fn default_preset(config: &Config, presets: &[Preset]) -> (String, Option<String>) {
    if presets
        .iter()
        .any(|preset| preset.id == config.default_preset)
    {
        (config.default_preset.clone(), None)
    } else {
        (
            DEFAULT_PRESET.to_owned(),
            Some(format!(
                "review config: `default_preset` `{}` is not a known preset; {DEFAULT_PRESET} used",
                config.default_preset
            )),
        )
    }
}

/// Apply, from lowest to highest: the config, the preset, then the run's own
/// tier and instructions.
///
/// # Errors
/// An explicit preset that is not in `presets`, or instructions over
/// [`MAX_INSTRUCTIONS_BYTES`].
pub fn resolve(
    config: &Config,
    rules: &[Rule],
    presets: &[Preset],
    preset: Option<&str>,
    tier: Option<Tier>,
    instructions: Option<&str>,
) -> Result<Resolved, String> {
    let instructions = instructions.unwrap_or("").trim();
    if instructions.len() > MAX_INSTRUCTIONS_BYTES {
        return Err(format!(
            "instructions are over {MAX_INSTRUCTIONS_BYTES} bytes"
        ));
    }
    let mut notes = Vec::new();
    let id = match preset {
        Some(id) => id.to_owned(),
        None => {
            let (id, note) = default_preset(config, presets);
            notes.extend(note);
            id
        }
    };
    let chosen = presets.iter().find(|p| p.id == id).ok_or_else(|| {
        let known: Vec<&str> = presets.iter().map(|p| p.id.as_str()).collect();
        format!("unknown preset `{id}`; known presets: {}", known.join(", "))
    })?;
    let mut categories = config.categories;
    for (category, on) in &chosen.categories {
        categories.set(category, *on);
    }
    let every = chosen.rules.iter().any(|rule| rule == "*");
    let rules = rules
        .iter()
        .filter(|rule| rule.enabled && (every || chosen.rules.contains(&rule.id)))
        .cloned()
        .collect();
    let instructions = [chosen.instructions.as_str(), instructions]
        .into_iter()
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    Ok(Resolved {
        preset: id,
        tier: tier.or(chosen.tier).unwrap_or(config.default_tier),
        categories,
        rules,
        instructions,
        language: config.language.clone(),
        strictness: config.strictness,
        max_findings: config.max_findings,
        ignore: config.ignore.clone(),
        context: config.context.clone(),
        notes,
    })
}
