//! Built-in provider launch and detection declarations.
//!
//! Descriptors carry only static data (candidate binaries, version probe,
//! capabilities); all behavior lives in [`crate::detection`] and
//! [`crate::descriptor`]. Every provider-specific fact of the MVP is encoded
//! here (principle P2): nothing else in the workspace branches on provider id.

use domain::{
    AcpSpec, AgentCapabilities, AgentDescriptor, AgentProviderId, ConfigDirSpec, PromptStyle,
    ResumeStyle, ReviewStyle, UsageSource, VersionProbe,
};

/// `claude --resume <session-id>`: the id its transcripts record, resumed from
/// the directory the run happened in.
fn claude_resume() -> Option<ResumeStyle> {
    Some(ResumeStyle::Flag {
        flag: "--resume".to_owned(),
    })
}

/// `codex resume <session-id>` — a subcommand, not a flag, so it leads the
/// command line.
fn codex_resume() -> Option<ResumeStyle> {
    Some(ResumeStyle::Subcommand {
        command: "resume".to_owned(),
    })
}

/// `opencode --session <id>`: `--continue` picks the last one, `--session`
/// names it.
fn opencode_resume() -> Option<ResumeStyle> {
    Some(ResumeStyle::Flag {
        flag: "--session".to_owned(),
    })
}

/// Claude Code as an ACP agent. Grok's usage path is the one that spawns an
/// ACP entry today; Claude's is declared the same way so the subcommand lives
/// in one place.
fn claude_acp() -> Option<AcpSpec> {
    Some(AcpSpec {
        args: vec!["acp".to_owned()],
    })
}

///
/// Each was read off the CLI's own `--help`, and each is the provider's own
/// answer to "look but do not touch" rather than a sandbox Forge imposes:
/// Claude's plan permission mode, Codex's read-only sandbox policy, the `plan`
/// agent OpenCode ships, and Cursor's ask mode. An automatic pull-request
/// review starts in a checkout the user is working in, so a provider that
/// could not be put in one of these would not be offered at all.
fn review(label: &str, args: &[&str]) -> Option<ReviewStyle> {
    Some(ReviewStyle {
        args: args.iter().map(|arg| (*arg).to_owned()).collect(),
        label: label.to_owned(),
    })
}

fn grok_resume() -> Option<ResumeStyle> {
    Some(ResumeStyle::Flag {
        flag: "--resume".to_owned(),
    })
}

/// Grok as an ACP agent: `grok agent stdio`. Usage reads billing over this
/// entry rather than spelling the subcommand again.
fn grok_acp() -> Option<AcpSpec> {
    Some(AcpSpec {
        args: vec!["agent".to_owned(), "stdio".to_owned()],
    })
}

fn grok_config_dir() -> Option<ConfigDirSpec> {
    Some(ConfigDirSpec {
        vars: vec!["GROK_HOME".to_owned()],
        help: "A separate account: its own config, credentials and sessions.".to_owned(),
    })
}

