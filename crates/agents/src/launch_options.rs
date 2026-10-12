use domain::{AgentDescriptor, AgentLaunchOptions, EffortSpelling};

use crate::AgentError;

pub fn validate(
    descriptor: &AgentDescriptor,
    options: &AgentLaunchOptions,
) -> Result<(), AgentError> {
    let invalid = |reason: &str| AgentError::InvalidLaunchOptions(reason.to_owned());
    if let Some(model) = &options.model {
        if descriptor.model.is_none() {
            return Err(invalid("provider does not support model selection"));
        }
        if model.is_empty()
            || model.len() > 128
            || model.starts_with('-')
            || !model
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._:/-".contains(&b))
        {
            return Err(invalid(
                "model must be 1–128 ASCII letters, digits or ._:/- and not start with -",
            ));
        }
    }
    if let Some(effort) = &options.effort {
        let spec = descriptor
            .effort
            .as_ref()
            .ok_or_else(|| invalid("provider does not support effort selection"))?;
        if !spec.allowed.contains(effort) {
            return Err(invalid("unsupported effort level"));
        }
        if matches!(spec.spelling, EffortSpelling::ModelSuffix) && options.model.is_none() {
            return Err(invalid(
                "effort requires an explicit model for this provider",
            ));
        }
    }
    Ok(())
}

pub(crate) fn append(
    descriptor: &AgentDescriptor,
    options: &AgentLaunchOptions,
    args: &mut Vec<String>,
) -> Result<(), AgentError> {
    validate(descriptor, options)?;
    if let (Some(model), Some(spec)) = (&options.model, &descriptor.model) {
        remove_flag(args, &[&spec.flag, "--model", "-m"]);
        let value = match (&options.effort, &descriptor.effort) {
            (Some(effort), Some(spec)) if matches!(spec.spelling, EffortSpelling::ModelSuffix) => {
                format!("{model}[effort={effort}]")
            }
            _ => model.clone(),
        };
        args.extend([spec.flag.clone(), value]);
    }
    if let (Some(effort), Some(spec)) = (&options.effort, &descriptor.effort) {
        match &spec.spelling {
            EffortSpelling::Flag(flag) => {
                remove_flag(args, &[flag]);
                args.extend([flag.clone(), effort.clone()]);
            }
            EffortSpelling::Config { flag, key } => {
                args.extend([flag.clone(), format!("{key}=\"{effort}\"")])
            }
            EffortSpelling::ModelSuffix => {}
            _ => {
                return Err(AgentError::InvalidLaunchOptions(
                    "unknown effort spelling".into(),
                ))
            }
        }
    }
    Ok(())
}

fn remove_flag(args: &mut Vec<String>, flags: &[&str]) {
    let mut i = 0;
    while i < args.len() {
        if flags.contains(&args[i].as_str()) {
            args.remove(i);
            if i < args.len() {
                args.remove(i);
            }
        } else if flags
            .iter()
            .any(|flag| args[i].starts_with(&format!("{flag}=")))
        {
            args.remove(i);
        } else {
            i += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtins;

    #[test]
    fn spellings_follow_each_providers_cli() {
        for (id, model, effort, expected) in [
            (
                "claude",
                "opus",
                "xhigh",
                vec!["--model", "opus", "--effort", "xhigh"],
            ),
            (
                "codex",
                "gpt-5",
                "high",
                vec!["-m", "gpt-5", "-c", "model_reasoning_effort=\"high\""],
            ),
            (
                "cursor",
                "opus",
                "high",
                vec!["--model", "opus[effort=high]"],
            ),
            (
                "grok",
                "grok-4",
                "high",
                vec!["-m", "grok-4", "--reasoning-effort", "high"],
            ),
        ] {
            let mut args = vec![];
            append(
                &builtins::builtin(id).unwrap(),
                &AgentLaunchOptions {
                    model: Some(model.into()),
                    effort: Some(effort.into()),
                },
                &mut args,
            )
            .unwrap();
            assert_eq!(args, expected, "{id}");
        }
    }

    #[test]
    fn unsafe_models_and_unsupported_effort_are_refused() {
        let descriptor = builtins::builtin("claude").unwrap();
        for model in [
            "",
            "-opus",
            "opus;echo",
            "opus[effort=high]",
            "with space",
            &"x".repeat(129),
        ] {
            assert!(validate(
                &descriptor,
                &AgentLaunchOptions {
                    model: Some(model.into()),
                    effort: None
                }
            )
            .is_err());
        }
        for (id, model, effort) in [
            ("opencode", Some("opus"), None),
            ("opencode", None, Some("high")),
            ("cursor", None, Some("high")),
            ("claude", None, Some("unlimited")),
        ] {
            assert!(validate(
                &builtins::builtin(id).unwrap(),
                &AgentLaunchOptions {
                    model: model.map(str::to_owned),
                    effort: effort.map(str::to_owned)
                }
            )
            .is_err());
        }
    }

    #[test]
    fn explicit_model_replaces_singleton_profile_flags() {
        let mut args = vec![
            "--model=sonnet".into(),
            "--model".into(),
            "old".into(),
            "--verbose".into(),
        ];
        append(
            &builtins::builtin("claude").unwrap(),
            &AgentLaunchOptions {
                model: Some("opus".into()),
                effort: None,
            },
            &mut args,
        )
        .unwrap();
        assert_eq!(args, ["--verbose", "--model", "opus"]);
    }
}
