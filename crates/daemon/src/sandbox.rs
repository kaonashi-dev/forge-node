//! TREX in the daemon: the repository's sandbox targets for an `apex`
//! review, or by hand. Runs on its caller's worker thread, never under the
//! core lock; the file is always the user's checkout's, never a pull request's.

use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use domain::{ReviewId, SandboxRun, SandboxTarget, Timestamp, WorkspaceId};
use protocol::{DaemonEvent, ErrorCode, ProtocolError, Response};

use crate::core::{db_err, Daemon};

const WORKTREES_DIR: &str = "review-sandbox";
const CACHE_DIR: &str = ".forge/review/sandbox/cache";

/// Remove every run worktree a crash left behind; run before any review can start.
pub(crate) fn sweep(home: &Path) {
    if let Err(error) = std::fs::remove_dir_all(home.join(WORKTREES_DIR)) {
        if error.kind() != std::io::ErrorKind::NotFound {
            tracing::warn!(%error, "could not remove leftover sandbox worktrees");
        }
    }
}

/// A pull request's head checked out on its own, removed with the run.
pub(crate) struct HeadWorktree {
    repo: PathBuf,
    pub(crate) path: PathBuf,
}

impl HeadWorktree {
    pub(crate) fn add(home: &Path, repo: &Path, run: &str, head: &str) -> Result<Self, String> {
        let parent = home.join(WORKTREES_DIR);
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&parent)
            .map_err(|error| error.to_string())?;
        let path = parent.join(run);
        let target = path.to_string_lossy().into_owned();
        let out = git_service::run_git(Some(repo), &["worktree", "add", "--detach", &target, head])
            .map_err(|error| error.to_string())?;
        if !out.success() {
            return Err(format!("git worktree add failed: {}", out.stderr.trim()));
        }
        Ok(Self {
            repo: repo.to_path_buf(),
            path,
        })
    }
}

impl Drop for HeadWorktree {
    fn drop(&mut self) {
        if let Err(error) =
            git_service::worktree::remove(&self.repo, &self.path, git_service::worktree::Force::Yes)
        {
            tracing::warn!(%error, "could not remove a sandbox worktree");
        }
    }
}

/// Clears the workspace's in-flight mark however the run ends.
struct SandboxGuard {
    daemon: Arc<Daemon>,
    workspace_id: WorkspaceId,
}

impl Drop for SandboxGuard {
    fn drop(&mut self) {
        self.daemon.lock().sandboxes.remove(&self.workspace_id);
    }
}

fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

impl Daemon {
    /// Every target of `checkout`'s `sandbox.toml` (or just `only`), run in
    /// `run_dir`, stored and returned. Never fails: what stopped it is `error`.
    pub(crate) fn run_sandbox_targets(
        &self,
        id: String,
        workspace_id: WorkspaceId,
        review_id: Option<ReviewId>,
        checkout: &Path,
        run_dir: &Path,
        only: Option<&str>,
    ) -> SandboxRun {
        let mut run = SandboxRun {
            id,
            workspace_id,
            review_id,
            targets: Vec::new(),
            error: None,
            started_at: Timestamp::now(),
            finished_at: None,
        };
        if let Err(error) = self.sandbox_targets(&mut run, checkout, run_dir, only) {
            run.error = Some(error);
        }
        run.finished_at = Some(Timestamp::now());
        if let Err(error) = self.review_store.save_sandbox_run(&run) {
            tracing::warn!(%error, "could not store a sandbox run");
        }
        run
    }

