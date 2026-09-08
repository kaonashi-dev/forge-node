//! The four MVP built-in agent descriptors (§7.5 table).
//!
//! Descriptors carry only static data (candidate binaries, version probe,
//! capabilities); all behavior lives in [`crate::detection`] and
//! [`crate::descriptor`]. Every provider-specific fact of the MVP is encoded
//! here (principle P2): nothing else in the workspace branches on provider id.

use domain::{
    AgentCapabilities, AgentDescriptor, AgentProviderId, HeadlessSpec, ProfileField,
    ProfileFieldEffect, PromptStyle, ResumeStyle, ReviewStyle, SchemaStyle, UsageSource,
    VersionProbe,
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

/// `claude -p`, streaming its events as JSON Lines.
///
/// `--verbose` is not optional decoration: `--output-format stream-json`
/// refuses to run in print mode without it. `--bare` is deliberately absent —
/// it would make the run skip the OAuth credentials and demand an API key,
/// which is the one thing this must not do.
fn claude_headless() -> Option<HeadlessSpec> {
    Some(HeadlessSpec {
        mode_args: vec!["-p".to_owned()],
        stream_args: vec![
            "--output-format".to_owned(),
            "stream-json".to_owned(),
            "--verbose".to_owned(),
        ],
        resume: claude_resume(),
        prompt: PromptStyle::Positional,
        // Takes the schema document itself on the command line.
        schema: Some(SchemaStyle::Inline {
            flag: "--json-schema".to_owned(),
        }),
        session_id_fields: vec!["session_id".to_owned()],
        // Claude does not sandbox file writes to its cwd.
        extra_writable_dir_flag: None,
    })
}

/// `codex exec --json`, and `codex exec resume <id>` to continue one.
///
/// The resume spelling is the same subcommand the interactive CLI uses, but it
/// belongs *after* `exec`, which is exactly what
/// [`HeadlessSpec::command_args`] does with it.
///
/// `model_reasoning_summary=detailed` is the second half of `--json` and not a
/// preference: without it the stream carries the agent's *actions* but not one
/// word of why, even on a turn that spent a thousand reasoning tokens
/// (`turn.completed` says so). With it, Codex emits `reasoning` items and a
/// person watching a step can follow the thinking rather than guess at it. It
/// changes what is reported, never what the model does, and it applies only to
/// harness runs — an interactive Codex still reads the user's own config.
fn codex_headless() -> Option<HeadlessSpec> {
    Some(HeadlessSpec {
        mode_args: vec!["exec".to_owned()],
        stream_args: vec![
            "--json".to_owned(),
            "-c".to_owned(),
            "model_reasoning_summary=detailed".to_owned(),
        ],
        resume: codex_resume(),
        prompt: PromptStyle::Positional,
        // Takes a *path* to the schema, not the schema.
        schema: Some(SchemaStyle::File {
            flag: "--output-schema".to_owned(),
        }),
        // Codex names it in its session events; `session_id` is the spelling
        // both CLIs share, `id` the one its older lines use.
        session_id_fields: vec!["session_id".to_owned(), "conversation_id".to_owned()],
        // Its sandbox holds file writes inside the cwd, which for a harness
        // step is a worktree — while the artefacts belong to the repository's
        // shared harness state outside it.
        extra_writable_dir_flag: Some("--add-dir".to_owned()),
    })
}

/// `opencode run <message>` — non-interactive, but it streams plain text
/// rather than events, so nothing can be read back out of it beyond the exit
/// code. Declared anyway: an exit code is already more than a PTY gives.
fn opencode_headless() -> Option<HeadlessSpec> {
    Some(HeadlessSpec {
        mode_args: vec!["run".to_owned()],
        stream_args: Vec::new(),
        resume: opencode_resume(),
        prompt: PromptStyle::Positional,
        schema: None,
        session_id_fields: Vec::new(),
        // Plain-text runner with no sandbox to widen.
        extra_writable_dir_flag: None,
    })
}

/// The read-only postures, one per built-in (§16.9).
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

/// Every built-in provider descriptor, in picker order (§7.5, §13.2).
#[must_use]
pub fn builtins() -> Vec<AgentDescriptor> {
    vec![
        // Claude and Codex read their existing local OAuth credentials to report
        // usage (§16.2). OpenCode bills per model provider and Cursor exposes no
        // local endpoint, so neither declares a usage source.
        descriptor(
            "claude",
            "Claude Code",
            &["claude"],
            None,
            Some(UsageSource::ClaudeOauth),
            claude_profile_fields(),
            claude_resume(),
            // `claude [options] [command] [prompt]` — its own usage line.
            Some(PromptStyle::Positional),
            claude_headless(),
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
            codex_profile_fields(),
            codex_resume(),
            // `codex [OPTIONS] [PROMPT]`, forwarded to the interactive CLI
            // when no subcommand is given.
            Some(PromptStyle::Positional),
            codex_headless(),
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
            opencode_profile_fields(),
            opencode_resume(),
            // A flag, and this is the one that would have been a bug to
            // guess: `opencode [project]` reads its positional as a
            // *directory*, so a prompt passed there is taken for a path.
            Some(PromptStyle::Flag {
                flag: "--prompt".to_owned(),
            }),
            opencode_headless(),
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
            Vec::new(),
            // `--resume` takes an *optional* chat id, so a following argument
            // is ambiguous to its parser. Nothing discovers Cursor history
            // either (`external_agents` reads Claude and opencode), so there is
            // no id to hand it and no reason to guess at the spelling.
            None,
            // `agent [options] [command] [prompt...]`, documented as "Initial
            // prompt for the agent".
            Some(PromptStyle::Positional),
            // Its non-interactive form takes `--print`, but nothing here has
            // read its event stream, and guessing one is how a job hangs
            // waiting for a session id that never arrives.
            None,
            // `--mode ask` is its own Q&A posture: explanations, no edits.
            review("ask mode", &["--mode", "ask"]),
        ),
    ]
}

/// Look up a single built-in descriptor by its provider id (e.g. `"cursor"`).
#[must_use]
pub fn builtin(id: &str) -> Option<AgentDescriptor> {
    builtins().into_iter().find(|d| d.id.as_str() == id)
}

/// The profile fields Claude Code understands (§13.4).
///
/// `CLAUDE_CONFIG_DIR` is the account switch: Claude Code keeps settings,
/// session history and plugins there instead of `~/.claude`, which is exactly
/// what a hand-written `claude-work` shell wrapper does. `--model` is the flag
/// that outranks both the `model` setting and `ANTHROPIC_MODEL`.
fn claude_profile_fields() -> Vec<ProfileField> {
    vec![
        ProfileField {
            label: "Config directory".to_owned(),
            help: "A separate account: its own login, settings, history and plugins.".to_owned(),
            effect: ProfileFieldEffect::Env {
                name: "CLAUDE_CONFIG_DIR".to_owned(),
                is_directory: true,
            },
        },
        ProfileField {
            label: "Default model".to_owned(),
            help: "An alias (opus, sonnet, haiku) or a full model id.".to_owned(),
            effect: ProfileFieldEffect::Flag {
                flag: "--model".to_owned(),
            },
        },
        ProfileField {
            label: "Default agent".to_owned(),
            help: "The Claude Code subagent to start with.".to_owned(),
            effect: ProfileFieldEffect::Flag {
                flag: "--agent".to_owned(),
            },
        },
    ]
}

/// The profile fields Codex CLI understands (§13.4). `CODEX_HOME` is its
/// `CLAUDE_CONFIG_DIR`; `usage::codex` already reads credentials through it.
fn codex_profile_fields() -> Vec<ProfileField> {
    vec![
        ProfileField {
            label: "Config directory".to_owned(),
            help: "A separate account: its own credentials and history.".to_owned(),
            effect: ProfileFieldEffect::Env {
                name: "CODEX_HOME".to_owned(),
                is_directory: true,
            },
        },
        ProfileField {
            label: "Default model".to_owned(),
            help: "The model Codex starts on.".to_owned(),
            effect: ProfileFieldEffect::Flag {
                flag: "--model".to_owned(),
            },
        },
        ProfileField {
            label: "Config profile".to_owned(),
            help: "A named profile from the Codex config file.".to_owned(),
            effect: ProfileFieldEffect::Flag {
                flag: "--profile".to_owned(),
            },
        },
    ]
}

/// The profile fields OpenCode understands (§13.4).
///
/// OpenCode splits what the other two keep together, so it gets two
/// directories instead of one. `OPENCODE_CONFIG_DIR` moves the directory
/// `opencode.json` is read from — with it the agents, commands and plugins
/// defined beside it — but leaves credentials where they were.
/// `XDG_DATA_HOME` is the one that switches accounts: `auth.json` and the
/// session storage live under `<data>/opencode`. It is a generic variable
/// rather than an OpenCode one, so the help text says so — every process
/// OpenCode itself starts inherits it.
///
/// `--model` takes a `provider/model` pair, not a bare alias, and `--agent`
/// picks which of the configured agents the TUI starts on.
fn opencode_profile_fields() -> Vec<ProfileField> {
    vec![
        ProfileField {
            label: "Config directory".to_owned(),
            help: "Its own opencode.json, and the agents, commands and plugins beside it."
                .to_owned(),
            effect: ProfileFieldEffect::Env {
                name: "OPENCODE_CONFIG_DIR".to_owned(),
                is_directory: true,
            },
        },
        ProfileField {
            label: "Data directory".to_owned(),
            help: "A separate account: its own credentials and session storage. \
                   Standard XDG variable — anything OpenCode launches sees it too."
                .to_owned(),
            effect: ProfileFieldEffect::Env {
                name: "XDG_DATA_HOME".to_owned(),
                is_directory: true,
            },
        },
        ProfileField {
            label: "Default model".to_owned(),
            help: "A provider/model pair, such as anthropic/claude-sonnet-4-5.".to_owned(),
            effect: ProfileFieldEffect::Flag {
                flag: "--model".to_owned(),
            },
        },
        ProfileField {
            label: "Default agent".to_owned(),
            help: "The configured agent OpenCode starts on.".to_owned(),
            effect: ProfileFieldEffect::Flag {
                flag: "--agent".to_owned(),
            },
        },
    ]
}

/// Construct a built-in descriptor. All four built-ins probe with `--version`
/// and take no default args; only the id, display name, candidate list,
/// expected marker, profile fields, resume, prompt and read-only spellings differ
/// (§7.5, §13.4, §16.8, §16.9).
#[allow(clippy::too_many_arguments)]
fn descriptor(
    id: &str,
    display_name: &str,
    binary_candidates: &[&str],
    expect_substring: Option<&str>,
    usage_source: Option<UsageSource>,
    profile_fields: Vec<ProfileField>,
    resume: Option<ResumeStyle>,
    prompt: Option<PromptStyle>,
    headless: Option<HeadlessSpec>,
    review: Option<ReviewStyle>,
) -> AgentDescriptor {
    let supports_resume = resume.is_some();
    let supports_initial_prompt = prompt.is_some();
    let supports_headless = headless.is_some();
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
        profile_fields,
        resume,
        prompt,
        headless,
        review,
        capabilities: AgentCapabilities {
            // All four built-ins are interactive TUIs. Neither flag below is a
            // second source of truth: each restates whether the matching
            // spelling was declared above.
            interactive_tui: true,
            supports_initial_prompt,
            supports_resume,
            supports_headless,
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
        for id in ["claude", "codex", "cursor"] {
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
    fn there_are_four_builtins_in_order() {
        let ids: Vec<String> = builtins().iter().map(|d| d.id.to_string()).collect();
        assert_eq!(ids, ["claude", "codex", "opencode", "cursor"]);
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

    /// The exact command line a headless job runs, fresh and resumed.
    ///
    /// Written out in full because the order is the part that breaks: `codex`
    /// takes its resume as a subcommand *after* `exec`, `claude` as a flag,
    /// and both take the prompt last.
    #[test]
    fn each_provider_runs_a_job_the_way_its_cli_spells_it() {
        let headless = |id: &str| builtin(id).unwrap().headless;

        let claude = headless("claude").unwrap();
        assert_eq!(
            claude.command_args("do the thing", None),
            [
                "-p",
                "--output-format",
                "stream-json",
                "--verbose",
                "do the thing"
            ]
        );
        assert_eq!(
            claude.command_args("and then this", Some("sess-1")),
            [
                "-p",
                "--resume",
                "sess-1",
                "--output-format",
                "stream-json",
                "--verbose",
                "and then this"
            ]
        );

        let codex = headless("codex").unwrap();
        assert_eq!(
            codex.command_args("do the thing", None),
            [
                "exec",
                "--json",
                "-c",
                "model_reasoning_summary=detailed",
                "do the thing"
            ]
        );
        assert_eq!(
            codex.command_args("and then this", Some("sess-2")),
            [
                "exec",
                "resume",
                "sess-2",
                "--json",
                "-c",
                "model_reasoning_summary=detailed",
                "and then this"
            ]
        );

        // A schema turns a verdict into a value rather than a line to grep.
        assert_eq!(
            claude.command_args_with("review it", None, Some(r#"{"type":"object"}"#)),
            [
                "-p",
                "--output-format",
                "stream-json",
                "--verbose",
                "--json-schema",
                r#"{"type":"object"}"#,
                "review it"
            ]
        );
        assert_eq!(
            codex.command_args_with("review it", None, Some("/tmp/verdict.json")),
            [
                "exec",
                "--json",
                "-c",
                "model_reasoning_summary=detailed",
                "--output-schema",
                "/tmp/verdict.json",
                "review it"
            ]
        );
        // Declared for none of them: an API key. A job is the same binary,
        // started the same way, on the same subscription login.
        for descriptor in builtins() {
            for arg in descriptor
                .headless
                .iter()
                .flat_map(|h| h.command_args("p", None))
            {
                assert_ne!(arg, "--bare", "{} must keep its OAuth login", descriptor.id);
            }
        }
    }

    /// Only the provider that sandboxes writes out of its cwd names an extra
    /// directory flag, and it lands before the positional prompt.
    #[test]
    fn only_a_sandboxed_provider_takes_an_extra_writable_dir() {
        use std::path::PathBuf;

        let dirs = vec![PathBuf::from("/repo/harness")];
        let codex = builtin("codex").unwrap().headless.unwrap();
        assert_eq!(
            codex.command_args_with_dirs("do the thing", None, None, &dirs),
            [
                "exec",
                "--json",
                "-c",
                "model_reasoning_summary=detailed",
                "--add-dir",
                "/repo/harness",
                "do the thing"
            ]
        );

        let claude = builtin("claude").unwrap().headless.unwrap();
        assert_eq!(
            claude.command_args_with_dirs("do the thing", None, None, &dirs),
            [
                "-p",
                "--output-format",
                "stream-json",
                "--verbose",
                "do the thing"
            ]
        );
    }

    /// The capability flag restates the spelling, as it does for prompt and
    /// resume — never a second source of truth.
    #[test]
    fn the_headless_capability_matches_the_declared_spelling() {
        for d in builtins() {
            assert_eq!(
                d.capabilities.supports_headless,
                d.headless.is_some(),
                "{} disagrees with itself about headless runs",
                d.id
            );
        }
        assert!(builtin("cursor").unwrap().headless.is_none());
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
    }

    #[test]
    fn every_provider_with_profile_fields_leads_with_a_directory() {
        use domain::ProfileFieldEffect;
        for id in ["claude", "codex", "opencode"] {
            let fields = builtin(id).unwrap().profile_fields;
            assert!(!fields.is_empty(), "{id}");
            // The first field is a directory — that is what makes the daemon
            // create it before launching.
            assert!(
                matches!(
                    &fields[0].effect,
                    ProfileFieldEffect::Env {
                        is_directory: true,
                        ..
                    }
                ),
                "{id}"
            );
        }
        // Cursor exposes no documented configuration switch, so it offers
        // nothing to suggest; a profile for it is still writable by hand.
        assert!(builtin("cursor").unwrap().profile_fields.is_empty());
    }

    /// OpenCode splits configuration from credentials, so its account switch is
    /// the data directory and not the one named after it.
    #[test]
    fn opencode_offers_both_of_its_directories_and_its_two_flags() {
        use domain::ProfileFieldEffect;
        let fields = builtin("opencode").unwrap().profile_fields;
        let effects: Vec<&ProfileFieldEffect> = fields.iter().map(|f| &f.effect).collect();
        assert!(matches!(
            effects.as_slice(),
            [
                ProfileFieldEffect::Env {
                    name: config,
                    is_directory: true
                },
                ProfileFieldEffect::Env {
                    name: data,
                    is_directory: true
                },
                ProfileFieldEffect::Flag { flag: model },
                ProfileFieldEffect::Flag { flag: agent },
            ] if config == "OPENCODE_CONFIG_DIR"
                && data == "XDG_DATA_HOME"
                && model == "--model"
                && agent == "--agent"
        ));
    }

    #[test]
    fn provider_native_agents_and_profiles_are_direct_fields() {
        use domain::ProfileFieldEffect;

        for (provider, expected) in [("claude", "--agent"), ("codex", "--profile")] {
            assert!(builtin(provider)
                .unwrap()
                .profile_fields
                .iter()
                .any(|field| {
                    matches!(&field.effect, ProfileFieldEffect::Flag { flag } if flag == expected)
                }));
        }
    }

    /// A profile field must never collide with the terminal contract (§13.3):
    /// a suggestion the daemon would then refuse is worse than no suggestion.
    #[test]
    fn no_suggested_variable_is_reserved() {
        use domain::{ProfileFieldEffect, RESERVED_PROFILE_VARS};
        for descriptor in builtins() {
            for field in &descriptor.profile_fields {
                if let ProfileFieldEffect::Env { name, .. } = &field.effect {
                    assert!(
                        !RESERVED_PROFILE_VARS.contains(&name.as_str()),
                        "{} suggests reserved {name}",
                        descriptor.id
                    );
                }
            }
        }
    }

    #[test]
    fn unknown_builtin_is_none() {
        assert!(builtin("nope").is_none());
    }

    #[test]
    fn only_claude_and_codex_declare_a_usage_source() {
        use domain::UsageSource;
        assert!(matches!(
            builtin("claude").unwrap().usage_source,
            Some(UsageSource::ClaudeOauth)
        ));
        assert!(matches!(
            builtin("codex").unwrap().usage_source,
            Some(UsageSource::CodexOAuth)
        ));
        assert!(builtin("opencode").unwrap().usage_source.is_none());
        assert!(builtin("cursor").unwrap().usage_source.is_none());
    }
}
