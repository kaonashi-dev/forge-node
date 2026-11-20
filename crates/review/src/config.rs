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
    /// `AGENTS.md` and `CLAUDE.md` at the checkout root, before `include`.
    pub project_docs: bool,
    pub include: Vec<String>,
    /// The budget for every context file together.
    pub max_bytes: usize,
}

impl Default for ContextSettings {
    fn default() -> Self {
        Self {
            project_docs: true,
            include: Vec::new(),
            max_bytes: 64 * 1024,
        }
    }
}

/// `[graph]`: the codebase graph and the other projects it may read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GraphSettings {
    pub enabled: bool,
    pub cross_repo: bool,
    /// Registered Forge projects, by name or by a path relative to the workspace.
    pub repos: Vec<String>,
}

impl Default for GraphSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            cross_repo: false,
            repos: Vec::new(),
        }
    }
}

/// How severe a finding an auto-approvable run may hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RiskCeiling {
    /// No findings at all: there is no severity below P2.
    Low,
    Medium,
    High,
    /// Anything but P0.
    Critical,
}

/// `[review.auto_approve]`: when a run would pass without a person. Forge
/// computes the answer and never acts on it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AutoApprove {
    pub enabled: bool,
    pub risk_ceiling: RiskCeiling,
    pub min_score: f32,
    pub require_clean_diff: bool,
}

impl Default for AutoApprove {
    fn default() -> Self {
        Self {
            enabled: false,
            risk_ceiling: RiskCeiling::Low,
            min_score: 5.0,
            require_clean_diff: true,
        }
    }
}

/// `[security]`: what a TREX target may reach.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SecuritySettings {
    pub sandbox_network: bool,
    /// Variables copied from the daemon's environment into a target's.
    pub sandbox_env_whitelist: Vec<String>,
}

impl Default for SecuritySettings {
    fn default() -> Self {
        Self {
            sandbox_network: false,
            sandbox_env_whitelist: vec!["PATH".to_owned(), "HOME".to_owned()],
        }
    }
}

