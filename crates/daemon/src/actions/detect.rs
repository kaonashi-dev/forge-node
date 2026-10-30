//! Suggest actions from the manifests at a workspace's root.
//!
//! [`read_root`] does the bounded IO and [`suggest`] is a pure function of what
//! it read, so each ecosystem is a unit test over strings. A suggestion is only
//! ever offered: nothing here is saved or run.

use std::collections::HashSet;
use std::io::Read;
use std::path::Path;

use domain::{
    ActionKind, ActionReady, ActionSource, ActionSuggestion, ProjectAction,
    MAX_ACTION_COMMAND_BYTES, MAX_ACTION_LABEL_BYTES,
};

/// Larger manifests are skipped and reported as `truncated`.
pub const MAX_MANIFEST_BYTES: u64 = 256 * 1024;
/// Per source, so one huge `scripts` table cannot crowd out the others.
const MAX_PER_SOURCE: usize = 24;
const DEV_READY_PATTERN: &str = "localhost:";
const DEV_READY_TIMEOUT_SECS: u64 = 60;

const COMPOSE_FILES: [&str; 4] = [
    "compose.yaml",
    "compose.yml",
    "docker-compose.yml",
    "docker-compose.yaml",
];
/// Root entries whose presence, not content, decides something.
const MARKERS: [&str; 9] = [
    "bun.lock",
    "bun.lockb",
    "pnpm-lock.yaml",
    "yarn.lock",
    "package-lock.json",
    "uv.lock",
    "src/main.rs",
    "scripts/dev",
    ".forge/actions.toml",
];

/// What [`read_root`] found. `None` is absent, unreadable or over the cap.
#[derive(Debug, Default)]
pub struct RootFiles {
    pub package_json: Option<String>,
    pub cargo_toml: Option<String>,
    pub makefile: Option<String>,
    pub justfile: Option<String>,
    pub pyproject: Option<String>,
    /// The first compose file name present.
    pub compose: Option<&'static str>,
    pub present: HashSet<&'static str>,
    /// `scripts/dev` exists and is executable.
    pub dev_script: bool,
}

/// Read the root manifests, `metadata()` before `open` and `.take(cap)` after.
/// Returns whether a file was skipped for its size.
#[must_use]
pub fn read_root(root: &Path) -> (RootFiles, bool) {
    let mut truncated = false;
    let mut read = |name: &str| match read_capped(&root.join(name), MAX_MANIFEST_BYTES) {
        Capped::Text(text) => Some(text),
        Capped::TooLarge => {
            truncated = true;
            None
        }
        Capped::Absent => None,
    };
    let mut files = RootFiles {
        package_json: read("package.json"),
        cargo_toml: read("Cargo.toml"),
        makefile: read("Makefile").or_else(|| read("makefile")),
        justfile: read("justfile").or_else(|| read("Justfile")),
        pyproject: read("pyproject.toml"),
        ..RootFiles::default()
    };
    files.compose = COMPOSE_FILES
        .into_iter()
        .find(|name| is_regular_file(&root.join(name)));
    for marker in MARKERS {
        if is_regular_file(&root.join(marker)) {
            files.present.insert(marker);
        }
    }
    files.dev_script =
        files.present.contains("scripts/dev") && is_executable(&root.join("scripts/dev"));
    (files, truncated)
}

pub(super) enum Capped {
    Text(String),
    TooLarge,
    Absent,
}

/// A symlink is not followed: a manifest pointing outside the checkout is not
/// the checkout's to suggest from.
pub(super) fn read_capped(path: &Path, cap: u64) -> Capped {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return Capped::Absent;
    };
    if !meta.is_file() {
        return Capped::Absent;
    }
    if meta.len() > cap {
        return Capped::TooLarge;
    }
    let Ok(file) = std::fs::File::open(path) else {
        return Capped::Absent;
    };
    // One byte past the cap: a file that grew after `metadata()` is refused, not truncated.
    let mut bytes = Vec::new();
    if file.take(cap + 1).read_to_end(&mut bytes).is_err() {
        return Capped::Absent;
    }
    if bytes.len() as u64 > cap {
        return Capped::TooLarge;
    }
    String::from_utf8(bytes).map_or(Capped::Absent, Capped::Text)
}

fn is_regular_file(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|meta| meta.is_file())
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|meta| meta.permissions().mode() & 0o111 != 0)
}

