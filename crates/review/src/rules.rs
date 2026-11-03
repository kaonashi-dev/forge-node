use domain::Severity;

/// One `.forge/review/rules/*.md` file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rule {
    pub id: String,
    pub title: String,
    /// The most severe level a finding under this rule may carry.
    pub severity: Severity,
    pub category: String,
    pub scope: Vec<String>,
    pub enabled: bool,
    pub body: String,
}

impl Rule {
    /// Whether a repository-relative path falls under the rule's scope.
    #[must_use]
    pub fn matches(&self, path: &str) -> bool {
        self.scope
            .iter()
            .filter_map(|glob| glob::Pattern::new(glob).ok())
            .any(|pattern| pattern.matches(path))
    }
}

/// The rules among `rules` whose scope matches at least one of `paths`.
#[must_use]
pub fn in_scope(rules: &[Rule], paths: &[&str]) -> Vec<Rule> {
    rules
        .iter()
        .filter(|rule| paths.iter().any(|path| rule.matches(path)))
        .cloned()
        .collect()
}

/// `(file name, text)` pairs, sorted by name, to the valid rules and one
/// `"<file name>: <problem>"` error per skipped file. A duplicate id keeps the first file.
#[must_use]
pub fn parse_rules(files: &[(String, String)]) -> (Vec<Rule>, Vec<String>) {
    let mut rules: Vec<Rule> = Vec::new();
    let mut errors = Vec::new();
    for (name, text) in files {
        match parse_rule(text) {
            Ok(rule) if rules.iter().any(|other| other.id == rule.id) => {
                errors.push(format!("{name}: duplicate rule id `{}`", rule.id));
            }
            Ok(rule) => rules.push(rule),
            Err(problem) => errors.push(format!("{name}: {problem}")),
        }
    }
    (rules, errors)
}

/// The documented frontmatter subset: `key: value` lines between two `---`
/// lines, a value being a JSON string array, a boolean or a (quoted) string.
fn parse_rule(text: &str) -> Result<Rule, String> {
    let mut lines = text.lines();
    if lines.next() != Some("---") {
        return Err("no frontmatter: the first line must be `---`".into());
    }
    let mut fields: Vec<(&str, &str)> = Vec::new();
    let mut closed = false;
    for line in lines.by_ref() {
        if line == "---" {
            closed = true;
            break;
        }
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (key, value) = line
            .split_once(':')
            .ok_or_else(|| format!("frontmatter line `{line}` is not `key: value`"))?;
        fields.push((key.trim(), value.trim()));
    }
    if !closed {
        return Err("the frontmatter is not closed by a `---` line".into());
    }
    let get = |key: &str| {
        fields
            .iter()
            .rev()
            .find(|(k, _)| *k == key)
            .map(|(_, value)| *value)
    };
    let string = |key: &str| -> Result<String, String> {
        let value = unquote(get(key).unwrap_or(""));
        if value.is_empty() {
            Err(format!("`{key}` is missing or empty"))
        } else {
            Ok(value.to_owned())
        }
    };

    let id = string("id")?;
    let kebab = id.split('-').all(|part| {
        !part.is_empty()
            && part
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
    });
    if !kebab {
        return Err(format!("`id` `{id}` is not kebab-case"));
    }
    let title = string("title")?;
    let severity = match string("severity")?.as_str() {
        "P0" => Severity::P0,
        "P1" => Severity::P1,
        "P2" => Severity::P2,
        other => return Err(format!("`severity` `{other}` must be P0, P1 or P2")),
    };
    let category = string("category")?;
    let scope = match get("scope") {
        Some(value) if value.starts_with('[') => serde_json::from_str::<Vec<String>>(value)
            .map_err(|_| "`scope` must be a JSON list of strings".to_owned())?,
        _ => return Err("`scope` must be a list of globs".into()),
    };
    if scope.is_empty() {
        return Err("`scope` is empty".into());
    }
    for glob in &scope {
        if glob.is_empty() || glob::Pattern::new(glob).is_err() {
            return Err(format!("`scope` entry `{glob}` is not a valid glob"));
        }
    }
    let enabled = match get("enabled") {
        None | Some("true") => true,
        Some("false") => false,
        Some(other) => return Err(format!("`enabled` `{other}` must be true or false")),
    };
    Ok(Rule {
        id,
        title,
        severity,
        category,
        scope,
        enabled,
        body: lines.collect::<Vec<_>>().join("\n").trim().to_owned(),
    })
}

fn unquote(value: &str) -> &str {
    for quote in ['"', '\''] {
        if let Some(inner) = value
            .strip_prefix(quote)
            .and_then(|rest| rest.strip_suffix(quote))
        {
            return inner;
        }
    }
    value
}
