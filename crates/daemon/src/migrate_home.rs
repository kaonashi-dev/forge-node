//! `forge-daemon migrate-home`: move a legacy install to `~/.forge-node`.
//!
//! The daemon must be stopped for the locks to be free, so this runs outside
//! the core lock entirely. [`decide`] is the pure part (facts in, outcome out);
//! the rest copies files and runs git. See `docs/worktrees.md`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use domain::{Workspace, WorkspaceId, WorkspaceKind};
use serde::Serialize;

use crate::config;
use crate::lockfile::{self, DaemonLock};
use crate::paths;
use client::paths::{LegacyDirs, PathInputs};

/// Command-line options for `migrate-home`.
#[derive(Clone, Debug, Default)]
pub struct Options {
    pub dry_run: bool,
    pub move_worktrees: bool,
    pub json: bool,
    pub wait_secs: u64,
    pub remove_legacy: bool,
}

/// What happens to one managed worktree.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Outcome {
    Stay,
    Move,
    #[serde(rename = "moved")]
    AlreadyMoved,
    Skip,
}

/// One worktree's report entry.
#[derive(Clone, Debug, Serialize)]
pub struct WorktreeReport {
    pub path: PathBuf,
    pub destination: PathBuf,
    pub outcome: Outcome,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub transcripts: usize,
}

/// The whole migration report.
#[derive(Clone, Debug, Serialize)]
pub struct Report {
    pub dry_run: bool,
    pub from: PathBuf,
    pub to: PathBuf,
    pub config: &'static str,
    pub runs_copied: usize,
    pub worktrees: Vec<WorktreeReport>,
    pub rollback: Vec<String>,
}

/// Facts a pure decision needs about one worktree.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Facts {
    pub old_exists: bool,
    pub new_exists: bool,
    pub locked: bool,
    pub submodules: bool,
    pub sequencer: bool,
}

/// One worktree and everything git or the disk says about it.
#[derive(Clone, Debug)]
struct Candidate {
    path: PathBuf,
    destination: PathBuf,
    git_root: PathBuf,
    transcripts: usize,
    facts: Facts,
}

/// One worktree's decision; pure.
#[must_use]
pub fn decide(facts: &Facts, move_worktrees: bool) -> (Outcome, Option<&'static str>) {
    if !move_worktrees {
        return (Outcome::Stay, None);
    }
    if !facts.old_exists {
        return if facts.new_exists {
            (Outcome::AlreadyMoved, None)
        } else {
            (Outcome::Skip, Some("directory missing"))
        };
    }
    if facts.new_exists {
        return (Outcome::Skip, Some("destination exists"));
    }
    if facts.locked {
        return (Outcome::Skip, Some("locked"));
    }
    if facts.submodules {
        return (Outcome::Skip, Some("submodules"));
    }
    if facts.sequencer {
        return (Outcome::Skip, Some("merge or rebase in progress"));
    }
    (Outcome::Move, None)
}

