//! What Cmd-Shift-A starts: the `ui.default_agent` preference.

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

/// The stored form `parse` reads back.
#[must_use]
pub fn value(agent: &DefaultAgent) -> String {
    match agent {
        DefaultAgent::Ask => "ask".into(),
        DefaultAgent::Shell => "shell".into(),
        DefaultAgent::Provider(id) => format!("provider:{id}"),
        DefaultAgent::Profile(id) => format!("profile:{id}"),
    }
}

/// The launch key `visibility_key` hides, for a provider or a profile.
#[must_use]
pub fn launch_key(agent: &DefaultAgent) -> Option<String> {
    match agent {
        DefaultAgent::Provider(id) => Some(id.clone()),
        DefaultAgent::Profile(id) => Some(format!("profile:{id}")),
        DefaultAgent::Ask | DefaultAgent::Shell => None,
    }
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

/// The agent last picked beside "Resolve with AI".
pub const RESOLVE_AGENT_KEY: &str = "ui.git.resolve_agent";

/// What can launch an agent: installed providers, their profiles, which
/// launch keys are shown, and which providers take a first prompt.
pub struct Launchables<'a, V: Fn(&str) -> bool, P: Fn(&AgentProviderId) -> bool> {
    pub visible: V,
    pub installed: &'a [AgentProviderId],
    pub profiles: &'a [(AgentProfileId, AgentProviderId)],
    pub takes_prompt: P,
}

/// The agent to hand a prompt to: a still-valid
/// pick, else the default (refused here if it cannot take a prompt), else the
/// only prompt-capable agent. Two candidates and no default is a guess this
/// refuses to make; the error is the sentence to show.
pub fn resolver<V: Fn(&str) -> bool, P: Fn(&AgentProviderId) -> bool>(
    preference: &DefaultAgent,
    picked: Option<&str>,
    can: &Launchables<'_, V, P>,
) -> Result<(AgentProviderId, Option<AgentProfileId>), String> {
    let capable = |provider: &AgentProviderId| {
        can.installed.contains(provider) && (can.takes_prompt)(provider)
    };
    if let Some(key) = picked {
        if let Some(provider) = can
            .installed
            .iter()
            .find(|provider| provider.as_str() == key && capable(provider) && (can.visible)(key))
        {
            return Ok((provider.clone(), None));
        }
        if let Some((profile, provider)) = can.profiles.iter().find(|(profile, provider)| {
            format!("profile:{profile}") == key && capable(provider) && (can.visible)(key)
        }) {
            return Ok((provider.clone(), Some(*profile)));
        }
    }
    if let Launch::Agent { provider, profile } =
        resolve(preference, &can.visible, can.installed, can.profiles)
    {
        if (can.takes_prompt)(&provider) {
            return Ok((provider, profile));
        }
        return Err(format!(
            "{provider} cannot be started with a prompt — pick another agent beside the button \
             or in Settings → Agents."
        ));
    }
    let candidates: Vec<&AgentProviderId> = can
        .installed
        .iter()
        .filter(|provider| capable(provider) && (can.visible)(provider.as_str()))
        .collect();
    match candidates.as_slice() {
        [only] => Ok(((*only).clone(), None)),
        [] => Err("No installed agent can be started with a prompt.".into()),
        _ => Err(
            "No default agent is set — pick one beside the button or in Settings → Agents.".into(),
        ),
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
    fn every_choice_round_trips_through_its_stored_form() {
        let profile = AgentProfileId::new().to_string();
        for agent in [
            DefaultAgent::Ask,
            DefaultAgent::Shell,
            DefaultAgent::Provider("claude".into()),
            DefaultAgent::Profile(profile.clone()),
        ] {
            assert_eq!(parse(Some(&value(&agent))), agent);
        }
        assert_eq!(
            launch_key(&DefaultAgent::Profile(profile.clone())),
            Some(format!("profile:{profile}"))
        );
    }

    #[test]
    fn the_preference_reads_as_the_original_client_writes_it() {
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

    mod resolver_for {
        use super::super::*;

        const PROFILE: &str = "6f1b0f52-0a1e-4a41-9d0c-2a2b7f9e1c33";

        fn id(name: &str) -> AgentProviderId {
            AgentProviderId::new(name)
        }

        fn run(
            preference: Option<&str>,
            picked: Option<&str>,
            installed: &[&str],
            prompt: &[&str],
            profiles: &[(&str, &str)],
        ) -> Result<(AgentProviderId, Option<AgentProfileId>), String> {
            let installed: Vec<AgentProviderId> = installed.iter().map(|name| id(name)).collect();
            let profiles: Vec<(AgentProfileId, AgentProviderId)> = profiles
                .iter()
                .map(|(profile, provider)| (profile.parse().expect("uuid"), id(provider)))
                .collect();
            let can = Launchables {
                visible: |_: &str| true,
                installed: &installed,
                profiles: &profiles,
                takes_prompt: |provider: &AgentProviderId| prompt.contains(&provider.as_str()),
            };
            resolver(&parse(preference), picked, &can)
        }

        #[test]
        fn the_default_agent_takes_the_prompt_or_is_refused_with_a_reason() {
            assert_eq!(
                run(Some("provider:claude"), None, &["claude"], &["claude"], &[]),
                Ok((id("claude"), None))
            );
            let refused = run(Some("provider:opencode"), None, &["opencode"], &[], &[]);
            assert!(refused.unwrap_err().contains("Settings → Agents"));
        }

        #[test]
        fn with_no_default_only_a_single_candidate_is_used() {
            assert_eq!(
                run(None, None, &["opencode", "claude"], &["claude"], &[]),
                Ok((id("claude"), None))
            );
            let two = run(
                None,
                None,
                &["claude", "cursor"],
                &["claude", "cursor"],
                &[],
            );
            assert!(two.unwrap_err().contains("No default agent is set"));
            assert_eq!(
                run(None, None, &["claude"], &["claude"], &[(PROFILE, "claude")]),
                Ok((id("claude"), None)),
                "a profile is not a second agent"
            );
            assert!(
                run(None, None, &[], &["claude"], &[]).is_err(),
                "never one not installed"
            );
        }

        #[test]
        fn the_default_carries_its_profile_and_wins_over_list_order() {
            let profile: AgentProfileId = PROFILE.parse().expect("uuid");
            assert_eq!(
                run(
                    Some(&format!("profile:{PROFILE}")),
                    None,
                    &["claude"],
                    &["claude"],
                    &[(PROFILE, "claude")]
                ),
                Ok((id("claude"), Some(profile)))
            );
            assert_eq!(
                run(
                    Some("provider:cursor"),
                    None,
                    &["claude", "cursor"],
                    &["claude", "cursor"],
                    &[]
                ),
                Ok((id("cursor"), None))
            );
            assert_eq!(
                run(
                    Some("provider:cursor"),
                    None,
                    &["claude"],
                    &["claude", "cursor"],
                    &[]
                ),
                Ok((id("claude"), None)),
                "an uninstalled default falls back"
            );
        }

        #[test]
        fn a_pick_wins_while_it_can_still_take_the_prompt() {
            let both = ["claude", "cursor"];
            assert_eq!(
                run(Some("provider:claude"), Some("cursor"), &both, &both, &[]),
                Ok((id("cursor"), None))
            );
            assert_eq!(
                run(
                    Some("provider:claude"),
                    Some("opencode"),
                    &["claude", "opencode"],
                    &["claude"],
                    &[]
                ),
                Ok((id("claude"), None)),
                "a pick that cannot take a prompt falls through"
            );
        }
    }
}