/// `.forge/review/config.toml`; every key it does not know is ignored.
#[derive(Clone, Debug, PartialEq)]
pub struct Config {
    pub language: String,
    /// Checked against the presets by [`crate::resolve`], not here.
    pub default_preset: String,
    /// `None` is `auto`: the tier is chosen from the diff, see [`crate::auto_tier`].
    pub default_tier: Option<Tier>,
    pub strictness: Strictness,
    pub max_findings: usize,
    /// A finding the model rates below this is dropped; `0.0..=1.0`.
    pub min_confidence: f32,
    pub categories: Categories,
    pub context: ContextSettings,
    /// Globs over repository-relative paths, `[ignore] paths`.
    pub ignore: Vec<String>,
    pub graph: GraphSettings,
    /// `[kb] enabled`: the knowledge base in `plus` and `apex` packs.
    pub kb_enabled: bool,
    pub auto_approve: AutoApprove,
    pub security: SecuritySettings,
    /// Instructions under the preset's, lowest first. Filled by the caller,
    /// never from `config.toml`.
    pub base_instructions: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            language: "en".to_owned(),
            default_preset: crate::presets::DEFAULT_PRESET.to_owned(),
            default_tier: None,
            strictness: Strictness::Normal,
            max_findings: 40,
            min_confidence: 0.5,
            categories: Categories::default(),
            context: ContextSettings::default(),
            ignore: Vec::new(),
            graph: GraphSettings::default(),
            kb_enabled: true,
            auto_approve: AutoApprove::default(),
            security: SecuritySettings::default(),
            base_instructions: Vec::new(),
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
            Some(Some("auto")) => config.default_tier = None,
            Some(Some("base")) => config.default_tier = Some(Tier::Base),
            Some(Some("plus")) => config.default_tier = Some(Tier::Plus),
            Some(Some("apex")) => config.default_tier = Some(Tier::Apex),
            Some(_) => notes.push(
                "review config: `default_tier` must be auto, base, plus or apex; auto used".into(),
            ),
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
        let min_confidence = match table.get("min_confidence") {
            Some(Value::Float(n)) if !n.is_nan() => Some(*n),
            Some(Value::Integer(n)) => Some(*n as f64),
            _ => None,
        };
        match (table.get("min_confidence"), min_confidence) {
            (None, _) => {}
            (Some(_), Some(n)) => {
                let clamped = n.clamp(0.0, 1.0);
                if clamped != n {
                    notes.push(format!(
                        "review config: `min_confidence` {n} is outside 0..=1; {clamped} used"
                    ));
                }
                config.min_confidence = clamped as f32;
            }
            (Some(_), None) => {
                notes.push("review config: `min_confidence` must be a number; 0.5 used".into())
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
        match table.get("graph") {
            None => {}
            Some(Value::Table(graph)) => parse_graph(graph, &mut config.graph, &mut notes),
            Some(_) => notes.push("review config: `[graph]` must be a table".into()),
        }
        match table
            .get("review")
            .and_then(|review| review.get("auto_approve"))
        {
            None => {}
            Some(Value::Table(auto)) => {
                parse_auto_approve(auto, &mut config.auto_approve, &mut notes)
            }
            Some(_) => notes.push("review config: `[review.auto_approve]` must be a table".into()),
        }
        match table.get("kb") {
            None => {}
            Some(Value::Table(kb)) => {
                parse_bool(kb, "kb", "enabled", &mut config.kb_enabled, &mut notes);
            }
            Some(_) => notes.push("review config: `[kb]` must be a table".into()),
        }
        match table.get("security") {
            None => {}
            Some(Value::Table(security)) => {
                parse_security(security, &mut config.security, &mut notes);
            }
            Some(_) => notes.push("review config: `[security]` must be a table".into()),
        }
        (config, notes)
    }
}

fn parse_security(table: &toml::Table, settings: &mut SecuritySettings, notes: &mut Vec<String>) {
    parse_bool(
        table,
        "security",
        "sandbox_network",
        &mut settings.sandbox_network,
        notes,
    );
    match table.get("sandbox_env_whitelist").map(Value::as_array) {
        None => {}
        Some(Some(names)) => {
            let valid: Option<Vec<String>> = names
                .iter()
                .map(|name| {
                    name.as_str()
                        .filter(|name| !name.is_empty() && !name.starts_with("FORGE_"))
                        .map(str::to_owned)
                })
                .collect();
            match valid {
                Some(names) => settings.sandbox_env_whitelist = names,
                None => notes.push(
                    "review config: `[security] sandbox_env_whitelist` must list variable names, none of them FORGE_*; PATH and HOME used".into(),
                ),
            }
        }
        Some(None) => {
            notes.push("review config: `[security] sandbox_env_whitelist` must be a list".into())
        }
    }
}

fn parse_auto_approve(table: &toml::Table, settings: &mut AutoApprove, notes: &mut Vec<String>) {
    let section = "review.auto_approve";
    parse_bool(table, section, "enabled", &mut settings.enabled, notes);
    parse_bool(
        table,
        section,
        "require_clean_diff",
        &mut settings.require_clean_diff,
        notes,
    );
    match table.get("risk_ceiling").map(Value::as_str) {
        None => {}
        Some(Some("low")) => settings.risk_ceiling = RiskCeiling::Low,
        Some(Some("medium")) => settings.risk_ceiling = RiskCeiling::Medium,
        Some(Some("high")) => settings.risk_ceiling = RiskCeiling::High,
        Some(Some("critical")) => settings.risk_ceiling = RiskCeiling::Critical,
        Some(_) => notes.push(format!(
            "review config: `[{section}] risk_ceiling` must be low, medium, high or critical; low used"
        )),
    }
    let score = match table.get("min_score") {
        None => return,
        Some(Value::Integer(n)) => Some(*n as f64),
        Some(Value::Float(n)) if !n.is_nan() => Some(*n),
        Some(_) => None,
    };
    match score {
        Some(score) if (0.0..=5.0).contains(&score) => settings.min_score = score as f32,
        _ => notes.push(format!(
            "review config: `[{section}] min_score` must be a number in 0..=5; 5 used"
        )),
    }
}

fn parse_bool(
    table: &toml::Table,
    section: &str,
    key: &str,
    out: &mut bool,
    notes: &mut Vec<String>,
) {
    match table.get(key) {
        None => {}
        Some(Value::Boolean(on)) => *out = *on,
        Some(_) => notes.push(format!(
            "review config: `[{section}] {key}` must be true or false; {out} used"
        )),
    }
}

fn parse_graph(graph: &toml::Table, settings: &mut GraphSettings, notes: &mut Vec<String>) {
    parse_bool(graph, "graph", "enabled", &mut settings.enabled, notes);
    parse_bool(
        graph,
        "graph",
        "cross_repo",
        &mut settings.cross_repo,
        notes,
    );
    match graph.get("repos").map(Value::as_array) {
        None => {}
        Some(Some(repos)) => {
            for repo in repos {
                match repo.as_str().map(str::trim) {
                    Some(repo) if !repo.is_empty() => settings.repos.push(repo.to_owned()),
                    _ => notes.push(format!(
                        "review config: ignored an invalid `[graph] repos` entry {repo}"
                    )),
                }
            }
        }
        Some(None) => notes.push("review config: `[graph] repos` must be a list".into()),
    }
}

fn parse_context(context: &Value, settings: &mut ContextSettings, notes: &mut Vec<String>) {
    match context.get("project_docs") {
        None => {}
        Some(Value::Boolean(on)) => settings.project_docs = *on,
        Some(_) => {
            notes.push("review config: `[context] project_docs` must be true or false".into())
        }
    }
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
