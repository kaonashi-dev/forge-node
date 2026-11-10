//! Provider-shaped activity and attention adapters; launches leave explicit user settings alone.

use std::path::{Path, PathBuf};

use domain::{AgentDescriptor, SpawnSpec};

/// OpenCode plugin filename under the agent-plugins directory.
pub const OPENCODE_PLUGIN_FILENAME: &str = "forge-attention.js";
/// Shared BEL script used by Claude / Cursor hooks.
pub const RING_BELL_FILENAME: &str = "forge-ring-bell.sh";
/// Claude `--settings` fragment naming the activity script.
pub const CLAUDE_SETTINGS_FILENAME: &str = "forge-claude-attention.json";
pub const ACTIVITY_FILENAME: &str = "forge-activity.sh";

const OPENCODE_CLI_CONFIG_CONTENT: &str = "OPENCODE_CLI_CONFIG_CONTENT";
const FORGE_HOOK_MARKER: &str = "forge-ring-bell.sh";

/// Absolute paths of the installed attention assets.
#[derive(Clone, Debug)]
pub struct AttentionAssets {
    pub opencode_plugin: PathBuf,
    pub ring_bell: PathBuf,
    pub claude_settings: PathBuf,
    pub activity: PathBuf,
}

/// Embedded OpenCode plugin source.
#[must_use]
pub fn opencode_plugin_source() -> &'static str {
    include_str!("forge-attention.js")
}

/// Embedded BEL script source.
#[must_use]
pub fn ring_bell_source() -> &'static str {
    include_str!("forge-ring-bell.sh")
}

/// Write every attention asset under `dir` (idempotent, content-hashed).
///
/// # Errors
/// Propagates filesystem failures from create/write/chmod.
pub fn install_assets(dir: &Path) -> std::io::Result<AttentionAssets> {
    install_assets_with_ctl(dir, None)
}

pub fn install_assets_with_ctl(
    dir: &Path,
    forgectl: Option<&Path>,
) -> std::io::Result<AttentionAssets> {
    ensure_private_dir(dir)?;

    let opencode_plugin = write_if_changed(
        dir.join(OPENCODE_PLUGIN_FILENAME),
        opencode_plugin_source().as_bytes(),
    )?;
    let ring_bell = write_if_changed(dir.join(RING_BELL_FILENAME), ring_bell_source().as_bytes())?;
    let activity_body = include_str!("forge-activity.sh").replace(
        "__FORGECTL__",
        &shell_quote(&forgectl.map(|p| p.to_string_lossy()).unwrap_or_default()),
    );
    let activity = write_if_changed(dir.join(ACTIVITY_FILENAME), activity_body.as_bytes())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&ring_bell, std::fs::Permissions::from_mode(0o700))?;
        std::fs::set_permissions(&activity, std::fs::Permissions::from_mode(0o700))?;
    }

    let hook = |state: &str| {
        serde_json::json!([{
            "type": "command",
            "command": format!("{} {state}", shell_quote(&activity.to_string_lossy())),
            "timeout": 5
        }])
    };
    let claude_settings_body = serde_json::json!({
        "hooks": {
            "UserPromptSubmit": [{ "hooks": hook("working") }],
            "PreToolUse": [{ "matcher": "AskUserQuestion", "hooks": hook("waiting") }],
            "Stop": [{ "hooks": hook("idle") }],
            "PermissionRequest": [{
                "hooks": hook("waiting")
            }],
            "Notification": [{
                "matcher": "permission_prompt|elicitation_dialog|agent_needs_input",
                "hooks": hook("waiting")
            }]
        }
    });
    let claude_settings = write_if_changed(
        dir.join(CLAUDE_SETTINGS_FILENAME),
        format!("{claude_settings_body}\n").as_bytes(),
    )?;

    Ok(AttentionAssets {
        opencode_plugin,
        ring_bell,
        claude_settings,
        activity,
    })
}

