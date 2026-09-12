//! Agent attention: make the rail's `needs-you` marker fire when a TUI agent asks.
//!
//! Forge listens for ASCII BEL. Providers that do not emit it get a Forge-owned
//! adapter at launch (env, CLI flags, or a FORGE-gated hook). Provider-shaped
//! knowledge stays here (P2); the daemon only supplies the on-disk asset dir.

use std::path::{Path, PathBuf};

use domain::{AgentDescriptor, SpawnSpec};

/// OpenCode plugin filename under the agent-plugins directory.
pub const OPENCODE_PLUGIN_FILENAME: &str = "forge-attention.js";
/// Shared BEL script used by Claude / Cursor / Grok hooks.
pub const RING_BELL_FILENAME: &str = "forge-ring-bell.sh";
/// Claude `--settings` fragment naming [`RING_BELL_FILENAME`].
pub const CLAUDE_SETTINGS_FILENAME: &str = "forge-claude-attention.json";

const OPENCODE_CONFIG_CONTENT: &str = "OPENCODE_CONFIG_CONTENT";
const FORGE_HOOK_MARKER: &str = "forge-ring-bell.sh";
/// Grok's opt-out of the Cursor hook compatibility scan (§13.3 is unaffected:
/// this is a launch detail, not a terminal-contract variable).
const GROK_CURSOR_HOOKS_ENABLED: &str = "GROK_CURSOR_HOOKS_ENABLED";

/// Absolute paths of the installed attention assets.
#[derive(Clone, Debug)]
pub struct AttentionAssets {
    pub opencode_plugin: PathBuf,
    pub ring_bell: PathBuf,
    pub claude_settings: PathBuf,
}

/// Embedded OpenCode plugin source.
pub fn opencode_plugin_source() -> &'static str {
    include_str!("forge-attention.js")
}

/// Embedded BEL script source.
pub fn ring_bell_source() -> &'static str {
    include_str!("forge-ring-bell.sh")
}

