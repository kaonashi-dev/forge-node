//! `forge-daemon migrate-home`, end to end with real git.
//!
//! The fixture is a legacy install: a real repository, four real managed
//! worktrees under `Application Support/Forge/worktrees`, a database seeded
//! through `persistence::Db`, a config, and one Claude transcript. Every test
//! spawns the actual binary with `HOME` and `FORGE_SOCKET` pointing into a
//! temporary tree, never the developer's real Forge dirs.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::SystemTime;

use domain::{
    AgentProfile, AgentProfileId, AgentProviderId, IgnoreScope, Project, ProjectId, Timestamp,
    Workspace, WorkspaceId, WorkspaceKind, WorkspaceStatus, WorktreeIgnore,
};
use persistence::Db;

struct Fixture {
    tmp: tempfile::TempDir,
    home: PathBuf,
    legacy: PathBuf,
    legacy_config: PathBuf,
    repo: test_support::TempRepo,
    project: Project,
    movable: Workspace,
    locked: Workspace,
    submodules: Workspace,
    rebasing: Workspace,
}

impl Fixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir_in("/tmp").expect("tempdir");
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&home).expect("home");
        let (legacy, legacy_config) = legacy_dirs(&home);
        std::fs::create_dir_all(&legacy).expect("legacy data");
        std::fs::create_dir_all(&legacy_config).expect("legacy config");
        let repo = test_support::init_repo().expect("git repo (git must be installed)");
        let project = Project {
            id: ProjectId::new(),
            project_group_id: None,
            name: "forge".to_string(),
            icon: None,
            root_path: repo.path().to_path_buf(),
            git_root: Some(repo.path().to_path_buf()),
            created_at: Timestamp::now(),
            last_opened_at: Timestamp::now(),
        };

        let project_dir = legacy.join("worktrees").join(project.id.to_string());
        std::fs::create_dir_all(&project_dir).expect("legacy worktrees");
        let mut workspaces = Vec::new();
        for (slug, branch) in [
            ("movable", "feature/movable"),
            ("locked", "feature/locked"),
            ("submodules", "feature/submodules"),
            ("rebasing", "feature/rebasing"),
        ] {
            let path = project_dir.join(slug);
            git(
                repo.path(),
                &[
                    "worktree",
                    "add",
                    path.to_str().expect("UTF-8 worktree path"),
                    "-b",
                    branch,
                ],
            );
            let workspace = Workspace {
                id: WorkspaceId::new(),
                project_id: project.id,
                kind: WorkspaceKind::GitWorktree,
                path,
                branch: Some(branch.to_string()),
                display_name: None,
                managed_by_app: true,
                created_at: Timestamp::now(),
                status: WorkspaceStatus::default(),
            };
            workspaces.push(workspace);
        }
        let [movable, locked, submodules, rebasing] =
            workspaces.try_into().expect("four worktrees");

        git(
            repo.path(),
            &[
                "worktree",
                "lock",
                locked.path.to_str().expect("UTF-8 worktree path"),
            ],
        );
        std::fs::write(submodules.path.join(".gitmodules"), "[submodule \"x\"]\n")
            .expect("write .gitmodules");
        let git_dir = git_output(&rebasing.path, &["rev-parse", "--absolute-git-dir"]);
        std::fs::create_dir_all(Path::new(git_dir.trim()).join("rebase-merge"))
            .expect("create rebase marker");

        let profile = AgentProfile {
            id: AgentProfileId::new(),
            provider_id: AgentProviderId::new("claude"),
            name: "legacy".to_string(),
            executable: None,
            config_dir: None,
            args: Vec::new(),
            created_at: Timestamp::now(),
        };
        {
            let db = Db::open(&legacy.join("app.db")).expect("seed db");
            db.projects().upsert(&project).expect("project");
            for workspace in [&movable, &locked, &submodules, &rebasing] {
                db.workspaces().upsert(workspace).expect("workspace");
            }
            db.agent_profiles().upsert(&profile).expect("profile");
            db.ignores()
                .upsert(&WorktreeIgnore {
                    project_id: project.id,
                    path: movable.path.join("sub"),
                    scope: IgnoreScope::Subtree,
                    created_at: Timestamp::now(),
                })
                .expect("ignore rule");
            let ts = "2026-01-01T00:00:00Z";
            db.conn()
                .execute(
                    "INSERT INTO runs \
                     (id, project_id, objective, brief, brief_version, status, revision, \
                      created_at, updated_at) \
                     VALUES ('run-1', ?1, 'o', 'b', 1, 'open', 1, ?2, ?2)",
                    rusqlite::params![project.id.to_string(), ts],
                )
                .expect("run");
            db.conn()
                .execute(
                    "INSERT INTO tasks \
                     (id, run_id, title, spec, acceptance, mode, allow_subruns, status, \
                      attempts_used, revision, created_at, updated_at) \
                     VALUES ('task-1', 'run-1', 't', 's', 'a', 'single', 0, 'pending', 0, 1, ?1, ?1)",
                    rusqlite::params![ts],
                )
                .expect("task");
            let result = legacy.join("runs/run-1/attempt-1.md");
            db.conn()
                .execute(
                    "INSERT INTO attempts \
                     (id, task_id, n, provider_id, read_only, phase, result_path, created_at) \
                     VALUES ('attempt-1', 'task-1', 1, 'claude', 0, 'starting', ?1, ?2)",
                    rusqlite::params![result.to_str().expect("UTF-8 result path"), ts],
                )
                .expect("attempt");
            db.conn()
                .execute_batch("PRAGMA wal_checkpoint(TRUNCATE); PRAGMA journal_mode=DELETE;")
                .expect("leave no WAL beside the legacy database");
        }
        std::fs::create_dir_all(legacy.join("runs/run-1")).expect("runs");
        std::fs::write(legacy.join("runs/run-1/attempt-1.md"), "legacy result\n")
            .expect("result file");
        std::fs::write(
            legacy_config.join("config.toml"),
            "[sessions]\npersist_history = false\n",
        )
        .expect("config");

        let slug = movable
            .path
            .to_string_lossy()
            .chars()
            .map(|c| if c == '/' || c == '.' { '-' } else { c })
            .collect::<String>();
        let transcripts = home.join(".claude/projects").join(slug);
        std::fs::create_dir_all(&transcripts).expect("transcript dir");
        std::fs::write(
            transcripts.join("legacy-session.jsonl"),
            format!(
                "{{\"type\":\"user\",\"cwd\":\"{}\",\"gitBranch\":\"feature/movable\",\
                 \"message\":{{\"content\":\"hello\"}},\"timestamp\":\"2026-01-01T00:00:00Z\"}}\n",
                movable.path.display()
            ),
        )
        .expect("transcript");

        Self {
            tmp,
            home,
            legacy,
            legacy_config,
            repo,
            project,
            movable,
            locked,
            submodules,
            rebasing,
        }
    }

    fn socket(&self) -> PathBuf {
        self.tmp.path().join("d.sock")
    }

    fn forge_home(&self) -> PathBuf {
        self.home.join(".forge-node")
    }

    fn run(&self, args: &[&str]) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_forge-daemon"));
        command
            .args(args)
            .env_clear()
            .env("HOME", &self.home)
            .env("FORGE_SOCKET", self.socket())
            .env("TMPDIR", self.tmp.path())
            .env(
                "PATH",
                std::env::var_os("PATH").unwrap_or_else(|| "/usr/bin:/bin".into()),
            );
        command.output().expect("run forge-daemon")
    }

    fn expected_destination(&self, workspace: &Workspace) -> PathBuf {
        self.home
            .join("forge-node/worktrees")
            .join(daemon::worktrees::project_dir_name(
                &self.project.root_path,
                self.project.id,
            ))
            .join(workspace.path.file_name().expect("worktree name"))
    }
}

