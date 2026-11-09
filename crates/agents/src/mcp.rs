//! Forge's own MCP server (`forgectl mcp`) registered in a launch, for the
//! providers that take a launch-local MCP configuration.

use std::path::PathBuf;

use domain::{AgentDescriptor, SpawnSpec};

/// The server name a provider shows tools under (Claude: `mcp__forge__report`).
pub const SERVER_NAME: &str = "forge";

const OPENCODE_CONFIG_CONTENT: &str = "OPENCODE_CONFIG_CONTENT";

/// `command mcp`, started by the agent with `env` as its identity: a provider
/// need not hand its own environment to an MCP child.
#[derive(Clone, Debug)]
pub struct ForgeMcp {
    pub command: PathBuf,
    pub env: Vec<(String, String)>,
}

/// Whether [`inject_forge_mcp`] can register the server for this provider.
#[must_use]
pub fn accepts_forge_mcp(descriptor: &AgentDescriptor) -> bool {
    matches!(descriptor.id.as_str(), "claude" | "opencode")
}

/// Register [`SERVER_NAME`] in `spec`. Returns whether anything changed; a
/// configuration the user already supplies through the same channel is left alone.
pub fn inject_forge_mcp(
    descriptor: &AgentDescriptor,
    spec: &mut SpawnSpec,
    mcp: &ForgeMcp,
) -> bool {
    match descriptor.id.as_str() {
        "claude" => inject_claude(spec, mcp),
        "opencode" => inject_opencode(spec, mcp),
        _ => false,
    }
}

fn env_object(mcp: &ForgeMcp) -> serde_json::Map<String, serde_json::Value> {
    mcp.env
        .iter()
        .map(|(key, value)| (key.clone(), serde_json::Value::String(value.clone())))
        .collect()
}

/// `--mcp-config` is variadic, so it goes last: after the prompt, where no
/// positional argument follows it. A second `--mcp-config` adds to a profile's own.
fn inject_claude(spec: &mut SpawnSpec, mcp: &ForgeMcp) -> bool {
    let config = serde_json::json!({
        "mcpServers": { SERVER_NAME: {
            "type": "stdio",
            "command": mcp.command,
            "args": ["mcp"],
            "env": env_object(mcp),
        } }
    });
    spec.args.push("--mcp-config".to_owned());
    spec.args.push(config.to_string());
    true
}

fn inject_opencode(spec: &mut SpawnSpec, mcp: &ForgeMcp) -> bool {
    if spec
        .env
        .iter()
        .any(|(key, _)| key == OPENCODE_CONFIG_CONTENT)
    {
        tracing::info!("OPENCODE_CONFIG_CONTENT already set; leaving the Forge MCP server unset");
        return false;
    }
    let config = serde_json::json!({
        "$schema": "https://opencode.ai/config.json",
        "mcp": { SERVER_NAME: {
            "type": "local",
            "command": [mcp.command, "mcp"],
            "environment": env_object(mcp),
            "enabled": true,
        } }
    });
    spec.env
        .push((OPENCODE_CONFIG_CONTENT.to_owned(), config.to_string()));
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtin;

    fn spec(args: &[&str]) -> SpawnSpec {
        SpawnSpec {
            program: "agent".into(),
            args: args.iter().map(|arg| (*arg).to_owned()).collect(),
            cwd: "/w".into(),
            env: vec![],
        }
    }

    fn server() -> ForgeMcp {
        ForgeMcp {
            command: "/a path/forgectl".into(),
            env: vec![("FORGE_SESSION_ID".into(), "s1".into())],
        }
    }

    #[test]
    fn claude_gets_a_trailing_inline_config_with_the_session_identity() {
        let descriptor = builtin("claude").unwrap();
        let mut launch = spec(&["--settings", "/x.json"]);
        assert!(inject_forge_mcp(&descriptor, &mut launch, &server()));
        assert_eq!(launch.args[..3], ["--settings", "/x.json", "--mcp-config"]);
        assert_eq!(launch.args.len(), 4);
        let config: serde_json::Value = serde_json::from_str(&launch.args[3]).unwrap();
        let forge = &config["mcpServers"]["forge"];
        assert_eq!(forge["command"], "/a path/forgectl");
        assert_eq!(forge["args"], serde_json::json!(["mcp"]));
        assert_eq!(forge["env"]["FORGE_SESSION_ID"], "s1");
    }

    #[test]
    fn opencode_gets_a_config_overlay_unless_the_user_set_one() {
        let descriptor = builtin("opencode").unwrap();
        let mut launch = spec(&[]);
        assert!(inject_forge_mcp(&descriptor, &mut launch, &server()));
        let (_, value) = launch
            .env
            .iter()
            .find(|(key, _)| key == OPENCODE_CONFIG_CONTENT)
            .unwrap();
        let config: serde_json::Value = serde_json::from_str(value).unwrap();
        assert_eq!(
            config["mcp"]["forge"]["command"],
            serde_json::json!(["/a path/forgectl", "mcp"])
        );
        assert_eq!(
            config["mcp"]["forge"]["environment"]["FORGE_SESSION_ID"],
            "s1"
        );

        let mut owned = spec(&[]);
        owned
            .env
            .push((OPENCODE_CONFIG_CONTENT.into(), "{\"mine\":true}".into()));
        assert!(!inject_forge_mcp(&descriptor, &mut owned, &server()));
        assert_eq!(owned.env.len(), 1);
    }

    #[test]
    fn a_provider_without_a_launch_local_mcp_config_is_untouched() {
        let descriptor = builtin("cursor").unwrap();
        assert!(!accepts_forge_mcp(&descriptor));
        let mut launch = spec(&["--x"]);
        assert!(!inject_forge_mcp(&descriptor, &mut launch, &server()));
        assert_eq!(launch.args, ["--x"]);
        assert!(launch.env.is_empty());
    }
}
