use domain::Tier;
use toml::Value;

pub const CATEGORIES: [&str; 5] = ["logic", "security", "performance", "style", "docs"];
const MAX_FINDINGS_LIMIT: i64 = 200;
const MAX_CONTEXT_BYTES: i64 = 256 * 1024;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Strictness {
    Relajado,
    #[default]
    Normal,
    Estricto,
}

/// On/off per entry of [`CATEGORIES`], in that order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Categories(pub [bool; 5]);

impl Default for Categories {
    fn default() -> Self {
        Self([true, true, true, false, false])
    }
}

impl Categories {
    /// `None` for a category outside [`CATEGORIES`].
    #[must_use]
    pub fn get(&self, name: &str) -> Option<bool> {
        CATEGORIES
            .iter()
            .position(|known| *known == name)
            .map(|at| self.0[at])
    }

    /// `false` for a category outside [`CATEGORIES`], which is left alone.
    pub fn set(&mut self, name: &str, on: bool) -> bool {
        match CATEGORIES.iter().position(|known| *known == name) {
            Some(at) => {
                self.0[at] = on;
                true
            }
            None => false,
        }
    }

    pub fn enabled(&self) -> impl Iterator<Item = &'static str> + '_ {
        CATEGORIES
            .iter()
            .zip(self.0)
            .filter_map(|(name, on)| on.then_some(*name))
    }
}

/// `[context]`: repository-relative files read after `.forge/review/context.md`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContextSettings {
    pub include: Vec<String>,
    /// The budget for every context file together.
    pub max_bytes: usize,
}

impl Default for ContextSettings {
    fn default() -> Self {
        Self {
            include: Vec::new(),
            max_bytes: 64 * 1024,
        }
    }
}

/// `.forge/review/config.toml`; every key it does not know is ignored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    pub language: String,
    /// Checked against the presets by [`crate::resolve`], not here.
    pub default_preset: String,
    pub default_tier: Tier,
    pub strictness: Strictness,
    pub max_findings: usize,
    pub categories: Categories,
    pub context: ContextSettings,
    /// Globs over repository-relative paths, `[ignore] paths`.
    pub ignore: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            language: "es".to_owned(),
            default_preset: crate::presets::DEFAULT_PRESET.to_owned(),
            default_tier: Tier::Base,
            strictness: Strictness::Normal,
            max_findings: 40,
            categories: Categories::default(),
            context: ContextSettings::default(),
            ignore: Vec::new(),
        }
    }
}

impl Config {
    /// Never fails: a bad file or value keeps the default and explains why in a note.
    #[must_use]
    pub fn parse(text: &str) -> (Self, Vec<String>) {
        let mut config = Self::default();
        let mut notes = Vec::new();
        let table = match text.parse::<toml::Table>() {
            Ok(table) => table,
            Err(error) => {
                notes.push(format!(
                    ".forge/review/config.toml is not valid TOML, defaults used: {}",
                    error.message()
                ));
                return (config, notes);
            }
        };
        match table.get("language") {
            None => {}
            Some(Value::String(language)) if !language.trim().is_empty() => {
                config.language = language.trim().to_owned();
            }
            Some(_) => notes.push("review config: `language` must be a non-empty string".into()),
        }
        match table.get("default_preset") {
            None => {}
            Some(Value::String(id)) if !id.trim().is_empty() => {
                config.default_preset = id.trim().to_owned();
            }
            Some(_) => notes.push(format!(
                "review config: `default_preset` must be a non-empty string; {} used",
                crate::presets::DEFAULT_PRESET
            )),
        }
        match table.get("default_tier").map(|v| v.as_str()) {
            None => {}
            Some(Some("base")) => config.default_tier = Tier::Base,
            Some(Some("plus")) => config.default_tier = Tier::Plus,
            Some(Some("apex")) => config.default_tier = Tier::Apex,
            Some(_) => notes
                .push("review config: `default_tier` must be base, plus or apex; base used".into()),
        }
        match table.get("strictness").map(|v| v.as_str()) {
            None => {}
            Some(Some("relajado")) => config.strictness = Strictness::Relajado,
            Some(Some("normal")) => config.strictness = Strictness::Normal,
            Some(Some("estricto")) => config.strictness = Strictness::Estricto,
            Some(_) => notes.push(
                "review config: `strictness` must be relajado, normal or estricto; normal used"
                    .into(),
            ),
        }
        match table.get("max_findings") {
            None => {}
            Some(Value::Integer(n)) => {
                let clamped = (*n).clamp(1, MAX_FINDINGS_LIMIT);
                if clamped != *n {
                    notes.push(format!(
                        "review config: `max_findings` {n} is outside 1..={MAX_FINDINGS_LIMIT}; {clamped} used"
                    ));
                }
                config.max_findings = usize::try_from(clamped).unwrap_or(1);
            }
            Some(_) => {
                notes.push("review config: `max_findings` must be an integer; 40 used".into())
            }
        }
        match table.get("categories") {
            None => {}
            Some(Value::Table(categories)) => {
                for (name, value) in categories {
                    let set = value
                        .as_bool()
                        .is_some_and(|on| config.categories.set(name, on));
                    if !set {
                        notes.push(format!(
                            "review config: ignored `[categories] {name}`, not a known category set to true or false"
                        ));
                    }
                }
            }
            Some(_) => notes.push("review config: `[categories]` must be a table".into()),
        }
        if let Some(context) = table.get("context") {
            parse_context(context, &mut config.context, &mut notes);
        }
        if let Some(paths) = table.get("ignore").and_then(|ignore| ignore.get("paths")) {
            match paths.as_array() {
                Some(paths) => {
                    for path in paths {
                        match path.as_str() {
                            Some(glob) if glob::Pattern::new(glob).is_ok() => {
                                config.ignore.push(glob.to_owned());
                            }
                            _ => notes.push(format!(
                                "review config: ignored an invalid `[ignore] paths` entry {path}"
                            )),
                        }
                    }
                }
                None => notes.push("review config: `[ignore] paths` must be a list".into()),
            }
        }
        (config, notes)
    }
}

fn parse_context(context: &Value, settings: &mut ContextSettings, notes: &mut Vec<String>) {
    match context.get("include").map(Value::as_array) {
        None => {}
        Some(Some(paths)) => {
            for path in paths {
                match path.as_str() {
                    Some(path) if !path.trim().is_empty() => settings.include.push(path.to_owned()),
                    _ => notes.push(format!(
                        "review config: ignored an invalid `[context] include` entry {path}"
                    )),
                }
            }
        }
        Some(None) => notes.push("review config: `[context] include` must be a list".into()),
    }
    match context.get("max_bytes") {
        None => {}
        Some(Value::Integer(n)) => {
            let clamped = (*n).clamp(1, MAX_CONTEXT_BYTES);
            if clamped != *n {
                notes.push(format!(
                    "review config: `[context] max_bytes` {n} is outside 1..={MAX_CONTEXT_BYTES}; {clamped} used"
                ));
            }
            settings.max_bytes = usize::try_from(clamped).unwrap_or(1);
        }
        Some(_) => notes.push("review config: `[context] max_bytes` must be an integer".into()),
    }
}
