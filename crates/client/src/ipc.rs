//! Synchronous protocol client over a Unix domain socket (§10, §10.5).
//!
//! This is the GUI side of the wire. It intentionally uses only `std`
//! (`std::os::unix::net::UnixStream` plus threads) and `flume` channels — never
//! `tokio` and never a GUI toolkit — so `client` stays on the `client → protocol →
//! domain` spine of §17 and the GUI thread never blocks on IO.
//!
//! [`Client::connect`] performs the §9.2 handshake, then spawns **one** reader
//! thread that decodes [`DaemonMessage`]s with a [`protocol::FrameDecoder`] and
//! routes them: a `Response` wakes the request waiter registered under its
//! `request_id`; an `Event` is pushed onto the events channel the GUI drains.
//! Requests are correlated with an [`AtomicU64`] id and a per-request bounded
//! oneshot; the socket's write half is guarded by a `Mutex` so request writers
//! and the reader thread (which owns the read half via
//! [`UnixStream::try_clone`]) never fight. On EOF or socket error the reader
//! marks the client disconnected, drops every pending waiter (waking each with
//! [`ClientError::Disconnected`]) and closes the events channel.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use domain::{
    AgentProfile, AgentProfileId, AgentProviderId, ProjectId, PtySize, SessionId, SessionRole,
    ShareAction, ShareCandidate, ShareCleanup, ShareRule, ShareRuleId, ShareStatusEntry,
    TerminalId, WorkspaceId,
};
use protocol::{
    decode_payload, encode_frame, ClientKind, ClientMessage, DaemonEvent, DaemonMessage,
    FrameDecoder, Hello, ProtocolCodecError, ProtocolError, RemoveProjectPolicy, Request, Response,
    PROTOCOL_VERSION,
};

/// Size of the reader thread's socket read buffer (matches the daemon PTY loop's
/// 64 KiB reads, §11.2).
const READ_BUF: usize = 64 * 1024;

/// Errors raised by the GUI protocol client.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ClientError {
    /// An underlying socket I/O operation failed.
    #[error("i/o error talking to the daemon")]
    Io(#[from] std::io::Error),
    /// The daemon answered the handshake with something other than a valid
    /// `HelloAck`/`HelloReject`, i.e. a protocol violation during connect.
    #[error("daemon rejected the handshake: {reason}")]
    HandshakeRejected {
        /// Human-readable reason.
        reason: String,
    },
    /// The client and daemon speak different protocol versions (§9.2). Also the
    /// mapping for a `HelloReject`.
    #[error("protocol version mismatch: client speaks {expected}, daemon requires {got}")]
    VersionMismatch {
        /// The version this client speaks ([`PROTOCOL_VERSION`]).
        expected: u32,
        /// The version the daemon requires.
        got: u32,
    },
    /// The connection to the daemon is closed.
    #[error("the connection to the daemon is closed")]
    Disconnected,
    /// The daemon returned a structured error for the request (§10.1).
    #[error(transparent)]
    Protocol(#[from] ProtocolError),
    /// Framing or (de)serialization of a message failed locally.
    #[error(transparent)]
    Codec(#[from] ProtocolCodecError),
    /// A [`Client::request_timeout`] elapsed before a response arrived.
    #[error("the request timed out")]
    Timeout,
    /// The daemon returned a successful response of the wrong shape.
    #[error("daemon returned an unexpected response; expected {expected}")]
    UnexpectedResponse { expected: &'static str },
}

/// Outcome of [`Client::send_context`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SendContextResult {
    /// Envelope stored; optional PTY paste to an existing target.
    Delivered,
    /// A child session was spawned with the context as its initial prompt.
    Spawned {
        session_id: SessionId,
        terminal_id: TerminalId,
    },
}

/// The daemon instance a [`Client`] is connected to (from the §9.2 `HelloAck`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DaemonInfo {
    /// The protocol version the daemon speaks.
    pub protocol_version: u32,
    /// The daemon's build version, informational.
    pub daemon_version: String,
    /// The daemon instance identifier from `daemon.lock` (§9.2).
    pub instance_id: String,
    /// When this daemon instance started.
    pub started_at: domain::Timestamp,
}

/// A project's branches and the context the picker needs around them.
///
/// A named struct rather than a tuple: the three parts are read together and
/// each answers a different question — what to list, whether to offer a fetch
/// at all, and what a new branch should start from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Branches {
    /// Local and remote-tracking branches, newest commit first.
    pub branches: Vec<domain::BranchRef>,
    /// Configured remotes; empty means a local-only repository.
    pub remotes: Vec<domain::Remote>,
    /// The repository's default branch, the base for a new one.
    pub default_branch: Option<String>,
}

/// A oneshot sender delivering one request's result to its blocked caller.
type Waiter = flume::Sender<Result<Response, ProtocolError>>;

/// State shared between the [`Client`] handle and its reader thread.
struct Shared {
    /// The socket's write half, serialized across request writers.
    write: Mutex<UnixStream>,
    /// Request waiters keyed by `request_id`.
    pending: Mutex<HashMap<u64, Waiter>>,
    /// Monotonic source of `request_id`s.
    next_id: AtomicU64,
    /// `false` once the reader observed EOF/error or the handle was dropped.
    connected: AtomicBool,
    /// The connected daemon's handshake info.
    daemon: DaemonInfo,
    /// Kept so [`Client::events`] can hand out clones; the reader owns the
    /// matching sender and closes the channel on disconnect.
    events_rx: flume::Receiver<DaemonEvent>,
}

impl Shared {
    /// Mark the connection dead and wake every pending waiter by dropping its
    /// sender (their receivers then observe [`ClientError::Disconnected`]).
    fn disconnect(&self) {
        self.connected.store(false, Ordering::SeqCst);
        if let Ok(mut pending) = self.pending.lock() {
            pending.clear();
        }
    }
}

/// The GUI protocol client and passive owner of the reader thread.
///
/// Clone-free by design: dropping it disconnects and joins the reader. Wrap it
/// in an `Arc` if several parts of the GUI need to share one connection.
pub struct Client {
    shared: Arc<Shared>,
    reader: Option<JoinHandle<()>>,
}

impl Client {
    /// Connect to the daemon at `socket_path`, perform the §9.2 handshake as a
    /// [`ClientKind::Gui`] client, and spawn the reader thread.
    ///
    /// # Errors
    /// - [`ClientError::Io`] if the socket cannot be reached or written.
    /// - [`ClientError::VersionMismatch`] on a `HelloReject` or a `HelloAck`
    ///   whose `protocol_version` differs from [`PROTOCOL_VERSION`].
    /// - [`ClientError::HandshakeRejected`] if the daemon sends an unexpected
    ///   first message.
    /// - [`ClientError::Disconnected`] if the daemon closes before replying.
    /// - [`ClientError::Codec`] if the handshake frame cannot be encoded/decoded.
    pub fn connect(
        socket_path: &Path,
        client_version: impl Into<String>,
    ) -> Result<Self, ClientError> {
        let mut stream = UnixStream::connect(socket_path)?;

        let hello = ClientMessage::Hello(Hello {
            protocol_version: PROTOCOL_VERSION,
            client_version: client_version.into(),
            client_kind: ClientKind::Gui,
        });
        write_frame(&mut stream, &hello)?;

        let mut decoder = FrameDecoder::new();
        let mut buf = vec![0u8; READ_BUF];
        let first =
            read_message(&mut stream, &mut decoder, &mut buf)?.ok_or(ClientError::Disconnected)?;

        let daemon = match first {
            DaemonMessage::HelloAck(ack) => {
                if ack.protocol_version != PROTOCOL_VERSION {
                    return Err(ClientError::VersionMismatch {
                        expected: PROTOCOL_VERSION,
                        got: ack.protocol_version,
                    });
                }
                DaemonInfo {
                    protocol_version: ack.protocol_version,
                    daemon_version: ack.daemon_version,
                    instance_id: ack.instance_id,
                    started_at: ack.started_at,
                }
            }
            DaemonMessage::HelloReject(reject) => {
                return Err(ClientError::VersionMismatch {
                    expected: PROTOCOL_VERSION,
                    got: reject.daemon_protocol_version,
                });
            }
            DaemonMessage::Response { .. } | DaemonMessage::Event(_) => {
                return Err(ClientError::HandshakeRejected {
                    reason: "daemon sent a non-handshake message before HelloAck".to_string(),
                });
            }
            _ => {
                return Err(ClientError::HandshakeRejected {
                    reason: "daemon sent an unrecognized message before HelloAck".to_string(),
                });
            }
        };

        // Split the socket so the reader thread and request writers use
        // independent halves (dups of the same underlying socket).
        let read_half = stream.try_clone()?;
        let write_half = stream;

        // Overflow invalidates the connection; blocking this reader would also block request replies.
        let (events_tx, events_rx) = flume::bounded(64);
        let shared = Arc::new(Shared {
            write: Mutex::new(write_half),
            pending: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            connected: AtomicBool::new(true),
            daemon,
            events_rx,
        });

        let reader_shared = Arc::clone(&shared);
        // Move the decoder (it may already hold bytes the daemon pipelined after
        // the HelloAck) into the reader so no frames are lost.
        let reader = std::thread::Builder::new()
            .name("forge-client-reader".to_string())
            .spawn(move || reader_loop(read_half, decoder, &reader_shared, &events_tx))?;

        Ok(Self {
            shared,
            reader: Some(reader),
        })
    }

