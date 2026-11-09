//! How a session is named and its state said, as Tauri's `sessionTitle`,
//! `promptPath` and `sessionStateLabel` (`contracts/runtime.ts`) do.

use domain::{Session, SessionKind, SessionState};

/// Empty or whitespace is unset, so a blank title never names a row.
fn present(value: Option<&str>) -> Option<&str> {
    value.filter(|text| !text.trim().is_empty())
}

fn is_agent(session: &Session) -> bool {
    session.kind == SessionKind::Agent || session.agent_provider_id.is_some()
}

fn fallback_kind(session: &Session) -> String {
    if session.kind == SessionKind::Editor {
        // Tauri shows editors in its Code tab, never among session tabs; here
        // they are tabs, so the file's name tells them apart.
        return session
            .editor
            .as_ref()
            .map(|editor| {
                editor
                    .path
                    .rsplit('/')
                    .next()
                    .unwrap_or(&editor.path)
                    .to_string()
            })
            .unwrap_or_else(|| "Editor".to_string());
    }
    if is_agent(session) {
        return session
            .agent_provider_id
            .as_ref()
            .map_or_else(|| "Agent".to_string(), ToString::to_string);
    }
    "Shell".to_string()
}

/// The directory in a shell prompt's `user@host:path` title, home-relative;
/// `user@host` is the same on every row of one machine, so it is dropped.
#[must_use]
pub fn prompt_path(title: &str) -> Option<String> {
    let title = title.trim();
    let (who, path) = title.split_once(':')?;
    let (user, host) = who.split_once('@')?;
    let word = |part: &str| {
        !part.is_empty()
            && part
                .chars()
                .all(|ch| ch.is_alphanumeric() || matches!(ch, '_' | '.' | '-'))
    };
    let path = path.trim_start();
    if !word(user) || !word(host) || path.is_empty() {
        return None;
    }
    for home in ["/Users/", "/home/"] {
        if let Some(rest) = path.strip_prefix(home) {
            let after = rest.find('/').map_or("", |at| &rest[at..]);
            return Some(format!("~{after}"));
        }
    }
    Some(path.to_string())
}

#[must_use]
pub fn session_title(session: &Session) -> String {
    let terminal = present(session.title.terminal.as_deref());
    let user = present(session.title.user.as_deref());
    // A dev server retitles its terminal as it likes; the action names the tab.
    if session.kind == SessionKind::Task {
        return user
            .map(str::to_string)
            .or_else(|| session.action.as_ref().map(|run| run.label.clone()))
            .or_else(|| terminal.map(str::to_string))
            .unwrap_or_else(|| "Task".to_string());
    }
    if let Some(user) = user {
        return user.to_string();
    }
    let prompt = terminal
        .filter(|_| !is_agent(session))
        .and_then(prompt_path);
    if let Some(prompt) = prompt {
        return format!("{} {prompt}", fallback_kind(session));
    }
    terminal.map_or_else(|| fallback_kind(session), str::to_string)
}

#[must_use]
pub fn state_label(state: &SessionState) -> String {
    match state {
        SessionState::Starting => "starting".into(),
        SessionState::Running => "running".into(),
        SessionState::Orphaned => "orphaned".into(),
        SessionState::Failed { reason } if !reason.is_empty() => reason.clone(),
        SessionState::Failed { .. } => "failed".into(),
        SessionState::Exited { code: Some(0), .. } => "finished".into(),
        SessionState::Exited {
            code: Some(code), ..
        } => format!("exited {code}"),
        SessionState::Exited {
            signal: Some(signal),
            ..
        } => format!("signal {signal}"),
        SessionState::Exited { .. } => "exited".into(),
        _ => "unknown".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_prompt_title_becomes_a_home_relative_path() {
        assert_eq!(
            prompt_path("me@mac:/Users/me/dev/forge"),
            Some("~/dev/forge".into())
        );
        assert_eq!(prompt_path("me@mac:/home/me"), Some("~".into()));
        assert_eq!(prompt_path("me@mac: /srv/app"), Some("/srv/app".into()));
        assert_eq!(prompt_path("vim README.md"), None);
    }

    #[test]
    fn states_are_said_in_tauris_words() {
        assert_eq!(state_label(&SessionState::Running), "running");
        assert_eq!(
            state_label(&SessionState::Exited {
                code: Some(0),
                signal: None
            }),
            "finished"
        );
        assert_eq!(
            state_label(&SessionState::Exited {
                code: Some(2),
                signal: None
            }),
            "exited 2"
        );
        assert_eq!(
            state_label(&SessionState::Exited {
                code: None,
                signal: Some(9)
            }),
            "signal 9"
        );
        assert_eq!(
            state_label(&SessionState::Failed {
                reason: "no such binary".into()
            }),
            "no such binary"
        );
    }
}