/// Inject the provider's attention adapter into `spec`.
///
/// Returns whether anything was changed. Never overwrites a user-supplied
/// OpenCode config overlay or an existing `--settings` the launch already
/// carries.
pub fn inject_attention(
    descriptor: &AgentDescriptor,
    spec: &mut SpawnSpec,
    assets: &AttentionAssets,
) -> bool {
    match descriptor.id.as_str() {
        "opencode" => inject_opencode(spec, assets),
        "claude" => inject_claude(spec, assets),
        "cursor" => inject_cursor(spec, assets),
        _ => false,
    }
}

fn inject_opencode(spec: &mut SpawnSpec, assets: &AttentionAssets) -> bool {
    if spec
        .env
        .iter()
        .any(|(k, _)| k == OPENCODE_CLI_CONFIG_CONTENT)
    {
        tracing::info!(
            "OPENCODE_CLI_CONFIG_CONTENT already set; leaving Forge attention plugin unset"
        );
        return false;
    }
    let path = assets.opencode_plugin.to_string_lossy();
    let value = serde_json::json!({
        "$schema": "https://opencode.ai/config.json",
        "plugins": [{ "package": path.as_ref(), "options": { "activity": assets.activity } }],
    })
    .to_string();
    spec.env
        .push((OPENCODE_CLI_CONFIG_CONTENT.to_owned(), value));
    true
}

fn inject_claude(spec: &mut SpawnSpec, assets: &AttentionAssets) -> bool {
    if args_contain(&spec.args, "--settings") {
        tracing::info!("claude launch already has --settings; leaving Forge attention unset");
        return false;
    }
    spec.args.push("--settings".to_owned());
    spec.args
        .push(assets.claude_settings.to_string_lossy().into_owned());
    true
}

fn inject_cursor(spec: &mut SpawnSpec, assets: &AttentionAssets) -> bool {
    let Some(home) = env_home(&spec.env) else {
        return false;
    };
    match ensure_cursor_hooks(&home, &assets.ring_bell, &assets.activity) {
        Ok(changed) => changed,
        Err(error) => {
            tracing::warn!(%error, "could not install Cursor attention hooks");
            false
        }
    }
}

/// Cursor has no permission-prompt hook; `beforeShellExecution` /
/// `beforeMCPExecution` are the closest signals (best-effort). Cursor runs a
/// hook as `/bin/sh -c <command>`, so every path is quoted.
fn ensure_cursor_hooks(home: &Path, ring_bell: &Path, activity: &Path) -> std::io::Result<bool> {
    let path = home.join(".cursor").join("hooks.json");
    let bell =
        serde_json::json!({ "command": shell_quote(&ring_bell.to_string_lossy()), "timeout": 5 });
    let report = |state: &str| {
        serde_json::json!({
            "command": format!("{} {state} cursor", shell_quote(&activity.to_string_lossy())),
            "timeout": 5,
        })
    };

    let invalid =
        |reason: &str| std::io::Error::new(std::io::ErrorKind::InvalidData, reason.to_owned());
    let existing = if path.is_file() {
        String::from_utf8(crate::capped::read(&path, MAX_CURSOR_HOOKS_BYTES)?)
            .map_err(|error| invalid(&error.to_string()))?
    } else {
        String::new()
    };
    // The file is the user's: one this cannot read is left alone, never rewritten.
    let mut root = if existing.trim().is_empty() {
        serde_json::json!({})
    } else {
        serde_json::from_str(&existing).map_err(|error| invalid(&error.to_string()))?
    };
    let root_object = root
        .as_object_mut()
        .ok_or_else(|| invalid("hooks.json root is not an object"))?;
    root_object
        .entry("version".to_owned())
        .or_insert_with(|| serde_json::json!(1));
    let hooks = root_object
        .entry("hooks".to_owned())
        .or_insert_with(|| serde_json::json!({}));

    let mut changed = false;
    for event in ["beforeShellExecution", "beforeMCPExecution"] {
        changed |= push_hook_command(hooks, event, &bell, FORGE_HOOK_MARKER);
    }
    changed |= push_hook_command(
        hooks,
        "beforeSubmitPrompt",
        &report("working"),
        ACTIVITY_FILENAME,
    );
    changed |= push_hook_command(hooks, "stop", &report("idle"), ACTIVITY_FILENAME);
    if !changed {
        return Ok(false);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let body = serde_json::to_string_pretty(&root)
        .map_err(|error| std::io::Error::other(format!("serialize hooks: {error}")))?;
    std::fs::write(&path, format!("{body}\n"))?;
    Ok(true)
}

fn push_hook_command(
    hooks: &mut serde_json::Value,
    event: &str,
    entry: &serde_json::Value,
    marker: &str,
) -> bool {
    let Some(obj) = hooks.as_object_mut() else {
        return false;
    };
    let list = obj
        .entry(event.to_owned())
        .or_insert_with(|| serde_json::json!([]));
    let Some(arr) = list.as_array_mut() else {
        return false;
    };
    let ours = |item: &serde_json::Value| {
        item.get("command")
            .and_then(|c| c.as_str())
            .is_some_and(|c| c.contains(marker))
    };
    // An earlier Forge entry is replaced, not kept: an unquoted path broke under `sh -c`.
    if let Some(item) = arr.iter_mut().find(|item| ours(item)) {
        if *item == *entry {
            return false;
        }
        *item = entry.clone();
        return true;
    }
    let already = arr.iter().any(|item| {
        item.get("hooks")
            .and_then(|h| h.as_array())
            .is_some_and(|inner| {
                inner.iter().any(|h| {
                    h.get("command")
                        .and_then(|c| c.as_str())
                        .is_some_and(|c| c.contains(marker))
                })
            })
    });
    if already {
        return false;
    }
    arr.push(entry.clone());
    true
}

fn args_contain(args: &[String], needle: &str) -> bool {
    args.iter()
        .any(|a| a == needle || a.starts_with(&format!("{needle}=")))
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn env_home(vars: &[(String, String)]) -> Option<PathBuf> {
    env_var(vars, "HOME").map(PathBuf::from)
}

fn env_var<'a>(vars: &'a [(String, String)], key: &str) -> Option<&'a str> {
    vars.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
}

