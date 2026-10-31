use std::collections::{BTreeMap, HashMap, VecDeque};
use std::io::{BufReader, Read as _, Write as _};
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use nix::sys::signal::Signal;
use serde_json::{json, Value};

use crate::framing::{self, Frame};
use crate::location::definition_locations;
use crate::state::{clip, State};
use crate::uri::path_to_uri;
use crate::{Limits, Location, LspError, ServerConfig, Status};

const OUTGOING_CAPACITY: usize = 256;
/// Longest a waiter goes without looking at its caller's cancel flag, which
/// cannot wake it.
const CANCEL_POLL: Duration = Duration::from_millis(50);
const REAP_WINDOW: Duration = Duration::from_millis(500);
const REAP_POLL: Duration = Duration::from_millis(5);
const TERM_GRACE: Duration = Duration::from_secs(1);
const STDERR_DRAIN_WAIT: Duration = Duration::from_millis(200);
const READ_BUFFER: usize = 64 * 1024;
const MAX_LOG_BYTES: usize = 300;
const EXIT_STDERR_BYTES: usize = 1024;
/// Most items one `workspace/configuration` request may ask for; each answer
/// may be a copy of the whole settings tree.
const MAX_CONFIGURATION_ITEMS: usize = 256;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;

type Reply = Result<Value, LspError>;

/// One running language server.
///
/// Cloning shares the same process; it is killed and reaped when the last
/// clone is dropped, which blocks for up to about a second.
#[derive(Clone)]
pub struct Server {
    handle: Arc<Handle>,
}

/// Owned only by [`Server`] clones; the worker threads hold [`Shared`], so
/// this drops when the last caller lets go even while they still run.
struct Handle {
    shared: Arc<Shared>,
}

struct Shared {
    id: String,
    root: PathBuf,
    root_uri: String,
    settings: Value,
    limits: Limits,
    pid: i32,
    outgoing: Mutex<Option<flume::Sender<Vec<u8>>>>,
    state: Mutex<State>,
    changed: Condvar,
    docs: Mutex<DocSync>,
    pending: Mutex<HashMap<i64, flume::Sender<Reply>>>,
    next_id: AtomicI64,
    stderr: Mutex<VecDeque<u8>>,
    process: Mutex<Process>,
    last_used: Mutex<Instant>,
}

/// Document notifications are held here until `initialize` is answered,
/// because the protocol forbids sending them earlier; only the latest text of
/// each open document is kept, and it is replayed as one `didOpen`.
#[derive(Default)]
struct DocSync {
    initialized: bool,
    deferred: BTreeMap<PathBuf, Deferred>,
}

struct Deferred {
    language_id: String,
    version: i32,
    text: String,
}

struct Process {
    child: Option<Child>,
    status: Option<ExitStatus>,
    /// Set once the leader is reaped and its group is empty: from then on the
    /// pgid may name someone else's group and is never signalled again.
    group_gone: bool,
}

