//! Headless agent runs: spawn a provider CLI, follow its event stream, exit.
//!
//! The sibling of [`crate::terminal`], and the contrast is the point. A
//! terminal is a PTY the user talks to for as long as they like; a job is a
//! plain child process with a piped stdout that answers one prompt and stops.
//! Nothing here allocates a grid, a `TerminalId` or a subscription, because a
//! job has nothing to render — it has a stream of JSON lines and an exit code.
//!
//! Three things follow from that, and they are the whole reason jobs exist:
//!
//! - **Completion is observable.** The harness knows a step finished because
//!   the process exited, not because someone read a terminal and decided it
//!   looked done.
//! - **Runs are cheap.** Several can be in flight without each one owning a
//!   tab, which is what lets one feature drive a spec, an implementation and a
//!   review as separate agents — across different providers.
//! - **Nothing new is trusted with credentials.** The binary is the one
//!   detection verified, started from the same login-shell environment as an
//!   interactive launch, so it reads the same subscription login the user
//!   already has. There is no API-key path here, deliberately.
//!
//! The stream itself is written to disk verbatim, one file per job, and only
//! *referenced* by the job row: a run of any length would otherwise be
//! re-broadcast whole on every state change.

use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use domain::{HeadlessSpec, Job, JobId, JobRequest, JobState, Timestamp};
use protocol::{error::ProtocolError, event::DaemonEvent, response::Response, ErrorCode};

use crate::core::Daemon;

/// How many jobs may run at once, by default.
///
/// The ceiling is not about CPU: every job spends the *same* account rate
/// limit as the interactive sessions next to it, and a fan-out that drains the
/// five-hour window leaves the user unable to work. Two is enough to overlap a
/// review with the next spec and small enough to stay out of the way.
pub(crate) const DEFAULT_MAX_CONCURRENT_JOBS: usize = 2;

/// How often output lines are flushed to clients while a job runs.
///
/// A busy provider emits hundreds of lines a second; one event each would cost
/// more than the run. Coalescing on this interval is the same bargain
/// `TerminalActivity` makes.
const OUTPUT_FLUSH: Duration = Duration::from_millis(120);

/// How often the *row* is republished while a job runs.
///
/// Slower than [`OUTPUT_FLUSH`] on purpose: a `Job` carries its whole prompt,
/// and a live line on a card does not need eight updates a second.
const ROW_FLUSH: Duration = Duration::from_secs(1);

/// How often the watchdog looks at what is running.
pub(crate) const WATCHDOG_TICK: Duration = Duration::from_secs(30);

/// How long the job thread waits for a child that has already closed its pipes.
///
/// Longer than `core.rs`'s PTY window because a headless agent flushes a
/// transcript and its own subprocesses on the way out, where a shell behind a
/// tty is simply gone. The number matters less than the bound existing:
/// `AGENTS.md` names an unbounded `wait()` on the calling thread as its own
/// bug, and this thread also drives the job queue.
const REAP_WINDOW: Duration = Duration::from_secs(2);
/// Interval between `try_wait` polls inside [`REAP_WINDOW`].
const REAP_POLL: Duration = Duration::from_millis(10);

/// Reap a finished job's child, or hand it to a detached thread.
///
/// Returns the exit code, or `None` when the child outlived the window — a job
/// that closed stdout and stderr but is still running. `Child::drop` does not
/// wait, so the process would otherwise stay a zombie for the life of the
/// daemon; the detached `wait` is what collects it.
fn reap_job(id: JobId, mut child: Child) -> Option<i32> {
    let deadline = Instant::now() + REAP_WINDOW;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.code().or(Some(-1)),
            // The wait itself failed: nothing left to collect.
            Err(_) => return None,
            Ok(None) => {}
        }
        if Instant::now() >= deadline {
            tracing::warn!(
                %id,
                "job output ended but the process is still alive; reaping in the background"
            );
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            return None;
        }
        std::thread::sleep(REAP_POLL);
    }
}