    /// Send a request and block until the matching response arrives.
    ///
    /// # Errors
    /// [`ClientError::Disconnected`] if the connection is (or becomes) closed,
    /// [`ClientError::Protocol`] if the daemon returns a structured error,
    /// [`ClientError::Io`]/[`ClientError::Codec`] if the write fails.
    pub fn request(&self, body: Request) -> Result<Response, ClientError> {
        self.request_inner(body, None)
    }

    /// Like [`Client::request`] but gives up after `timeout`.
    ///
    /// # Errors
    /// As [`Client::request`], plus [`ClientError::Timeout`] if no response
    /// arrives within `timeout`.
    pub fn request_timeout(
        &self,
        body: Request,
        timeout: Duration,
    ) -> Result<Response, ClientError> {
        self.request_inner(body, Some(timeout))
    }

    fn request_inner(
        &self,
        body: Request,
        timeout: Option<Duration>,
    ) -> Result<Response, ClientError> {
        if !self.is_connected() {
            return Err(ClientError::Disconnected);
        }

        let request_id = self.shared.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = flume::bounded::<Result<Response, ProtocolError>>(1);
        self.shared
            .pending
            .lock()
            .expect("pending mutex poisoned")
            .insert(request_id, tx);

        // Close the race with a reader that disconnected between our check and
        // our insert: if it already ran its cleanup pass, ours would linger.
        if !self.is_connected() {
            self.remove_waiter(request_id);
            return Err(ClientError::Disconnected);
        }

        let message = ClientMessage::Request { request_id, body };
        if let Err(err) = self.send_message(&message) {
            self.remove_waiter(request_id);
            return Err(err);
        }

        match timeout {
            Some(dur) => match rx.recv_timeout(dur) {
                Ok(body) => body.map_err(ClientError::Protocol),
                Err(flume::RecvTimeoutError::Timeout) => {
                    self.remove_waiter(request_id);
                    Err(ClientError::Timeout)
                }
                Err(flume::RecvTimeoutError::Disconnected) => Err(ClientError::Disconnected),
            },
            None => match rx.recv() {
                Ok(body) => body.map_err(ClientError::Protocol),
                Err(_) => Err(ClientError::Disconnected),
            },
        }
    }

    /// A fresh receiver for the daemon's unsolicited events (§10.3). The GUI
    /// drains this to update its [`crate::Store`]. The channel closes when the
    /// connection drops.
    #[must_use]
    pub fn events(&self) -> flume::Receiver<DaemonEvent> {
        self.shared.events_rx.clone()
    }

    /// The handshake info of the connected daemon (§9.2).
    #[must_use]
    pub fn daemon_info(&self) -> &DaemonInfo {
        &self.shared.daemon
    }

    /// Whether the connection is still live.
    #[must_use]
    pub fn is_connected(&self) -> bool {
        self.shared.connected.load(Ordering::SeqCst)
    }

    /// Refresh an existing replica's domain lists from the daemon's global
    /// snapshot, leaving the attached terminal replicas alone.
    ///
    /// Most state arrives as a broadcast, but a few things are only ever
    /// computed when a snapshot is asked for — transcripts discovered on disk
    /// are recomputed there and nowhere else — so a client that wants them
    /// current has to ask again.
    pub fn refresh_store(&self, store: &mut crate::Store) -> Result<(), ClientError> {
        let response = self.request(Request::GetSnapshot)?;
        if !matches!(&response, Response::Snapshot { .. }) {
            return Err(ClientError::UnexpectedResponse {
                expected: "Snapshot",
            });
        }
        store.apply_snapshot(response);
        Ok(())
    }

    /// Load a fresh GUI replica from the daemon's global snapshot.
    ///
    /// Usage now rides in the snapshot itself (`Response::Snapshot::usage`),
    /// served from the daemon's sweeper-maintained cache: loading a store no
    /// longer costs a per-provider subprocess or HTTPS round trip on the calling
    /// thread (L1). Fresh readings continue to arrive as `ProviderUsageChanged`.
    pub fn load_store(&self) -> Result<crate::Store, ClientError> {
        let mut store = crate::Store::new();
        self.refresh_store(&mut store)?;
        Ok(store)
    }

    /// Clear Forge-owned state while retaining repository contents and config.
    pub fn factory_reset(&self) -> Result<(), ClientError> {
        self.expect_ack(Request::FactoryReset)
    }

    /// Add a project and its main workspace.
    pub fn add_project(&self, path: &Path) -> Result<(), ClientError> {
        self.expect_ack(Request::AddProject {
            path: path.to_path_buf(),
        })
    }

    /// Add a project and its main workspace to an organizational group.
    pub fn add_project_to_group(
        &self,
        path: &Path,
        project_group_id: domain::ProjectGroupId,
    ) -> Result<(), ClientError> {
        self.expect_ack(Request::AddProjectToGroup {
            path: path.to_path_buf(),
            project_group_id,
        })
    }

    /// Create an organizational group for related projects.
    pub fn create_project_group(&self, name: &str) -> Result<(), ClientError> {
        self.expect_ack(Request::CreateProjectGroup {
            name: name.to_owned(),
        })
    }

    /// Rename an organizational project group.
    pub fn rename_project_group(
        &self,
        project_group_id: domain::ProjectGroupId,
        name: &str,
    ) -> Result<(), ClientError> {
        self.expect_ack(Request::RenameProjectGroup {
            project_group_id,
            name: name.to_owned(),
        })
    }

    /// Remove an organizational group. Project directories are untouched.
    pub fn remove_project_group(
        &self,
        project_group_id: domain::ProjectGroupId,
    ) -> Result<(), ClientError> {
        self.expect_ack(Request::RemoveProjectGroup { project_group_id })
    }

    /// Remove a project subject to `policy` (§10.2).
    ///
    /// No policy deletes a branch. `KeepEverything` is refused while the
    /// project has running sessions — that refusal is the daemon telling the
    /// caller to ask the person for a stronger policy, not an error to retry.
    pub fn remove_project(
        &self,
        project_id: domain::ProjectId,
        policy: RemoveProjectPolicy,
    ) -> Result<(), ClientError> {
        self.expect_ack(Request::RemoveProject { project_id, policy })
    }

    /// Move a project between groups, or to General with `None`.
    pub fn move_project(
        &self,
        project_id: domain::ProjectId,
        project_group_id: Option<domain::ProjectGroupId>,
    ) -> Result<(), ClientError> {
        self.expect_ack(Request::MoveProject {
            project_id,
            project_group_id,
        })
    }

    /// Set a project's icon, or clear it with `None` so the UI falls back to
    /// the initials it derives from the name.
    pub fn set_project_icon(
        &self,
        project_id: domain::ProjectId,
        icon: Option<String>,
    ) -> Result<(), ClientError> {
        self.expect_ack(Request::SetProjectIcon { project_id, icon })
    }

    /// Create a managed git worktree for `project_id` (§14.3).
    ///
    /// `base` is the start point for a branch that does not exist yet; pass
    /// `origin/<branch>` to start from a remote-tracking branch, which is what
    /// makes git configure the upstream by itself. `name` overrides the
    /// directory slug, which otherwise comes from the branch.
    pub fn create_worktree(
        &self,
        project_id: domain::ProjectId,
        branch: &str,
        base: Option<String>,
        name: Option<String>,
    ) -> Result<(), ClientError> {
        self.expect_ack(Request::CreateWorktree {
            project_id,
            branch: branch.to_owned(),
            base,
            name,
        })
    }

    /// Remove a worktree (§14.4). Never deletes a branch.
    ///
    /// Without `force` the daemon refuses while the tree is dirty, a merge is
    /// in progress, or sessions are running, and says which — that message is
    /// meant to be shown before re-issuing with `force`.
    pub fn remove_worktree(
        &self,
        workspace_id: WorkspaceId,
        force: bool,
    ) -> Result<(), ClientError> {
        self.expect_ack(Request::RemoveWorktree {
            workspace_id,
            force,
        })
    }

    /// Set or clear a workspace's human label. `None` falls back to the branch.
    pub fn rename_workspace(
        &self,
        workspace_id: WorkspaceId,
        display_name: Option<String>,
    ) -> Result<(), ClientError> {
        self.expect_ack(Request::RenameWorkspace {
            workspace_id,
            display_name,
        })
    }

    /// Recompute a workspace's branch and working-tree status.
    ///
    /// The daemon throttles this to one `git status` per workspace every 2 s
    /// (ADR-008), so calling it on a view change is safe.
    pub fn refresh_workspace_status(&self, workspace_id: WorkspaceId) -> Result<(), ClientError> {
        self.expect_ack(Request::RefreshWorkspaceStatus { workspace_id })
    }

    /// Re-detect a project's git root, branches and worktrees (§10.2).
    pub fn refresh_project(&self, project_id: domain::ProjectId) -> Result<(), ClientError> {
        self.expect_ack(Request::RefreshProject { project_id })
    }

    /// Every branch of a project, with its remotes and default branch (§14.3).
    ///
    /// Reads refs from disk — it never opens a socket, so it is safe to call
    /// on the same thread that carries keystrokes. Use
    /// [`Client::fetch_remote`] to make those refs current first.
    pub fn list_branches(&self, project_id: domain::ProjectId) -> Result<Branches, ClientError> {
        match self.request(Request::ListBranches { project_id })? {
            Response::Branches {
                branches,
                remotes,
                default_branch,
            } => Ok(Branches {
                branches,
                remotes,
                default_branch,
            }),
            _ => Err(ClientError::UnexpectedResponse {
                expected: "Branches",
            }),
        }
    }