impl Server {
    /// Spawn the server and begin `initialize` in the background; the
    /// returned server is [`Status::Starting`].
    ///
    /// # Errors
    /// [`LspError::Spawn`] when the executable cannot be started, or
    /// [`LspError::Io`] when a pipe or worker thread cannot be set up (the
    /// process is then killed and reaped).
    pub fn start(config: ServerConfig) -> Result<Server, LspError> {
        let ServerConfig {
            id,
            command,
            args,
            root,
            env,
            initialization_options,
            settings,
            limits,
        } = config;
        let mut child = Command::new(&command)
            .args(&args)
            .current_dir(&root)
            .env_clear()
            .envs(env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .spawn()
            .map_err(LspError::Spawn)?;
        let pid = i32::try_from(child.id()).unwrap_or(0);
        let pipes = (child.stdin.take(), child.stdout.take(), child.stderr.take());
        let (outgoing, incoming) = flume::bounded(OUTGOING_CAPACITY);
        let shared = Arc::new(Shared {
            root_uri: path_to_uri(&root),
            id,
            root,
            settings,
            limits,
            pid,
            outgoing: Mutex::new(Some(outgoing)),
            state: Mutex::new(State::default()),
            changed: Condvar::new(),
            docs: Mutex::new(DocSync::default()),
            pending: Mutex::new(HashMap::new()),
            next_id: AtomicI64::new(1),
            stderr: Mutex::new(VecDeque::new()),
            process: Mutex::new(Process {
                child: Some(child),
                status: None,
                group_gone: false,
            }),
            last_used: Mutex::new(Instant::now()),
        });
        let handle = Handle {
            shared: Arc::clone(&shared),
        };
        let (Some(stdin), Some(stdout), Some(stderr)) = pipes else {
            return Err(LspError::Io(std::io::Error::other(
                "language server pipes were not created",
            )));
        };
        if pid <= 0 {
            if let Some(child) = lock(&shared.process).child.as_mut() {
                let _ = child.kill();
            }
            return Err(LspError::Io(std::io::Error::other(
                "language server pid does not fit a process group",
            )));
        }
        tracing::debug!(server = %shared.id, pid, command = %command.display(), "language server spawned");

        let (stderr_done_tx, stderr_done) = flume::bounded::<()>(0);
        shared.spawn_worker("stderr", move |shared| {
            drain_stderr(&shared, stderr);
            drop(stderr_done_tx);
        })?;
        shared.spawn_worker("writer", move |shared| {
            write_loop(&shared, stdin, &incoming)
        })?;
        shared.spawn_worker("reader", move |shared| {
            read_loop(&shared, stdout, &stderr_done);
        })?;
        shared.spawn_worker("init", move |shared| {
            shared.initialize(initialization_options);
        })?;
        Ok(Server {
            handle: Arc::new(handle),
        })
    }

    #[must_use]
    pub fn id(&self) -> &str {
        &self.shared().id
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.shared().root
    }

    /// The server's pid, which is also its process group id.
    #[must_use]
    pub fn process_id(&self) -> u32 {
        self.shared().pid.unsigned_abs()
    }

    #[must_use]
    pub fn status(&self) -> Status {
        lock(&self.shared().state).status()
    }

    /// Block until the server is [`Status::Ready`] or [`Status::Exited`], or
    /// `deadline` passes; returns the status at that moment.
    #[must_use]
    pub fn wait_ready(&self, deadline: Instant) -> Status {
        self.wait_for(deadline, |status| {
            matches!(status, Status::Ready | Status::Exited { .. })
        })
    }

    /// Block until `done` holds for the status or `deadline` passes; returns
    /// the status at that moment.
    pub fn wait_for(&self, deadline: Instant, done: impl Fn(&Status) -> bool) -> Status {
        self.shared().wait_until(deadline, None, done)
    }

    /// Notifications block their caller at most [`Limits::writer_stall`] on a
    /// full queue: dropping one would desynchronise the server's copy of the
    /// document, so a stalled server is declared exited and killed instead.
    pub fn did_open(&self, path: &Path, language_id: &str, version: i32, text: &str) {
        self.shared().document(
            path,
            DocOp::Open {
                language_id,
                version,
                text,
            },
        );
    }

    pub fn did_change_full(&self, path: &Path, version: i32, text: &str) {
        self.shared()
            .document(path, DocOp::Change { version, text });
    }

    pub fn did_save(&self, path: &Path) {
        self.shared().document(path, DocOp::Save);
    }

    pub fn did_close(&self, path: &Path) {
        self.shared().document(path, DocOp::Close);
    }

    /// `textDocument/definition` at a 0-based line and UTF-16 character.
    ///
    /// Waits for `initialize` within the same `deadline`. `cancel` is observed
    /// within ~50 ms; a cancelled or timed-out request is withdrawn with
    /// `$/cancelRequest`.
    ///
    /// # Errors
    /// [`LspError::Cancelled`], [`LspError::Timeout`], [`LspError::Busy`] when
    /// [`Limits::max_pending`] requests are in flight, [`LspError::Exited`],
    /// [`LspError::ResponseError`] for the server's own error, and
    /// [`LspError::Protocol`] for an answer over [`Limits::max_message_bytes`].
    pub fn definition(
        &self,
        path: &Path,
        line: u32,
        character: u32,
        deadline: Instant,
        cancel: &AtomicBool,
    ) -> Result<Vec<Location>, LspError> {
        let shared = self.shared();
        shared.touch();
        let status = shared.wait_until(deadline, Some(cancel), |status| {
            !matches!(status, Status::Starting)
        });
        if let Status::Exited { reason } = status {
            return Err(LspError::Exited(reason));
        }
        if cancel.load(Ordering::Relaxed) {
            return Err(LspError::Cancelled);
        }
        let params = json!({
            "textDocument": {"uri": shared.uri(path)},
            "position": {"line": line, "character": character},
        });
        let (id, reply) = {
            let docs = lock(&shared.docs);
            if !docs.initialized {
                return Err(LspError::Timeout);
            }
            let sent = shared.send_request("textDocument/definition", params, Some(deadline));
            drop(docs);
            sent?
        };
        let result = shared.await_reply(id, &reply, deadline, cancel)?;
        Ok(definition_locations(&result))
    }

    /// `shutdown` (waited for at most `grace`), `exit`, up to `grace` for the
    /// process to leave, then `SIGTERM` and `SIGKILL` to its whole process
    /// group and a bounded reap. Blocks at most about `3 × grace + 2 s`.
    pub fn shutdown(&self, grace: Duration) {
        let shared = &self.handle.shared;
        let never = AtomicBool::new(false);
        if lock(&shared.docs).initialized && lock(&shared.state).exited.is_none() {
            let deadline = Instant::now() + grace;
            match shared.send_request("shutdown", Value::Null, Some(deadline)) {
                Ok((id, reply)) => {
                    if let Err(error) = shared.await_reply(id, &reply, deadline, &never) {
                        tracing::debug!(server = %shared.id, %error, "shutdown request unanswered");
                    }
                }
                Err(error) => {
                    tracing::debug!(server = %shared.id, %error, "shutdown request not sent");
                }
            }
            shared.notify("exit", Value::Null, Some(Instant::now() + grace));
        }
        shared.wait_until(Instant::now() + grace, None, |status| {
            matches!(status, Status::Exited { .. })
        });
        shared.terminate(grace.min(TERM_GRACE));
        shared.mark_exited("shut down".to_owned());
    }

    /// The last [`Limits::stderr_ring_bytes`] the server wrote to stderr.
    #[must_use]
    pub fn stderr_tail(&self) -> String {
        self.shared().stderr_tail(usize::MAX)
    }

    /// When a document notification or request was last made.
    #[must_use]
    pub fn last_used(&self) -> Instant {
        *lock(&self.shared().last_used)
    }

    fn shared(&self) -> &Shared {
        &self.handle.shared
    }
}

impl std::fmt::Debug for Server {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Server")
            .field("id", &self.id())
            .field("pid", &self.shared().pid)
            .field("status", &self.status())
            .finish()
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        let shared = &self.shared;
        lock(&shared.outgoing).take();
        shared.signal(Signal::SIGKILL);
        shared.reap_or_delegate();
        shared.mark_exited("dropped".to_owned());
    }
}