fn ensure_private_dir(dir: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        match std::fs::symlink_metadata(dir) {
            Ok(meta) if meta.is_dir() => {
                let mode = meta.permissions().mode() & 0o777;
                if mode != 0o700 {
                    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
                }
            }
            Ok(_) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::AlreadyExists,
                    format!("{} exists and is not a directory", dir.display()),
                ));
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                std::fs::DirBuilder::new()
                    .recursive(true)
                    .mode(0o700)
                    .create(dir)?;
            }
            Err(e) => return Err(e),
        }
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(dir)?;
    }
    Ok(())
}

/// Largest `~/.cursor/hooks.json` merged into; a bigger one is left alone.
const MAX_CURSOR_HOOKS_BYTES: u64 = 1024 * 1024;

fn write_if_changed(path: PathBuf, source: &[u8]) -> std::io::Result<PathBuf> {
    // One byte past the asset is enough to know it differs.
    match crate::capped::read(&path, source.len() as u64) {
        Ok(existing) if existing == source => return Ok(path),
        _ => {}
    }
    std::fs::write(&path, source)?;
    Ok(path)
}

pub const PLUGIN_FILENAME: &str = OPENCODE_PLUGIN_FILENAME;

/// Install assets and return the OpenCode plugin path (legacy helper).
pub fn install_plugin(dir: &Path) -> std::io::Result<PathBuf> {
    Ok(install_assets(dir)?.opencode_plugin)
}