    /// Read one checkout's uncommitted changes, a patch per file (§16.7).
    ///
    /// Local and synchronous: git runs on the daemon, so this blocks like
    /// every other `request` and belongs on the runtime thread, never on the
    /// render one.
    ///
    /// # Errors
    /// [`ClientError`] when the daemon refuses the read or answers with
    /// something else.
    pub fn workspace_diff(
        &self,
        workspace_id: domain::WorkspaceId,
        context_lines: Option<u32>,
    ) -> Result<domain::WorkspaceDiff, ClientError> {
        match self.request(Request::GetWorkspaceDiff {
            workspace_id,
            context_lines,
        })? {
            Response::WorkspaceDiff(diff) => Ok(diff),
            _ => Err(ClientError::UnexpectedResponse {
                expected: "WorkspaceDiff",
            }),
        }
    }

    /// One session's changes since its baseline (§16.7).
    ///
    /// # Errors
    /// [`ClientError`] on a transport failure or a refusal from the daemon.
    pub fn session_changes(
        &self,
        session_id: domain::SessionId,
    ) -> Result<domain::SessionChanges, ClientError> {
        match self.request(Request::GetSessionChanges { session_id })? {
            Response::SessionChanges(changes) => Ok(changes),
            _ => Err(ClientError::UnexpectedResponse {
                expected: "SessionChanges",
            }),
        }
    }

    /// One checkout's changes since its sessions began (§16.7).
    ///
    /// # Errors
    /// [`ClientError`] on a transport failure or a refusal from the daemon.
    pub fn workspace_review(
        &self,
        workspace_id: domain::WorkspaceId,
        context_lines: Option<u32>,
    ) -> Result<domain::WorkspaceReview, ClientError> {
        match self.request(Request::GetWorkspaceReview {
            workspace_id,
            context_lines,
        })? {
            Response::WorkspaceReview(review) => Ok(*review),
            _ => Err(ClientError::UnexpectedResponse {
                expected: "WorkspaceReview",
            }),
        }
    }

    /// The tail of a session's terminal as plain text, for a handoff.
    ///
    /// # Errors
    /// [`ClientError`] on a transport failure or a refusal from the daemon.
    pub fn session_transcript(
        &self,
        session_id: domain::SessionId,
        max_lines: Option<u32>,
        max_bytes: Option<u32>,
    ) -> Result<domain::SessionTranscript, ClientError> {
        match self.request(Request::GetSessionTranscript {
            session_id,
            max_lines,
            max_bytes,
        })? {
            Response::SessionTranscript(transcript) => Ok(transcript),
            _ => Err(ClientError::UnexpectedResponse {
                expected: "SessionTranscript",
            }),
        }
    }

    /// A discovered agent run's conversation as plain text, for a handoff.
    ///
    /// The on-disk counterpart of [`Client::session_transcript`]: the run has
    /// no PTY, so the daemon reads its transcript file instead of the VT
    /// engine. Named by identity, never by path.
    ///
    /// # Errors
    /// [`ClientError`] on a transport failure or a refusal from the daemon.
    pub fn external_transcript(
        &self,
        session_id: String,
        provider: String,
        profile_id: Option<domain::AgentProfileId>,
        max_turns: Option<u32>,
        max_bytes: Option<u32>,
    ) -> Result<domain::ExternalTranscript, ClientError> {
        match self.request(Request::GetExternalTranscript {
            session_id,
            provider,
            profile_id,
            max_turns,
            max_bytes,
        })? {
            Response::ExternalTranscript(transcript) => Ok(transcript),
            _ => Err(ClientError::UnexpectedResponse {
                expected: "ExternalTranscript",
            }),
        }
    }

    /// Remove a discovered agent run's transcript from disk.
    ///
    /// History reaches a client only through the snapshot, so a caller
    /// re-reads it afterwards; there is no event for this.
    ///
    /// # Errors
    /// [`ClientError`] on a transport failure, or when the daemon refuses —
    /// a run recorded in a shared store cannot be removed on its own.
    pub fn delete_external_session(
        &self,
        session_id: String,
        provider: String,
        profile_id: Option<domain::AgentProfileId>,
    ) -> Result<(), ClientError> {
        match self.request(Request::DeleteExternalSession {
            session_id,
            provider,
            profile_id,
        })? {
            Response::Ack => Ok(()),
            _ => Err(ClientError::UnexpectedResponse { expected: "Ack" }),
        }
    }

    /// Read one checkout's stopped rebase/merge/cherry-pick state (§14).
    ///
    /// Local and synchronous like [`Client::workspace_diff`]: belongs on the
    /// runtime thread, never the render one.
    ///
    /// # Errors
    /// [`ClientError`] when the daemon refuses the read or answers with
    /// something else.
    pub fn rebase_state(
        &self,
        workspace_id: domain::WorkspaceId,
    ) -> Result<domain::RebaseState, ClientError> {
        match self.request(Request::GetRebaseState { workspace_id })? {
            Response::RebaseState(state) => Ok(state),
            _ => Err(ClientError::UnexpectedResponse {
                expected: "RebaseState",
            }),
        }
    }

    /// Continue the stopped operation, answering with what git left behind.
    ///
    /// A continue that stops again on the next commit's conflicts is a normal
    /// answer, not an error: the returned state says which paths to look at.
    ///
    /// # Errors
    /// [`ClientError`] when nothing is in progress, git fails, or the daemon
    /// answers with something else.
    pub fn continue_rebase(
        &self,
        workspace_id: domain::WorkspaceId,
    ) -> Result<domain::RebaseState, ClientError> {
        match self.request(Request::ContinueRebase { workspace_id })? {
            Response::RebaseState(state) => Ok(state),
            _ => Err(ClientError::UnexpectedResponse {
                expected: "RebaseState",
            }),
        }
    }

    /// Abort the stopped operation, restoring the branch git started from.
    ///
    /// Destructive: every resolution made since it stopped goes with it. The
    /// GUI confirms before calling this.
    ///
    /// # Errors
    /// [`ClientError`] when nothing is in progress or git refuses.
    pub fn abort_rebase(&self, workspace_id: domain::WorkspaceId) -> Result<(), ClientError> {
        match self.request(Request::AbortRebase { workspace_id })? {
            Response::Ack => Ok(()),
            _ => Err(ClientError::UnexpectedResponse { expected: "Ack" }),
        }
    }

    /// Stage resolved paths so a continue can move, answering with the state
    /// read back after the write.
    ///
    /// # Errors
    /// [`ClientError`] when a path is refused, git fails, or the daemon answers
    /// with something else.
    pub fn mark_conflict_resolved(
        &self,
        workspace_id: domain::WorkspaceId,
        paths: Vec<String>,
    ) -> Result<domain::RebaseState, ClientError> {
        match self.request(Request::MarkConflictResolved {
            workspace_id,
            paths,
        })? {
            Response::RebaseState(state) => Ok(state),
            _ => Err(ClientError::UnexpectedResponse {
                expected: "RebaseState",
            }),
        }
    }

    /// List files under a workspace (ADR-012).
    ///
    /// Local and synchronous like [`Client::workspace_diff`]: belongs on the
    /// runtime thread, never the render one.
    pub fn list_files(
        &self,
        workspace_id: domain::WorkspaceId,
    ) -> Result<domain::FileTree, ClientError> {
        match self.request(Request::ListFiles { workspace_id })? {
            Response::FileTree(tree) => Ok(tree),
            _ => Err(ClientError::UnexpectedResponse {
                expected: "FileTree",
            }),
        }
    }

    /// Read one file relative to a workspace (ADR-012).
    pub fn read_file(
        &self,
        workspace_id: domain::WorkspaceId,
        path: impl Into<String>,
    ) -> Result<domain::FileContents, ClientError> {
        match self.request(Request::ReadFile {
            workspace_id,
            path: path.into(),
        })? {
            Response::FileContents(contents) => Ok(contents),
            _ => Err(ClientError::UnexpectedResponse {
                expected: "FileContents",
            }),
        }
    }

    /// Read one image relative to a workspace, for a Markdown preview (ADR-012).
    pub fn read_image(
        &self,
        workspace_id: domain::WorkspaceId,
        path: impl Into<String>,
    ) -> Result<domain::ImageContents, ClientError> {
        match self.request(Request::ReadImage {
            workspace_id,
            path: path.into(),
        })? {
            Response::ImageContents(image) => Ok(image),
            _ => Err(ClientError::UnexpectedResponse {
                expected: "ImageContents",
            }),
        }
    }

    /// Write one file conditioned on a revision (ADR-012).
    ///
    /// On a stale revision the daemon answers
    /// [`protocol::ErrorCode::PreconditionFailed`]; re-read with
    /// [`Client::read_file`] before offering reload-or-keep.
    pub fn write_file(
        &self,
        workspace_id: domain::WorkspaceId,
        path: impl Into<String>,
        text: impl Into<String>,
        expected_revision: impl Into<String>,
    ) -> Result<(), ClientError> {
        self.expect_ack(Request::WriteFile {
            workspace_id,
            path: path.into(),
            text: text.into(),
            expected_revision: expected_revision.into(),
        })
    }

    /// Create an empty file or a directory (ADR-012, A11).
    pub fn create_path(
        &self,
        workspace_id: domain::WorkspaceId,
        path: impl Into<String>,
        directory: bool,
    ) -> Result<(), ClientError> {
        self.expect_ack(Request::CreatePath {
            workspace_id,
            path: path.into(),
            directory,
        })
    }