enum DocOp<'a> {
    Open {
        language_id: &'a str,
        version: i32,
        text: &'a str,
    },
    Change {
        version: i32,
        text: &'a str,
    },
    Save,
    Close,
}

impl Shared {
    fn spawn_worker(
        self: &Arc<Self>,
        role: &str,
        work: impl FnOnce(Arc<Shared>) + Send + 'static,
    ) -> Result<(), LspError> {
        let shared = Arc::clone(self);
        std::thread::Builder::new()
            .name(format!("lsp-{}-{role}", self.id))
            .spawn(move || work(shared))
            .map(drop)
            .map_err(LspError::Io)
    }

    fn touch(&self) {
        *lock(&self.last_used) = Instant::now();
    }

    fn uri(&self, path: &Path) -> String {
        if path.is_absolute() {
            path_to_uri(path)
        } else {
            path_to_uri(&self.root.join(path))
        }
    }

    fn wait_until(
        &self,
        deadline: Instant,
        cancel: Option<&AtomicBool>,
        done: impl Fn(&Status) -> bool,
    ) -> Status {
        loop {
            let status = lock(&self.state).status();
            if done(&status) || cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
                return status;
            }
            let now = Instant::now();
            if now >= deadline {
                return status;
            }
            let mut wait = deadline - now;
            if cancel.is_some() {
                wait = wait.min(CANCEL_POLL);
            }
            let state = lock(&self.state);
            // A writer may have changed the status between the two locks; the
            // notification would already be spent, so re-check before sleeping.
            if state.status() != status {
                continue;
            }
            let _ = self
                .changed
                .wait_timeout(state, wait)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }

    fn initialize(&self, initialization_options: Value) {
        let deadline = Instant::now() + self.limits.initialize_timeout;
        let never = AtomicBool::new(false);
        let params = self.initialize_params(initialization_options);
        let result = self
            .send_request("initialize", params, Some(deadline))
            .and_then(|(id, reply)| self.await_reply(id, &reply, deadline, &never));
        let result = match result {
            Ok(result) => result,
            Err(error) => {
                self.fail(format!("initialize failed: {error}"));
                return;
            }
        };
        if let Some(encoding) = result
            .pointer("/capabilities/positionEncoding")
            .and_then(Value::as_str)
        {
            if encoding != "utf-16" {
                self.fail(format!(
                    "server requires position encoding {:?}; only utf-16 is spoken",
                    clip(encoding, MAX_LOG_BYTES)
                ));
                return;
            }
        }
        let mut docs = lock(&self.docs);
        let stall = Some(Instant::now() + self.limits.writer_stall);
        if !self.notify("initialized", json!({}), stall) {
            return;
        }
        for (path, doc) in std::mem::take(&mut docs.deferred) {
            let params = json!({"textDocument": {
                "uri": path_to_uri(&path),
                "languageId": doc.language_id,
                "version": doc.version,
                "text": doc.text,
            }});
            if !self.notify("textDocument/didOpen", params, stall) {
                return;
            }
        }
        docs.initialized = true;
        drop(docs);
        lock(&self.state).initialized = true;
        self.changed.notify_all();
        tracing::debug!(server = %self.id, "language server initialized");
    }

    fn initialize_params(&self, initialization_options: Value) -> Value {
        let name = self
            .root
            .file_name()
            .map_or_else(|| self.root.to_string_lossy(), |n| n.to_string_lossy());
        json!({
            "processId": std::process::id(),
            "clientInfo": {"name": "forge-node"},
            "rootPath": self.root.to_string_lossy(),
            "rootUri": self.root_uri,
            "workspaceFolders": [{"uri": self.root_uri, "name": name}],
            "initializationOptions": initialization_options,
            "capabilities": {
                "general": {"positionEncodings": ["utf-16"]},
                "textDocument": {
                    "definition": {"dynamicRegistration": false, "linkSupport": true},
                    "synchronization": {"dynamicRegistration": false, "didSave": true},
                },
                "window": {"workDoneProgress": true},
                "workspace": {"configuration": true, "workspaceFolders": true},
                "experimental": {"serverStatusNotification": true},
            },
            "trace": "off",
        })
    }