/// Run the command.
///
/// # Errors
/// Any refusal (no legacy install, environment overrides, a held lock), or an
/// I/O failure that would leave the migration inconsistent.
pub fn run(options: &Options) -> anyhow::Result<()> {
    if options.remove_legacy {
        return remove_legacy(options);
    }

    let inputs = PathInputs::from_env();
    if inputs.data_dir.is_some() || inputs.config_dir.is_some() {
        anyhow::bail!(
            "refusing to migrate while FORGE_DATA_DIR or FORGE_CONFIG_DIR is set; \
             unset both so the target is $FORGE_HOME or ~/.forge-node"
        );
    }
    let legacy = inputs.legacy.clone().ok_or_else(|| {
        let dir = directories::ProjectDirs::from("", "", "Forge")
            .map(|dirs| dirs.data_dir().display().to_string())
            .unwrap_or_else(|| "the platform data directory".to_string());
        anyhow::anyhow!("no legacy install at {dir}")
    })?;
    let target = client::paths::resolve(&PathInputs {
        legacy: None,
        home_db_exists: false,
        ..inputs.clone()
    })?;
    let target_config = if target.config_file.exists() {
        target.config_file.clone()
    } else {
        legacy.config.join("config.toml")
    };
    let cfg = config::Config::load(&target_config)?;
    let target_root = client::paths::worktrees_root(
        &cfg.worktrees.root,
        inputs.home.as_deref(),
        &target.default_worktrees_root,
    )
    .unwrap_or_else(|_| target.default_worktrees_root.clone());

    let first_migration = !target.db.exists();
    let _locks: Vec<DaemonLock> = if options.dry_run {
        Vec::new()
    } else {
        acquire_locks(&legacy, &target.home, options.wait_secs)?
    };
    let _dry_run_dir;
    let live_db;
    if options.dry_run {
        let copy = tempfile::TempDir::new()?;
        live_db = copy.path().join("app.db");
        persistence::vacuum_into(&legacy.data.join("app.db"), &live_db)?;
        _dry_run_dir = Some(copy);
    } else if first_migration {
        live_db = target.home.join("app.db.migrating");
        let _ = std::fs::remove_file(&live_db);
        persistence::vacuum_into(&legacy.data.join("app.db"), &live_db)?;
        _dry_run_dir = None;
    } else {
        live_db = target.db.clone();
        _dry_run_dir = None;
    }
    let db = persistence::Db::open(&live_db)?;

    let config_outcome = if first_migration {
        if !legacy.config.join("config.toml").is_file() {
            "absent"
        } else if target.config_file.exists() {
            "kept"
        } else if options.dry_run {
            "copied"
        } else {
            copy_config(&legacy.config.join("config.toml"), &target.config_file)?;
            "copied"
        }
    } else if target.config_file.exists() {
        "kept"
    } else {
        "absent"
    };

    let runs_copied = if first_migration {
        if options.dry_run {
            count_files(&legacy.data.join("runs"))
        } else {
            let copied = copy_tree_no_overwrite(&legacy.data.join("runs"), &target.runs)?;
            db.replace_path_prefix(&legacy.data.join("runs"), &target.runs)?;
            copied
        }
    } else {
        0
    };

    let candidates = candidates(&db, &legacy, &target_root)?;
    let old_root = legacy.data.join("worktrees");
    let mut worktrees = Vec::with_capacity(candidates.len());
    let mut rollback = Vec::new();

    for candidate in &candidates {
        let (decision, reason) =
            decide(&candidate.facts, options.move_worktrees || options.dry_run);
        let mut reason = reason.map(str::to_owned);
        let mut outcome = decision;
        if !options.dry_run {
            match decision {
                Outcome::Move => match move_candidate(&db, candidate, &old_root) {
                    Ok(()) => outcome = Outcome::AlreadyMoved,
                    Err(error) => {
                        outcome = Outcome::Skip;
                        reason = Some(error.to_string());
                    }
                },
                Outcome::AlreadyMoved => {
                    db.replace_path_prefix(&candidate.path, &candidate.destination)?;
                    remove_emptied_project_dir(&candidate.path, &old_root);
                }
                Outcome::Stay | Outcome::Skip => {}
            }
        }
        if matches!(outcome, Outcome::Move | Outcome::AlreadyMoved) {
            rollback.push(format!(
                "git -C {} worktree move {} {}",
                candidate.git_root.display(),
                candidate.destination.display(),
                candidate.path.display()
            ));
        }
        worktrees.push(WorktreeReport {
            path: candidate.path.clone(),
            destination: candidate.destination.clone(),
            outcome,
            reason,
            transcripts: candidate.transcripts,
        });
    }

    if !options.dry_run && first_migration {
        db.conn()
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
        drop(db);
        std::fs::rename(
            target.home.join("app.db.migrating"),
            target.home.join("app.db"),
        )?;
        let _ = std::fs::remove_file(target.home.join("app.db.migrating-wal"));
        let _ = std::fs::remove_file(target.home.join("app.db.migrating-shm"));
    }

    let report = Report {
        dry_run: options.dry_run,
        from: legacy.data,
        to: target.home.clone(),
        config: config_outcome,
        runs_copied,
        worktrees,
        rollback,
    };

    if !options.dry_run {
        std::fs::write(
            report.from.join("MIGRATED-TO"),
            format!("{}\n", target.home.display()),
        )?;
    }

    if options.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_human(&report);
    }
    Ok(())
}

