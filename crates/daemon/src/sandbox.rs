//! Sandbox checks in the daemon: the repository's targets for a `deep`
//! review, or by hand. Runs on its caller's worker thread, never under the
//! core lock; the file is always the user's checkout's, never a pull request's,
//! and runs only while it hashes to the copy the person approved for the project.

use std::io::Write as _;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use domain::{ReviewId, SandboxRun, SandboxTarget, Timestamp, WorkspaceId};
use protocol::{DaemonEvent, ErrorCode, ProtocolError, Response};

use crate::core::{db_err, Daemon};

const WORKTREES_DIR: &str = "review-sandbox";
const CACHE_DIR: &str = ".forge/review/sandbox/cache";

/// The review a run belongs to, and what ends its targets early: both are
/// checked before each target starts.
pub(crate) struct ForReview<'a> {
    pub(crate) id: ReviewId,
    pub(crate) deadline: Instant,
    pub(crate) cancelled: &'a dyn Fn() -> bool,
}

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

pub(crate) fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

impl Daemon {
    /// Forge's home and socket directory, then the socket itself: what no sandbox may reach.
    pub(crate) fn forge_denied(&self) -> (Vec<PathBuf>, Option<PathBuf>) {
        let socket = self.bound_socket_path();
        let mut denied = vec![canonical(&self.forge_paths().home)];
        if let Some(dir) = socket.as_deref().and_then(Path::parent) {
            denied.push(canonical(dir));
        }
        (denied, socket.map(|socket| canonical(&socket)))
    }
}

/// `<checkout>/.forge/review/sandbox/cache`, created and canonical. The
/// repository commits what it likes, so a symlink on the way, or a path that
/// resolves outside the checkout, is refused rather than made writable.
fn cache_dir(checkout: &Path) -> Result<PathBuf, String> {
    let root = std::fs::canonicalize(checkout)
        .map_err(|error| format!("cannot resolve the checkout: {error}"))?;
    let mut path = root.clone();
    for part in Path::new(CACHE_DIR).components() {
        path.push(part);
        match std::fs::symlink_metadata(&path) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(format!(
                    "{} is a symlink; sandbox checks will not write through it",
                    path.strip_prefix(&root).unwrap_or(&path).display()
                ));
            }
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                return Err(error.to_string());
            }
            _ => {}
        }
    }
    std::fs::create_dir_all(&path)
        .map_err(|error| format!("could not create {CACHE_DIR}: {error}"))?;
    let cache = std::fs::canonicalize(&path).map_err(|error| error.to_string())?;
    if !cache.starts_with(&root) {
        return Err(format!("{CACHE_DIR} resolves outside the checkout"));
    }
    // `create_new` never follows a symlink, dangling or not.
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(cache.with_file_name(".gitignore"))
    {
        Ok(mut file) => file.write_all(b"*\n").map_err(|error| error.to_string())?,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.to_string()),
    }
    Ok(cache)
}