    fn document(&self, path: &Path, op: DocOp<'_>) {
        self.touch();
        let path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.root.join(path)
        };
        let mut docs = lock(&self.docs);
        if lock(&self.state).exited.is_some() {
            return;
        }
        if !docs.initialized {
            match op {
                DocOp::Open {
                    language_id,
                    version,
                    text,
                } => {
                    docs.deferred.insert(
                        path,
                        Deferred {
                            language_id: language_id.to_owned(),
                            version,
                            text: text.to_owned(),
                        },
                    );
                }
                DocOp::Change { version, text } => {
                    if let Some(doc) = docs.deferred.get_mut(&path) {
                        doc.version = version;
                        text.clone_into(&mut doc.text);
                    }
                }
                DocOp::Save => {}
                DocOp::Close => {
                    docs.deferred.remove(&path);
                }
            }
            return;
        }
        let uri = path_to_uri(&path);
        let (method, params) = match op {
            DocOp::Open {
                language_id,
                version,
                text,
            } => (
                "textDocument/didOpen",
                json!({"textDocument": {"uri": uri, "languageId": language_id, "version": version, "text": text}}),
            ),
            DocOp::Change { version, text } => (
                "textDocument/didChange",
                json!({"textDocument": {"uri": uri, "version": version}, "contentChanges": [{"text": text}]}),
            ),
            DocOp::Save => (
                "textDocument/didSave",
                json!({"textDocument": {"uri": uri}}),
            ),
            DocOp::Close => (
                "textDocument/didClose",
                json!({"textDocument": {"uri": uri}}),
            ),
        };
        self.notify(method, params, None);
        drop(docs);
    }

    /// Queue a notification; `false` when the server is gone or stalled.
    fn notify(&self, method: &str, params: Value, deadline: Option<Instant>) -> bool {
        let mut message = json!({"jsonrpc": "2.0", "method": method});
        if !params.is_null() {
            message["params"] = params;
        }
        self.enqueue(&message, deadline).is_ok()
    }

    fn send_request(
        &self,
        method: &str,
        params: Value,
        deadline: Option<Instant>,
    ) -> Result<(i64, flume::Receiver<Reply>), LspError> {
        if let Some(reason) = lock(&self.state).exited.clone() {
            return Err(LspError::Exited(reason));
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = flume::bounded(1);
        {
            let mut pending = lock(&self.pending);
            if pending.len() >= self.limits.max_pending {
                return Err(LspError::Busy);
            }
            pending.insert(id, tx);
        }
        let mut message = json!({"jsonrpc": "2.0", "id": id, "method": method});
        if !params.is_null() {
            message["params"] = params;
        }
        if let Err(error) = self.enqueue(&message, deadline) {
            lock(&self.pending).remove(&id);
            return Err(error);
        }
        Ok((id, rx))
    }

    fn await_reply(
        &self,
        id: i64,
        reply: &flume::Receiver<Reply>,
        deadline: Instant,
        cancel: &AtomicBool,
    ) -> Reply {
        loop {
            if cancel.load(Ordering::Relaxed) {
                self.abandon(id);
                return Err(LspError::Cancelled);
            }
            let now = Instant::now();
            if now >= deadline {
                self.abandon(id);
                return Err(LspError::Timeout);
            }
            match reply.recv_timeout((deadline - now).min(CANCEL_POLL)) {
                Ok(answer) => return answer,
                Err(flume::RecvTimeoutError::Timeout) => {}
                Err(flume::RecvTimeoutError::Disconnected) => return Err(self.exited_error()),
            }
        }
    }

    /// Withdraw a request; `$/cancelRequest` is advisory, so it is dropped
    /// rather than waited for when the queue is full.
    fn abandon(&self, id: i64) {
        if lock(&self.pending).remove(&id).is_none() {
            return;
        }
        let message = json!({"jsonrpc": "2.0", "method": "$/cancelRequest", "params": {"id": id}});
        let Some(tx) = lock(&self.outgoing).clone() else {
            return;
        };
        if let Ok(body) = serde_json::to_vec(&message) {
            let _ = tx.try_send(framing::encode(&body));
        }
    }

    /// Queue one message. `deadline` is the caller's own budget; past
    /// [`Limits::writer_stall`] the server is declared stalled and killed.
    fn enqueue(&self, message: &Value, deadline: Option<Instant>) -> Result<(), LspError> {
        let body = serde_json::to_vec(message).map_err(|e| LspError::Protocol(e.to_string()))?;
        let Some(tx) = lock(&self.outgoing).clone() else {
            return Err(self.exited_error());
        };
        let stall_at = Instant::now() + self.limits.writer_stall;
        let until = deadline.map_or(stall_at, |d| d.min(stall_at));
        match tx.send_deadline(framing::encode(&body), until) {
            Ok(()) => Ok(()),
            Err(flume::SendTimeoutError::Timeout(_)) if until < stall_at => Err(LspError::Timeout),
            Err(flume::SendTimeoutError::Timeout(_)) => {
                let reason = format!(
                    "writer stalled: outgoing queue full for {:?}",
                    self.limits.writer_stall
                );
                self.fail(reason.clone());
                Err(LspError::Exited(reason))
            }
            Err(flume::SendTimeoutError::Disconnected(_)) => Err(self.exited_error()),
        }
    }

    fn exited_error(&self) -> LspError {
        LspError::Exited(
            lock(&self.state)
                .exited
                .clone()
                .unwrap_or_else(|| "server connection closed".to_owned()),
        )
    }

    /// Give up on the server: it is marked exited and its group killed, which
    /// ends the reader and lets it reap.
    fn fail(&self, reason: String) {
        tracing::debug!(server = %self.id, %reason, "language server failed");
        self.mark_exited(reason);
        self.signal(Signal::SIGKILL);
    }

    /// The first reason wins; every pending request fails with it.
    fn mark_exited(&self, reason: String) {
        {
            let mut state = lock(&self.state);
            if state.exited.is_none() {
                state.exited = Some(reason);
            }
        }
        self.changed.notify_all();
        lock(&self.outgoing).take();
        lock(&self.pending).clear();
    }

    fn dispatch(&self, mut message: Value) {
        let method = message
            .get("method")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let id = message.get_mut("id").map(Value::take);
        let params = message.get_mut("params").map_or(Value::Null, Value::take);
        match (method, id) {
            (Some(method), Some(id)) => self.answer(&method, id, &params),
            (Some(method), None) => self.on_notification(&method, &params),
            (None, Some(id)) => self.on_response(&id, &mut message),
            (None, None) => {
                tracing::debug!(server = %self.id, "message without method or id dropped")
            }
        }
    }

    fn on_response(&self, id: &Value, message: &mut Value) {
        let Some(sender) = id.as_i64().and_then(|id| lock(&self.pending).remove(&id)) else {
            tracing::debug!(server = %self.id, %id, "reply for unknown request dropped");
            return;
        };
        let reply = match message.get("error") {
            Some(error) => Err(LspError::ResponseError {
                code: error.get("code").and_then(Value::as_i64).unwrap_or(0),
                message: clip(
                    error.get("message").and_then(Value::as_str).unwrap_or(""),
                    MAX_LOG_BYTES,
                )
                .to_owned(),
            }),
            None => Ok(message.get_mut("result").map_or(Value::Null, Value::take)),
        };
        let _ = sender.try_send(reply);
    }

    fn on_oversized(&self, len: usize, prefix: &[u8]) {
        tracing::debug!(server = %self.id, len, limit = self.limits.max_message_bytes, "oversized message discarded");
        let Some(sender) =
            framing::response_id_hint(prefix).and_then(|id| lock(&self.pending).remove(&id))
        else {
            return;
        };
        let _ = sender.try_send(Err(LspError::Protocol(format!(
            "response of {len} bytes exceeds the {}-byte limit",
            self.limits.max_message_bytes
        ))));
    }

    fn on_notification(&self, method: &str, params: &Value) {
        match method {
            "$/progress" => {
                lock(&self.state).apply_progress(params);
                self.changed.notify_all();
            }
            "experimental/serverStatus" => {
                lock(&self.state).apply_server_status(params);
                self.changed.notify_all();
            }
            "window/logMessage" | "window/showMessage" => {
                let message = params.get("message").and_then(Value::as_str).unwrap_or("");
                tracing::debug!(server = %self.id, method, message = clip(message, MAX_LOG_BYTES), "server message");
            }
            _ => tracing::debug!(server = %self.id, method, "notification dropped"),
        }
    }

    /// Every server request is answered: a server that waits on an unanswered
    /// one can stall its whole queue.
    fn answer(&self, method: &str, id: Value, params: &Value) {
        let result = match method {
            "workspace/configuration" => self.configuration(params),
            "client/registerCapability"
            | "client/unregisterCapability"
            | "window/workDoneProgress/create"
            | "window/showMessageRequest" => Ok(Value::Null),
            "workspace/applyEdit" => Ok(json!({"applied": false})),
            "workspace/workspaceFolders" => {
                let name = self
                    .root
                    .file_name()
                    .map_or_else(|| self.root.to_string_lossy(), |n| n.to_string_lossy());
                Ok(json!([{"uri": self.root_uri, "name": name}]))
            }
            method if method.ends_with("/refresh") => Ok(Value::Null),
            _ => Err((
                METHOD_NOT_FOUND,
                format!("method not found: {}", clip(method, MAX_LOG_BYTES)),
            )),
        };
        let message = match result {
            Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
            Err((code, message)) => {
                json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
            }
        };
        if let Err(error) = self.enqueue(&message, None) {
            tracing::debug!(server = %self.id, method, %error, "answer to server request not sent");
        }
    }

    fn configuration(&self, params: &Value) -> Result<Value, (i64, String)> {
        let items = params
            .get("items")
            .and_then(Value::as_array)
            .map_or(&[][..], Vec::as_slice);
        if items.len() > MAX_CONFIGURATION_ITEMS {
            return Err((INVALID_PARAMS, "too many configuration items".to_owned()));
        }
        Ok(Value::Array(
            items
                .iter()
                .map(|item| {
                    settings_section(&self.settings, item.get("section").and_then(Value::as_str))
                })
                .collect(),
        ))
    }

    fn push_stderr(&self, bytes: &[u8]) {
        let cap = self.limits.stderr_ring_bytes;
        if cap == 0 {
            return;
        }
        let bytes = &bytes[bytes.len().saturating_sub(cap)..];
        let mut ring = lock(&self.stderr);
        let excess = (ring.len() + bytes.len()).saturating_sub(cap);
        ring.drain(..excess);
        ring.extend(bytes);
    }

    fn stderr_tail(&self, max: usize) -> String {
        let ring = lock(&self.stderr);
        let skip = ring.len().saturating_sub(max);
        let bytes: Vec<u8> = ring.iter().skip(skip).copied().collect();
        String::from_utf8_lossy(&bytes).into_owned()
    }

    /// Signal the whole group, then the leader directly while it is still
    /// ours to signal: a group kill alone misses a leader that left its group.
    fn signal(&self, signal: Signal) {
        let mut process = lock(&self.process);
        if process.group_gone || self.pid <= 0 {
            return;
        }
        // A reaped leader whose group is empty must not be signalled again:
        // the pgid is its old pid, and a recycled pid may now lead someone
        // else's group.
        if process.status.is_some() && !group_alive(self.pid) {
            process.group_gone = true;
            return;
        }
        let _ = nix::sys::signal::kill(nix::unistd::Pid::from_raw(-self.pid), signal);
        if process.child.is_some() && process.status.is_none() {
            let _ = nix::sys::signal::kill(nix::unistd::Pid::from_raw(self.pid), signal);
        }
        if process.status.is_some() && !group_alive(self.pid) {
            process.group_gone = true;
        }
    }

    /// Reap the leader within `window` by polling `WNOHANG`; nothing can wake
    /// a waiter on another thread's child, so this is the bounded exception to
    /// the no-polling rule.
    fn reap(&self, window: Duration) -> Option<ExitStatus> {
        let deadline = Instant::now() + window;
        loop {
            {
                let mut process = lock(&self.process);
                if let Some(status) = process.status {
                    return Some(status);
                }
                let child = process.child.as_mut()?;
                match child.try_wait() {
                    Ok(Some(status)) => {
                        process.status = Some(status);
                        process.child = None;
                        return Some(status);
                    }
                    Ok(None) => {}
                    Err(error) => {
                        tracing::debug!(server = %self.id, %error, "language server cannot be waited on");
                        process.child = None;
                        return None;
                    }
                }
            }
            if Instant::now() >= deadline {
                return None;
            }
            std::thread::sleep(REAP_POLL);
        }
    }

    /// Reap within [`REAP_WINDOW`], killing the group if the leader outlives
    /// it, and hand a still-unreaped leader to a thread that waits for it.
    fn reap_or_delegate(self: &Arc<Self>) -> Option<ExitStatus> {
        if let Some(status) = self.reap(REAP_WINDOW) {
            return Some(status);
        }
        self.signal(Signal::SIGKILL);
        if let Some(status) = self.reap(REAP_WINDOW) {
            return Some(status);
        }
        let mut child = lock(&self.process).child.take()?;
        tracing::warn!(server = %self.id, pid = self.pid, "language server still alive after SIGKILL; reaping in the background");
        let owner = Arc::clone(self);
        let spawned = std::thread::Builder::new()
            .name(format!("lsp-{}-reap", self.id))
            .spawn(move || {
                if let Ok(status) = child.wait() {
                    lock(&owner.process).status = Some(status);
                }
            });
        if let Err(error) = spawned {
            tracing::warn!(server = %self.id, %error, "cannot spawn reaper; the process may stay a zombie");
        }
        None
    }

    /// `SIGTERM` the group, wait up to `grace` for it to empty, then `SIGKILL`
    /// and reap. Group death has no notification, so the wait polls.
    fn terminate(self: &Arc<Self>, grace: Duration) {
        if !lock(&self.process).group_gone && group_alive(self.pid) {
            self.signal(Signal::SIGTERM);
            let deadline = Instant::now() + grace;
            while Instant::now() < deadline {
                self.reap(Duration::ZERO);
                if !group_alive(self.pid) {
                    break;
                }
                std::thread::sleep(REAP_POLL * 2);
            }
        }
        self.signal(Signal::SIGKILL);
        self.reap_or_delegate();
        self.signal(Signal::SIGKILL);
    }
}