/// Every suggestion the files support, deduplicated by label ignoring case
/// (the first source wins), Dev then Test, Lint, Build and the rest.
#[must_use]
pub fn suggest(files: &RootFiles) -> Vec<ActionSuggestion> {
    let mut out = Vec::new();
    if let Some(text) = &files.package_json {
        out.extend(from_package_json(text, &files.present));
    }
    if let Some(text) = &files.cargo_toml {
        out.extend(from_cargo(text, files));
    }
    if let Some(text) = &files.makefile {
        out.extend(from_targets(text, "make", "Makefile", make_target));
    }
    if let Some(text) = &files.justfile {
        out.extend(from_targets(text, "just", "justfile", just_recipe));
    }
    if let Some(name) = files.compose {
        out.push(suggestion(
            "compose up",
            "docker compose up".to_owned(),
            ActionKind::Dev,
            name,
        ));
    }
    if let Some(text) = &files.pyproject {
        out.extend(from_pyproject(text, &files.present));
    }

    let mut seen = HashSet::new();
    out.retain(|s| {
        s.label.len() <= MAX_ACTION_LABEL_BYTES
            && s.command.len() <= MAX_ACTION_COMMAND_BYTES
            && seen.insert(s.label.to_lowercase())
    });
    out.sort_by_key(|s| kind_rank(s.kind));
    out
}

/// Point each suggestion at the saved action with its label, ignoring case.
pub fn mark_saved(suggestions: &mut [ActionSuggestion], saved: &[ProjectAction]) {
    for suggestion in suggestions {
        suggestion.saved_as = saved
            .iter()
            .find(|action| action.label.eq_ignore_ascii_case(&suggestion.label))
            .map(|action| action.id);
    }
}

fn kind_rank(kind: ActionKind) -> u8 {
    match kind {
        ActionKind::Dev => 0,
        ActionKind::Test => 1,
        ActionKind::Lint => 2,
        ActionKind::Build => 3,
        _ => 4,
    }
}

fn suggestion(label: &str, command: String, kind: ActionKind, from: &str) -> ActionSuggestion {
    ActionSuggestion {
        label: label.to_owned(),
        command,
        kind,
        pinned: false,
        cwd: None,
        single_instance: true,
        ready: ActionReady::None,
        port: None,
        depends_on: Vec::new(),
        source: ActionSource::Detected,
        detected_from: from.to_owned(),
        saved_as: None,
    }
}

/// Kind from a script, target or recipe name.
fn kind_of(name: &str) -> ActionKind {
    let lower = name.to_ascii_lowercase();
    let head = lower.split([':', '-', '_', '.']).next().unwrap_or(&lower);
    match head {
        "dev" | "start" | "serve" | "preview" => ActionKind::Dev,
        "test" | "tests" | "e2e" | "spec" => ActionKind::Test,
        "lint" | "format" | "fmt" | "clippy" => ActionKind::Lint,
        "build" | "typecheck" | "check" | "compile" | "tsc" => ActionKind::Build,
        _ => ActionKind::Custom,
    }
}

/// npm runs these on its own (`preX`/`postX` only around an existing `X`), so
/// they are not a thing to click.
fn is_lifecycle(name: &str, scripts: &serde_json::Map<String, serde_json::Value>) -> bool {
    let wraps = |prefix: &str| {
        name.strip_prefix(prefix)
            .is_some_and(|rest| scripts.contains_key(rest))
    };
    matches!(
        name,
        "install"
            | "postinstall"
            | "preinstall"
            | "prepare"
            | "prepublishOnly"
            | "prepack"
            | "postpack"
    ) || wraps("pre")
        || wraps("post")
}