/// OpenCode-only inject (legacy helper).
pub fn inject_attention_plugin(
    descriptor: &AgentDescriptor,
    vars: &mut Vec<(String, String)>,
    plugin_path: &Path,
) -> bool {
    if descriptor.id.as_str() != "opencode" {
        return false;
    }
    let mut spec = SpawnSpec {
        program: PathBuf::from("opencode"),
        args: Vec::new(),
        cwd: PathBuf::from("/"),
        env: std::mem::take(vars),
    };
    let assets = AttentionAssets {
        opencode_plugin: plugin_path.to_path_buf(),
        ring_bell: PathBuf::new(),
        claude_settings: PathBuf::new(),
        activity: plugin_path.with_file_name(ACTIVITY_FILENAME),
    };
    let changed = inject_opencode(&mut spec, &assets);
    *vars = spec.env;
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtins;
    use domain::AgentProviderId;

    fn empty_spec() -> SpawnSpec {
        SpawnSpec {
            program: PathBuf::from("agent"),
            args: Vec::new(),
            cwd: PathBuf::from("/tmp"),
            env: vec![("HOME".into(), "/tmp/forge-home".into())],
        }
    }

    #[test]
    fn install_is_idempotent_and_rewrites_when_stale() {
        let dir = tempfile::tempdir().unwrap();
        let plugins = dir.path().join("agent-plugins");

        let first = install_assets(&plugins).unwrap();
        assert!(first.opencode_plugin.exists());
        assert!(first.ring_bell.exists());
        assert!(first.claude_settings.exists());
        assert!(std::fs::read_to_string(&first.claude_settings)
            .unwrap()
            .contains("PermissionRequest"));

        let mtime_before = std::fs::metadata(&first.opencode_plugin)
            .unwrap()
            .modified()
            .unwrap();
        let second = install_assets(&plugins).unwrap();
        assert_eq!(first.opencode_plugin, second.opencode_plugin);
        let mtime_after = std::fs::metadata(&second.opencode_plugin)
            .unwrap()
            .modified()
            .unwrap();
        assert_eq!(mtime_before, mtime_after);

        std::fs::write(&first.opencode_plugin, b"// stale\n").unwrap();
        let third = install_assets(&plugins).unwrap();
        assert_eq!(
            std::fs::read_to_string(&third.opencode_plugin).unwrap(),
            opencode_plugin_source()
        );
    }

    #[test]
    fn opencode_gets_the_config_content_variable() {
        let dir = tempfile::tempdir().unwrap();
        let assets = install_assets(&dir.path().join("p")).unwrap();
        let descriptor = builtins::builtin("opencode").unwrap();
        let mut spec = empty_spec();
        assert!(inject_attention(&descriptor, &mut spec, &assets));
        let value = spec
            .env
            .iter()
            .find(|(k, _)| k == OPENCODE_CLI_CONFIG_CONTENT)
            .map(|(_, v)| v.as_str())
            .expect("OPENCODE_CLI_CONFIG_CONTENT");
        assert!(value.contains("forge-attention.js"));
    }

    #[test]
    fn claude_gets_settings_flag() {
        let dir = tempfile::tempdir().unwrap();
        let assets = install_assets(&dir.path().join("p")).unwrap();
        let descriptor = builtins::builtin("claude").unwrap();
        let mut spec = empty_spec();
        assert!(inject_attention(&descriptor, &mut spec, &assets));
        assert_eq!(spec.args[0], "--settings");
        assert_eq!(spec.args[1], assets.claude_settings.to_string_lossy());
    }

    #[test]
    fn claude_keeps_a_preexisting_settings_flag() {
        let dir = tempfile::tempdir().unwrap();
        let assets = install_assets(&dir.path().join("p")).unwrap();
        let descriptor = builtins::builtin("claude").unwrap();
        let mut spec = empty_spec();
        spec.args = vec!["--settings".into(), "/mine.json".into()];
        assert!(!inject_attention(&descriptor, &mut spec, &assets));
        assert_eq!(spec.args, ["--settings", "/mine.json"]);
    }

    #[test]
    fn claude_hooks_report_turns_questions_and_permissions_not_subagents() {
        let dir = tempfile::tempdir().unwrap();
        let assets =
            install_assets_with_ctl(dir.path(), Some(Path::new("/a path/forgectl"))).unwrap();
        let settings: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&assets.claude_settings).unwrap()).unwrap();
        for (event, state) in [
            ("UserPromptSubmit", "working"),
            ("PreToolUse", "waiting"),
            ("PermissionRequest", "waiting"),
            ("Notification", "waiting"),
            ("Stop", "idle"),
        ] {
            assert!(settings["hooks"][event][0]["hooks"][0]["command"]
                .as_str()
                .unwrap()
                .ends_with(state));
        }
        assert_eq!(
            settings["hooks"]["PreToolUse"][0]["matcher"],
            "AskUserQuestion"
        );
        assert!(settings["hooks"]["SubagentStop"].is_null());
        assert!(settings["hooks"]["PostToolUse"].is_null());
        assert!(std::fs::read_to_string(assets.activity)
            .unwrap()
            .contains("ctl='/a path/forgectl'"));
    }

    #[test]
    fn legacy_opencode_injection_names_the_installed_activity_script() {
        let dir = tempfile::tempdir().unwrap();
        let plugin = install_plugin(dir.path()).unwrap();
        let mut vars = vec![];
        assert!(inject_attention_plugin(
            &builtins::builtin("opencode").unwrap(),
            &mut vars,
            &plugin
        ));
        let overlay: serde_json::Value = serde_json::from_str(&vars[0].1).unwrap();
        assert_eq!(
            overlay["plugins"][0]["options"]["activity"],
            dir.path()
                .join(ACTIVITY_FILENAME)
                .to_string_lossy()
                .as_ref()
        );
    }

    #[test]
    fn activity_script_reports_with_an_absolute_ctl_and_always_exits_zero() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let ctl = dir.path().join("ctl with ' quote");
        let capture = dir.path().join("args");
        std::fs::write(
            &ctl,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" > {}\nexit 7\n",
                shell_quote(&capture.to_string_lossy())
            ),
        )
        .unwrap();
        std::fs::set_permissions(&ctl, std::fs::Permissions::from_mode(0o700)).unwrap();
        let assets = install_assets_with_ctl(dir.path(), Some(&ctl)).unwrap();
        let status = crate::detection::command_for(&assets.activity)
            .args(["idle", "claude"])
            .env("FORGE_SESSION_ID", "s")
            .stdin(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(status.success());
        assert_eq!(
            std::fs::read_to_string(&capture).unwrap(),
            "hook\n--state\nidle\n--source\nclaude\n"
        );
        std::fs::remove_file(&capture).unwrap();
        assert!(crate::detection::command_for(&assets.activity)
            .arg("idle")
            .env_remove("FORGE_SESSION_ID")
            .stdin(std::process::Stdio::null())
            .status()
            .unwrap()
            .success());
        assert!(!capture.exists());
    }

    #[test]
    fn claude_keeps_equals_settings() {
        let dir = tempfile::tempdir().unwrap();
        let assets = install_assets(dir.path()).unwrap();
        let mut spec = empty_spec();
        spec.args = vec!["--settings=/mine.json".into()];
        assert!(!inject_attention(
            &builtins::builtin("claude").unwrap(),
            &mut spec,
            &assets
        ));
    }

    #[test]
    fn cursor_merges_best_effort_hooks() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        std::fs::create_dir_all(home.join(".cursor")).unwrap();
        std::fs::write(
            home.join(".cursor/hooks.json"),
            r#"{"version":1,"hooks":{"stop":[{"command":"/other"}]}}"#,
        )
        .unwrap();
        let assets = install_assets(&dir.path().join("p")).unwrap();
        let descriptor = builtins::builtin("cursor").unwrap();
        let mut spec = empty_spec();
        spec.env = vec![("HOME".into(), home.to_string_lossy().into_owned())];
        assert!(inject_attention(&descriptor, &mut spec, &assets));
        let hooks: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(home.join(".cursor/hooks.json")).unwrap(),
        )
        .unwrap();
        let stop = hooks["hooks"]["stop"].as_array().unwrap();
        assert_eq!(stop[0]["command"], "/other");
        assert_eq!(
            stop[1]["command"],
            format!(
                "{} idle cursor",
                shell_quote(&assets.activity.to_string_lossy())
            )
        );
        assert!(hooks["hooks"]["beforeSubmitPrompt"][0]["command"]
            .as_str()
            .unwrap()
            .ends_with("forge-activity.sh' working cursor"));
        assert_eq!(
            hooks["hooks"]["beforeShellExecution"][0]["command"],
            shell_quote(&assets.ring_bell.to_string_lossy())
        );
        assert!(!inject_attention(&descriptor, &mut spec, &assets));
    }

    #[test]
    fn cursor_hooks_replace_an_unquoted_forge_entry() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        std::fs::create_dir_all(home.join(".cursor")).unwrap();
        let assets = install_assets(&dir.path().join("Application Support")).unwrap();
        let bare = assets.ring_bell.to_string_lossy().into_owned();
        std::fs::write(
            home.join(".cursor/hooks.json"),
            serde_json::json!({"version":1,"hooks":{"beforeShellExecution":[{"command":bare,"timeout":5}]}})
                .to_string(),
        )
        .unwrap();
        let descriptor = builtins::builtin("cursor").unwrap();
        let mut spec = empty_spec();
        spec.env = vec![("HOME".into(), home.to_string_lossy().into_owned())];
        assert!(inject_attention(&descriptor, &mut spec, &assets));
        let hooks: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(home.join(".cursor/hooks.json")).unwrap(),
        )
        .unwrap();
        let shell = hooks["hooks"]["beforeShellExecution"].as_array().unwrap();
        assert_eq!(shell.len(), 1);
        assert_eq!(shell[0]["command"], shell_quote(&bare));
    }

    #[test]
    fn cursor_hooks_leave_a_file_they_cannot_use_untouched() {
        let assets_dir = tempfile::tempdir().unwrap();
        let assets = install_assets(&assets_dir.path().join("p")).unwrap();
        let descriptor = builtins::builtin("cursor").unwrap();
        // A hand-edited file with a comment and a trailing comma, and roots
        // that parse but are not objects: none may be replaced or panic.
        for body in [
            "{\n  // mine\n  \"version\": 1,\n}\n",
            "[]",
            "123",
            "\"x\"",
            "null",
        ] {
            let dir = tempfile::tempdir().unwrap();
            let home = dir.path().join("home");
            std::fs::create_dir_all(home.join(".cursor")).unwrap();
            std::fs::write(home.join(".cursor/hooks.json"), body).unwrap();
            let mut spec = empty_spec();
            spec.env = vec![("HOME".into(), home.to_string_lossy().into_owned())];

            assert!(!inject_attention(&descriptor, &mut spec, &assets));
            assert_eq!(
                std::fs::read_to_string(home.join(".cursor/hooks.json")).unwrap(),
                body
            );
        }
    }

    #[test]
    fn cursor_hooks_treat_an_empty_file_as_absent() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        std::fs::create_dir_all(home.join(".cursor")).unwrap();
        std::fs::write(home.join(".cursor/hooks.json"), "  \n").unwrap();
        let assets = install_assets(&dir.path().join("p")).unwrap();
        let descriptor = builtins::builtin("cursor").unwrap();
        let mut spec = empty_spec();
        spec.env = vec![("HOME".into(), home.to_string_lossy().into_owned())];

        assert!(inject_attention(&descriptor, &mut spec, &assets));
        let hooks: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(home.join(".cursor/hooks.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(hooks["version"], 1);
    }

    #[test]
    fn a_preexisting_opencode_config_survives() {
        let dir = tempfile::tempdir().unwrap();
        let assets = install_assets(&dir.path().join("p")).unwrap();
        let descriptor = builtins::builtin("opencode").unwrap();
        let mut spec = empty_spec();
        spec.env.push((
            OPENCODE_CLI_CONFIG_CONTENT.into(),
            r#"{"plugins":["/mine.js"]}"#.into(),
        ));
        assert!(!inject_attention(&descriptor, &mut spec, &assets));
        assert_eq!(
            spec.env
                .iter()
                .find(|(k, _)| k == OPENCODE_CLI_CONFIG_CONTENT)
                .map(|(_, v)| v.as_str()),
            Some(r#"{"plugins":["/mine.js"]}"#)
        );
    }

    #[test]
    fn unknown_providers_are_untouched() {
        // No sixth provider today; the wildcard arm is the contract.
        let dir = tempfile::tempdir().unwrap();
        let assets = install_assets(&dir.path().join("p")).unwrap();
        let mut descriptor = builtins::builtin("claude").unwrap();
        descriptor.id = AgentProviderId::new("nope");
        let mut spec = empty_spec();
        assert!(!inject_attention(&descriptor, &mut spec, &assets));
        assert!(spec.args.is_empty());
    }
}
