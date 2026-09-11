//! The IPC server: bind the UDS, accept connections, run the handshake, and
//! pump requests/events (§9, §10). Built on tokio; blocking work (git, PTY
//! spawn) is offloaded with `spawn_blocking` so the accept loop never stalls.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};

use domain::ClientId;
use protocol::{
    decode_payload, encode_frame, ClientMessage, DaemonMessage, Hello, HelloAck, HelloReject,
    Request, MAX_FRAME_SIZE, PROTOCOL_VERSION,
};

use crate::core::Daemon;

/// Bind the socket with private permissions (ADR-004), replacing a stale socket.
pub fn bind(socket_path: &Path) -> std::io::Result<UnixListener> {
    if let Some(parent) = socket_path.parent() {
        crate::paths::ensure_private_dir(parent)
            .map_err(|e| std::io::Error::other(e.to_string()))?;
    }
    // Unlink a stale socket (the flock singleton guarantees no live listener).
    if socket_path.exists() {
        let _ = std::fs::remove_file(socket_path);
    }
    let listener = UnixListener::bind(socket_path)?;
    // Socket 0600 (ADR-004).
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(socket_path, std::fs::Permissions::from_mode(0o600))?;
    Ok(listener)
}

/// Run the accept loop until a shutdown is requested (§9.1).
pub async fn serve(daemon: Arc<Daemon>, listener: UnixListener, socket_path: PathBuf) {
    loop {
        if daemon.is_shutting_down() {
            break;
        }
        let accept = tokio::select! {
            r = listener.accept() => r,
            () = wait_for_shutdown(&daemon) => break,
        };
        match accept {
            Ok((stream, _addr)) => {
                let daemon = daemon.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle_connection(daemon, stream).await {
                        tracing::debug!(error = %e, "connection ended");
                    }
                });
            }
            Err(e) => {
                tracing::warn!(error = %e, "accept failed");
                break;
            }
        }
    }
    let _ = std::fs::remove_file(&socket_path);
    tracing::info!("accept loop stopped");
}

/// Poll the shutdown flag so the accept loop can exit promptly after StopDaemon.
async fn wait_for_shutdown(daemon: &Arc<Daemon>) {
    loop {
        if daemon.is_shutting_down() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

/// Read one length-prefixed frame payload (§ADR-004 framing) from `reader`.
async fn read_frame(reader: &mut (impl AsyncReadExt + Unpin)) -> std::io::Result<Option<Vec<u8>>> {
    let mut len_buf = [0u8; 4];
    match reader.read_exact(&mut len_buf).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let len = u32::from_be_bytes(len_buf) as usize;
    if len > MAX_FRAME_SIZE {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "frame exceeds MAX_FRAME_SIZE",
        ));
    }
    let mut payload = vec![0u8; len];
    reader.read_exact(&mut payload).await?;
    Ok(Some(payload))
}

/// Handle a single client connection: handshake, then request/event pumping.
async fn handle_connection(daemon: Arc<Daemon>, stream: UnixStream) -> std::io::Result<()> {
    let (mut read_half, mut write_half) = stream.into_split();

    // ---- Handshake (§9.2) ----
    let Some(first) = read_frame(&mut read_half).await? else {
        return Ok(());
    };
    let hello: ClientMessage = match decode_payload(&first) {
        Ok(ClientMessage::Hello(h)) => ClientMessage::Hello(h),
        _ => {
            // Not a Hello: reject and close.
            let reject = DaemonMessage::HelloReject(HelloReject {
                daemon_protocol_version: PROTOCOL_VERSION,
                reason: "expected Hello".into(),
            });
            write_message(&mut write_half, &reject).await?;
            return Ok(());
        }
    };
    let ClientMessage::Hello(Hello {
        protocol_version, ..
    }) = hello
    else {
        unreachable!()
    };
    if protocol_version != PROTOCOL_VERSION {
        let reject = DaemonMessage::HelloReject(HelloReject {
            daemon_protocol_version: PROTOCOL_VERSION,
            reason: format!("protocol {protocol_version} != {PROTOCOL_VERSION}"),
        });
        write_message(&mut write_half, &reject).await?;
        return Ok(());
    }

    let client_id = ClientId::new();
    let rx = daemon.registry().register(client_id);

    let ack = DaemonMessage::HelloAck(HelloAck {
        protocol_version: PROTOCOL_VERSION,
        daemon_version: daemon.version.clone(),
        instance_id: daemon.instance_id.clone(),
        started_at: daemon.started_at,
    });
    // Deliver the ack through the per-client channel so a single writer owns the
    // socket write half.
    if !daemon.registry().send_to(client_id, ack) {
        daemon.registry().unregister(client_id);
        return Ok(());
    }

    // ---- Writer task: drain the client channel to the socket ----
    let writer_daemon = Arc::clone(&daemon);
    let writer = tokio::spawn(async move {
        while let Ok(msg) = rx.recv_async().await {
            if write_message(&mut write_half, &msg).await.is_err() {
                break;
            }
            if writer_daemon.registry().needs_resync(client_id) {
                let daemon = Arc::clone(&writer_daemon);
                if tokio::task::spawn_blocking(move || daemon.recover_client(client_id))
                    .await
                    .is_err()
                {
                    break;
                }
            }
        }
    });

    // ---- Read loop: decode requests, dispatch, reply ----
    let result = read_loop(&daemon, client_id, &mut read_half).await;

    // Cleanup on disconnect (§9.1): drop subscriptions + channel, ending the writer.
    daemon.registry().unregister(client_id);
    writer.abort();
    result
}