/// The script runner: `packageManager` wins, then the lockfile, then npm.
fn runner(manifest: &serde_json::Value, present: &HashSet<&'static str>) -> &'static str {
    let declared = manifest
        .get("packageManager")
        .and_then(serde_json::Value::as_str)
        .and_then(|value| value.split('@').next());
    match declared {
        Some("bun") => return "bun run",
        Some("pnpm") => return "pnpm run",
        Some("yarn") => return "yarn run",
        Some("npm") => return "npm run",
        _ => {}
    }
    if present.contains("bun.lock") || present.contains("bun.lockb") {
        "bun run"
    } else if present.contains("pnpm-lock.yaml") {
        "pnpm run"
    } else if present.contains("yarn.lock") {
        "yarn run"
    } else {
        "npm run"
    }
}

fn from_package_json(text: &str, present: &HashSet<&'static str>) -> Vec<ActionSuggestion> {
    let Ok(manifest) = serde_json::from_str::<serde_json::Value>(text) else {
        return Vec::new();
    };
    let Some(scripts) = manifest
        .get("scripts")
        .and_then(serde_json::Value::as_object)
    else {
        return Vec::new();
    };
    let runner = runner(&manifest, present);
    let mut out: Vec<ActionSuggestion> = scripts
        .iter()
        .filter(|(name, body)| body.is_string() && !is_lifecycle(name, scripts))
        .take(MAX_PER_SOURCE)
        .map(|(name, _)| {
            suggestion(
                name,
                format!("{runner} {}", shell_word(name)),
                kind_of(name),
                "package.json",
            )
        })
        .collect();
    // One pinned server per project: `dev` if there is one, else `start`.
    for preferred in ["dev", "start", "serve"] {
        if let Some(dev) = out.iter_mut().find(|s| s.label == preferred) {
            dev.pinned = true;
            break;
        }
    }
    for s in out.iter_mut().filter(|s| s.kind == ActionKind::Dev) {
        s.ready = ActionReady::Output {
            pattern: DEV_READY_PATTERN.to_owned(),
            timeout_secs: DEV_READY_TIMEOUT_SECS,
        };
    }
    out
}

fn from_cargo(text: &str, files: &RootFiles) -> Vec<ActionSuggestion> {
    let Ok(manifest) = text.parse::<toml::Table>() else {
        return Vec::new();
    };
    let is_package = manifest.contains_key("package");
    if !is_package && !manifest.contains_key("workspace") {
        return Vec::new();
    }
    let mut out = vec![
        suggestion(
            "cargo test",
            "cargo test".to_owned(),
            ActionKind::Test,
            "Cargo.toml",
        ),
        suggestion(
            "cargo clippy",
            "cargo clippy --all-targets".to_owned(),
            ActionKind::Lint,
            "Cargo.toml",
        ),
        suggestion(
            "cargo check",
            "cargo check".to_owned(),
            ActionKind::Build,
            "Cargo.toml",
        ),
    ];
    let has_bin = manifest.contains_key("bin") || files.present.contains("src/main.rs");
    if is_package && has_bin {
        out.push(suggestion(
            "cargo run",
            "cargo run".to_owned(),
            ActionKind::Dev,
            "Cargo.toml",
        ));
    }
    if files.dev_script {
        out.push(suggestion(
            "scripts/dev",
            "scripts/dev".to_owned(),
            ActionKind::Custom,
            "scripts/dev",
        ));
    }
    out
}

fn from_targets(
    text: &str,
    tool: &str,
    file: &str,
    parse: fn(&str) -> Option<&str>,
) -> Vec<ActionSuggestion> {
    let mut seen = HashSet::new();
    text.lines()
        .filter_map(parse)
        .filter(|name| seen.insert(*name))
        .take(MAX_PER_SOURCE)
        .map(|name| {
            suggestion(
                &format!("{tool} {name}"),
                format!("{tool} {name}"),
                kind_of(name),
                file,
            )
        })
        .collect()
}

/// A plain `target:` line. Not a variable (`:=`), a special target (`.PHONY`),
/// a pattern rule (`%`), or a recipe line.
fn make_target(line: &str) -> Option<&str> {
    if line.starts_with(['\t', ' ', '#', '.']) {
        return None;
    }
    let (name, rest) = line.split_once(':')?;
    if rest.starts_with([':', '=']) || name.contains(['=', '%', '$', ' ', '\t']) {
        return None;
    }
    is_plain_name(name).then_some(name)
}

/// A `recipe:` line taking no parameters; one that needs arguments would fail
/// when clicked.
fn just_recipe(line: &str) -> Option<&str> {
    if line.starts_with(['\t', ' ', '#', '[']) {
        return None;
    }
    let (head, rest) = line.split_once(':')?;
    if rest.starts_with('=') {
        return None;
    }
    let name = head.trim().trim_start_matches('@');
    if name.split_whitespace().nth(1).is_some() {
        return None;
    }
    if matches!(name, "set" | "alias" | "import" | "export" | "mod") {
        return None;
    }
    is_plain_name(name).then_some(name)
}

fn is_plain_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/'))
}