/// `kill(-pgid, 0)`; a group that holds only zombies still answers.
fn group_alive(pgid: i32) -> bool {
    pgid > 0 && nix::sys::signal::kill(nix::unistd::Pid::from_raw(-pgid), None).is_ok()
}

fn settings_section(settings: &Value, section: Option<&str>) -> Value {
    let Some(section) = section.filter(|s| !s.is_empty()) else {
        return settings.clone();
    };
    if let Some(value) = settings.get(section) {
        return value.clone();
    }
    section
        .split('.')
        .try_fold(settings, |value, key| value.get(key))
        .cloned()
        .unwrap_or(Value::Null)
}

fn drain_stderr(shared: &Shared, mut stderr: ChildStderr) {
    let mut buffer = [0u8; 4096];
    loop {
        match stderr.read(&mut buffer) {
            Ok(0) => return,
            Ok(read) => shared.push_stderr(&buffer[..read]),
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return,
        }
    }
}

fn write_loop(shared: &Shared, mut stdin: ChildStdin, incoming: &flume::Receiver<Vec<u8>>) {
    while let Ok(frame) = incoming.recv() {
        if let Err(error) = stdin.write_all(&frame) {
            tracing::debug!(server = %shared.id, %error, "write to language server failed");
            // A server that cannot be written to is useless; killing it lets
            // the reader report the real exit status rather than a broken pipe.
            shared.signal(Signal::SIGKILL);
            return;
        }
    }
}