fn move_candidate(
    db: &persistence::Db,
    candidate: &Candidate,
    old_root: &Path,
) -> anyhow::Result<()> {
    if let Some(parent) = candidate.destination.parent() {
        paths::ensure_private_dir(parent)?;
    }
    git_service::move_worktree(&candidate.git_root, &candidate.path, &candidate.destination)?;
    db.replace_path_prefix(&candidate.path, &candidate.destination)?;
    remove_emptied_project_dir(&candidate.path, old_root);
    Ok(())
}

fn remove_emptied_project_dir(path: &Path, old_root: &Path) {
    if let Some(parent) = path.parent() {
        if parent.parent() == Some(old_root) {
            let _ = std::fs::remove_dir(parent);
        }
    }
}

fn candidates(
    db: &persistence::Db,
    legacy: &LegacyDirs,
    target_root: &Path,
) -> anyhow::Result<Vec<Candidate>> {
    let projects = db.projects().list()?;
    let profiles = db.agent_profiles().list()?;
    let mut workspaces: Vec<Workspace> = Vec::new();
    for project in &projects {
        workspaces.extend(db.workspaces().list_by_project(project.id)?);
    }
    let sessions = agents::history::discover(&projects, &workspaces, &profiles);
    let mut transcripts: HashMap<WorkspaceId, usize> = HashMap::new();
    for session in sessions {
        if let Some(id) = session.workspace_id {
            *transcripts.entry(id).or_default() += 1;
        }
    }
    let old_root = legacy.data.join("worktrees");
    let mut out = Vec::new();
    for workspace in workspaces.iter().filter(|workspace| {
        workspace.managed_by_app
            && workspace.kind == WorkspaceKind::GitWorktree
            && workspace.path.starts_with(&old_root)
    }) {
        let Some(project) = projects
            .iter()
            .find(|project| project.id == workspace.project_id)
        else {
            continue;
        };
        let destination = target_root
            .join(crate::worktrees::project_dir_name(
                &project.root_path,
                project.id,
            ))
            .join(workspace.path.file_name().unwrap_or_default());
        let locked = project
            .git_root
            .as_deref()
            .and_then(|root| git_service::list_worktrees(root).ok())
            .is_some_and(|entries| {
                entries
                    .iter()
                    .any(|entry| entry.locked && same_path(&entry.path, &workspace.path))
            });
        out.push(Candidate {
            path: workspace.path.clone(),
            destination: destination.clone(),
            git_root: project
                .git_root
                .clone()
                .unwrap_or_else(|| project.root_path.clone()),
            transcripts: transcripts.get(&workspace.id).copied().unwrap_or(0),
            facts: Facts {
                old_exists: workspace.path.exists(),
                new_exists: destination.exists(),
                locked,
                submodules: workspace.path.join(".gitmodules").exists(),
                sequencer: git_service::sequencer_in_progress(&workspace.path),
            },
        });
    }
    Ok(out)
}

fn same_path(a: &Path, b: &Path) -> bool {
    a == b || canonical_or_self(a) == canonical_or_self(b)
}

fn canonical_or_self(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn copy_config(source: &Path, target: &Path) -> anyhow::Result<()> {
    if let Some(parent) = target.parent() {
        paths::ensure_private_dir(parent)?;
    }
    std::fs::copy(source, target)?;
    Ok(())
}

fn count_files(root: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(root) else {
        return 0;
    };
    entries
        .flatten()
        .map(|entry| {
            let path = entry.path();
            if path.is_dir() {
                count_files(&path)
            } else if path.is_file() {
                1
            } else {
                0
            }
        })
        .sum()
}

fn copy_tree_no_overwrite(from: &Path, to: &Path) -> anyhow::Result<usize> {
    if !from.is_dir() {
        return Ok(0);
    }
    std::fs::create_dir_all(to)?;
    let mut copied = 0;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            copied += copy_tree_no_overwrite(&entry.path(), &target)?;
        } else if file_type.is_file() && !target.exists() {
            std::fs::copy(entry.path(), &target)?;
            copied += 1;
        }
    }
    Ok(copied)
}