fn from_pyproject(text: &str, present: &HashSet<&'static str>) -> Vec<ActionSuggestion> {
    let Ok(manifest) = text.parse::<toml::Table>() else {
        return Vec::new();
    };
    let tool = manifest.get("tool").and_then(toml::Value::as_table);
    let mentions = |name: &str| {
        tool.is_some_and(|tool| tool.contains_key(name)) || text.contains(&format!("\"{name}"))
    };
    let prefix = if present.contains("uv.lock") {
        "uv run "
    } else {
        "python -m "
    };
    let mut out = Vec::new();
    if mentions("pytest") {
        out.push(suggestion(
            "pytest",
            format!("{prefix}pytest"),
            ActionKind::Test,
            "pyproject.toml",
        ));
    }
    if mentions("ruff") {
        out.push(suggestion(
            "ruff",
            format!("{prefix}ruff check"),
            ActionKind::Lint,
            "pyproject.toml",
        ));
    }
    out
}

/// A script name as one shell word. Names are usually plain; one with a quote
/// or a space is single-quoted rather than trusted.
fn shell_word(name: &str) -> String {
    if name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || ":-_./".contains(c))
    {
        name.to_owned()
    } else {
        format!("'{}'", name.replace('\'', r"'\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files() -> RootFiles {
        RootFiles::default()
    }

    fn package(scripts: &str, extra: &str) -> String {
        format!(r#"{{ {extra} "scripts": {{ {scripts} }} }}"#)
    }

    fn labels(out: &[ActionSuggestion]) -> Vec<&str> {
        out.iter().map(|s| s.label.as_str()).collect()
    }

    #[test]
    fn package_scripts_pick_the_runner_from_the_lockfile() {
        for (lock, runner) in [
            ("bun.lock", "bun run"),
            ("bun.lockb", "bun run"),
            ("pnpm-lock.yaml", "pnpm run"),
            ("yarn.lock", "yarn run"),
            ("package-lock.json", "npm run"),
        ] {
            let mut root = files();
            root.package_json = Some(package(r#""test": "vitest""#, ""));
            root.present.insert(lock);
            assert_eq!(
                suggest(&root)[0].command,
                format!("{runner} test"),
                "{lock}"
            );
        }
        let mut bare = files();
        bare.package_json = Some(package(r#""test": "vitest""#, ""));
        assert_eq!(suggest(&bare)[0].command, "npm run test");
    }

    #[test]
    fn package_manager_wins_over_the_lockfile() {
        let mut root = files();
        root.package_json = Some(package(
            r#""lint": "oxlint""#,
            r#""packageManager": "pnpm@9.1.0","#,
        ));
        root.present.insert("bun.lock");
        assert_eq!(suggest(&root)[0].command, "pnpm run lint");
    }

    #[test]
    fn scripts_are_kinded_ordered_and_only_dev_is_pinned_and_probed() {
        let mut root = files();
        root.package_json = Some(package(
            r#""build": "vite build", "dev": "vite", "format": "oxfmt", "lint:css": "x",
               "postinstall": "x", "prebuild": "x", "preview": "vite preview", "release": "x", "test:unit": "vitest""#,
            "",
        ));
        let out = suggest(&root);
        assert_eq!(
            labels(&out),
            [
                "dev",
                "preview",
                "test:unit",
                "format",
                "lint:css",
                "build",
                "release"
            ]
        );
        assert!(out[0].pinned);
        assert!(!out[1].pinned);
        assert_eq!(
            out[0].ready,
            ActionReady::Output {
                pattern: "localhost:".into(),
                timeout_secs: 60
            }
        );
        assert_eq!(out[2].ready, ActionReady::None);
        assert!(out.iter().all(|s| s.detected_from == "package.json"));
    }

    #[test]
    fn a_broken_or_scriptless_package_json_suggests_nothing() {
        let mut root = files();
        root.package_json = Some("{ \"scripts\": ".into());
        assert!(suggest(&root).is_empty());
        root.package_json = Some("{ \"name\": \"x\" }".into());
        assert!(suggest(&root).is_empty());
    }

    #[test]
    fn an_odd_script_name_is_quoted() {
        let mut root = files();
        root.package_json = Some(package(r#""it's on": "x""#, ""));
        assert_eq!(suggest(&root)[0].command, r"npm run 'it'\''s on'");
    }

    #[test]
    fn cargo_offers_run_only_for_a_binary() {
        let mut root = files();
        root.cargo_toml = Some("[package]\nname = \"x\"\n".into());
        assert_eq!(
            labels(&suggest(&root)),
            ["cargo test", "cargo clippy", "cargo check"]
        );
        root.present.insert("src/main.rs");
        assert_eq!(labels(&suggest(&root))[0], "cargo run");

        let mut workspace = files();
        workspace.cargo_toml = Some("[workspace]\nmembers = []\n".into());
        workspace.present.insert("src/main.rs");
        workspace.dev_script = true;
        assert_eq!(
            labels(&suggest(&workspace)),
            ["cargo test", "cargo clippy", "cargo check", "scripts/dev"]
        );
    }

    #[test]
    fn makefile_targets_skip_variables_specials_and_patterns() {
        let mut root = files();
        root.makefile = Some(
            ".PHONY: test\nCC := cc\nFOO = 1\n%.o: %.c\ntest: build\n\tcargo test\nbuild:\nlint: ## run lint\n# comment:\n"
                .into(),
        );
        assert_eq!(
            labels(&suggest(&root)),
            ["make test", "make lint", "make build"]
        );
    }

    #[test]
    fn just_recipes_with_parameters_are_skipped() {
        let mut root = files();
        root.justfile = Some(
            "set shell := [\"bash\"]\nversion := \"1\"\n@dev:\n  bun dev\ndeploy env:\n  x\n[private]\ntest: build\n  x\n"
                .into(),
        );
        assert_eq!(labels(&suggest(&root)), ["just dev", "just test"]);
    }

    #[test]
    fn compose_suggests_bringing_the_stack_up() {
        let mut root = files();
        root.compose = Some("compose.yaml");
        let out = suggest(&root);
        assert_eq!(out[0].command, "docker compose up");
        assert_eq!(out[0].detected_from, "compose.yaml");
    }

    #[test]
    fn pyproject_uses_uv_only_with_its_lockfile() {
        let mut root = files();
        root.pyproject = Some(
            "[tool.ruff]\nline-length = 100\n[project]\ndependencies = [\"pytest>=8\"]\n".into(),
        );
        let out = suggest(&root);
        assert_eq!(out[0].command, "python -m pytest");
        assert_eq!(out[1].command, "python -m ruff check");
        root.present.insert("uv.lock");
        assert_eq!(suggest(&root)[0].command, "uv run pytest");
    }

    #[test]
    fn the_first_source_keeps_a_shared_label_and_saved_ones_are_marked() {
        let mut root = files();
        root.package_json = Some(package(r#""Test": "vitest""#, ""));
        root.pyproject = Some("[tool.pytest]\n".into());
        root.justfile = Some("test:\n  x\n".into());
        let mut out = suggest(&root);
        assert_eq!(labels(&out), ["Test", "just test", "pytest"]);

        let saved = ProjectAction {
            id: domain::ProjectActionId::new(),
            project_id: domain::ProjectId::new(),
            label: "test".into(),
            command: "bun run test".into(),
            kind: ActionKind::Test,
            cwd: None,
            single_instance: true,
            pinned: false,
            ready: ActionReady::None,
            port: None,
            depends_on: Vec::new(),
            source: ActionSource::Manual,
            position: 0,
            created_at: domain::Timestamp::now(),
        };
        mark_saved(&mut out, std::slice::from_ref(&saved));
        assert_eq!(out[0].saved_as, Some(saved.id));
        assert_eq!(out[1].saved_as, None);
    }

    #[test]
    fn read_root_skips_an_oversized_manifest_and_a_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let big = "x".repeat(usize::try_from(MAX_MANIFEST_BYTES).unwrap() + 1);
        std::fs::write(dir.path().join("package.json"), big).unwrap();
        std::fs::write(dir.path().join("outside.toml"), "[package]\n").unwrap();
        std::os::unix::fs::symlink(
            dir.path().join("outside.toml"),
            dir.path().join("Cargo.toml"),
        )
        .unwrap();
        std::fs::write(dir.path().join("bun.lock"), "").unwrap();

        let (files, truncated) = read_root(dir.path());
        assert!(truncated);
        assert!(files.package_json.is_none());
        assert!(files.cargo_toml.is_none());
        assert!(files.present.contains("bun.lock"));
    }
}