/// A running job's process handle, kept only so it can be killed.
pub(crate) struct JobProcess {
    pub(crate) child: Child,
}

impl Daemon {
    /// Accept a headless run: `Response::Job` as soon as it is queued.
    ///
    /// Answering before the process starts is deliberate. Jobs queue behind
    /// [`DEFAULT_MAX_CONCURRENT_JOBS`], so "accepted" and "running" are
    /// genuinely different moments, and a caller that needs the second one
    /// waits for the `JobUpdated` that announces it — the same shape as
    /// `FetchRemote` acking before the fetch completes.
    pub(crate) fn start_job(
        self: &Arc<Self>,
        request: JobRequest,
    ) -> Result<Response, ProtocolError> {
        self.start_job_with_id(JobId::new(), request)
    }

    /// Start a job whose id the caller already knows.
    ///
    /// The harness needs the id *before* the run, because the event it writes
    /// to `harness/` has to name the job — that record is the only thing left
    /// pointing at the transcript once the daemon that held the job is gone.
    pub(crate) fn start_job_with_id(
        self: &Arc<Self>,
        id: JobId,
        request: JobRequest,
    ) -> Result<Response, ProtocolError> {
        if self.resetting.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(ProtocolError::conflict("factory reset is in progress"));
        }
        let descriptor = {
            let inner = self.lock();
            inner
                .agents
                .descriptor(&request.provider_id)
                .cloned()
                .ok_or_else(|| ProtocolError::not_found("agent provider"))?
        };
        let Some(headless) = descriptor.headless.clone() else {
            return Err(ProtocolError::new(
                ErrorCode::InvalidRequest,
                format!(
                    "{} has no headless mode; it can only run in a terminal",
                    descriptor.display_name
                ),
            ));
        };
        if request.resume_from.is_some() && headless.resume.is_none() {
            return Err(ProtocolError::new(
                ErrorCode::InvalidRequest,
                format!("{} cannot resume a headless run", descriptor.display_name),
            ));
        }
        let cwd = {
            let inner = self.lock();
            inner
                .workspaces
                .get(&request.workspace_id)
                .map(|workspace| workspace.path.clone())
                .ok_or_else(|| ProtocolError::not_found("workspace"))?
        };
        // Resolved before the job is queued: a provider that is not installed
        // is a refusal, not a job that fails a minute later for a reason the
        // caller has to read a log to discover.
        let executable = self.verified_agent_executable(&request.provider_id)?;