/// Returns descriptors in picker order.
#[must_use]
pub fn builtins() -> Vec<AgentDescriptor> {
    vec![
        // Claude and Codex read their existing local OAuth credentials to report
        // usage, and Grok answers the same question over its ACP entry.
        // OpenCode bills per model provider and Cursor exposes no local endpoint
        // of any kind, so neither declares a usage source.
        descriptor(
            "claude",
            "Claude Code",
            &["claude"],
            None,
            Some(UsageSource::ClaudeOauth),
            claude_config_dir(),
            claude_resume(),
            // `claude [options] [command] [prompt]` — its own usage line.
            Some(PromptStyle::Positional),
            claude_acp(),
            // `--permission-mode plan`: reads and runs read-only tools, and
            // asks before anything that would write.
            review("plan mode", &["--permission-mode", "plan"]),
        ),
        descriptor(
            "codex",
            "Codex CLI",
            &["codex"],
            None,
            Some(UsageSource::CodexOAuth),
            codex_config_dir(),
            codex_resume(),
            // `codex [OPTIONS] [PROMPT]`, forwarded to the interactive CLI
            // when no subcommand is given.
            Some(PromptStyle::Positional),
            None,
            // `-s read-only` is the sandbox policy applied to every command
            // the model runs, not a prompt it can talk its way past.
            review("read-only sandbox", &["-s", "read-only"]),
        ),
        descriptor(
            "opencode",
            "OpenCode",
            &["opencode", "opencode2"],
            None,
            None,
            opencode_config_dir(),
            opencode_resume(),
            // A flag, and this is the one that would have been a bug to
            // guess: `opencode [project]` reads its positional as a
            // *directory*, so a prompt passed there is taken for a path.
            Some(PromptStyle::Flag {
                flag: "--prompt".to_owned(),
            }),
            None,
            // OpenCode ships a `plan` agent whose tools cannot write; naming
            // it is how a launch asks for one.
            review("plan agent", &["--agent", "plan"]),
        ),
        descriptor(
            "cursor",
            "Cursor CLI",
            // `agent` is preferred but ambiguous — on many machines it is the
            // Grok CLI — so the `cursor` marker below is what keeps it honest.
            &["agent", "cursor-agent"],
            Some("cursor"),
            None,
            // Cursor CLI documents no directory of its own, so a profile for it
            // can change the binary and the arguments and nothing else.
            None,
            // `--resume` takes an *optional* chat id, so a following argument
            // is ambiguous to its parser. Nothing discovers Cursor history
            // either (`external_agents` reads Claude and opencode), so there is
            // no id to hand it and no reason to guess at the spelling.
            None,
            // `agent [options] [command] [prompt...]`, documented as "Initial
            // prompt for the agent".
            Some(PromptStyle::Positional),
            None,
            // `--mode ask` is its own Q&A posture: explanations, no edits.
            review("ask mode", &["--mode", "ask"]),
        ),
        descriptor(
            "grok",
            "Grok",
            // Unambiguous `grok` only — `agent` on PATH is often this CLI, but
            // the cursor descriptor already claims that name with a marker.
            &["grok"],
            Some("grok"),
            // Grok publishes no usage URL; `usage::grok` asks its ACP entry
            // instead, which is why this is the one source that needs the
            // binary rather than a credentials file.
            Some(UsageSource::GrokAcp),
            grok_config_dir(),
            grok_resume(),
            Some(PromptStyle::Positional),
            grok_acp(),
            review("plan mode", &["--permission-mode", "plan"]),
        ),
    ]
}

/// Look up a single built-in descriptor by its provider id (e.g. `"cursor"`).
#[must_use]
pub fn builtin(id: &str) -> Option<AgentDescriptor> {
    builtins().into_iter().find(|d| d.id.as_str() == id)
}

///
/// `CLAUDE_CONFIG_DIR` is what a hand-written `claude-work` wrapper exported:
/// settings, login, history and plugins move there instead of `~/.claude`.
fn claude_config_dir() -> Option<ConfigDirSpec> {
    Some(ConfigDirSpec {
        vars: vec!["CLAUDE_CONFIG_DIR".to_owned()],
        help: "A separate account: its own login, settings, history and plugins.".to_owned(),
    })
}

/// `CODEX_HOME` selects Codex's account;
/// `usage::codex` already reads credentials through it.
fn codex_config_dir() -> Option<ConfigDirSpec> {
    Some(ConfigDirSpec {
        vars: vec!["CODEX_HOME".to_owned()],
        help: "A separate account: its own credentials and history.".to_owned(),
    })
}

/// OpenCode needs two variables to switch both config and credentials.
///
/// OpenCode splits what the other two keep together: `OPENCODE_CONFIG_DIR`
/// moves `opencode.json` and the agents, commands and plugins beside it, while
/// the credentials and session storage live under `<XDG_DATA_HOME>/opencode`.
/// Pointing both at the profile's directory is what makes it one account and
/// not half of one. `XDG_DATA_HOME` is a generic variable, so the help text
/// says so: every process OpenCode itself starts inherits it.
fn opencode_config_dir() -> Option<ConfigDirSpec> {
    Some(ConfigDirSpec {
        vars: vec!["OPENCODE_CONFIG_DIR".to_owned(), "XDG_DATA_HOME".to_owned()],
        help: "A separate account: its own opencode.json, credentials and sessions. \
               Also sets the standard XDG_DATA_HOME, which anything OpenCode launches sees."
            .to_owned(),
    })
}