fn read_loop(shared: &Arc<Shared>, stdout: ChildStdout, stderr_done: &flume::Receiver<()>) {
    let mut reader = BufReader::with_capacity(READ_BUFFER, stdout);
    let failure = loop {
        match framing::read_frame(&mut reader, shared.limits.max_message_bytes) {
            Ok(Some(Frame::Message(body))) => match serde_json::from_slice(&body) {
                Ok(message) => shared.dispatch(message),
                Err(error) => {
                    tracing::debug!(server = %shared.id, %error, "unparseable message dropped");
                }
            },
            Ok(Some(Frame::Oversized { len, prefix })) => shared.on_oversized(len, &prefix),
            Ok(None) => break None,
            Err(error) => break Some(format!("unreadable server output: {error}")),
        }
    };
    if failure.is_some() {
        shared.signal(Signal::SIGKILL);
    }
    let _ = stderr_done.recv_timeout(STDERR_DRAIN_WAIT);
    let status = shared.reap_or_delegate();
    let mut reason = failure.unwrap_or_else(|| match status {
        Some(status) => format!("{} exited ({status})", shared.id),
        None => format!("{} closed its output", shared.id),
    });
    let tail = shared.stderr_tail(EXIT_STDERR_BYTES);
    let tail = tail.trim();
    if !tail.is_empty() {
        reason.push_str("; stderr: ");
        reason.push_str(tail);
    }
    tracing::debug!(server = %shared.id, %reason, "language server exited");
    shared.mark_exited(reason);
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_sections_resolve_by_key_then_path() {
        let settings = json!({
            "rust-analyzer": {"cargo": {"features": "all"}},
            "python.analysis": {"typeCheckingMode": "basic"},
        });
        assert_eq!(settings_section(&settings, None), settings);
        assert_eq!(settings_section(&settings, Some("")), settings);
        assert_eq!(
            settings_section(&settings, Some("rust-analyzer.cargo.features")),
            json!("all")
        );
        assert_eq!(
            settings_section(&settings, Some("python.analysis")),
            json!({"typeCheckingMode": "basic"})
        );
        assert_eq!(
            settings_section(&settings, Some("rust-analyzer.missing")),
            Value::Null
        );
        assert_eq!(settings_section(&settings, Some("nope")), Value::Null);
    }
}