        let log_path = self.job_log_path(id)?;
        // Harness steps write their artefacts under the canonical harness
        // root, outside the worktree checkout they run in — a sandboxed
        // provider cannot reach it otherwise. Scoped by feature, which only
        // harness jobs carry: the lieutenant only reads.
        let extra_writable_dirs: Vec<PathBuf> = match request.feature_id {
            Some(_) => {
                let inner = self.lock();
                crate::core::harness_root_in(&inner, &cwd)
                    .into_iter()
                    .collect()
            }
            None => Vec::new(),
        };
        let job = Job {
            id,
            provider_id: request.provider_id.clone(),
            workspace_id: request.workspace_id,
            role: request.role,
            feature_id: request.feature_id,
            parent_session_id: request.parent_session_id,
            state: JobState::Queued,
            summary: Job::summarize(&request.prompt),
            prompt: request.prompt.clone(),
            provider_session_id: None,
            exit_code: None,
            last_line: None,
            last_output_at: None,
            started_at: Timestamp::now(),
            finished_at: None,
            log_path: log_path.clone(),
        };
        {
            let mut inner = self.lock();
            inner.jobs.insert(id, job.clone());
            inner.job_queue.push_back(PendingJob {
                id,
                log_path: log_path.clone(),
                executable,
                headless,
                cwd,
                extra_writable_dirs,
                prompt: request.prompt,
                resume_from: request.resume_from,
                schema: request.schema,
            });
        }
        self.broadcast_job(&job);
        self.pump_job_queue();
        Ok(Response::Job(Box::new(job)))
    }

    /// Kill a running job, drop a queued one, ignore a finished one.
    pub(crate) fn cancel_job(self: &Arc<Self>, id: JobId) -> Result<Response, ProtocolError> {
        let process;
        let job = {
            let mut inner = self.lock();
            let Some(job) = inner.jobs.get(&id).cloned() else {
                return Err(ProtocolError::not_found("job"));
            };
            if job.state.is_final() {
                return Ok(Response::Ack);
            }
            inner.job_queue.retain(|pending| pending.id != id);
            process = inner.job_processes.remove(&id);
            let job = inner.jobs.get_mut(&id).expect("checked above");
            job.state = JobState::Cancelled;
            job.finished_at = Some(Timestamp::now());
            job.clone()
        };
        if let Some(process) = process {
            stop_job_process(process);
        }
        self.broadcast_job(&job);
        self.pump_job_queue();
        Ok(Response::Ack)
    }

    /// Atomically stop the queue and forget every job for a factory reset.
    pub(crate) fn cancel_all_jobs(&self) {
        let processes = {
            let mut inner = self.lock();
            inner.job_queue.clear();
            inner.jobs.clear();
            inner
                .job_processes
                .drain()
                .map(|(_, process)| process)
                .collect::<Vec<_>>()
        };
        for process in processes {
            stop_job_process(process);
        }
    }

    /// Every job this daemon has run, oldest first.
    pub(crate) fn list_jobs(&self) -> Response {
        let inner = self.lock();
        let mut jobs: Vec<Job> = inner.jobs.values().cloned().collect();
        jobs.sort_by_key(|job| job.id.as_uuid());
        Response::Jobs(jobs)
    }

    /// A slice of one job's event stream, from `from_line`.
    pub(crate) fn read_job_log(
        &self,
        id: JobId,
        from_line: u64,
    ) -> Result<Response, ProtocolError> {
        let known = {
            let inner = self.lock();
            inner
                .jobs
                .get(&id)
                .map(|job| (job.log_path.clone(), job.state.is_final()))
        };
        // A job this daemon never ran is not an error: jobs live in memory, so
        // a restart forgets every row while the transcripts stay on disk, and
        // the harness event log still names them by id. The path is derived
        // from the id, so an id is enough to serve one — finished, necessarily,
        // since nothing here is running it.
        let (path, finished) = match known {
            Some(known) => known,
            None => (self.job_log_path(id)?, true),
        };
        let lines = match File::open(&path) {
            Ok(file) => BufReader::new(file)
                .lines()
                .skip(usize::try_from(from_line).unwrap_or(usize::MAX))
                .map_while(Result::ok)
                // The same summary the live events carry, so a step opened
                // after it started reads as one stream and not two.
                .map(|line| agents::summarize_stream_line(&line))
                .collect(),
            // A job that has not written its first line yet has no file, which
            // is an empty log rather than an error.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(ProtocolError::new(ErrorCode::IoError, e.to_string())),
        };
        Ok(Response::JobLog {
            from_line,
            lines,
            finished,
        })
    }

    /// Start queued jobs while there is room under the concurrency ceiling.
    fn pump_job_queue(self: &Arc<Self>) {
        loop {
            let pending = {
                let mut inner = self.lock();
                if inner.job_processes.len() >= inner.max_concurrent_jobs {
                    return;
                }
                match inner.job_queue.pop_front() {
                    Some(pending) => pending,
                    None => return,
                }
            };
            // A job cancelled while it waited is simply gone from the map.
            if !self
                .lock()
                .jobs
                .get(&pending.id)
                .is_some_and(|job| job.state == JobState::Queued)
            {
                continue;
            }
            self.spawn_job(pending);
        }
    }

    /// Spawn one job's process and the thread that follows it.
    fn spawn_job(self: &Arc<Self>, pending: PendingJob) {
        let id = pending.id;
        // A schema the provider takes as a *file* has to become one first; the
        // file sits beside the job's log so it outlives the run and can be
        // read back when a verdict looks wrong.
        let schema_arg = match (&pending.schema, pending.headless.schema.as_ref()) {
            (Some(schema), Some(domain::SchemaStyle::File { .. })) => {
                let path = pending.log_path_sibling("schema.json");
                match std::fs::write(&path, schema) {
                    Ok(()) => Some(path.to_string_lossy().into_owned()),
                    Err(error) => {
                        tracing::warn!(%id, %error, "could not write the job's schema file");
                        None
                    }
                }
            }
            (Some(schema), Some(domain::SchemaStyle::Inline { .. })) => Some(schema.clone()),
            _ => None,
        };
        let args = pending.headless.command_args_with_dirs(
            &pending.prompt,
            pending.resume_from.as_deref(),
            schema_arg.as_deref(),
            &pending.extra_writable_dirs,
        );
        let (vars, harness_root) = self.job_environment(&pending.cwd);
        let mut command = Command::new(&pending.executable);
        command
            .args(&args)
            .current_dir(&pending.cwd)
            .env_clear()
            .envs(vars)
            // Nothing types at a job, and a CLI that finds an open stdin may
            // wait on it forever instead of exiting.
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // Its own process group, so a cancel can take the CLI's own
            // children — a bash tool call, an MCP server — with it. Without
            // this the child shares the daemon's group and killing the group
            // would kill the daemon.
            .process_group(0);
        if let Some(root) = harness_root {
            command.env("FORGE_HARNESS_ROOT", root);
        }
        command.env("FORGE_JOB_ID", id.to_string());

        let child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                self.finish_job(
                    id,
                    None,
                    Some(format!("could not start the agent: {error}")),
                );
                return;
            }
        };
        let started = {
            let mut inner = self.lock();
            inner.jobs.get_mut(&id).and_then(|job| {
                if job.state != JobState::Queued {
                    return None;
                }
                job.state = JobState::Running;
                job.started_at = Timestamp::now();
                Some(job.clone())
            })
        };
        let Some(started) = started else {
            stop_job_process(JobProcess { child });
            return;
        };
        self.broadcast_job(&started);

        let daemon = Arc::clone(self);
        let session_id_fields = pending.headless.session_id_fields.clone();
        let log_path = started.log_path.clone();
        std::thread::spawn(move || {
            daemon.follow_job(id, child, log_path, session_id_fields);
        });
    }

    /// Read one job's stream to the end, then reap it.
    ///
    /// Runs on its own thread and never holds the core lock across a read: the
    /// process is the slow part, and everything else in the daemon — a
    /// keystroke included — goes through that lock.
    fn follow_job(
        self: &Arc<Self>,
        id: JobId,
        mut child: Child,
        log_path: PathBuf,
        session_id_fields: Vec<String>,
    ) {
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let mut child = Some(child);
        let accepted = {
            let mut inner = self.lock();
            if inner
                .jobs
                .get(&id)
                .is_some_and(|job| job.state == JobState::Running)
            {
                if let Some(child) = child.take() {
                    inner.job_processes.insert(id, JobProcess { child });
                    true
                } else {
                    false
                }
            } else {
                false
            }
        };
        if !accepted {
            if let Some(child) = child {
                stop_job_process(JobProcess { child });
            }
            return;
        }
        // stderr is drained on its own thread so a provider that writes a lot
        // there cannot fill the pipe and deadlock the run. The drain reports
        // through a channel rather than a `JoinHandle`, because the wait below
        // has to be *bounded*: a grandchild holding the write end keeps this
        // pipe open after the child is gone, and a plain `join()` there would
        // block the job thread for the life of the daemon.
        let (stderr_done, stderr_drained) = std::sync::mpsc::channel::<()>();
        if let Some(stderr) = stderr {
            std::thread::spawn(move || {
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    tracing::debug!(%id, line, "job stderr");
                }
                let _ = stderr_done.send(());
            });
        } else {
            drop(stderr_done);
        }

        let mut log = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .ok();
        let mut next_line: u64 = 0;
        let mut batch: Vec<String> = Vec::new();
        let mut batch_start = next_line;
        let mut last_flush = Instant::now();
        let mut last_row = Instant::now();
        let mut last_line = None;

        if let Some(stdout) = stdout {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if let Some(log) = log.as_mut() {
                    let _ = writeln!(log, "{line}");
                }
                if let Some(found) = session_id_in(&line, &session_id_fields) {
                    self.record_provider_session(id, found);
                }
                // Same summary JobOutput carries: the row is what a card
                // prints, and the raw JSON belongs in the log file only.
                last_line = Some(agents::summarize_stream_line(&line));
                if batch.is_empty() {
                    batch_start = next_line;
                }
                batch.push(line);
                next_line += 1;
                if last_flush.elapsed() >= OUTPUT_FLUSH {
                    self.flush_job_output(id, batch_start, &mut batch);
                    // The heartbeat, on the row rather than in this frame: the
                    // watchdog and the Features panel then read one fact
                    // instead of each inventing its own. Broadcast on a slower
                    // clock than the output events because a `Job` carries the
                    // whole prompt and a card only needs the newest line.
                    let announce = last_row.elapsed() >= ROW_FLUSH;
                    self.note_job_output(id, last_line.as_deref(), announce);
                    if announce {
                        last_row = Instant::now();
                    }
                    last_flush = Instant::now();
                }
            }
        }
        self.flush_job_output(id, batch_start, &mut batch);
        // Bounded: the drain ends on EOF, and a detached grandchild can hold
        // that pipe open indefinitely. Past the window the thread is left to
        // finish on its own — it still logs what arrives, this thread just
        // stops waiting for it.
        if stderr_drained.recv_timeout(REAP_WINDOW).is_err() {
            tracing::debug!(%id, "job stdout closed while stderr is still open");
        }

        let code = {
            let process = {
                let mut inner = self.lock();
                inner.job_processes.remove(&id)
            };
            // `None` means it was reaped elsewhere — a cancel got there first.
            process.and_then(|process| reap_job(id, process.child))
        };
        self.finish_job(id, code, last_line);
        self.pump_job_queue();
    }

    /// Send the lines gathered since the last flush.
    fn flush_job_output(&self, id: JobId, from_line: u64, batch: &mut Vec<String>) {
        if batch.is_empty() {
            return;
        }
        // Summarised on the way out, not on disk: the `.jsonl` file keeps the
        // provider's own words as the record, and this event exists to be
        // *watched*. A client that wants the raw stream reads the file.
        let lines = std::mem::take(batch)
            .iter()
            .map(|line| agents::summarize_stream_line(line))
            .collect();
        self.registry.broadcast_domain(DaemonEvent::JobOutput {
            job_id: id,
            from_line,
            lines,
        });
    }

    /// Record that a job is still producing output.
    ///
    /// `announce` decides whether clients hear about it; the row is updated
    /// either way, because the watchdog reads the row and not the events.
    fn note_job_output(self: &Arc<Self>, id: JobId, last_line: Option<&str>, announce: bool) {
        let job = {
            let mut inner = self.lock();
            let Some(job) = inner.jobs.get_mut(&id) else {
                return;
            };
            if let Some(line) = last_line {
                job.last_line = Some(line.to_owned());
            }
            job.last_output_at = Some(Timestamp::now());
            if announce {
                Some(job.clone())
            } else {
                None
            }
        };
        if let Some(job) = job {
            self.broadcast_job(&job);
        }
    }

    /// End a job the watchdog has decided is over, as a *failure*.
    ///
    /// Not [`Self::cancel_job`]: a cancel is a person's decision and the
    /// harness deliberately leaves the feature where it is, while a job that
    /// spent its wall clock is a failed attempt and has to reach the retry
    /// budget like any other.
    pub(crate) fn expire_job(self: &Arc<Self>, id: JobId, reason: String) {
        let process = {
            let mut inner = self.lock();
            inner.job_queue.retain(|pending| pending.id != id);
            inner.job_processes.remove(&id)
        };
        if let Some(process) = process {
            stop_job_process(process);
        }
        tracing::warn!(%id, reason, "job expired");
        self.finish_job(id, Some(-1), Some(reason));
        self.pump_job_queue();
    }

    /// Note the provider's own session id the first time the stream says it.
    fn record_provider_session(self: &Arc<Self>, id: JobId, provider_session_id: String) {
        let job = {
            let mut inner = self.lock();
            let Some(job) = inner.jobs.get_mut(&id) else {
                return;
            };
            if job.provider_session_id.as_deref() == Some(provider_session_id.as_str()) {
                return;
            }
            job.provider_session_id = Some(provider_session_id.clone());
            job.clone()
        };
        self.broadcast_job(&job);
        // Persist onto the harness attempt so a RetryStep can resume after the
        // daemon forgets the in-memory job row.
        if let Some(feature_id) = job.feature_id {
            let root = {
                let inner = self.lock();
                let cwd = inner
                    .workspaces
                    .get(&job.workspace_id)
                    .map(|workspace| workspace.path.clone());
                cwd.and_then(|cwd| crate::core::harness_root_in(&inner, &cwd))
            };
            if let Some(root) = root {
                if let Err(error) = harness_service::attach_provider_session(
                    &root,
                    feature_id,
                    &id.to_string(),
                    &provider_session_id,
                ) {
                    tracing::warn!(%id, %error, "could not persist provider session on attempt");
                } else if let Some(project_id) = {
                    let inner = self.lock();
                    inner
                        .workspaces
                        .get(&job.workspace_id)
                        .map(|workspace| workspace.project_id)
                } {
                    self.broadcast_harness_feature(project_id, &root, feature_id);
                }
            }
        }
    }

    /// Move a job to its final state and tell everyone.
    ///
    /// A job cancelled while it ran keeps `Cancelled`: the exit code of a
    /// process we killed says nothing the user wants to read.
    fn finish_job(self: &Arc<Self>, id: JobId, exit_code: Option<i32>, last_line: Option<String>) {
        let job = {
            let mut inner = self.lock();
            let Some(job) = inner.jobs.get_mut(&id) else {
                return;
            };
            // Idempotent: a job killed by the watchdog or by a cancel is
            // finished here *and* again when its output thread reaches EOF, and
            // settling a harness step twice would spend two retries for one
            // failure.
            if job.state.is_final() {
                return;
            }
            if job.state != JobState::Cancelled {
                job.state = match exit_code {
                    Some(0) => JobState::Succeeded,
                    _ => JobState::Failed,
                };
                job.exit_code = exit_code;
            }
            if last_line.is_some() {
                job.last_line = last_line;
            }
            job.finished_at = Some(Timestamp::now());
            job.clone()
        };
        tracing::info!(%id, state = ?job.state, exit_code = ?job.exit_code, "job finished");
        self.broadcast_job(&job);
        // A job that belongs to a harness feature moves it: this is the whole
        // point of a run that ends observably (`crate::harness_runner`).
        self.advance_harness_after_job(&job);
    }

    fn broadcast_job(&self, job: &Job) {
        self.registry
            .broadcast_domain(DaemonEvent::JobUpdated(Box::new(job.clone())));
    }
}