    /// Move one path to another inside the same checkout (ADR-012, A11).
    pub fn rename_path(
        &self,
        workspace_id: domain::WorkspaceId,
        from: impl Into<String>,
        to: impl Into<String>,
    ) -> Result<(), ClientError> {
        self.expect_ack(Request::RenamePath {
            workspace_id,
            from: from.into(),
            to: to.into(),
        })
    }

    /// Delete one path, recursively for a directory (ADR-012, A11).
    pub fn delete_path(
        &self,
        workspace_id: domain::WorkspaceId,
        path: impl Into<String>,
    ) -> Result<(), ClientError> {
        self.expect_ack(Request::DeletePath {
            workspace_id,
            path: path.into(),
        })
    }

    /// Search files by name or content (ADR-012).
    pub fn search_files(
        &self,
        workspace_id: domain::WorkspaceId,
        query: impl Into<String>,
        kind: domain::SearchKind,
        limit: Option<u32>,
    ) -> Result<domain::SearchResults, ClientError> {
        match self.request(Request::SearchFiles {
            workspace_id,
            query: query.into(),
            kind,
            limit,
        })? {
            Response::SearchResults(results) => Ok(results),
            _ => Err(ClientError::UnexpectedResponse {
                expected: "SearchResults",
            }),
        }
    }

    /// Read aggregated agent token analytics (§16.2).
    ///
    /// Local and synchronous like [`Client::workspace_diff`]: the daemon reads
    /// the transcript stores on disk, so this blocks like every other `request`
    /// and belongs on the runtime thread, never on the render one. `window_days`
    /// of `None` takes the daemon's default horizon.
    ///
    /// # Errors
    /// [`ClientError`] when the daemon refuses the read or answers with
    /// something else.
    pub fn usage_analytics(
        &self,
        window_days: Option<u16>,
    ) -> Result<domain::UsageAnalytics, ClientError> {
        match self.request(Request::GetUsageAnalytics { window_days })? {
            Response::UsageAnalytics(analytics) => Ok(*analytics),
            _ => Err(ClientError::UnexpectedResponse {
                expected: "UsageAnalytics",
            }),
        }
    }

    /// Read runtime statistics from a live daemon (§22).
    ///
    /// Synchronous like [`Client::usage_analytics`]: counts under the daemon
    /// core lock plus the client registry. Suitable for the CLI; the GUI
    /// should keep this off the render thread.
    ///
    /// # Errors
    /// [`ClientError`] when the daemon refuses the read or answers with
    /// something else.
    pub fn get_stats(&self) -> Result<protocol::DaemonStats, ClientError> {
        match self.request(Request::GetStats)? {
            Response::DaemonStats(stats) => Ok(stats),
            _ => Err(ClientError::UnexpectedResponse {
                expected: "DaemonStats",
            }),
        }
    }

    /// Start a background fetch (§14.1).
    ///
    /// Returns as soon as the daemon has *started* the fetch, not when it
    /// finishes: the result arrives as `DaemonEvent::RemoteRefsUpdated`. This
    /// is deliberate — the GUI drains its commands on one thread that also
    /// carries terminal input, so a synchronous fetch would freeze typing for
    /// as long as the network took.
    pub fn fetch_remote(
        &self,
        project_id: domain::ProjectId,
        remote: Option<String>,
    ) -> Result<(), ClientError> {
        self.expect_ack(Request::FetchRemote { project_id, remote })
    }

    /// Start a global pull-request refresh in the background.
    ///
    /// Returns after the daemon acknowledges that the operation started; the
    /// resulting state arrives as `DaemonEvent::PullRequestsUpdated`.
    pub fn refresh_pull_requests(&self) -> Result<(), ClientError> {
        self.expect_ack(Request::RefreshPullRequests)
    }

    /// Working-tree status and bounded patch for a workspace.
    pub fn get_change_context(
        &self,
        workspace_id: WorkspaceId,
    ) -> Result<domain::ChangeContext, ClientError> {
        match self.request(Request::GetChangeContext { workspace_id })? {
            Response::ChangeContext(ctx) => Ok(ctx),
            _ => Err(ClientError::UnexpectedResponse {
                expected: "ChangeContext",
            }),
        }
    }

    /// Ask Juva to draft a commit message or PR description.
    /// Start a draft. The text arrives as `DaemonEvent::JuvaDraftReady`.
    ///
    /// Acks rather than answers because the `[juva]` endpoint opens a socket:
    /// the same shape as `fetch_remote` and `create_pull_request`.
    ///
    /// # Errors
    /// [`ClientError`] on a transport failure or a refusal — a clean tree
    /// refuses a commit-message draft here rather than through the event.
    pub fn draft_with_juva(
        &self,
        workspace_id: WorkspaceId,
        kind: domain::JuvaKind,
    ) -> Result<(), ClientError> {
        match self.request(Request::DraftWithJuva { workspace_id, kind })? {
            Response::Ack => Ok(()),
            _ => Err(ClientError::UnexpectedResponse { expected: "Ack" }),
        }
    }

    /// Stage all changes and create a local commit (no push).
    pub fn create_commit(
        &self,
        workspace_id: WorkspaceId,
        message: &str,
    ) -> Result<(), ClientError> {
        self.expect_ack(Request::CreateCommit {
            workspace_id,
            message: message.to_owned(),
        })
    }

    /// Start push + open PR; outcome arrives as `PullRequestOpened`.
    pub fn create_pull_request(
        &self,
        workspace_id: WorkspaceId,
        title: &str,
        body: &str,
        base: Option<String>,
    ) -> Result<(), ClientError> {
        self.expect_ack(Request::CreatePullRequest {
            workspace_id,
            title: title.to_owned(),
            body: body.to_owned(),
            base,
        })
    }

    /// Create a shell in `workspace_id`, returning the ids the daemon minted so
    /// the caller can attach without reloading the snapshot (§10.2, L3).
    pub fn create_shell_session(
        &self,
        workspace_id: WorkspaceId,
    ) -> Result<(SessionId, TerminalId), ClientError> {
        self.expect_session_created(Request::CreateShellSession {
            workspace_id,
            parent: None,
            role: SessionRole::Generic,
        })
    }

    /// Create an agent TUI in `workspace_id`, optionally through a launch
    /// profile (§13.4), optionally re-entering one of the provider's own
    /// earlier sessions (§13.5), and optionally handing it a prompt to start
    /// from rather than an empty one (§16.8).
    ///
    /// # Errors
    /// [`ClientError`] when the daemon refuses the launch — an uninstalled
    /// provider, or one that takes neither a resume nor a prompt.
    pub fn create_agent_session(
        &self,
        workspace_id: WorkspaceId,
        provider_id: AgentProviderId,
        profile_id: Option<AgentProfileId>,
        resume: Option<String>,
        initial_prompt: Option<String>,
    ) -> Result<(SessionId, TerminalId), ClientError> {
        self.create_agent_session_with_role(
            workspace_id,
            provider_id,
            profile_id,
            None,
            SessionRole::Generic,
            resume,
            initial_prompt,
            false,
        )
    }

    /// Like [`Self::create_agent_session`] but sets graph role and optional
    /// parent, and can ask for the provider's own read-only mode (§16.9).
    #[allow(clippy::too_many_arguments)]
    pub fn create_agent_session_with_role(
        &self,
        workspace_id: WorkspaceId,
        provider_id: AgentProviderId,
        profile_id: Option<AgentProfileId>,
        parent: Option<SessionId>,
        role: SessionRole,
        resume: Option<String>,
        initial_prompt: Option<String>,
        read_only: bool,
    ) -> Result<(SessionId, TerminalId), ClientError> {
        self.expect_session_created(Request::CreateAgentSession {
            workspace_id,
            provider_id,
            profile_id,
            parent,
            role,
            resume,
            initial_prompt,
            read_only,
        })
    }

    /// Spawn a child agent/shell under a parent session (harness workflow).
    #[allow(clippy::too_many_arguments)]
    pub fn create_child_session(
        &self,
        parent_session_id: SessionId,
        kind: domain::SessionKind,
        provider_id: Option<AgentProviderId>,
        profile_id: Option<AgentProfileId>,
        role: SessionRole,
        workspace_policy: domain::ChildWorkspacePolicy,
        initial_prompt: Option<String>,
    ) -> Result<(SessionId, TerminalId), ClientError> {
        self.expect_session_created(Request::CreateChildSession {
            parent_session_id,
            kind,
            provider_id,
            profile_id,
            role,
            workspace_policy,
            initial_prompt,
        })
    }

    /// Persist and deliver a context envelope (§8.3).
    #[allow(clippy::too_many_arguments)]
    pub fn send_context(
        &self,
        source_session_id: SessionId,
        target_session_id: Option<SessionId>,
        spawn: Option<protocol::SendContextSpawn>,
        summary: Option<String>,
        instructions: Option<String>,
        include_transcript: bool,
        max_transcript_bytes: Option<u32>,
    ) -> Result<SendContextResult, ClientError> {
        match self.request(Request::SendContext {
            source_session_id,
            target_session_id,
            spawn,
            summary,
            instructions,
            include_transcript,
            max_transcript_bytes,
        })? {
            Response::Ack => Ok(SendContextResult::Delivered),
            Response::SessionCreated {
                session_id,
                terminal_id,
            } => Ok(SendContextResult::Spawned {
                session_id,
                terminal_id,
            }),
            _ => Err(ClientError::UnexpectedResponse {
                expected: "Ack or SessionCreated",
            }),
        }
    }