/// Construct a built-in descriptor. Built-ins probe with `--version`
/// and take no default args; only the id, display name, candidate list,
/// expected marker, config directory, resume, prompt and read-only spellings
/// differ.
#[allow(clippy::too_many_arguments)]
fn descriptor(
    id: &str,
    display_name: &str,
    binary_candidates: &[&str],
    expect_substring: Option<&str>,
    usage_source: Option<UsageSource>,
    config_dir: Option<ConfigDirSpec>,
    resume: Option<ResumeStyle>,
    prompt: Option<PromptStyle>,
    acp: Option<AcpSpec>,
    review: Option<ReviewStyle>,
) -> AgentDescriptor {
    let supports_resume = resume.is_some();
    let supports_initial_prompt = prompt.is_some();
    let supports_review = review.is_some();
    AgentDescriptor {
        id: AgentProviderId::new(id),
        display_name: display_name.to_owned(),
        binary_candidates: binary_candidates.iter().map(|s| (*s).to_owned()).collect(),
        default_args: Vec::new(),
        version_probe: VersionProbe {
            args: vec!["--version".to_owned()],
            expect_substring: expect_substring.map(str::to_owned),
            timeout_ms: 3000,
        },
        usage_source,
        config_dir,
        resume,
        prompt,
        acp,
        review,
        capabilities: AgentCapabilities {
            // Built-ins are interactive TUIs. Neither flag below is a
            // second source of truth: each restates whether the matching
            // spelling was declared above.
            interactive_tui: true,
            supports_initial_prompt,
            supports_resume,
            supports_review,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each spelling was read off the CLI's own `--help`, and OpenCode is why
    /// the flag form exists: `opencode [project]` takes a *directory* in the
    /// positional, so its prompt has to be named.
    #[test]
    fn every_builtin_declares_how_it_takes_a_prompt() {
        for id in ["claude", "codex", "cursor", "grok"] {
            let descriptor = builtin(id).unwrap();
            assert_eq!(
                descriptor.prompt,
                Some(PromptStyle::Positional),
                "{id} should take a positional prompt"
            );
            assert!(descriptor.capabilities.supports_initial_prompt);
        }

        let opencode = builtin("opencode").unwrap();
        assert_eq!(
            opencode.prompt,
            Some(PromptStyle::Flag {
                flag: "--prompt".to_owned()
            })
        );
        assert!(opencode.capabilities.supports_initial_prompt);
    }

    /// A pull-request review starts in a checkout the user is working in, so
    /// a provider with no read-only posture would be one that could edit it.
    #[test]
    fn every_builtin_declares_a_read_only_mode() {
        for descriptor in builtins() {
            let style = descriptor
                .review
                .as_ref()
                .unwrap_or_else(|| panic!("{} declares no read-only mode", descriptor.id));
            assert!(
                !style.args.is_empty(),
                "{} declares no flags",
                descriptor.id
            );
            assert!(!style.label.is_empty(), "{} names no mode", descriptor.id);
            assert!(descriptor.capabilities.supports_review);
        }
    }

    #[test]
    fn there_are_five_builtins_in_order() {
        let ids: Vec<String> = builtins().iter().map(|d| d.id.to_string()).collect();
        assert_eq!(ids, ["claude", "codex", "opencode", "cursor", "grok"]);
    }

    #[test]
    fn every_builtin_probes_version_with_a_timeout() {
        for d in builtins() {
            assert_eq!(d.version_probe.args, ["--version"]);
            assert_eq!(d.version_probe.timeout_ms, 3000);
            assert!(d.capabilities.interactive_tui);
            assert!(d.default_args.is_empty());
        }
    }

    /// The capability flag is a restatement of the resume spelling, never a
    /// second opinion about it: the GUI reads the flag to decide whether a
    /// history card can be clicked, and the launch builder reads the spelling.
    #[test]
    fn the_resume_capability_matches_the_declared_spelling() {
        for d in builtins() {
            assert_eq!(
                d.capabilities.supports_resume,
                d.resume.is_some(),
                "{}",
                d.id
            );
        }
    }

    #[test]
    fn claude_and_grok_declare_their_acp_entry() {
        assert_eq!(
            builtin("claude")
                .unwrap()
                .acp
                .expect("claude speaks ACP")
                .args,
            ["acp"]
        );
        assert_eq!(
            builtin("grok").unwrap().acp.expect("grok speaks ACP").args,
            ["agent", "stdio"]
        );
        for id in ["codex", "opencode", "cursor"] {
            assert!(builtin(id).unwrap().acp.is_none(), "{id}");
        }
    }

    /// The exact command line each provider re-enters a session with; these are
    /// the four facts this file exists to hold.
    #[test]
    fn each_provider_resumes_the_way_its_cli_spells_it() {
        let args = |id: &str| builtin(id).unwrap().resume.map(|r| r.args("s-1"));
        assert_eq!(
            args("claude"),
            Some(vec!["--resume".to_owned(), "s-1".to_owned()])
        );
        assert_eq!(
            args("codex"),
            Some(vec!["resume".to_owned(), "s-1".to_owned()])
        );
        assert_eq!(
            args("opencode"),
            Some(vec!["--session".to_owned(), "s-1".to_owned()])
        );
        assert_eq!(args("cursor"), None);
    }

    #[test]
    fn cursor_expects_a_marker_and_lists_both_candidates() {
        let cursor = builtin("cursor").unwrap();
        assert_eq!(cursor.binary_candidates, ["agent", "cursor-agent"]);
        assert_eq!(
            cursor.version_probe.expect_substring.as_deref(),
            Some("cursor")
        );
    }

    #[test]
    fn opencode_has_a_lower_priority_secondary_candidate() {
        let oc = builtin("opencode").unwrap();
        assert_eq!(oc.binary_candidates, ["opencode", "opencode2"]);
    }

    #[test]
    fn providers_without_a_marker_do_not_expect_one() {
        for id in ["claude", "codex", "opencode"] {
            assert!(builtin(id)
                .unwrap()
                .version_probe
                .expect_substring
                .is_none());
        }
        assert_eq!(
            builtin("grok")
                .unwrap()
                .version_probe
                .expect_substring
                .as_deref(),
            Some("grok")
        );
    }

    #[test]
    fn every_provider_but_cursor_can_move_its_config_directory() {
        for id in ["claude", "codex", "opencode", "grok"] {
            let spec = builtin(id).unwrap().config_dir.expect(id);
            assert!(!spec.vars.is_empty(), "{id}");
            assert!(!spec.help.trim().is_empty(), "{id}");
        }
        // Cursor CLI documents no such switch, so its profiles change the
        // binary and the arguments only.
        assert!(builtin("cursor").unwrap().config_dir.is_none());
    }

    /// OpenCode splits configuration from credentials, so one profile directory
    /// has to arrive as two variables or the account is only half switched.
    #[test]
    fn opencode_points_both_of_its_directories_at_one_place() {
        let spec = builtin("opencode").unwrap().config_dir.unwrap();
        assert_eq!(spec.vars, ["OPENCODE_CONFIG_DIR", "XDG_DATA_HOME"]);
    }

    /// A config-directory variable must never collide with the terminal
    /// contract: the launch would break the emulator instead of
    /// switching accounts.
    #[test]
    fn no_config_directory_variable_is_reserved() {
        use domain::RESERVED_PROFILE_VARS;
        for descriptor in builtins() {
            for var in descriptor.config_dir.iter().flat_map(|spec| &spec.vars) {
                assert!(
                    !RESERVED_PROFILE_VARS.contains(&var.as_str()),
                    "{} points at reserved {var}",
                    descriptor.id
                );
            }
        }
    }

    #[test]
    fn unknown_builtin_is_none() {
        assert!(builtin("nope").is_none());
    }

    #[test]
    fn three_providers_declare_a_usage_source_each_of_its_own_kind() {
        use domain::UsageSource;
        assert!(matches!(
            builtin("claude").unwrap().usage_source,
            Some(UsageSource::ClaudeOauth)
        ));
        assert!(matches!(
            builtin("codex").unwrap().usage_source,
            Some(UsageSource::CodexOAuth)
        ));
        assert!(matches!(
            builtin("grok").unwrap().usage_source,
            Some(UsageSource::GrokAcp)
        ));
        // OpenCode bills per model provider; Cursor exposes nothing local.
        assert!(builtin("opencode").unwrap().usage_source.is_none());
        assert!(builtin("cursor").unwrap().usage_source.is_none());
    }

    /// `usage::grok` spawns [`AcpSpec::args`] rather than spelling
    /// `agent stdio` a second time, so a provider reading its meter over ACP
    /// has to declare that entry or the reading has nothing to spawn.
    #[test]
    fn an_acp_usage_source_comes_with_the_acp_entry_it_spawns() {
        use domain::UsageSource;
        for descriptor in builtins() {
            if matches!(descriptor.usage_source, Some(UsageSource::GrokAcp)) {
                assert!(
                    descriptor.acp.is_some(),
                    "{} reads usage over ACP but declares no ACP entry",
                    descriptor.id
                );
            }
        }
    }
}