fn stop_job_process(mut process: JobProcess) {
    // The whole group, never the single pid: the CLI's own children would
    // otherwise outlive it, exactly as for a PTY.
    if let Ok(pgid) = i32::try_from(process.child.id()) {
        crate::core::signal_group(pgid, nix::sys::signal::Signal::SIGTERM);
    }
    let _ = process.child.kill();
    let _ = process.child.wait();
}

/// A job that has been accepted but has no process yet.
pub(crate) struct PendingJob {
    pub(crate) id: JobId,
    /// Where this job's own files go, so a schema can sit beside its log.
    pub(crate) log_path: PathBuf,
    pub(crate) executable: PathBuf,
    pub(crate) headless: HeadlessSpec,
    pub(crate) cwd: PathBuf,
    /// Canonical roots the provider may write outside its cwd (the harness
    /// root for harness steps, empty otherwise).
    pub(crate) extra_writable_dirs: Vec<PathBuf>,
    pub(crate) prompt: String,
    pub(crate) resume_from: Option<String>,
    pub(crate) schema: Option<String>,
}

impl PendingJob {
    /// A path next to this job's log, named `<job id>.<suffix>`.
    fn log_path_sibling(&self, suffix: &str) -> PathBuf {
        self.log_path
            .with_file_name(format!("{}.{suffix}", self.id))
    }
}