fn acquire_locks(
    legacy: &LegacyDirs,
    home: &Path,
    wait_secs: u64,
) -> anyhow::Result<Vec<DaemonLock>> {
    let instance = uuid::Uuid::now_v7().to_string();
    let version = env!("CARGO_PKG_VERSION");
    let started_at = domain::Timestamp::now();
    let wait = Duration::from_secs(wait_secs);
    let mut locks = Vec::new();
    for path in [
        paths::lock_path()?,
        legacy.data.join("app.db.lock"),
        home.join("app.db.lock"),
    ] {
        match try_lock(&path, &instance, version, started_at, wait) {
            Ok(lock) => locks.push(lock),
            Err(error) => {
                return Err(anyhow::anyhow!(
                    "the daemon is running; stop it first ({error})"
                ));
            }
        }
    }
    Ok(locks)
}

fn try_lock(
    path: &Path,
    instance: &str,
    version: &str,
    started_at: domain::Timestamp,
    wait: Duration,
) -> anyhow::Result<DaemonLock> {
    if wait.is_zero() {
        return lockfile::acquire(path, instance, version, started_at).map_err(anyhow::Error::from);
    }
    let (sender, receiver) = std::sync::mpsc::channel();
    let thread_path = path.to_path_buf();
    let instance = instance.to_string();
    let version = version.to_string();
    std::thread::spawn(move || {
        let result = lockfile::acquire_blocking(&thread_path, &instance, &version, started_at);
        let _ = sender.send(result);
    });
    match receiver.recv_timeout(wait) {
        Ok(Ok(lock)) => Ok(lock),
        Ok(Err(error)) => Err(error.into()),
        Err(_) => anyhow::bail!("timed out waiting for {path:?}"),
    }
}

fn remove_legacy(options: &Options) -> anyhow::Result<()> {
    let inputs = PathInputs::from_env();
    let target = client::paths::resolve(&inputs)?;
    let legacy = match (&target.layout, &target.legacy) {
        (client::Layout::Home, Some(dir)) => dir.clone(),
        _ => anyhow::bail!("the current layout is not a migrated home; nothing to remove"),
    };
    let instance = uuid::Uuid::now_v7().to_string();
    let version = env!("CARGO_PKG_VERSION");
    let _lock = lockfile::acquire(
        &legacy.join("app.db.lock"),
        &instance,
        version,
        domain::Timestamp::now(),
    )
    .map_err(|error| anyhow::anyhow!("the daemon is running; stop it first ({error})"))?;

    let old_root = legacy.join("worktrees");
    let remaining = persistence::managed_worktree_paths(&target.db)?
        .into_iter()
        .filter(|path| path.starts_with(&old_root))
        .count();
    if remaining > 0 {
        anyhow::bail!(
            "{remaining} managed worktree(s) still live under {}; move or remove them first",
            old_root.display()
        );
    }

    let mut removed = Vec::new();
    for name in ["app.db", "app.db-wal", "app.db-shm"] {
        let path = legacy.join(name);
        if path.exists() {
            std::fs::remove_file(&path)?;
            removed.push(path);
        }
    }
    let plugins = legacy.join("agent-plugins");
    if plugins.is_dir() {
        std::fs::remove_dir_all(&plugins)?;
        removed.push(plugins);
    }
    if options.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "removed": removed.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
            }))?
        );
    } else {
        for path in &removed {
            println!("removed {}", path.display());
        }
    }
    Ok(())
}

fn print_human(report: &Report) {
    let prefix = if report.dry_run { "would " } else { "" };
    println!("from: {}", report.from.display());
    println!("to:   {}", report.to.display());
    println!("config: {prefix}{}", report.config);
    println!("runs copied: {}", report.runs_copied);
    for worktree in &report.worktrees {
        let outcome = match worktree.outcome {
            Outcome::Stay => "stay",
            Outcome::Move => "move",
            Outcome::AlreadyMoved => "moved",
            Outcome::Skip => "skip",
        };
        let reason = worktree
            .reason
            .as_deref()
            .map(|reason| format!(": {reason}"))
            .unwrap_or_default();
        println!(
            "  {} -> {} [{outcome}{reason}] ({} transcript(s))",
            worktree.path.display(),
            worktree.destination.display(),
            worktree.transcripts
        );
    }
    if !report.rollback.is_empty() {
        println!("rollback:");
        for line in &report.rollback {
            println!("  {line}");
        }
    }
    if !report.dry_run && report.worktrees.iter().all(|w| w.outcome == Outcome::Stay) {
        println!("already migrated; nothing to move");
    }
}