    /// Envelopes where the session is source or target (§8.3).
    pub fn list_context_envelopes(
        &self,
        session_id: SessionId,
    ) -> Result<Vec<domain::ContextEnvelope>, ClientError> {
        match self.request(Request::ListContextEnvelopes { session_id })? {
            Response::ContextEnvelopes(list) => Ok(list),
            _ => Err(ClientError::UnexpectedResponse {
                expected: "ContextEnvelopes",
            }),
        }
    }

    pub fn list_harness_features(
        &self,
        project_id: domain::ProjectId,
    ) -> Result<domain::HarnessFeatureList, ClientError> {
        match self.request(Request::ListHarnessFeatures { project_id })? {
            Response::HarnessFeatureList(list) => Ok(list),
            _ => Err(ClientError::UnexpectedResponse {
                expected: "HarnessFeatureList",
            }),
        }
    }

    pub fn get_harness_feature(
        &self,
        project_id: domain::ProjectId,
        feature_id: u32,
    ) -> Result<domain::HarnessFeature, ClientError> {
        match self.request(Request::GetHarnessFeature {
            project_id,
            feature_id,
        })? {
            Response::HarnessFeature(feature) => Ok(feature),
            _ => Err(ClientError::UnexpectedResponse {
                expected: "HarnessFeature",
            }),
        }
    }

    pub fn get_harness_timeline(
        &self,
        project_id: domain::ProjectId,
        feature_id: u32,
    ) -> Result<Vec<domain::HarnessEvent>, ClientError> {
        match self.request(Request::GetHarnessTimeline {
            project_id,
            feature_id,
        })? {
            Response::HarnessTimeline(events) => Ok(events),
            _ => Err(ClientError::UnexpectedResponse {
                expected: "HarnessTimeline",
            }),
        }
    }

    pub fn read_harness_artifact(
        &self,
        project_id: domain::ProjectId,
        feature_id: u32,
        artifact: domain::HarnessArtifactKind,
    ) -> Result<String, ClientError> {
        match self.request(Request::ReadHarnessArtifact {
            project_id,
            feature_id,
            artifact,
        })? {
            Response::HarnessArtifact { text } => Ok(text),
            _ => Err(ClientError::UnexpectedResponse {
                expected: "HarnessArtifact",
            }),
        }
    }

    pub fn register_harness_feature(
        &self,
        project_id: domain::ProjectId,
        workspace_id: Option<domain::WorkspaceId>,
        spec_raw: String,
        title: Option<String>,
    ) -> Result<domain::HarnessFeature, ClientError> {
        match self.request(Request::RegisterHarnessFeature {
            project_id,
            workspace_id,
            spec_raw,
            title,
        })? {
            Response::HarnessFeature(feature) => Ok(feature),
            _ => Err(ClientError::UnexpectedResponse {
                expected: "HarnessFeature",
            }),
        }
    }

    pub fn register_harness_from_issue(
        &self,
        project_id: domain::ProjectId,
        workspace_id: Option<domain::WorkspaceId>,
        issue_number: u32,
    ) -> Result<domain::HarnessFeature, ClientError> {
        match self.request(Request::RegisterHarnessFromIssue {
            project_id,
            workspace_id,
            issue_number,
        })? {
            Response::HarnessFeature(feature) => Ok(feature),
            _ => Err(ClientError::UnexpectedResponse {
                expected: "HarnessFeature",
            }),
        }
    }

    pub fn harness_advance(
        &self,
        project_id: domain::ProjectId,
        feature_id: u32,
        revision: Option<u64>,
        action: domain::HarnessAdvanceAction,
    ) -> Result<domain::HarnessFeature, ClientError> {
        match self.request(Request::HarnessAdvance {
            project_id,
            feature_id,
            revision,
            action,
        })? {
            Response::HarnessFeature(feature) => Ok(feature),
            _ => Err(ClientError::UnexpectedResponse {
                expected: "HarnessFeature",
            }),
        }
    }

    pub fn link_harness_session(
        &self,
        project_id: domain::ProjectId,
        feature_id: u32,
        session_id: SessionId,
    ) -> Result<(), ClientError> {
        self.expect_ack(Request::LinkHarnessSession {
            project_id,
            feature_id,
            session_id,
        })
    }

    /// Run one step of the harness cycle as a job.
    ///
    /// The daemon takes it from there: when the job exits it records the
    /// outcome and starts the next step, stopping at the human gate.
    pub fn run_harness_step(
        &self,
        project_id: domain::ProjectId,
        feature_id: u32,
        step: domain::HarnessStep,
    ) -> Result<domain::Job, ClientError> {
        match self.request(Request::RunHarnessStep {
            project_id,
            feature_id,
            step,
            force: false,
        })? {
            Response::Job(job) => Ok(*job),
            _ => Err(ClientError::UnexpectedResponse { expected: "Job" }),
        }
    }

    /// Ask a question about the harness; the answer arrives on the job's
    /// stream.
    ///
    /// `resume_from` continues the previous answer's provider session, which
    /// is what makes a series of questions a conversation.
    pub fn ask_harness(
        &self,
        project_id: domain::ProjectId,
        question: String,
        resume_from: Option<String>,
    ) -> Result<domain::Job, ClientError> {
        match self.request(Request::AskHarness {
            project_id,
            question,
            resume_from,
        })? {
            Response::Job(job) => Ok(*job),
            _ => Err(ClientError::UnexpectedResponse { expected: "Job" }),
        }
    }

    /// Start a headless run of one provider (§ jobs).
    ///
    /// Answers as soon as the job is *accepted*: it may still be queued behind
    /// the daemon's concurrency limit. Follow `JobUpdated` for the rest.
    pub fn start_job(&self, request: domain::JobRequest) -> Result<domain::Job, ClientError> {
        match self.request(Request::StartJob { request })? {
            Response::Job(job) => Ok(*job),
            _ => Err(ClientError::UnexpectedResponse { expected: "Job" }),
        }
    }

    /// Kill a running job, or drop a queued one.
    pub fn cancel_job(&self, job_id: domain::JobId) -> Result<(), ClientError> {
        self.expect_ack(Request::CancelJob { job_id })
    }

    /// Every headless run the daemon knows of, oldest first.
    pub fn list_jobs(&self) -> Result<Vec<domain::Job>, ClientError> {
        match self.request(Request::ListJobs)? {
            Response::Jobs(jobs) => Ok(jobs),
            _ => Err(ClientError::UnexpectedResponse { expected: "Jobs" }),
        }
    }

    /// One job's event stream from `from_line`, and whether it has finished.
    pub fn read_job_log(
        &self,
        job_id: domain::JobId,
        from_line: u64,
    ) -> Result<(Vec<String>, bool), ClientError> {
        match self.request(Request::ReadJobLog { job_id, from_line })? {
            Response::JobLog {
                lines, finished, ..
            } => Ok((lines, finished)),
            _ => Err(ClientError::UnexpectedResponse { expected: "JobLog" }),
        }
    }

    pub fn validate_harness(
        &self,
        project_id: domain::ProjectId,
    ) -> Result<(bool, String), ClientError> {
        match self.request(Request::ValidateHarness { project_id })? {
            Response::HarnessValidate { ok, output } => Ok((ok, output)),
            _ => Err(ClientError::UnexpectedResponse {
                expected: "HarnessValidate",
            }),
        }
    }

    /// Create or replace a launch profile (§13.4).
    pub fn save_agent_profile(&self, profile: AgentProfile) -> Result<(), ClientError> {
        self.expect_ack(Request::SaveAgentProfile { profile })
    }

    /// Delete a launch profile (§13.4).
    pub fn remove_agent_profile(&self, profile_id: AgentProfileId) -> Result<(), ClientError> {
        self.expect_ack(Request::RemoveAgentProfile { profile_id })
    }

    /// Ignored paths a project could share between its workspaces (§14.2).
    ///
    /// # Errors
    /// Returns [`ClientError`] when the daemon refuses or the answer is not
    /// [`Response::ShareCandidates`].
    pub fn detect_share_candidates(
        &self,
        project_id: ProjectId,
    ) -> Result<(Vec<ShareCandidate>, bool), ClientError> {
        match self.request(Request::DetectShareCandidates { project_id })? {
            Response::ShareCandidates {
                candidates,
                truncated,
                ..
            } => Ok((candidates, truncated)),
            _ => Err(ClientError::UnexpectedResponse {
                expected: "ShareCandidates",
            }),
        }
    }

    /// Replace a project's whole rule set (§14.2).
    ///
    /// # Errors
    /// Returns [`ClientError`] when the daemon refuses the set.
    pub fn set_project_shares(
        &self,
        project_id: ProjectId,
        rules: Vec<ShareRule>,
    ) -> Result<(), ClientError> {
        self.expect_ack(Request::SetProjectShares { project_id, rules })
    }

    /// Drop one rule, saying what happens to what it already wrote (§14.2).
    ///
    /// # Errors
    /// Returns [`ClientError`] when the daemon refuses or answers with
    /// something other than a plan.
    pub fn remove_share_rule(
        &self,
        project_id: ProjectId,
        rule_id: ShareRuleId,
        cleanup: ShareCleanup,
    ) -> Result<Vec<ShareAction>, ClientError> {
        match self.request(Request::RemoveShareRule {
            project_id,
            rule_id,
            cleanup,
        })? {
            Response::SharePlan { actions, .. } => Ok(actions),
            _ => Err(ClientError::UnexpectedResponse {
                expected: "SharePlan",
            }),
        }
    }

