//! Whether a process still owns a discovered conversation, read from the
//! provider's own registry rather than guessed from transcript mtimes.

use std::collections::HashSet;
use std::path::Path;

use domain::{AgentProfile, AgentProfileId, ExternalAgentSession};
use nix::errno::Errno;
use nix::sys::signal::kill;
use nix::unistd::Pid;
use serde::Deserialize;

use super::{accounts, default_dir, home_dir, Account, CLAUDE_PROVIDER};

/// Registry files read per account. Claude writes one per running process.
const REGISTRY_LIMIT: usize = 256;
/// A registry entry is a few hundred bytes of JSON.
const ENTRY_BYTES: u64 = 16 * 1024;

/// Claude's `<config>/sessions/<pid>.json`.
#[derive(Deserialize)]
struct ClaudeEntry {
    pid: i32,
    #[serde(rename = "sessionId")]
    session_id: String,
}

/// One read of every account's registry, answered per run without more IO.
#[derive(Debug, Default)]
pub struct Liveness {
    /// Accounts whose Claude registry directory exists; a run in any other
    /// account is "unknown", not "ended".
    claude_accounts: HashSet<Option<AgentProfileId>>,
    claude_live: HashSet<(Option<AgentProfileId>, String)>,
}

impl Liveness {
    /// Filesystem IO and one `kill(pid, 0)` per entry: never under the core lock.
    #[must_use]
    pub fn scan(profiles: &[AgentProfile]) -> Self {
        let Some(home) = home_dir() else {
            return Self::default();
        };
        let claude = accounts(
            &default_dir(&home, "CLAUDE_CONFIG_DIR", ".claude"),
            CLAUDE_PROVIDER,
            profiles,
            &home,
            "",
        );
        Self::scan_claude(&claude)
    }

    fn scan_claude(accounts: &[Account]) -> Self {
        let mut liveness = Self::default();
        for account in accounts {
            let Ok(entries) = std::fs::read_dir(account.dir.join("sessions")) else {
                continue;
            };
            let registered: Vec<_> = entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
                .take(REGISTRY_LIMIT + 1)
                .collect();
            // An entry the cap left unread may be the live one: unknown, never ended.
            if registered.len() > REGISTRY_LIMIT {
                continue;
            }
            liveness.claude_accounts.insert(account.profile);
            for path in registered {
                if let Some(id) = live_claude_entry(&path) {
                    liveness.claude_live.insert((account.profile, id));
                }
            }
        }
        liveness
    }

    /// `None` for a provider or account with no registry to ask.
    #[must_use]
    pub fn of(&self, session: &ExternalAgentSession) -> Option<bool> {
        if session.provider != CLAUDE_PROVIDER
            || !self.claude_accounts.contains(&session.profile_id)
        {
            return None;
        }
        Some(
            self.claude_live
                .contains(&(session.profile_id, session.session_id.clone())),
        )
    }
}

fn live_claude_entry(path: &Path) -> Option<String> {
    let bytes = crate::capped::read(path, ENTRY_BYTES).ok()?;
    let entry: ClaudeEntry = serde_json::from_slice(&bytes).ok()?;
    process_alive(entry.pid).then_some(entry.session_id)
}

/// A pid we may not signal still exists; `0` and negatives would name a group.
fn process_alive(pid: i32) -> bool {
    pid > 0 && matches!(kill(Pid::from_raw(pid), None), Ok(()) | Err(Errno::EPERM))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn session(id: &str, profile: Option<AgentProfileId>, provider: &str) -> ExternalAgentSession {
        ExternalAgentSession {
            session_id: id.to_owned(),
            project_id: domain::ProjectId::new(),
            workspace_id: None,
            provider: provider.to_owned(),
            profile_id: profile,
            title: id.to_owned(),
            branch: None,
            preview: None,
            model: None,
            message_count: 0,
            subagent_count: 0,
            transcript_path: PathBuf::new(),
            store: domain::TranscriptStore::File,
            started_at: domain::Timestamp::now(),
            last_activity: domain::Timestamp::now(),
        }
    }

    fn register(dir: &Path, pid: u32, id: &str) {
        std::fs::create_dir_all(dir.join("sessions")).unwrap();
        std::fs::write(
            dir.join("sessions").join(format!("{pid}.json")),
            format!(r#"{{"pid":{pid},"sessionId":"{id}","status":"idle"}}"#),
        )
        .unwrap();
    }

    #[test]
    fn a_run_is_live_only_while_its_registered_pid_runs_in_the_same_account() {
        let default = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        let none = tempfile::tempdir().unwrap();
        let profile = AgentProfileId::new();
        let mut ended = std::process::Command::new("true").spawn().unwrap();
        let ended_pid = ended.id();
        ended.wait().unwrap();
        register(default.path(), std::process::id(), "running");
        register(default.path(), ended_pid, "ended");
        register(work.path(), std::process::id(), "work-running");
        std::fs::write(default.path().join("sessions/junk.json"), b"{").unwrap();

        let liveness = Liveness::scan_claude(&[
            Account {
                dir: default.path().to_path_buf(),
                profile: None,
            },
            Account {
                dir: work.path().to_path_buf(),
                profile: Some(profile),
            },
            Account {
                dir: none.path().to_path_buf(),
                profile: Some(AgentProfileId::new()),
            },
        ]);
        assert_eq!(liveness.of(&session("running", None, "claude")), Some(true));
        assert_eq!(liveness.of(&session("ended", None, "claude")), Some(false));
        assert_eq!(
            liveness.of(&session("work-running", None, "claude")),
            Some(false),
            "the same id in another account's registry says nothing about this one"
        );
        assert_eq!(
            liveness.of(&session("work-running", Some(profile), "claude")),
            Some(true)
        );
        assert_eq!(liveness.of(&session("running", None, "opencode")), None);
        assert_eq!(
            liveness.of(&session("x", Some(AgentProfileId::new()), "claude")),
            None
        );
    }

    #[test]
    fn a_registry_past_the_cap_is_unknown_and_key_files_do_not_count() {
        let full = tempfile::tempdir().unwrap();
        let keyed = tempfile::tempdir().unwrap();
        for n in 0..=REGISTRY_LIMIT {
            std::fs::create_dir_all(full.path().join("sessions")).unwrap();
            std::fs::write(full.path().join(format!("sessions/{n}.json")), b"{}").unwrap();
        }
        register(keyed.path(), std::process::id(), "running");
        for n in 0..=REGISTRY_LIMIT {
            std::fs::write(keyed.path().join(format!("sessions/{n}.abc.key")), b"k").unwrap();
        }
        let (over, under) = (AgentProfileId::new(), AgentProfileId::new());
        let liveness = Liveness::scan_claude(&[
            Account {
                dir: full.path().to_path_buf(),
                profile: Some(over),
            },
            Account {
                dir: keyed.path().to_path_buf(),
                profile: Some(under),
            },
        ]);
        assert_eq!(liveness.of(&session("any", Some(over), "claude")), None);
        assert_eq!(
            liveness.of(&session("running", Some(under), "claude")),
            Some(true)
        );
    }
}