    fn sandbox_targets(
        &self,
        run: &mut SandboxRun,
        checkout: &Path,
        run_dir: &Path,
        only: Option<&str>,
    ) -> Result<(), String> {
        if !cfg!(target_os = "macos") {
            return Err(runtime_sandbox::UNSUPPORTED.into());
        }
        let text = crate::review::read_repo_file(
            checkout,
            runtime_sandbox::CONFIG_PATH,
            runtime_sandbox::MAX_CONFIG_BYTES,
        )?
        .ok_or_else(|| format!("no {}", runtime_sandbox::CONFIG_PATH))?;
        let config = runtime_sandbox::parse(&text)?;
        let security = crate::review::repo_config(checkout).security;
        let cache = checkout.join(CACHE_DIR);
        std::fs::create_dir_all(&cache)
            .map_err(|error| format!("could not create {CACHE_DIR}: {error}"))?;
        let ignore = checkout.join(".forge/review/sandbox/.gitignore");
        if !ignore.exists() {
            std::fs::write(&ignore, "*\n").map_err(|error| error.to_string())?;
        }
        let mut denied = vec![canonical(&self.forge_paths().home)];
        if let Some(socket) = self
            .bound_socket_path()
            .and_then(|socket| socket.parent().map(Path::to_path_buf))
        {
            denied.push(canonical(&socket));
        }
        let policy = runtime_sandbox::Policy {
            run_dir: canonical(run_dir),
            cache_dir: canonical(&cache),
            tmp_dir: canonical(&std::env::temp_dir()),
            denied_reads: denied,
            network: security.sandbox_network,
            socket: self.bound_socket_path().map(|socket| canonical(&socket)),
        };
        let resolved = |name: &str| self.launch_var(name).or_else(|| std::env::var(name).ok());
        let env = runtime_sandbox::environment(
            &security.sandbox_env_whitelist,
            &resolved,
            &config.env,
            &policy.cache_dir,
        );
        let targets: Vec<&runtime_sandbox::Target> = config
            .targets
            .iter()
            .filter(|target| only.is_none_or(|name| target.name == name))
            .collect();
        if targets.is_empty() {
            return Err(format!(
                "sandbox.toml has no target `{}`",
                only.unwrap_or_default()
            ));
        }
        for target in targets {
            let result = runtime_sandbox::run_target(target, config.timeout, &policy, &env)?;
            run.targets.push(SandboxTarget {
                name: result.name,
                exit_code: result.exit_code,
                duration_ms: u64::try_from(result.duration.as_millis()).unwrap_or(u64::MAX),
                timed_out: result.timed_out,
                stdout: review::redact(&result.stdout).0,
                stderr: review::redact(&result.stderr).0,
            });
        }
        Ok(())
    }

    pub(crate) fn run_sandbox(
        self: &Arc<Self>,
        workspace_id: WorkspaceId,
        target: Option<String>,
    ) -> Result<Response, ProtocolError> {
        let checkout = self.workspace_path(workspace_id)?;
        if !self.lock().sandboxes.insert(workspace_id) {
            return Err(ProtocolError::conflict(
                "a sandbox run is already in flight for this workspace",
            ));
        }
        let guard = SandboxGuard {
            daemon: Arc::clone(self),
            workspace_id,
        };
        let id = domain::SessionId::new().as_uuid().to_string();
        let daemon = Arc::clone(self);
        let run_id = id.clone();
        std::thread::Builder::new()
            .name("forge-sandbox".into())
            .spawn(move || {
                let _guard = guard;
                let run = daemon.run_sandbox_targets(
                    run_id,
                    workspace_id,
                    None,
                    &checkout,
                    &checkout,
                    target.as_deref(),
                );
                daemon
                    .registry
                    .broadcast_domain(DaemonEvent::SandboxFinished(Box::new(run)));
            })
            .map_err(|error| {
                ProtocolError::new(
                    ErrorCode::IoError,
                    format!("cannot start the sandbox: {error}"),
                )
            })?;
        Ok(Response::SandboxStarted { sandbox_run_id: id })
    }

    pub(crate) fn get_sandbox_run(&self, id: &str) -> Result<Response, ProtocolError> {
        self.review_store
            .sandbox_run(id)
            .map_err(db_err)?
            .map(|run| Response::SandboxRun(Box::new(run)))
            .ok_or_else(|| ProtocolError::not_found("sandbox run"))
    }
}