    /// What applying a project's rules to a workspace would do (§14.2).
    ///
    /// # Errors
    /// Returns [`ClientError`] when the daemon refuses or the answer is not
    /// [`Response::SharePlan`].
    pub fn preview_shares(
        &self,
        workspace_id: WorkspaceId,
    ) -> Result<Vec<ShareAction>, ClientError> {
        match self.request(Request::PreviewShares { workspace_id })? {
            Response::SharePlan { actions, .. } => Ok(actions),
            _ => Err(ClientError::UnexpectedResponse {
                expected: "SharePlan",
            }),
        }
    }

    /// Per-rule state of one workspace (§14.2).
    ///
    /// # Errors
    /// Returns [`ClientError`] when the daemon refuses or the answer is not
    /// [`Response::ShareStatus`].
    pub fn share_status(
        &self,
        workspace_id: WorkspaceId,
    ) -> Result<Vec<ShareStatusEntry>, ClientError> {
        match self.request(Request::GetShareStatus { workspace_id })? {
            Response::ShareStatus { entries, .. } => Ok(entries),
            _ => Err(ClientError::UnexpectedResponse {
                expected: "ShareStatus",
            }),
        }
    }

    /// Start provisioning a workspace (§14.2). The ack means *started*: the
    /// outcome arrives as `SharesApplied`.
    ///
    /// # Errors
    /// Returns [`ClientError`] when the daemon refuses to start.
    pub fn apply_shares(
        &self,
        workspace_id: WorkspaceId,
        only: Option<Vec<ShareRuleId>>,
    ) -> Result<(), ClientError> {
        self.expect_ack(Request::ApplyShares { workspace_id, only })
    }

    /// Move a real file into the project's shared store, leaving a link (§14.2).
    ///
    /// # Errors
    /// Returns [`ClientError`] when the daemon refuses the path.
    pub fn adopt_into_share_store(
        &self,
        project_id: ProjectId,
        path: String,
    ) -> Result<(), ClientError> {
        self.expect_ack(Request::AdoptIntoShareStore { project_id, path })
    }

    /// Copy a store file back into a workspace as a real file (§14.2).
    ///
    /// # Errors
    /// Returns [`ClientError`] when the daemon refuses the path.
    pub fn materialize_from_share_store(
        &self,
        project_id: ProjectId,
        path: String,
        workspace_id: Option<WorkspaceId>,
    ) -> Result<(), ClientError> {
        self.expect_ack(Request::MaterializeFromShareStore {
            project_id,
            path,
            workspace_id,
        })
    }

    /// Subscribe to a terminal and return its authoritative snapshot.
    pub fn attach_terminal(
        &self,
        terminal_id: TerminalId,
        size: PtySize,
    ) -> Result<domain::TerminalSnapshot, ClientError> {
        match self.request(Request::AttachTerminal { terminal_id, size })? {
            Response::AttachAck { snapshot } => Ok(snapshot),
            _ => Err(ClientError::UnexpectedResponse {
                expected: "AttachAck",
            }),
        }
    }

    /// Unsubscribe from a terminal.
    pub fn detach_terminal(&self, terminal_id: TerminalId) -> Result<(), ClientError> {
        self.expect_ack(Request::DetachTerminal { terminal_id })
    }

    /// Write encoded input bytes to a terminal.
    pub fn write_terminal_input(
        &self,
        terminal_id: TerminalId,
        bytes: Vec<u8>,
    ) -> Result<(), ClientError> {
        self.expect_ack(Request::WriteTerminalInput { terminal_id, bytes })
    }

    /// Resize the PTY owned by the daemon.
    pub fn resize_terminal(
        &self,
        terminal_id: TerminalId,
        size: PtySize,
    ) -> Result<(), ClientError> {
        self.expect_ack(Request::ResizeTerminal { terminal_id, size })
    }

    /// Kill one running shell or agent session.
    pub fn kill_session(&self, session_id: SessionId) -> Result<(), ClientError> {
        self.expect_ack(Request::KillSession { session_id })
    }

    /// Remove a session from the tree. The daemon rejects this while the
    /// session is still active, so kill it first (§7.3).
    pub fn close_session(&self, session_id: SessionId) -> Result<(), ClientError> {
        self.expect_ack(Request::CloseSession { session_id })
    }

    /// Bring an exited or orphaned session back with a fresh PTY (§7.3).
    pub fn restart_session(&self, session_id: SessionId) -> Result<(), ClientError> {
        self.expect_ack(Request::RestartSession { session_id })
    }

    /// Set the user title, or clear it with `None` so the terminal-reported
    /// one takes over again (§7.3).
    pub fn rename_session(
        &self,
        session_id: SessionId,
        title: Option<String>,
    ) -> Result<(), ClientError> {
        self.expect_ack(Request::RenameSession { session_id, title })
    }

    /// Read every installed provider's account usage (§16.2).
    ///
    /// The daemon re-probes rather than serving a cache, so this is a request
    /// the UI makes deliberately — on connect and on demand — not per frame.
    pub fn list_provider_usage(&self) -> Result<Vec<domain::ProviderUsage>, ClientError> {
        match self.request(Request::ListProviderUsage)? {
            Response::ProviderUsage(usage) => Ok(usage),
            _ => Err(ClientError::UnexpectedResponse {
                expected: "ProviderUsage",
            }),
        }
    }

    /// Re-probe the agent CLIs. `None` refreshes every provider (§13.1).
    pub fn refresh_detection(
        &self,
        provider_id: Option<AgentProviderId>,
    ) -> Result<(), ClientError> {
        // The daemon acks and broadcasts `AgentDetectionChanged`, so the
        // caller learns the results from the event stream like any other
        // change rather than from this return value.
        self.expect_ack(Request::RefreshAgentDetection { provider_id })
    }

    /// Pin the executable used for a provider, or clear the override with
    /// `None` (`Request::SetProviderExecutable`).
    pub fn set_provider_executable(
        &self,
        provider_id: &AgentProviderId,
        path: Option<PathBuf>,
    ) -> Result<(), ClientError> {
        self.expect_ack(Request::SetProviderExecutable {
            provider_id: provider_id.clone(),
            path,
        })
    }

    /// Persist an opaque GUI preference (`Request::SetAppState`, §15.2).
    pub fn set_app_state(&self, key: &str, value: &str) -> Result<(), ClientError> {
        self.expect_ack(Request::SetAppState {
            key: key.to_string(),
            value: value.to_string(),
        })
    }

    /// Read one persisted GUI preference (`Request::GetAppState`, §15.2);
    /// `None` when the key was never written.
    pub fn get_app_state(&self, key: &str) -> Result<Option<String>, ClientError> {
        match self.request(Request::GetAppState {
            key: key.to_string(),
        })? {
            Response::AppState { value } => Ok(value),
            _ => Err(ClientError::UnexpectedResponse {
                expected: "AppState",
            }),
        }
    }

    /// Ask the daemon to shut down. Without `kill_sessions` it refuses while
    /// sessions are live rather than taking them down silently.
    pub fn stop_daemon(&self, kill_sessions: bool) -> Result<(), ClientError> {
        self.expect_ack(Request::StopDaemon { kill_sessions })
    }

    /// Fetch a block of scrollback. `from_line` is an *absolute* index into the
    /// daemon's history (0 = oldest), which is what
    /// [`CellGrid`](crate::CellGrid) converts its negative viewport offsets to.
    pub fn fetch_scrollback(
        &self,
        terminal_id: TerminalId,
        from_line: i64,
        count: u32,
    ) -> Result<domain::ScrollbackRows, ClientError> {
        match self.request(Request::FetchScrollback {
            terminal_id,
            from_line,
            count,
        })? {
            Response::ScrollbackRows(block) => Ok(block),
            _ => Err(ClientError::UnexpectedResponse {
                expected: "ScrollbackRows",
            }),
        }
    }

    fn expect_ack(&self, request: Request) -> Result<(), ClientError> {
        match self.request(request)? {
            Response::Ack => Ok(()),
            _ => Err(ClientError::UnexpectedResponse { expected: "Ack" }),
        }
    }

    /// Send a session-creating request and return the `(SessionId, TerminalId)`
    /// the daemon minted (§10.2, L3).
    fn expect_session_created(
        &self,
        request: Request,
    ) -> Result<(SessionId, TerminalId), ClientError> {
        match self.request(request)? {
            Response::SessionCreated {
                session_id,
                terminal_id,
            } => Ok((session_id, terminal_id)),
            _ => Err(ClientError::UnexpectedResponse {
                expected: "SessionCreated",
            }),
        }
    }

    fn send_message(&self, message: &ClientMessage) -> Result<(), ClientError> {
        let mut write = self.shared.write.lock().expect("write mutex poisoned");
        write_frame(&mut write, message)
    }