impl Daemon {
    /// Every target of `checkout`'s `checks.toml` (or just `only`), run in
    /// `run_dir`, stored and returned. Never fails: what stopped it is `error`.
    pub(crate) fn run_sandbox_targets(
        &self,
        id: String,
        workspace_id: WorkspaceId,
        review: Option<ForReview<'_>>,
        checkout: &Path,
        run_dir: &Path,
        only: Option<&str>,
    ) -> SandboxRun {
        let mut run = SandboxRun {
            id,
            workspace_id,
            review_id: review.as_ref().map(|review| review.id),
            targets: Vec::new(),
            error: None,
            unapproved_hash: None,
            started_at: Timestamp::now(),
            finished_at: None,
        };
        if let Err(error) = self.sandbox_targets(&mut run, review.as_ref(), checkout, run_dir, only)
        {
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
        review: Option<&ForReview<'_>>,
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
        .ok_or_else(|| runtime_sandbox::missing(checkout))?;
        self.require_approved(run, &text)?;
        let config = runtime_sandbox::parse(&text)?;
        let cache = cache_dir(checkout)?;
        let (denied_reads, socket) = self.forge_denied();
        let policy = runtime_sandbox::Policy {
            run_dir: canonical(run_dir),
            cache_dir: cache,
            tmp_dir: canonical(&std::env::temp_dir()),
            extra_writes: Vec::new(),
            denied_reads,
            network: self.config.review.sandbox_network,
            socket,
        };
        let resolved = |name: &str| self.launch_var(name).or_else(|| std::env::var(name).ok());
        let env = runtime_sandbox::environment(
            &self.config.review.sandbox_env_whitelist,
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
                "checks.toml has no target `{}`",
                only.unwrap_or_default()
            ));
        }
        for target in targets {
            let timeout = match review {
                None => config.timeout,
                Some(review) => {
                    let stopped = if (review.cancelled)() {
                        Some("the review was cancelled")
                    } else if Instant::now() >= review.deadline {
                        Some("the review's time budget ran out")
                    } else {
                        None
                    };
                    if let Some(why) = stopped {
                        return Err(format!("stopped before `{}`: {why}", target.name));
                    }
                    config
                        .timeout
                        .min(review.deadline.saturating_duration_since(Instant::now()))
                }
            };
            let result = runtime_sandbox::run_target(target, timeout, &policy, &env)?;
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

    /// Refuses, recording the hash for `ApproveSandboxChecks`, unless `text`
    /// is the copy the person approved for the workspace's project.
    fn require_approved(&self, run: &mut SandboxRun, text: &str) -> Result<(), String> {
        let project = self
            .workspace_project(run.workspace_id)
            .map_err(|error| error.message)?;
        let approved = self
            .review_store
            .approved_sandbox(project)
            .map_err(|error| error.to_string())?;
        let file_hash = crate::actions::repo::hash(text.as_bytes());
        if approved.as_deref() == Some(file_hash.as_str()) {
            return Ok(());
        }
        run.unapproved_hash = Some(file_hash);
        Err(format!(
            "{} {}; nothing ran",
            runtime_sandbox::CONFIG_PATH,
            if approved.is_some() {
                "changed since it was approved"
            } else {
                "is not approved for this project"
            }
        ))
    }

    pub(crate) fn approve_sandbox_checks(
        &self,
        workspace_id: WorkspaceId,
        file_hash: &str,
    ) -> Result<Response, ProtocolError> {
        let checkout = self.workspace_path(workspace_id)?;
        let project = self.workspace_project(workspace_id)?;
        let text = crate::review::read_repo_file(
            &checkout,
            runtime_sandbox::CONFIG_PATH,
            runtime_sandbox::MAX_CONFIG_BYTES,
        )
        .map_err(|error| {
            ProtocolError::invalid_request(format!("{}: {error}", runtime_sandbox::CONFIG_PATH))
        })?
        .ok_or_else(|| ProtocolError::not_found(runtime_sandbox::CONFIG_PATH))?;
        if crate::actions::repo::hash(text.as_bytes()) != file_hash {
            return Err(ProtocolError::precondition_failed(format!(
                "{} changed; read it again before approving it",
                runtime_sandbox::CONFIG_PATH
            )));
        }
        runtime_sandbox::parse(&text).map_err(ProtocolError::invalid_request)?;
        self.review_store
            .approve_sandbox(project, file_hash)
            .map_err(db_err)?;
        Ok(Response::Ack)
    }

    pub(crate) fn run_sandbox_checks(
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
                    .broadcast_domain(DaemonEvent::SandboxChecksFinished(Box::new(run)));
            })
            .map_err(|error| {
                ProtocolError::new(
                    ErrorCode::IoError,
                    format!("cannot start the sandbox: {error}"),
                )
            })?;
        Ok(Response::SandboxChecksStarted { sandbox_run_id: id })
    }

    pub(crate) fn get_sandbox_checks(&self, id: &str) -> Result<Response, ProtocolError> {
        self.review_store
            .sandbox_run(id)
            .map_err(db_err)?
            .map(|run| Response::SandboxChecks(Box::new(run)))
            .ok_or_else(|| ProtocolError::not_found("sandbox run"))
    }
}
