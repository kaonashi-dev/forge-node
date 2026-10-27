//! `.forge/actions.toml`: actions a repository offers for import.
//!
//! The file never runs. It is parsed into suggestions the dialog shows, and
//! saving copies them into the project's own rows with the file's hash, so a
//! checkout, a pull or a branch switch can change what is *offered* but never
//! what a click executes. A changed file is reported as drift on the copies.

use std::collections::HashSet;
use std::path::Path;

use domain::{
    ActionKind, ActionPort, ActionReady, ActionSource, ActionSuggestion, ProjectAction,
    RepoActionsFile, MAX_PROJECT_ACTIONS,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use super::detect::{read_capped, Capped};

pub const REPO_FILE: &str = ".forge/actions.toml";
pub const MAX_REPO_FILE_BYTES: u64 = 64 * 1024;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FileShape {
    #[serde(default)]
    action: Vec<Entry>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    label: String,
    command: String,
    #[serde(default = "custom")]
    kind: ActionKind,
    #[serde(default)]
    pinned: bool,
    #[serde(default = "yes")]
    single_instance: bool,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    port: Option<ActionPort>,
    #[serde(default)]
    ready: ActionReady,
    #[serde(default)]
    depends_on: Vec<String>,
}

const fn custom() -> ActionKind {
    ActionKind::Custom
}

const fn yes() -> bool {
    true
}

/// Read and parse the workspace's file; `None` when it has none.
#[must_use]
pub fn read(root: &Path, saved: &[ProjectAction]) -> Option<RepoActionsFile> {
    let path = root.join(REPO_FILE);
    let text = match read_capped(&path, MAX_REPO_FILE_BYTES) {
        Capped::Text(text) => text,
        Capped::TooLarge => {
            return Some(RepoActionsFile {
                file_hash: String::new(),
                actions: Vec::new(),
                error: Some(format!(
                    "{REPO_FILE} is over {} KiB",
                    MAX_REPO_FILE_BYTES / 1024
                )),
                drift: Vec::new(),
            })
        }
        Capped::Absent => return None,
    };
    Some(parse(&text, saved))
}

/// Parse the file's text into suggestions sourced from its hash.
#[must_use]
pub fn parse(text: &str, saved: &[ProjectAction]) -> RepoActionsFile {
    let file_hash = hash(text.as_bytes());
    let drift = saved
        .iter()
        .filter(|action| {
            matches!(&action.source, ActionSource::Repo { file_hash: approved } if *approved != file_hash)
        })
        .map(|action| action.id)
        .collect();
    let (actions, error) = match entries(text) {
        Ok(entries) => (
            entries
                .into_iter()
                .map(|entry| suggestion(entry, &file_hash, saved))
                .collect(),
            None,
        ),
        Err(error) => (Vec::new(), Some(error)),
    };
    RepoActionsFile {
        file_hash,
        actions,
        error,
        drift,
    }
}

#[must_use]
pub fn hash(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn entries(text: &str) -> Result<Vec<Entry>, String> {
    let shape: FileShape = toml::from_str(text).map_err(|e| format!("{REPO_FILE}: {e}"))?;
    if shape.action.len() > MAX_PROJECT_ACTIONS {
        return Err(format!(
            "{REPO_FILE} lists more than {MAX_PROJECT_ACTIONS} actions"
        ));
    }
    let labels: HashSet<String> = shape
        .action
        .iter()
        .map(|entry| entry.label.trim().to_lowercase())
        .collect();
    for entry in &shape.action {
        if let Some(missing) = entry
            .depends_on
            .iter()
            .find(|dep| !labels.contains(&dep.trim().to_lowercase()))
        {
            return Err(format!(
                "{REPO_FILE}: {:?} depends on {missing:?}, which the file does not define",
                entry.label
            ));
        }
    }
    Ok(shape.action)
}

fn suggestion(entry: Entry, file_hash: &str, saved: &[ProjectAction]) -> ActionSuggestion {
    let label = entry.label.trim().to_owned();
    ActionSuggestion {
        saved_as: saved
            .iter()
            .find(|action| action.label.eq_ignore_ascii_case(&label))
            .map(|action| action.id),
        label,
        command: entry.command,
        kind: entry.kind,
        pinned: entry.pinned,
        cwd: entry.cwd,
        single_instance: entry.single_instance,
        ready: entry.ready,
        port: entry.port,
        depends_on: entry.depends_on,
        source: ActionSource::Repo {
            file_hash: file_hash.to_owned(),
        },
        detected_from: REPO_FILE.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::{ProjectActionId, ProjectId, Timestamp};

    const FILE: &str = r#"
[[action]]
label = "dev"
command = "bun run dev -- --port {port}"
kind = "dev"
pinned = true
port = { base = 5173, per_worktree = true }
ready = { kind = "port", timeout_secs = 60 }
depends_on = ["docker up"]

[[action]]
label = "docker up"
command = "docker compose up"
"#;

    fn saved(label: &str, source: ActionSource) -> ProjectAction {
        ProjectAction {
            id: ProjectActionId::new(),
            project_id: ProjectId::new(),
            label: label.to_owned(),
            command: "x".into(),
            kind: ActionKind::Custom,
            cwd: None,
            single_instance: true,
            pinned: false,
            ready: ActionReady::None,
            port: None,
            depends_on: Vec::new(),
            source,
            position: 0,
            created_at: Timestamp::now(),
        }
    }

    #[test]
    fn a_file_parses_into_suggestions_sourced_from_its_hash() {
        let file = parse(FILE, &[]);
        assert_eq!(file.error, None);
        assert_eq!(file.file_hash.len(), 64);
        let dev = &file.actions[0];
        assert_eq!(dev.kind, ActionKind::Dev);
        assert!(dev.pinned);
        assert_eq!(dev.ready, ActionReady::Port { timeout_secs: 60 });
        assert_eq!(dev.depends_on, ["docker up"]);
        assert_eq!(
            dev.source,
            ActionSource::Repo {
                file_hash: file.file_hash.clone()
            }
        );
        let docker = &file.actions[1];
        assert_eq!(docker.kind, ActionKind::Custom);
        assert!(docker.single_instance);
    }

    #[test]
    fn a_saved_copy_of_another_version_is_drift() {
        let current = hash(FILE.as_bytes());
        let fresh = saved("dev", ActionSource::Repo { file_hash: current });
        let stale = saved(
            "docker up",
            ActionSource::Repo {
                file_hash: hash(b"older"),
            },
        );
        let manual = saved("lint", ActionSource::Manual);
        let file = parse(FILE, &[fresh.clone(), stale.clone(), manual]);
        assert_eq!(file.drift, [stale.id]);
        assert_eq!(file.actions[0].saved_as, Some(fresh.id));
    }

    #[test]
    fn a_broken_file_is_reported_not_raised() {
        let file = parse("[[action]]\nlabel = \"dev\"\n", &[]);
        assert!(file.actions.is_empty());
        assert!(file.error.unwrap().contains("command"));

        let typo = parse(
            "[[action]]\nlabel = \"a\"\ncommand = \"x\"\ncomand = \"y\"\n",
            &[],
        );
        assert!(typo.error.unwrap().contains("comand"));

        let dangling = parse(
            "[[action]]\nlabel = \"a\"\ncommand = \"x\"\ndepends_on = [\"b\"]\n",
            &[],
        );
        assert!(dangling.error.unwrap().contains("does not define"));
    }

    #[test]
    fn an_oversized_file_is_reported_and_an_absent_one_is_none() {
        let dir = tempfile::tempdir().unwrap();
        assert!(read(dir.path(), &[]).is_none());
        std::fs::create_dir(dir.path().join(".forge")).unwrap();
        let big = "#".repeat(usize::try_from(MAX_REPO_FILE_BYTES).unwrap() + 1);
        std::fs::write(dir.path().join(REPO_FILE), big).unwrap();
        let file = read(dir.path(), &[]).unwrap();
        assert!(file.error.unwrap().contains("KiB"));
    }
}