    fn remove_waiter(&self, request_id: u64) {
        self.shared
            .pending
            .lock()
            .expect("pending mutex poisoned")
            .remove(&request_id);
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        // Wake the reader (blocked in `read`) by shutting the socket down, then
        // join it so no thread outlives the handle.
        self.shared.disconnect();
        if let Ok(write) = self.shared.write.lock() {
            let _ = write.shutdown(std::net::Shutdown::Both);
        }
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

/// The reader thread: decode frames from `read_half` and route them until EOF or
/// error, then disconnect. Owns `events_tx`, so returning closes the events
/// channel.
fn reader_loop(
    mut read_half: UnixStream,
    mut decoder: FrameDecoder,
    shared: &Arc<Shared>,
    events_tx: &flume::Sender<DaemonEvent>,
) {
    let mut buf = vec![0u8; READ_BUF];
    loop {
        // Drain every complete frame already buffered before reading more.
        loop {
            match decoder.next_frame() {
                Ok(Some(payload)) => match decode_payload::<DaemonMessage>(&payload) {
                    Ok(message) => {
                        if !route_message(shared, events_tx, message) {
                            shared.disconnect();
                            return;
                        }
                    }
                    Err(err) => {
                        tracing::error!(%err, "failed to decode daemon message; closing");
                        shared.disconnect();
                        return;
                    }
                },
                Ok(None) => break,
                Err(err) => {
                    tracing::error!(%err, "framing error from daemon; closing");
                    shared.disconnect();
                    return;
                }
            }
        }

        match read_half.read(&mut buf) {
            Ok(0) => {
                tracing::debug!("daemon closed the connection");
                shared.disconnect();
                return;
            }
            Ok(n) => decoder.push(&buf[..n]),
            Err(ref err) if err.kind() == std::io::ErrorKind::Interrupted => {}
            Err(err) => {
                tracing::warn!(%err, "socket read error; closing");
                shared.disconnect();
                return;
            }
        }
    }
}

/// Route one decoded [`DaemonMessage`] to its waiter or the events channel.
fn route_message(
    shared: &Arc<Shared>,
    events_tx: &flume::Sender<DaemonEvent>,
    message: DaemonMessage,
) -> bool {
    match message {
        DaemonMessage::Response { request_id, body } => {
            let waiter = shared
                .pending
                .lock()
                .expect("pending mutex poisoned")
                .remove(&request_id);
            match waiter {
                Some(waiter) => {
                    // The caller may have already timed out and dropped its
                    // receiver; a failed send is then harmless.
                    let _ = waiter.send(body);
                }
                None => tracing::warn!(request_id, "response for an unknown request id"),
            }
        }
        DaemonMessage::Event(event) => {
            if events_tx.try_send(event).is_err() {
                tracing::warn!("event queue overflow; reconnect for an authoritative snapshot");
                return false;
            }
        }
        DaemonMessage::HelloAck(_) | DaemonMessage::HelloReject(_) => {
            tracing::warn!("unexpected handshake message after connect");
        }
        _ => tracing::warn!("ignoring unrecognized daemon message"),
    }
    true
}

/// Encode a client `message` and write the whole frame to `stream`.
fn write_frame(stream: &mut UnixStream, message: &ClientMessage) -> Result<(), ClientError> {
    let frame = encode_frame(message)?;
    stream.write_all(&frame)?;
    stream.flush()?;
    Ok(())
}

/// Read from `stream` into `decoder` until one complete [`DaemonMessage`] is
/// available; `Ok(None)` on EOF.
fn read_message(
    stream: &mut UnixStream,
    decoder: &mut FrameDecoder,
    buf: &mut [u8],
) -> Result<Option<DaemonMessage>, ClientError> {
    loop {
        if let Some(payload) = decoder.next_frame()? {
            return Ok(Some(decode_payload(&payload)?));
        }
        let n = stream.read(buf)?;
        if n == 0 {
            return Ok(None);
        }
        decoder.push(&buf[..n]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::{ProjectGroupId, ProjectId, TerminalId, Timestamp};
    use protocol::{ErrorCode, HelloAck, HelloReject};
    use std::os::unix::net::UnixListener;
    use std::thread;
    use std::time::{Duration, Instant};

    /// A short socket path under `/tmp` (macOS caps `sun_path` at ~104 bytes, so
    /// the long scratchpad path will not fit).
    struct SockPath {
        _dir: tempfile::TempDir,
        path: PathBuf,
    }

    fn socket_path() -> SockPath {
        let dir = tempfile::tempdir_in("/tmp").expect("tempdir in /tmp");
        let path = dir.path().join("s");
        SockPath { _dir: dir, path }
    }

    fn read_client_message(
        stream: &mut UnixStream,
        decoder: &mut FrameDecoder,
        buf: &mut [u8],
    ) -> ClientMessage {
        loop {
            if let Some(payload) = decoder.next_frame().unwrap() {
                return decode_payload(&payload).unwrap();
            }
            let n = stream.read(buf).unwrap();
            assert!(n > 0, "unexpected EOF from client");
            decoder.push(&buf[..n]);
        }
    }

    fn write_daemon_message(stream: &mut UnixStream, message: &DaemonMessage) {
        let frame = encode_frame(message).unwrap();
        stream.write_all(&frame).unwrap();
        stream.flush().unwrap();
    }

    fn hello_ack() -> HelloAck {
        HelloAck {
            protocol_version: PROTOCOL_VERSION,
            daemon_version: "test-daemon".to_string(),
            instance_id: "instance-1".to_string(),
            started_at: Timestamp::now(),
        }
    }

    /// Handshake, then answer the client's requests in order with `replies`,
    /// one per request. The join handle yields the [`Request`]s that arrived, so
    /// a test can assert on the exact wire shape a wrapper produced.
    fn scripted_server(
        listener: UnixListener,
        replies: Vec<Result<Response, ProtocolError>>,
    ) -> thread::JoinHandle<Vec<Request>> {
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut decoder = FrameDecoder::new();
            let mut buf = vec![0u8; 8192];

            match read_client_message(&mut stream, &mut decoder, &mut buf) {
                ClientMessage::Hello(hello) => assert_eq!(hello.client_kind, ClientKind::Gui),
                other => panic!("expected Hello, got {other:?}"),
            }
            write_daemon_message(&mut stream, &DaemonMessage::HelloAck(hello_ack()));

            let mut requests = Vec::with_capacity(replies.len());
            for reply in replies {
                match read_client_message(&mut stream, &mut decoder, &mut buf) {
                    ClientMessage::Request { request_id, body } => {
                        requests.push(body);
                        write_daemon_message(
                            &mut stream,
                            &DaemonMessage::Response {
                                request_id,
                                body: reply,
                            },
                        );
                    }
                    other => panic!("expected Request, got {other:?}"),
                }
            }

            // Block until the client drops (EOF), then hand back what it sent.
            let _ = stream.read(&mut buf);
            requests
        })
    }

    #[test]
    fn connect_request_and_event_round_trip() {
        let sock = socket_path();
        let listener = UnixListener::bind(&sock.path).unwrap();

        let terminal_id = TerminalId::new();
        let expected = Response::Snapshot {
            project_groups: vec![],
            projects: vec![],
            workspaces: vec![],
            sessions: vec![],
            providers: vec![],
            agent_profiles: vec![],
            worktree_shares: vec![],
            app_state: vec![("sidebar_width".to_string(), "280".to_string())],
            external_agents: vec![],
            pull_requests: domain::PullRequestState::default(),
            jobs: vec![],
            usage: vec![],
        };
        let server_expected = expected.clone();

        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut decoder = FrameDecoder::new();
            let mut buf = vec![0u8; 8192];

            match read_client_message(&mut stream, &mut decoder, &mut buf) {
                ClientMessage::Hello(hello) => {
                    assert_eq!(hello.protocol_version, PROTOCOL_VERSION);
                    assert_eq!(hello.client_kind, ClientKind::Gui);
                }
                other => panic!("expected Hello, got {other:?}"),
            }
            write_daemon_message(&mut stream, &DaemonMessage::HelloAck(hello_ack()));

            let request_id = match read_client_message(&mut stream, &mut decoder, &mut buf) {
                ClientMessage::Request { request_id, body } => {
                    assert_eq!(body, Request::GetSnapshot);
                    request_id
                }
                other => panic!("expected Request, got {other:?}"),
            };
            write_daemon_message(
                &mut stream,
                &DaemonMessage::Response {
                    request_id,
                    body: Ok(server_expected),
                },
            );
            write_daemon_message(
                &mut stream,
                &DaemonMessage::Event(DaemonEvent::TerminalActivity { terminal_id }),
            );

            // Block until the client drops (EOF), then exit.
            let _ = stream.read(&mut buf);
        });

        let client = Client::connect(&sock.path, "0.1.0").unwrap();
        assert!(client.is_connected());
        assert_eq!(client.daemon_info().instance_id, "instance-1");
        assert_eq!(client.daemon_info().daemon_version, "test-daemon");

        let response = client.request(Request::GetSnapshot).unwrap();
        assert_eq!(response, expected);

        let event = client.events().recv().unwrap();
        assert_eq!(event, DaemonEvent::TerminalActivity { terminal_id });

        drop(client);
        server.join().unwrap();
    }