/// Decode requests from one client and dispatch them (§10.1).
///
/// Each request is handled in its own task rather than awaited inline. Requests
/// are correlated by `request_id`, so the protocol has always allowed a client
/// to have several in flight; awaiting each one in turn meant a single slow
/// handler — `git status` may take up to 30 s (ADR-008) — blocked every later
/// request on the same connection, keystrokes included.
async fn read_loop(
    daemon: &Arc<Daemon>,
    client_id: ClientId,
    read_half: &mut (impl AsyncReadExt + Unpin),
) -> std::io::Result<()> {
    let mut in_flight = tokio::task::JoinSet::new();

    while let Some(payload) = read_frame(read_half).await? {
        // Reap finished handlers so the set does not grow across a long session.
        while in_flight.try_join_next().is_some() {}

        let msg: ClientMessage = match decode_payload(&payload) {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!(error = %e, "undecodable client frame; closing");
                break;
            }
        };
        let ClientMessage::Request { request_id, body } = msg else {
            // A second Hello or unknown message mid-stream: ignore.
            continue;
        };

        // Terminal subscription is per-client, so it is applied here where the
        // client id is known (the core builds the snapshot).
        let sub: Option<(bool, domain::TerminalId)> = match &body {
            Request::AttachTerminal { terminal_id, .. } => Some((true, *terminal_id)),
            Request::DetachTerminal { terminal_id } => Some((false, *terminal_id)),
            _ => None,
        };

        // §22: one span per request, carrying the `request_id` and the request
        // *variant* only. The payload never goes near the log — a
        // `WriteTerminalInput` body is the user's keystrokes and a
        // `CreateContextEnvelope` carries prompt text (§23).
        let span = tracing::info_span!(
            "ipc.request",
            request_id,
            request = request_name(&body),
            client = %client_id,
        );

        let handler_daemon = daemon.clone();
        in_flight.spawn(async move {
            let daemon = handler_daemon;
            let dispatch = daemon.clone();
            let response = tokio::task::spawn_blocking(move || {
                let _entered = span.entered();
                dispatch.handle_request(body)
            })
            .await
            .unwrap_or_else(|_| {
                Err(protocol::ProtocolError::new(
                    protocol::ErrorCode::Internal,
                    "handler task panicked",
                ))
            });

            // Apply/clear the subscription only on a successful attach/detach.
            if let Some((attach, terminal_id)) = sub {
                if response.is_ok() {
                    if attach {
                        daemon.registry().subscribe(client_id, terminal_id);
                    } else {
                        daemon.registry().unsubscribe(client_id, terminal_id);
                    }
                }
            }

            // A dropped response is not recoverable for the client: it blocks on
            // its `request_id` with no timeout of its own (§10.1). The outbound
            // queue is bounded (§10.5) and a busy terminal can fill it, so on a
            // failed send the connection is torn down and the client sees a clean
            // disconnect instead of hanging forever.
            let delivered = daemon.registry().send_to(
                client_id,
                DaemonMessage::Response {
                    request_id,
                    body: response,
                },
            );
            if !delivered {
                tracing::warn!(
                    %request_id,
                    "could not deliver a response; dropping the client connection"
                );
                daemon.registry().unregister(client_id);
            }
        });

        if daemon.is_shutting_down() {
            break;
        }
    }

    // Let the handlers still running finish and answer before the caller
    // unregisters the client and aborts the writer.
    while in_flight.join_next().await.is_some() {}
    Ok(())
}