/// Write every attention asset under `dir` (idempotent, content-hashed).
///
/// # Errors
/// Propagates filesystem failures from create/write/chmod.
pub fn install_assets(dir: &Path) -> std::io::Result<AttentionAssets> {
    ensure_private_dir(dir)?;

    let opencode_plugin = write_if_changed(
        dir.join(OPENCODE_PLUGIN_FILENAME),
        opencode_plugin_source().as_bytes(),
    )?;
    let ring_bell = write_if_changed(dir.join(RING_BELL_FILENAME), ring_bell_source().as_bytes())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&ring_bell, std::fs::Permissions::from_mode(0o700))?;
    }

    let claude_settings_body = serde_json::json!({
        "hooks": {
            "PermissionRequest": [{
                "hooks": [{
                    "type": "command",
                    "command": ring_bell.to_string_lossy(),
                    "timeout": 5
                }]
            }],
            "Notification": [{
                "matcher": "permission_prompt|elicitation_dialog|agent_needs_input",
                "hooks": [{
                    "type": "command",
                    "command": ring_bell.to_string_lossy(),
                    "timeout": 5
                }]
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
    })
}

/// Inject the provider's attention adapter into `spec`.
///
/// Returns whether anything was changed. Never overwrites a user-supplied
/// OpenCode config overlay or an existing `--settings` / `tui.notification_*`
/// the launch already carries.
pub fn inject_attention(
    descriptor: &AgentDescriptor,
    spec: &mut SpawnSpec,
    assets: &AttentionAssets,
) -> bool {
    match descriptor.id.as_str() {
        "opencode" => inject_opencode(spec, assets),
        "claude" => inject_claude(spec, assets),
        "codex" => inject_codex(spec),
        "cursor" => inject_cursor(spec, assets),
        "grok" => inject_grok(spec, assets),
        _ => false,
    }
}

fn inject_opencode(spec: &mut SpawnSpec, assets: &AttentionAssets) -> bool {
    if spec.env.iter().any(|(k, _)| k == OPENCODE_CONFIG_CONTENT) {
        tracing::info!("OPENCODE_CONFIG_CONTENT already set; leaving Forge attention plugin unset");
        return false;
    }
    let path = assets.opencode_plugin.to_string_lossy();
    let value = serde_json::json!({
        "$schema": "https://opencode.ai/config.json",
        "plugin": [path.as_ref()],
    })
    .to_string();
    spec.env.push((OPENCODE_CONFIG_CONTENT.to_owned(), value));
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

fn inject_codex(spec: &mut SpawnSpec) -> bool {
    if args_contain_config_key(&spec.args, "tui.notification_method")
        || args_contain_config_key(&spec.args, "tui.notifications")
    {
        tracing::info!(
            "codex launch already overrides TUI notifications; leaving Forge attention unset"
        );
        return false;
    }
    // approval-requested only — agent-turn-complete would paint needs-you on every idle turn.
    // condition=always: Forge's PTY has no focus state, so "unfocused" never fires.
    spec.args.extend([
        "-c".to_owned(),
        r#"tui.notifications=["approval-requested"]"#.to_owned(),
        "-c".to_owned(),
        r#"tui.notification_method="bel""#.to_owned(),
        "-c".to_owned(),
        r#"tui.notification_condition="always""#.to_owned(),
    ]);
    true
}

fn inject_cursor(spec: &mut SpawnSpec, assets: &AttentionAssets) -> bool {
    let Some(home) = env_home(&spec.env) else {
        return false;
    };
    match ensure_cursor_hooks(&home, &assets.ring_bell) {
        Ok(changed) => changed,
        Err(error) => {
            tracing::warn!(%error, "could not install Cursor attention hooks");
            false
        }
    }
}

/// Grok's own hook file, and an opt-out of the *Cursor* hooks Grok also reads.
///
/// The opt-out is the half that is not obvious. Grok scans `~/.cursor/hooks.json`
/// by default (its Cursor compatibility layer, always trusted at the global
/// scope) and maps `beforeShellExecution` / `beforeMCPExecution` onto its own
/// `PreToolUse`. That file is exactly where [`ensure_cursor_hooks`] already put
/// `forge-ring-bell.sh`, so on a machine where Cursor has ever been launched
/// from Forge a Grok session would ring BEL on *every tool call* — `needs-you`
/// lit permanently rather than on the permission prompts the marker means. The
/// hook Grok should ring from is the one written below; the Cursor file is
/// Cursor's.
fn inject_grok(spec: &mut SpawnSpec, assets: &AttentionAssets) -> bool {
    let home = env_var(&spec.env, "GROK_HOME")
        .map(PathBuf::from)
        .or_else(|| env_home(&spec.env).map(|h| h.join(".grok")));
    let Some(grok_home) = home else {
        return false;
    };
    let mut changed = set_env_if_unset(spec, GROK_CURSOR_HOOKS_ENABLED, "false");
    match ensure_grok_hooks(&grok_home, &assets.ring_bell) {
        Ok(wrote) => changed |= wrote,
        Err(error) => tracing::warn!(%error, "could not install Grok attention hooks"),
    }
    changed
}

/// Set `key` on the launch unless it already carries one — a profile that says
/// so itself wins, as it does for every other injection here.
fn set_env_if_unset(spec: &mut SpawnSpec, key: &str, value: &str) -> bool {
    if spec.env.iter().any(|(k, _)| k == key) {
        tracing::info!(key, "launch already sets it; leaving the Forge value unset");
        return false;
    }
    spec.env.push((key.to_owned(), value.to_owned()));
    true
}

/// Cursor has no permission-prompt hook; `beforeShellExecution` /
/// `beforeMCPExecution` are the closest signals (best-effort).
fn ensure_cursor_hooks(home: &Path, ring_bell: &Path) -> std::io::Result<bool> {
    let path = home.join(".cursor").join("hooks.json");
    let command = ring_bell.to_string_lossy().into_owned();
    let entry = serde_json::json!({ "command": command, "timeout": 5 });

    let mut root = if path.is_file() {
        serde_json::from_str(&std::fs::read_to_string(&path)?)
            .unwrap_or_else(|_| serde_json::json!({ "version": 1, "hooks": {} }))
    } else {
        serde_json::json!({ "version": 1, "hooks": {} })
    };
    if root.get("version").is_none() {
        root["version"] = serde_json::json!(1);
    }
    let hooks = root
        .as_object_mut()
        .map(|o| {
            o.entry("hooks".to_owned())
                .or_insert_with(|| serde_json::json!({}))
        })
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "hooks.json root"))?;

    let mut changed = false;
    for event in ["beforeShellExecution", "beforeMCPExecution"] {
        changed |= push_hook_command(hooks, event, &entry, FORGE_HOOK_MARKER);
    }
    if !changed {
        return Ok(false);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(
        &path,
        format!("{}\n", serde_json::to_string_pretty(&root).unwrap()),
    )?;
    Ok(true)
}

fn ensure_grok_hooks(grok_home: &Path, ring_bell: &Path) -> std::io::Result<bool> {
    let dir = grok_home.join("hooks");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("forge-attention.json");
    let body = serde_json::json!({
        "hooks": {
            "Notification": [{
                "matcher": "permission_prompt",
                "hooks": [{
                    "type": "command",
                    "command": ring_bell.to_string_lossy(),
                    "timeout": 5
                }]
            }]
        }
    });
    let bytes = format!("{body}\n").into_bytes();
    match std::fs::read(&path) {
        Ok(existing) if existing == bytes => return Ok(false),
        _ => {}
    }
    std::fs::write(&path, bytes)?;
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
    let already = arr.iter().any(|item| {
        item.get("command")
            .and_then(|c| c.as_str())
            .is_some_and(|c| c.contains(marker))
            || item
                .get("hooks")
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
    args.iter().any(|a| a == needle)
}

fn args_contain_config_key(args: &[String], key: &str) -> bool {
    args.windows(2)
        .any(|w| w[0] == "-c" && w[1].starts_with(key))
        || args.iter().any(|a| {
            a.starts_with(&format!("-c{key}")) || a.starts_with(&format!("--config={key}"))
        })
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

fn write_if_changed(path: PathBuf, source: &[u8]) -> std::io::Result<PathBuf> {
    match std::fs::read(&path) {
        Ok(existing) if existing == source => return Ok(path),
        _ => {}
    }
    std::fs::write(&path, source)?;
    Ok(path)
}

// Compatibility aliases used by older call sites / re-exports.
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
            .find(|(k, _)| k == OPENCODE_CONFIG_CONTENT)
            .map(|(_, v)| v.as_str())
            .expect("OPENCODE_CONFIG_CONTENT");
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
    fn codex_gets_bel_notification_overrides() {
        let dir = tempfile::tempdir().unwrap();
        let assets = install_assets(&dir.path().join("p")).unwrap();
        let descriptor = builtins::builtin("codex").unwrap();
        let mut spec = empty_spec();
        assert!(inject_attention(&descriptor, &mut spec, &assets));
        assert!(spec.args.iter().any(|a| a.contains("approval-requested")));
        assert!(spec
            .args
            .iter()
            .any(|a| a.contains(r#"notification_method="bel""#)));
        assert!(spec
            .args
            .iter()
            .any(|a| a.contains(r#"notification_condition="always""#)));
    }

    #[test]
    fn grok_writes_a_permission_notification_hook() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        std::fs::create_dir_all(home.join(".grok")).unwrap();
        let assets = install_assets(&dir.path().join("p")).unwrap();
        let descriptor = builtins::builtin("grok").unwrap();
        let mut spec = empty_spec();
        spec.env = vec![("HOME".into(), home.to_string_lossy().into_owned())];
        assert!(inject_attention(&descriptor, &mut spec, &assets));
        let hook = std::fs::read_to_string(home.join(".grok/hooks/forge-attention.json")).unwrap();
        assert!(hook.contains("permission_prompt"));
        assert!(hook.contains("forge-ring-bell.sh"));
        assert!(!inject_attention(&descriptor, &mut spec, &assets));
    }

    /// The bug this guards: Grok reads `~/.cursor/hooks.json` too, and Forge's
    /// *Cursor* injection put `forge-ring-bell.sh` on `beforeShellExecution` /
    /// `beforeMCPExecution` there. Grok maps both to `PreToolUse`, so without
    /// the opt-out a Grok session rings BEL on every tool call and `needs-you`
    /// never goes out.
    #[test]
    fn grok_ignores_the_cursor_hooks_forge_wrote_for_cursor() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        std::fs::create_dir_all(home.join(".grok")).unwrap();
        let assets = install_assets(&dir.path().join("p")).unwrap();

        // Exactly the state a machine is in after one Cursor launch.
        let cursor = builtins::builtin("cursor").unwrap();
        let mut cursor_spec = empty_spec();
        cursor_spec.env = vec![("HOME".into(), home.to_string_lossy().into_owned())];
        assert!(inject_attention(&cursor, &mut cursor_spec, &assets));
        let cursor_hooks = std::fs::read_to_string(home.join(".cursor/hooks.json")).unwrap();
        assert!(cursor_hooks.contains("forge-ring-bell.sh"));

        let mut spec = empty_spec();
        spec.env = vec![("HOME".into(), home.to_string_lossy().into_owned())];
        assert!(inject_attention(
            &builtins::builtin("grok").unwrap(),
            &mut spec,
            &assets
        ));
        assert_eq!(
            env_var(&spec.env, "GROK_CURSOR_HOOKS_ENABLED"),
            Some("false")
        );
    }

    /// A profile that says so itself wins, as it does for every other
    /// injection here.
    #[test]
    fn grok_leaves_an_explicit_cursor_hooks_choice_alone() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        std::fs::create_dir_all(home.join(".grok")).unwrap();
        let assets = install_assets(&dir.path().join("p")).unwrap();
        let mut spec = empty_spec();
        spec.env = vec![
            ("HOME".into(), home.to_string_lossy().into_owned()),
            ("GROK_CURSOR_HOOKS_ENABLED".into(), "true".into()),
        ];
        inject_attention(&builtins::builtin("grok").unwrap(), &mut spec, &assets);
        assert_eq!(
            env_var(&spec.env, "GROK_CURSOR_HOOKS_ENABLED"),
            Some("true")
        );
        assert_eq!(
            spec.env
                .iter()
                .filter(|(k, _)| k == "GROK_CURSOR_HOOKS_ENABLED")
                .count(),
            1
        );
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
        assert!(hooks["hooks"]["stop"].as_array().unwrap().len() == 1);
        assert!(hooks["hooks"]["beforeShellExecution"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["command"].as_str().unwrap().contains("forge-ring-bell")));
        assert!(!inject_attention(&descriptor, &mut spec, &assets));
    }

    #[test]
    fn a_preexisting_opencode_config_survives() {
        let dir = tempfile::tempdir().unwrap();
        let assets = install_assets(&dir.path().join("p")).unwrap();
        let descriptor = builtins::builtin("opencode").unwrap();
        let mut spec = empty_spec();
        spec.env.push((
            OPENCODE_CONFIG_CONTENT.into(),
            r#"{"plugin":["/mine.js"]}"#.into(),
        ));
        assert!(!inject_attention(&descriptor, &mut spec, &assets));
        assert_eq!(
            spec.env
                .iter()
                .find(|(k, _)| k == OPENCODE_CONFIG_CONTENT)
                .map(|(_, v)| v.as_str()),
            Some(r#"{"plugin":["/mine.js"]}"#)
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