fn legacy_dirs(home: &Path) -> (PathBuf, PathBuf) {
    #[cfg(target_os = "macos")]
    {
        let dir = home.join("Library/Application Support/Forge");
        (dir.clone(), dir)
    }
    #[cfg(target_os = "linux")]
    {
        (home.join(".local/share/forge"), home.join(".config/forge"))
    }
}

fn git(repo: &Path, args: &[&str]) {
    let status = Command::new("git")
        .arg("-c")
        .arg("commit.gpgsign=false")
        .args(args)
        .current_dir(repo)
        .status()
        .expect("git");
    assert!(status.success(), "git {args:?}");
}

fn git_output(repo: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-c")
        .arg("commit.gpgsign=false")
        .args(args)
        .current_dir(repo)
        .output()
        .expect("git");
    assert!(output.status.success(), "git {args:?}");
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn tree(root: &Path) -> Vec<(PathBuf, Option<SystemTime>)> {
    fn walk(root: &Path, out: &mut Vec<(PathBuf, Option<SystemTime>)>) {
        let Ok(entries) = std::fs::read_dir(root) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let modified = std::fs::metadata(&path)
                .ok()
                .and_then(|meta| meta.modified().ok());
            out.push((path.clone(), modified));
            if path.is_dir() {
                walk(&path, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(root, &mut out);
    out.sort();
    out
}

fn json(output: &Output) -> serde_json::Value {
    assert!(
        output.status.success(),
        "forge-daemon failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("JSON report")
}

#[test]
fn dry_run_writes_nothing_and_reports_outcomes_and_transcripts() {
    let fixture = Fixture::new();
    let before = tree(&fixture.legacy);
    let output = fixture.run(&["migrate-home", "--dry-run", "--json"]);
    let report = json(&output);

    assert_eq!(report["dry_run"], true);
    assert_eq!(report["config"], "copied");
    assert_eq!(report["runs_copied"], 1);
    assert_eq!(
        before,
        tree(&fixture.legacy),
        "the legacy tree must not change"
    );
    assert!(
        !fixture.forge_home().exists(),
        "a dry run must not create the home"
    );

    let entries = report["worktrees"].as_array().expect("worktrees array");
    assert_eq!(entries.len(), 4);
    let find = |workspace: &Workspace| {
        entries
            .iter()
            .find(|entry| entry["path"] == workspace.path.to_string_lossy().as_ref())
            .unwrap_or_else(|| panic!("no report entry for {}", workspace.path.display()))
            .clone()
    };
    let movable = find(&fixture.movable);
    assert_eq!(movable["outcome"], "move");
    assert_eq!(movable["transcripts"], 1);
    assert_eq!(find(&fixture.locked)["reason"], "locked");
    assert_eq!(find(&fixture.submodules)["reason"], "submodules");
    assert_eq!(
        find(&fixture.rebasing)["reason"],
        "merge or rebase in progress"
    );
}

#[test]
fn a_running_daemon_lock_refuses_the_migration() {
    let fixture = Fixture::new();
    let _legacy_lock = daemon::lockfile::acquire(
        &fixture.legacy.join("app.db.lock"),
        "test",
        "0",
        Timestamp::now(),
    )
    .expect("hold the legacy data lock");
    let output = fixture.run(&["migrate-home", "--json"]);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("daemon is running"),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !fixture.forge_home().join("app.db").exists(),
        "nothing may be migrated while the daemon is alive"
    );

    let _singleton = daemon::lockfile::acquire(
        &fixture.socket().with_extension("sock.lock"),
        "test",
        "0",
        Timestamp::now(),
    )
    .expect("hold the singleton");
    let output = fixture.run(&["migrate-home", "--json"]);
    assert!(!output.status.success());
    assert!(!fixture.forge_home().join("app.db").exists());
}

#[test]
fn migration_copies_metadata_config_and_runs_and_leaves_worktrees_in_place() {
    let fixture = Fixture::new();
    let legacy_db = Db::open(&fixture.legacy.join("app.db")).expect("legacy db");
    let projects = legacy_db.projects().list().expect("legacy projects");
    let workspaces = legacy_db
        .workspaces()
        .list_by_project(fixture.project.id)
        .expect("legacy workspaces");
    let profiles = legacy_db.agent_profiles().list().expect("legacy profiles");
    drop(legacy_db);

    let output = fixture.run(&["migrate-home", "--json"]);
    let report = json(&output);
    assert_eq!(report["dry_run"], false);
    assert_eq!(report["config"], "copied");

    let new_db = Db::open(&fixture.forge_home().join("app.db")).expect("new db");
    let mut new_projects = new_db.projects().list().expect("new projects");
    let mut old_projects = projects.clone();
    new_projects.sort_by_key(|project| project.id.to_string());
    old_projects.sort_by_key(|project| project.id.to_string());
    assert_eq!(new_projects, old_projects);
    let mut new_workspaces = new_db
        .workspaces()
        .list_by_project(fixture.project.id)
        .expect("new workspaces");
    let mut old_workspaces = workspaces.clone();
    new_workspaces.sort_by_key(|workspace| workspace.id.to_string());
    old_workspaces.sort_by_key(|workspace| workspace.id.to_string());
    assert_eq!(new_workspaces, old_workspaces);
    assert_eq!(
        new_db.agent_profiles().list().expect("new profiles"),
        profiles
    );

    assert_eq!(
        std::fs::read_to_string(fixture.forge_home().join("config.toml")).expect("config"),
        std::fs::read_to_string(fixture.legacy_config.join("config.toml")).expect("legacy config")
    );
    assert!(fixture
        .forge_home()
        .join("runs/run-1/attempt-1.md")
        .is_file());
    let result_path: String = new_db
        .conn()
        .query_row(
            "SELECT result_path FROM attempts WHERE id='attempt-1'",
            [],
            |row| row.get(0),
        )
        .expect("result path");
    assert_eq!(
        PathBuf::from(result_path),
        fixture.forge_home().join("runs/run-1/attempt-1.md")
    );
    assert!(fixture.legacy.join("MIGRATED-TO").is_file());
    assert!(
        fixture.movable.path.is_dir(),
        "without --move-worktrees the old checkout stays"
    );

    let info = fixture.run(&["info"]);
    assert!(info.status.success());
    let stdout = String::from_utf8_lossy(&info.stdout);
    assert!(stdout.contains("layout:        home"), "{stdout}");
    assert!(
        stdout.contains("worktrees:     ~/forge-node/worktrees"),
        "{stdout}"
    );
}

#[test]
fn remove_legacy_refuses_while_a_managed_worktree_still_lives_there() {
    let fixture = Fixture::new();
    json(&fixture.run(&["migrate-home", "--json"]));

    let refused = fixture.run(&["migrate-home", "--remove-legacy", "--json"]);
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("worktree"));
    assert!(fixture.legacy.join("app.db").exists());

    // Once the new database holds no legacy worktree, only the old DB and the
    // regenerated plugins go; worktrees, config, logs and runs stay.
    let db = Db::open(&fixture.forge_home().join("app.db")).expect("new db");
    db.conn()
        .execute("DELETE FROM worktree_ignores", [])
        .expect("ignores");
    db.conn()
        .execute("DELETE FROM workspaces", [])
        .expect("workspaces");
    drop(db);
    let removed = json(&fixture.run(&["migrate-home", "--remove-legacy", "--json"]));
    assert!(!removed["removed"]
        .as_array()
        .expect("removed array")
        .is_empty());
    assert!(!fixture.legacy.join("app.db").exists());
    assert!(fixture.legacy_config.join("config.toml").exists());
    assert!(fixture.legacy.join("runs/run-1/attempt-1.md").is_file());
}

#[test]
fn move_worktrees_relocates_the_movable_one_and_skips_the_rest() {
    let fixture = Fixture::new();
    let destination = fixture.expected_destination(&fixture.movable);

    let output = fixture.run(&["migrate-home", "--move-worktrees", "--json"]);
    let report = json(&output);
    let entries = report["worktrees"].as_array().expect("worktrees array");
    let find = |workspace: &Workspace| {
        entries
            .iter()
            .find(|entry| entry["path"] == workspace.path.to_string_lossy().as_ref())
            .expect("matching worktree report")
            .clone()
    };
    assert_eq!(find(&fixture.movable)["outcome"], "moved");
    assert_eq!(find(&fixture.locked)["reason"], "locked");
    assert_eq!(find(&fixture.submodules)["reason"], "submodules");
    assert_eq!(
        find(&fixture.rebasing)["reason"],
        "merge or rebase in progress"
    );
    assert!(!fixture.movable.path.exists(), "the old checkout is gone");
    assert!(destination.join("README.md").is_file());

    let listing = git_output(&fixture.repo.path, &["worktree", "list", "--porcelain"]);
    assert!(
        listing.contains(destination.to_str().expect("UTF-8 destination")),
        "{listing}"
    );

    let new_db = Db::open(&fixture.forge_home().join("app.db")).expect("new db");
    let moved = new_db
        .workspaces()
        .get(fixture.movable.id)
        .expect("workspace")
        .expect("moved row");
    assert_eq!(moved.path, destination);
    let ignores = new_db.ignores().list_all().expect("ignores");
    assert_eq!(ignores.len(), 1);
    assert_eq!(ignores[0].path, destination.join("sub"));
    assert!(
        report["rollback"]
            .as_array()
            .expect("rollback")
            .iter()
            .any(|line| line.as_str().is_some_and(
                |line| line.contains(destination.to_str().expect("UTF-8 destination"))
            )),
        "rollback names the new path"
    );
    drop(new_db);

    let rerun = json(&fixture.run(&["migrate-home", "--move-worktrees", "--json"]));
    let rerun_entries = rerun["worktrees"].as_array().expect("worktrees array");
    assert!(
        rerun_entries.iter().all(|entry| entry["outcome"] != "move"),
        "a rerun moves nothing: {rerun_entries:?}"
    );
    assert!(destination.is_dir(), "a rerun is a no-op");
}