/// The variant name of a request, for the `ipc.request` span (§22).
///
/// Returns a `&'static str` on purpose: it is impossible for a payload —
/// keystrokes, environment values, prompt text — to reach the log through this
/// (§22 "never terminal content nor environment values", §23).
fn request_name(request: &Request) -> &'static str {
    match request {
        Request::GetSnapshot => "GetSnapshot",
        Request::StopDaemon { .. } => "StopDaemon",
        Request::FactoryReset => "FactoryReset",
        Request::GetAppState { .. } => "GetAppState",
        Request::SetAppState { .. } => "SetAppState",
        Request::RefreshPullRequests => "RefreshPullRequests",
        Request::GetStats => "GetStats",
        Request::ListHarnessFeatures { .. } => "ListHarnessFeatures",
        Request::GetHarnessFeature { .. } => "GetHarnessFeature",
        Request::GetHarnessTimeline { .. } => "GetHarnessTimeline",
        Request::ReadHarnessArtifact { .. } => "ReadHarnessArtifact",
        Request::RegisterHarnessFeature { .. } => "RegisterHarnessFeature",
        Request::RegisterHarnessFromIssue { .. } => "RegisterHarnessFromIssue",
        Request::HarnessAdvance { .. } => "HarnessAdvance",
        Request::LinkHarnessSession { .. } => "LinkHarnessSession",
        Request::RunHarnessStep { .. } => "RunHarnessStep",
        Request::AskHarness { .. } => "AskHarness",
        Request::StartJob { .. } => "StartJob",
        Request::CancelJob { .. } => "CancelJob",
        Request::ListJobs => "ListJobs",
        Request::ReadJobLog { .. } => "ReadJobLog",
        Request::ValidateHarness { .. } => "ValidateHarness",
        Request::AddProject { .. } => "AddProject",
        Request::AddProjectToGroup { .. } => "AddProjectToGroup",
        Request::CreateProjectGroup { .. } => "CreateProjectGroup",
        Request::RenameProjectGroup { .. } => "RenameProjectGroup",
        Request::RemoveProjectGroup { .. } => "RemoveProjectGroup",
        Request::MoveProject { .. } => "MoveProject",
        Request::RemoveProject { .. } => "RemoveProject",
        Request::RefreshProject { .. } => "RefreshProject",
        Request::RenameProject { .. } => "RenameProject",
        Request::ListWorkspaces { .. } => "ListWorkspaces",
        Request::CreateWorktree { .. } => "CreateWorktree",
        Request::RemoveWorktree { .. } => "RemoveWorktree",
        Request::RefreshWorkspaceStatus { .. } => "RefreshWorkspaceStatus",
        Request::ListBranches { .. } => "ListBranches",
        Request::FetchRemote { .. } => "FetchRemote",
        Request::GetChangeContext { .. } => "GetChangeContext",
        Request::GetWorkspaceDiff { .. } => "GetWorkspaceDiff",
        Request::ListFiles { .. } => "ListFiles",
        Request::ReadFile { .. } => "ReadFile",
        Request::WriteFile { .. } => "WriteFile",
        Request::CreatePath { .. } => "CreatePath",
        Request::RenamePath { .. } => "RenamePath",
        Request::DeletePath { .. } => "DeletePath",
        Request::SearchFiles { .. } => "SearchFiles",
        Request::DraftWithJuva { .. } => "DraftWithJuva",
        Request::CreateCommit { .. } => "CreateCommit",
        Request::CreatePullRequest { .. } => "CreatePullRequest",
        Request::CreateShellSession { .. } => "CreateShellSession",
        Request::CreateAgentSession { .. } => "CreateAgentSession",
        Request::CreateChildSession { .. } => "CreateChildSession",
        Request::KillSession { .. } => "KillSession",
        Request::CloseSession { .. } => "CloseSession",
        Request::RestartSession { .. } => "RestartSession",
        Request::RenameSession { .. } => "RenameSession",
        Request::SetSessionRole { .. } => "SetSessionRole",
        Request::CreateContextEnvelope { .. } => "CreateContextEnvelope",
        Request::SendContext { .. } => "SendContext",
        Request::ListContextEnvelopes { .. } => "ListContextEnvelopes",
        Request::AttachTerminal { .. } => "AttachTerminal",
        Request::DetachTerminal { .. } => "DetachTerminal",
        Request::WriteTerminalInput { .. } => "WriteTerminalInput",
        Request::ResizeTerminal { .. } => "ResizeTerminal",
        Request::FetchScrollback { .. } => "FetchScrollback",
        Request::SendSignal { .. } => "SendSignal",
        Request::ListAgentProviders => "ListAgentProviders",
        Request::ListProviderUsage => "ListProviderUsage",
        Request::RefreshAgentDetection { .. } => "RefreshAgentDetection",
        Request::SetProviderExecutable { .. } => "SetProviderExecutable",
        Request::SaveAgentProfile { .. } => "SaveAgentProfile",
        Request::RemoveAgentProfile { .. } => "RemoveAgentProfile",
        // `Request` is `#[non_exhaustive]`: a variant this daemon does not know
        // still gets a span, without ever formatting its body.
        _ => "Unknown",
    }
}

/// Frame and write one daemon message.
async fn write_message(
    writer: &mut (impl AsyncWriteExt + Unpin),
    msg: &DaemonMessage,
) -> std::io::Result<()> {
    let frame = encode_frame(msg)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
    writer.write_all(&frame).await?;
    writer.flush().await
}