/// The provider's own session id carried by one line of its event stream.
///
/// Lives here rather than on [`HeadlessSpec`] because the `domain` crate
/// depends on `serde` alone and has no JSON parser (§17); the descriptor
/// carries the field names, this reads them. A line that is not JSON at all —
/// a provider that streams plain text — is simply not a line that carries one.
pub(crate) fn session_id_in(line: &str, fields: &[String]) -> Option<String> {
    if fields.is_empty() {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    fields.iter().find_map(|field| {
        value
            .get(field)
            .and_then(serde_json::Value::as_str)
            .filter(|id| !id.is_empty())
            .map(str::to_owned)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The ordinary case: a child that has already exited is reaped at once,
    /// with its code, and no thread is left behind.
    #[test]
    fn a_finished_child_reports_its_code() {
        let child = Command::new("sh")
            .args(["-c", "exit 3"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn");
        assert_eq!(reap_job(JobId::new(), child), Some(3));
    }

    #[test]
    fn a_signalled_child_reports_a_code_rather_than_nothing() {
        let child = Command::new("sh")
            .args(["-c", "kill -TERM $$"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn");
        // Killed by a signal: `status.code()` is `None`, and the job still has
        // to finish with *something* rather than look like a cancel.
        assert_eq!(reap_job(JobId::new(), child), Some(-1));
    }

    /// The bug this bounds. A child that closes its pipes and keeps running
    /// used to block the job thread — and with it the whole job queue — in an
    /// unbounded `wait()`, which `AGENTS.md` names as its own defect.
    #[test]
    fn a_child_that_outlives_its_output_does_not_block_the_queue() {
        let mut child = Command::new("sh")
            .args(["-c", "exec sleep 30"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn");
        let pid = child.id();

        let start = Instant::now();
        let code = reap_job(JobId::new(), child_from(&mut child));
        let waited = start.elapsed();

        assert_eq!(code, None, "a child still running has no code to report");
        assert!(
            waited < REAP_WINDOW * 3,
            "reap_job waited {waited:?}, past its own window"
        );
        // Clean up the process the detached reaper is now waiting on.
        let _ = Command::new("kill").arg(pid.to_string()).status();
    }

    /// `reap_job` takes the `Child` by value; the test needs the pid first, so
    /// this hands the same process over without cloning the handle.
    fn child_from(child: &mut Child) -> Child {
        std::mem::replace(
            child,
            Command::new("true").spawn().expect("spawn placeholder"),
        )
    }

    #[test]
    fn a_session_id_is_read_from_the_first_field_that_carries_one() {
        let fields = vec!["session_id".to_owned(), "conversation_id".to_owned()];
        assert_eq!(
            session_id_in(r#"{"type":"system","session_id":"abc"}"#, &fields),
            Some("abc".to_owned())
        );
        // Falls through to the second spelling.
        assert_eq!(
            session_id_in(r#"{"conversation_id":"def"}"#, &fields),
            Some("def".to_owned())
        );
    }

    /// The lines a stream is mostly made of carry no id, and a provider that
    /// emits plain text carries none at all. Neither is an error.
    #[test]
    fn a_line_without_an_id_is_not_a_failure() {
        let fields = vec!["session_id".to_owned()];
        assert_eq!(session_id_in(r#"{"type":"assistant"}"#, &fields), None);
        assert_eq!(session_id_in("thinking about it…", &fields), None);
        assert_eq!(session_id_in(r#"{"session_id":""}"#, &fields), None);
        assert_eq!(session_id_in(r#"{"session_id":"x"}"#, &[]), None);
    }
}