    #[test]
    fn overflowing_events_disconnect_the_client() {
        let sock = socket_path();
        let listener = UnixListener::bind(&sock.path).unwrap();
        let terminal_id = TerminalId::new();

        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut decoder = FrameDecoder::new();
            let mut buf = vec![0u8; 8192];
            match read_client_message(&mut stream, &mut decoder, &mut buf) {
                ClientMessage::Hello(_) => {}
                other => panic!("expected Hello, got {other:?}"),
            }
            write_daemon_message(&mut stream, &DaemonMessage::HelloAck(hello_ack()));
            let request_id = match read_client_message(&mut stream, &mut decoder, &mut buf) {
                ClientMessage::Request { request_id, .. } => request_id,
                other => panic!("expected Request, got {other:?}"),
            };
            write_daemon_message(
                &mut stream,
                &DaemonMessage::Response {
                    request_id,
                    body: Ok(Response::Ack),
                },
            );
            for _ in 0..80 {
                write_daemon_message(
                    &mut stream,
                    &DaemonMessage::Event(DaemonEvent::TerminalActivity { terminal_id }),
                );
            }
            let _ = stream.read(&mut buf);
        });

        let client = Client::connect(&sock.path, "0.1.0").unwrap();
        let _ = client.request(Request::GetSnapshot);
        let deadline = Instant::now() + Duration::from_secs(2);
        while client.is_connected() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        assert!(
            !client.is_connected(),
            "the reader must drop a stalled event queue rather than block replies"
        );
        drop(client);
        server.join().unwrap();
    }

    #[test]
    fn set_app_state_round_trip() {
        let sock = socket_path();
        let listener = UnixListener::bind(&sock.path).unwrap();
        let server = scripted_server(listener, vec![Ok(Response::Ack)]);

        let client = Client::connect(&sock.path, "0.1.0").unwrap();
        client.set_app_state("sidebar_width", "280").unwrap();
        drop(client);

        assert_eq!(
            server.join().unwrap(),
            vec![Request::SetAppState {
                key: "sidebar_width".to_string(),
                value: "280".to_string(),
            }]
        );
    }

    #[test]
    fn refresh_pull_requests_requires_an_ack() {
        let sock = socket_path();
        let listener = UnixListener::bind(&sock.path).unwrap();
        let server = scripted_server(
            listener,
            vec![Ok(Response::Ack), Ok(Response::AppState { value: None })],
        );

        let client = Client::connect(&sock.path, "0.1.0").unwrap();
        client.refresh_pull_requests().unwrap();
        match client.refresh_pull_requests() {
            Err(ClientError::UnexpectedResponse { expected }) => assert_eq!(expected, "Ack"),
            other => panic!("expected UnexpectedResponse, got {other:?}"),
        }
        drop(client);

        assert_eq!(
            server.join().unwrap(),
            vec![Request::RefreshPullRequests, Request::RefreshPullRequests]
        );
    }

    #[test]
    fn project_group_creation_requests_round_trip() {
        let sock = socket_path();
        let listener = UnixListener::bind(&sock.path).unwrap();
        let server = scripted_server(
            listener,
            vec![
                Ok(Response::Ack),
                Ok(Response::Ack),
                Ok(Response::Ack),
                Ok(Response::Ack),
                Ok(Response::Ack),
            ],
        );
        let group_id = ProjectGroupId::new();
        let project_id = ProjectId::new();
        let project_path = PathBuf::from("/tmp/product-x-api");

        let client = Client::connect(&sock.path, "0.1.0").unwrap();
        client.create_project_group("Product X").unwrap();
        client
            .add_project_to_group(&project_path, group_id)
            .unwrap();
        client
            .rename_project_group(group_id, "Product X Local")
            .unwrap();
        client.move_project(project_id, None).unwrap();
        client.remove_project_group(group_id).unwrap();
        drop(client);

        assert_eq!(
            server.join().unwrap(),
            vec![
                Request::CreateProjectGroup {
                    name: "Product X".to_string(),
                },
                Request::AddProjectToGroup {
                    path: project_path,
                    project_group_id: group_id,
                },
                Request::RenameProjectGroup {
                    project_group_id: group_id,
                    name: "Product X Local".to_string(),
                },
                Request::MoveProject {
                    project_id,
                    project_group_id: None,
                },
                Request::RemoveProjectGroup {
                    project_group_id: group_id,
                },
            ]
        );
    }

    #[test]
    fn get_app_state_round_trip() {
        let sock = socket_path();
        let listener = UnixListener::bind(&sock.path).unwrap();
        let server = scripted_server(
            listener,
            vec![
                Ok(Response::AppState {
                    value: Some("280".to_string()),
                }),
                Ok(Response::AppState { value: None }),
            ],
        );

        let client = Client::connect(&sock.path, "0.1.0").unwrap();
        assert_eq!(
            client.get_app_state("sidebar_width").unwrap(),
            Some("280".to_string())
        );
        assert_eq!(client.get_app_state("never_written").unwrap(), None);
        drop(client);

        assert_eq!(
            server.join().unwrap(),
            vec![
                Request::GetAppState {
                    key: "sidebar_width".to_string(),
                },
                Request::GetAppState {
                    key: "never_written".to_string(),
                },
            ]
        );
    }

    #[test]
    fn get_app_state_rejects_a_non_app_state_response() {
        let sock = socket_path();
        let listener = UnixListener::bind(&sock.path).unwrap();
        let server = scripted_server(listener, vec![Ok(Response::Ack)]);

        let client = Client::connect(&sock.path, "0.1.0").unwrap();
        match client.get_app_state("sidebar_width") {
            Err(ClientError::UnexpectedResponse { expected }) => assert_eq!(expected, "AppState"),
            other => panic!("expected UnexpectedResponse, got {other:?}"),
        }
        drop(client);
        server.join().unwrap();
    }

    #[test]
    fn get_app_state_surfaces_a_daemon_error() {
        let sock = socket_path();
        let listener = UnixListener::bind(&sock.path).unwrap();
        let server = scripted_server(
            listener,
            vec![Err(ProtocolError::new(ErrorCode::Internal, "db is gone"))],
        );

        let client = Client::connect(&sock.path, "0.1.0").unwrap();
        match client.get_app_state("sidebar_width") {
            Err(ClientError::Protocol(err)) => assert_eq!(err.code, ErrorCode::Internal),
            other => panic!("expected Protocol error, got {other:?}"),
        }
        drop(client);
        server.join().unwrap();
    }

    #[test]
    fn set_provider_executable_pins_and_clears_the_override() {
        let sock = socket_path();
        let listener = UnixListener::bind(&sock.path).unwrap();
        let server = scripted_server(listener, vec![Ok(Response::Ack), Ok(Response::Ack)]);

        let provider_id = AgentProviderId::new("claude");
        let client = Client::connect(&sock.path, "0.1.0").unwrap();
        client
            .set_provider_executable(&provider_id, Some(PathBuf::from("/opt/bin/claude")))
            .unwrap();
        client.set_provider_executable(&provider_id, None).unwrap();
        drop(client);

        assert_eq!(
            server.join().unwrap(),
            vec![
                Request::SetProviderExecutable {
                    provider_id: provider_id.clone(),
                    path: Some(PathBuf::from("/opt/bin/claude")),
                },
                Request::SetProviderExecutable {
                    provider_id,
                    path: None,
                },
            ]
        );
    }

    #[test]
    fn set_provider_executable_rejects_a_non_ack_response() {
        let sock = socket_path();
        let listener = UnixListener::bind(&sock.path).unwrap();
        let server = scripted_server(listener, vec![Ok(Response::AppState { value: None })]);

        let client = Client::connect(&sock.path, "0.1.0").unwrap();
        match client.set_provider_executable(&AgentProviderId::new("claude"), None) {
            Err(ClientError::UnexpectedResponse { expected }) => assert_eq!(expected, "Ack"),
            other => panic!("expected UnexpectedResponse, got {other:?}"),
        }
        drop(client);
        server.join().unwrap();
    }

    #[test]
    fn hello_reject_maps_to_version_mismatch() {
        let sock = socket_path();
        let listener = UnixListener::bind(&sock.path).unwrap();

        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut decoder = FrameDecoder::new();
            let mut buf = vec![0u8; 8192];
            let _ = read_client_message(&mut stream, &mut decoder, &mut buf);
            write_daemon_message(
                &mut stream,
                &DaemonMessage::HelloReject(HelloReject {
                    daemon_protocol_version: PROTOCOL_VERSION + 1,
                    reason: "protocol version mismatch".to_string(),
                }),
            );
            let _ = stream.read(&mut buf);
        });

        let err = match Client::connect(&sock.path, "0.1.0") {
            Ok(_) => panic!("expected connect to fail on HelloReject"),
            Err(err) => err,
        };
        match err {
            ClientError::VersionMismatch { expected, got } => {
                assert_eq!(expected, PROTOCOL_VERSION);
                assert_eq!(got, PROTOCOL_VERSION + 1);
            }
            other => panic!("expected VersionMismatch, got {other:?}"),
        }
        server.join().unwrap();
    }

    #[test]
    fn request_after_daemon_close_reports_disconnected() {
        let sock = socket_path();
        let listener = UnixListener::bind(&sock.path).unwrap();

        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut decoder = FrameDecoder::new();
            let mut buf = vec![0u8; 8192];
            let _ = read_client_message(&mut stream, &mut decoder, &mut buf);
            write_daemon_message(&mut stream, &DaemonMessage::HelloAck(hello_ack()));
            // Drop the connection immediately after the handshake.
        });

        let client = Client::connect(&sock.path, "0.1.0").unwrap();
        server.join().unwrap();

        // Wait for the reader to observe EOF and flip the flag.
        for _ in 0..200 {
            if !client.is_connected() {
                break;
            }
            thread::sleep(Duration::from_millis(5));
        }
        assert!(!client.is_connected());
        assert!(matches!(
            client.request(Request::GetSnapshot),
            Err(ClientError::Disconnected)
        ));
    }
}
