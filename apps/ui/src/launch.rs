//! What Cmd-Shift-A starts: the `ui.default_agent` preference, read the way
//! Tauri's `defaultAgent.ts` reads it, so both clients launch the same thing.

use domain::{AgentProfileId, AgentProviderId};

pub const DEFAULT_AGENT_KEY: &str = "ui.default_agent";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DefaultAgent {
    Ask,
    Shell,
    Provider(String),
    Profile(String),
}

/// What the chord does once the preference meets what is installed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Launch {
    /// Open the palette scoped to launching.
    Ask,
    Shell,
    Agent {
        provider: AgentProviderId,
        profile: Option<AgentProfileId>,
    },
}

/// `shell`, `provider:<id>`, `profile:<uuid>`, or anything else as "ask".
#[must_use]
pub fn parse(value: Option<&str>) -> DefaultAgent {
    let trimmed = value.unwrap_or_default().trim();
    if trimmed == "shell" {
        return DefaultAgent::Shell;
    }
    if let Some(id) = trimmed
        .strip_prefix("provider:")
        .filter(|id| !id.is_empty())
    {
        return DefaultAgent::Provider(id.to_string());
    }
    if let Some(id) = trimmed.strip_prefix("profile:")
        && id.parse::<AgentProfileId>().is_ok()
    {
        return DefaultAgent::Profile(id.to_string());
    }
    DefaultAgent::Ask
}

/// Resolves the preference against what can launch. A preference that points
/// at something hidden, uninstalled or deleted is stale and asks, rather than
/// making the chord do nothing.
#[must_use]
pub fn resolve(
    agent: &DefaultAgent,
    visible: impl Fn(&str) -> bool,
    installed: &[AgentProviderId],
    profiles: &[(AgentProfileId, AgentProviderId)],
) -> Launch {
    match agent {
        DefaultAgent::Ask => Launch::Ask,
        DefaultAgent::Shell => Launch::Shell,
        DefaultAgent::Provider(id) => installed
            .iter()
            .find(|provider| provider.as_str() == id && visible(id))
            .map_or(Launch::Ask, |provider| Launch::Agent {
                provider: provider.clone(),
                profile: None,
            }),
        DefaultAgent::Profile(id) => profiles
            .iter()
            .find(|(profile, provider)| {
                profile.to_string() == *id
                    && installed.contains(provider)
                    && visible(&format!("profile:{id}"))
            })
            .map_or(Launch::Ask, |(profile, provider)| Launch::Agent {
                provider: provider.clone(),
                profile: Some(*profile),
            }),
    }
}

/// The `app_state` key that hides an agent from the launch lists.
#[must_use]
pub fn visibility_key(key: &str) -> String {
    format!("ui.agent_visible.{key}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_preference_reads_as_tauri_writes_it() {
        assert_eq!(parse(None), DefaultAgent::Ask);
        assert_eq!(parse(Some("shell")), DefaultAgent::Shell);
        assert_eq!(
            parse(Some("provider:claude")),
            DefaultAgent::Provider("claude".into())
        );
        assert_eq!(parse(Some("profile:not-a-uuid")), DefaultAgent::Ask);
        let id = AgentProfileId::new().to_string();
        assert_eq!(
            parse(Some(&format!("profile:{id}"))),
            DefaultAgent::Profile(id)
        );
    }

    #[test]
    fn a_stale_preference_asks_instead_of_doing_nothing() {
        let claude = AgentProviderId::new("claude");
        let installed = vec![claude.clone()];
        let all = |_: &str| true;
        assert_eq!(
            resolve(
                &DefaultAgent::Provider("claude".into()),
                all,
                &installed,
                &[]
            ),
            Launch::Agent {
                provider: claude,
                profile: None
            }
        );
        assert_eq!(
            resolve(&DefaultAgent::Provider("gone".into()), all, &installed, &[]),
            Launch::Ask
        );
        let hidden = |key: &str| key != "claude";
        assert_eq!(
            resolve(
                &DefaultAgent::Provider("claude".into()),
                hidden,
                &installed,
                &[]
            ),
            Launch::Ask
        );
    }
}
