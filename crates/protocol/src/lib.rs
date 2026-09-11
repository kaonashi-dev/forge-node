//! # protocol
//!
//! The wire protocol between the Forge GUI client and the daemon (§10). This
//! crate is transport-agnostic: it defines the message types, the
//! length-prefixed MessagePack framing (ADR-004, [`framing`]), the handshake
//! (§9.2, [`hello`]), and the request/response/event vocabulary. It depends
//! only on `domain` for the shared wire types and never on `tokio` or any
//! socket, so both the daemon's blocking accept loop and the client's async IPC
//! thread build on it (§17).
//!
//! Modules mirror §17: [`framing`], [`hello`], [`request`], [`response`],
//! [`event`], [`error`].
//!
//! Every protocol enum is `#[non_exhaustive]`; closed-set unit enums
//! ([`error::ErrorCode`], [`hello::ClientKind`], [`request::Signal`],
//! [`event::NoticeLevel`]) additionally carry a `#[serde(other)] Unknown`
//! variant so decoding tolerates variants added by a newer peer (§10.1).

pub mod error;
pub mod event;
pub mod framing;
pub mod hello;
pub mod request;
pub mod response;

use serde::{Deserialize, Serialize};

pub use error::{ErrorCode, ProtocolError};
pub use event::{DaemonEvent, NoticeLevel};
pub use framing::{
    decode_frame, decode_payload, encode_frame, to_json_string, FrameDecoder, ProtocolCodecError,
    MAX_FRAME_SIZE,
};
pub use hello::{ClientKind, Hello, HelloAck, HelloReject};
pub use request::{RemoveProjectPolicy, Request, SendContextSpawn, Signal};
pub use response::{DaemonStats, ProviderInfo, Response, SessionsByState};

/// The single integer protocol version (§9.2). In the MVP the GUI and daemon
/// are distributed together and require exact equality; N/N-1 compatibility is
/// a post-MVP decision.
///
/// Bump this whenever [`Request`], [`Response`] or [`DaemonEvent`] gains,
/// loses or reshapes a variant. A daemon left running from an older build
/// still passes the handshake on an unchanged number and then fails every new
/// request with an undecodable frame — the connection dies with no `Response`,
/// so the caller waits on a reply that never comes. Equality at connect turns
/// that silent stall into `ClientError::VersionMismatch`.
///
/// - 7 → 8: the harness requests (`ListHarnessFeatures` … `ValidateHarness`).
/// - 8 → 9: `workspace_id` on the two harness register requests.
/// - 9 → 10: jobs — headless agent runs (`StartJob` … `ReadJobLog`,
///   `JobUpdated`, `JobOutput`, and `jobs` in the snapshot).
/// - 10 → 11: `RunHarnessStep`, `HarnessFeatureChanged`, and `schema` on a
///   `JobRequest`.
/// - 11 → 12: `prompt` on a `Job` and `job` on a `HarnessEvent`, so a client
///   can show what a step was asked and can still find the transcript of a
///   step the daemon no longer remembers running; and `AskHarness`.
/// - 12 → 13: `FactoryReset` request and event.
/// - 13 → 14: session baselines and the surfaces that read them —
///   `GetSessionChanges`, `GetWorkspaceReview`, `GetSessionTranscript`,
///   `base_commit` on a `Session`, and `JuvaDraftReady`, which is what
///   `DraftWithJuva` now answers with instead of `Response::JuvaDraft`.
/// - 14 → 15: `SearchKind::Definition` and `query` on `SearchResults`. Both
///   are additions and neither is optional on the wire: a fieldless variant an
///   old peer has never heard of, and a struct field that changes the encoded
///   arity, are decode failures rather than ignored extras — which is a closed
///   connection with no explanation unless this number moves with them.
/// - 15 → 16: a launch profile is a directory, not an environment:
///   `AgentProfile.env` is gone and `config_dir` takes its place, and
///   `AgentDescriptor.profile_fields` is now `config_dir`. Both change the
///   encoded arity of a struct every snapshot carries.
/// - 16 → 17: two struct fields that change an encoded arity, plus the surface
///   that reads them — `ignored` on a `FileEntry`, `store` on an
///   `ExternalAgentSession`, and `GetExternalTranscript` /
///   `DeleteExternalSession` with `Response::ExternalTranscript`.
/// - 17 → 18: cross-session mediation — `SendContext` / `ListContextEnvelopes`
///   with `Response::ContextEnvelopes`, so one Forge session can cite another
///   and spawn a child with any provider without peer-to-peer agent APIs.
/// - 18 → 19: terminal history generation, column patches, and compact cells.
///   `TerminalSnapshot`/`TerminalDelta`/`ScrollbackRows` change encoded arity,
///   and MessagePack cells drop repeated field names.
pub const PROTOCOL_VERSION: u32 = 19;

/// A message sent by a client to the daemon (§10.1).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum ClientMessage {
    /// The opening handshake (§9.2).
    Hello(Hello),
    /// A request correlated by `request_id` with its future response.
    Request {
        /// Client-chosen id echoed back in the matching response.
        request_id: u64,
        /// The request payload.
        body: Request,
    },
}

/// A message sent by the daemon to a client (§10.1).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum DaemonMessage {
    /// Handshake accepted (§9.2).
    HelloAck(HelloAck),
    /// Handshake rejected on version mismatch (§9.2).
    HelloReject(HelloReject),
    /// The response to a client request, correlated by `request_id`.
    Response {
        /// The `request_id` from the originating [`ClientMessage::Request`].
        request_id: u64,
        /// `Ok` on success, `Err` with a [`ProtocolError`] on failure.
        body: Result<Response, ProtocolError>,
    },
    /// An unsolicited event (§10.3).
    Event(DaemonEvent),
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::{
        AgentCapabilities, AgentDescriptor, AgentProviderId, Cell, CellFlags, Color,
        DetectionResult, DetectionStatus, Project, ProjectGroup, ProjectGroupId, ProjectId,
        PtySize, PullRequest, PullRequestFailure, PullRequestFailureKind, PullRequestLabel,
        PullRequestRelations, PullRequestSource, PullRequestSourceStatus, PullRequestState,
        PullRequestViewer, ReviewDecision, Session, SessionKind, SessionRole, SessionState,
        SessionTitle, Timestamp, VersionProbe, Workspace, WorkspaceId, WorkspaceKind,
        WorkspaceStatus,
    };
    use std::path::PathBuf;

    /// Encode a message to a frame, feed it through a fresh [`FrameDecoder`],
    /// and decode the yielded payload back into `T`.
    fn frame_round_trip<T>(msg: &T) -> T
    where
        T: Serialize + serde::de::DeserializeOwned,
    {
        let frame = encode_frame(msg).unwrap();
        let mut dec = FrameDecoder::new();
        dec.push(&frame);
        let payload = dec.next_frame().unwrap().expect("a complete frame");
        assert!(dec.next_frame().unwrap().is_none(), "exactly one frame");
        decode_payload(&payload).unwrap()
    }

    #[test]
    fn client_message_round_trips_through_framing() {
        let msg = ClientMessage::Request {
            request_id: 42,
            body: Request::CreateShellSession {
                workspace_id: WorkspaceId::new(),
                parent: None,
                role: SessionRole::Generic,
            },
        };
        let back = frame_round_trip(&msg);
        assert_eq!(msg, back);
    }

    #[test]
    fn daemon_message_ok_and_err_round_trip_through_framing() {
        let ok = DaemonMessage::Response {
            request_id: 7,
            body: Ok(Response::Ack),
        };
        assert_eq!(ok, frame_round_trip(&ok));

        let err = DaemonMessage::Response {
            request_id: 8,
            body: Err(ProtocolError::precondition_failed("session is Running")),
        };
        assert_eq!(err, frame_round_trip(&err));

        let event = DaemonMessage::Event(DaemonEvent::TerminalActivity {
            terminal_id: domain::TerminalId::new(),
        });
        assert_eq!(event, frame_round_trip(&event));
    }

    fn sample_timestamp() -> Timestamp {
        Timestamp::now()
    }

    fn sample_project() -> Project {
        Project {
            id: ProjectId::new(),
            project_group_id: None,
            name: "forge".to_string(),
            icon: Some("🦀".to_string()),
            root_path: PathBuf::from("/home/dev/forge"),
            git_root: Some(PathBuf::from("/home/dev/forge")),
            created_at: sample_timestamp(),
            last_opened_at: sample_timestamp(),
        }
    }

    fn sample_project_group() -> ProjectGroup {
        ProjectGroup {
            id: ProjectGroupId::new(),
            name: "Forge".to_string(),
            created_at: sample_timestamp(),
        }
    }

    fn sample_workspace(project_id: ProjectId) -> Workspace {
        Workspace {
            id: WorkspaceId::new(),
            project_id,
            kind: WorkspaceKind::Main,
            path: PathBuf::from("/home/dev/forge"),
            branch: Some("main".to_string()),
            display_name: None,
            managed_by_app: false,
            created_at: sample_timestamp(),
            status: WorkspaceStatus::default(),
        }
    }

    fn sample_session(workspace_id: WorkspaceId) -> Session {
        let id = domain::SessionId::new();
        Session {
            id,
            workspace_id,
            kind: SessionKind::Shell,
            role: SessionRole::Generic,
            parent_session_id: None,
            root_session_id: id,
            terminal_id: Some(domain::TerminalId::new()),
            agent_provider_id: None,
            agent_profile_id: None,
            title: SessionTitle::default(),
            state: SessionState::Running,
            created_at: sample_timestamp(),
            launch_command: None,
            last_activity_at: sample_timestamp(),
            ended_at: None,
            base_commit: None,
        }
    }

    fn sample_profile() -> domain::AgentProfile {
        domain::AgentProfile {
            id: domain::AgentProfileId::new(),
            provider_id: AgentProviderId::new("claude"),
            name: "Work".to_string(),
            executable: None,
            config_dir: Some(std::path::PathBuf::from(".claude-work")),
            args: vec!["--model".to_string(), "opus".to_string()],
            created_at: sample_timestamp(),
        }
    }

    fn sample_pull_request() -> PullRequest {
        let now = sample_timestamp();
        PullRequest {
            project_id: Some(ProjectId::new()),
            repository: "forge/forge-node".to_string(),
            host: "github.com".to_string(),
            number: 42,
            title: "Add pull-request state".to_string(),
            body: "Carries the complete remote state.".to_string(),
            body_truncated: false,
            url: "https://github.com/forge/forge-node/pull/42".to_string(),
            author: "octocat".to_string(),
            base_ref: "main".to_string(),
            head_ref: "feature/pull-requests".to_string(),
            is_draft: false,
            review_decision: Some(ReviewDecision::ReviewRequired),
            labels: vec![PullRequestLabel {
                name: "enhancement".to_string(),
                color: "84b6eb".to_string(),
            }],
            assignees: vec!["octocat".to_string()],
            review_requests: vec!["reviewer".to_string()],
            additions: 120,
            deletions: 8,
            changed_files: 6,
            comment_count: 3,
            created_at: now,
            updated_at: now,
            relations: PullRequestRelations {
                assigned: true,
                review_requested: false,
                authored: true,
            },
        }
    }

    fn sample_pull_request_state() -> PullRequestState {
        let pull_request = sample_pull_request();
        let project_id = pull_request.project_id.unwrap();
        PullRequestState {
            pull_requests: vec![pull_request],
            viewers: vec![PullRequestViewer {
                host: "github.com".to_string(),
                login: "octocat".to_string(),
            }],
            sources: vec![PullRequestSource {
                project_id,
                host: Some("github.com".to_string()),
                repository: Some("forge/forge-node".to_string()),
                status: PullRequestSourceStatus::Ready,
            }],
            failures: vec![PullRequestFailure {
                host: "github.example.com".to_string(),
                repository: Some("forge/private".to_string()),
                kind: PullRequestFailureKind::RepositoryRefused,
                message: "repository unavailable".to_string(),
            }],
            error: None,
            refreshed_at: Some(sample_timestamp()),
        }
    }

    fn sample_provider() -> ProviderInfo {
        let id = AgentProviderId::new("claude");
        ProviderInfo {
            descriptor: AgentDescriptor {
                id: id.clone(),
                display_name: "Claude".to_string(),
                binary_candidates: vec!["claude".to_string()],
                default_args: vec![],
                version_probe: VersionProbe {
                    args: vec!["--version".to_string()],
                    expect_substring: None,
                    timeout_ms: 2000,
                },
                usage_source: None,
                config_dir: None,
                resume: Some(domain::ResumeStyle::Flag {
                    flag: "--resume".to_string(),
                }),
                prompt: None,
                headless: None,
                acp: None,
                review: Some(domain::ReviewStyle {
                    args: vec!["--permission-mode".to_string(), "plan".to_string()],
                    label: "plan mode".to_string(),
                }),
                capabilities: AgentCapabilities {
                    interactive_tui: true,
                    supports_initial_prompt: true,
                    supports_headless: false,
                    supports_resume: true,
                    supports_review: true,
                },
            },
            detection: DetectionResult {
                provider_id: id,
                status: DetectionStatus::Installed {
                    executable: PathBuf::from("/usr/local/bin/claude"),
                    version: Some("1.0.0".to_string()),
                },
                checked_at: sample_timestamp(),
            },
        }
    }

    #[test]
    fn snapshot_with_domain_objects_round_trips_through_messagepack() {
        let project = sample_project();
        let workspace = sample_workspace(project.id);
        let session = sample_session(workspace.id);
        let snapshot = Response::Snapshot {
            project_groups: vec![sample_project_group()],
            projects: vec![project],
            workspaces: vec![workspace],
            sessions: vec![session],
            providers: vec![sample_provider()],
            agent_profiles: vec![sample_profile()],
            worktree_shares: vec![],
            app_state: vec![("sidebar_width".to_string(), "280".to_string())],
            external_agents: vec![],
            pull_requests: sample_pull_request_state(),
            jobs: vec![],
            usage: vec![],
        };
        let msg = DaemonMessage::Response {
            request_id: 1,
            body: Ok(snapshot),
        };
        assert_eq!(msg, frame_round_trip(&msg));
    }

    #[test]
    fn pull_request_type_round_trips_through_messagepack() {
        let pull_request = sample_pull_request();
        let bytes = rmp_serde::to_vec_named(&pull_request).unwrap();
        let back: PullRequest = rmp_serde::from_slice(&bytes).unwrap();
        assert_eq!(pull_request, back);
    }

    #[test]
    fn pull_request_event_round_trips_through_messagepack() {
        let message = DaemonMessage::Event(DaemonEvent::PullRequestsUpdated {
            state: sample_pull_request_state(),
        });
        assert_eq!(message, frame_round_trip(&message));
    }

    #[test]
    fn attach_ack_and_scrollback_round_trip() {
        let snapshot = TerminalSnapshotFixture::make();
        let ack = Response::AttachAck { snapshot };
        let bytes = encode_frame(&ack).unwrap();
        let mut dec = FrameDecoder::new();
        dec.push(&bytes);
        let payload = dec.next_frame().unwrap().unwrap();
        let back: Response = decode_payload(&payload).unwrap();
        assert_eq!(ack, back);
    }

    #[test]
    fn a_cell_round_trips_as_a_compact_messagepack_tuple() {
        let cell = Cell {
            text: "x".into(),
            fg: Color::Indexed(2),
            bg: Color::Rgb(1, 2, 3),
            flags: CellFlags::BOLD,
        };
        let bytes = rmp_serde::to_vec_named(&cell).unwrap();
        assert!(
            !bytes.windows(4).any(|window| window == b"text"),
            "binary cells must not repeat field names"
        );
        let back: Cell = rmp_serde::from_slice(&bytes).unwrap();
        assert_eq!(back, cell);
        let json = serde_json::to_string(&cell).unwrap();
        assert!(json.contains("\"text\""));
        assert_eq!(serde_json::from_str::<Cell>(&json).unwrap(), cell);
    }

    /// Minimal terminal snapshot to exercise the nested `domain` grid types.
    struct TerminalSnapshotFixture;
    impl TerminalSnapshotFixture {
        fn make() -> domain::TerminalSnapshot {
            domain::TerminalSnapshot {
                scrollback_generation: 0,
                seq: 1,
                size: PtySize::default(),
                visible: vec![domain::Row::blank(80)],
                scrollback_tail: vec![],
                scrollback_len: 0,
                cursor: domain::Cursor::default(),
                modes: domain::TermModes::default(),
                title: Some("bash".to_string()),
            }
        }
    }

    /// A `Hello` as a *newer* peer would send it: same fields plus one this
    /// build has never heard of. Encoded with `to_vec_named` exactly like
    /// [`encode_frame`] does.
    #[derive(Serialize)]
    struct FutureHello {
        protocol_version: u32,
        client_version: String,
        client_kind: ClientKind,
        /// A field added after this build was compiled.
        negotiated_capabilities: Vec<String>,
    }

    #[test]
    fn unknown_struct_fields_from_a_newer_peer_are_ignored() {
        // §10.1 forward compatibility: messages are MessagePack *maps* keyed by
        // field name, so an older peer skips a field it does not know instead of
        // misreading a positional array.
        let future = FutureHello {
            protocol_version: PROTOCOL_VERSION,
            client_version: "9.9.9".to_string(),
            client_kind: ClientKind::Gui,
            negotiated_capabilities: vec!["streaming".to_string()],
        };
        let payload = rmp_serde::to_vec_named(&future).unwrap();

        let back: Hello = decode_payload(&payload).expect("extra fields must not break decoding");
        assert_eq!(back.protocol_version, PROTOCOL_VERSION);
        assert_eq!(back.client_version, "9.9.9");
        assert_eq!(back.client_kind, ClientKind::Gui);
    }

    #[test]
    fn unknown_enum_variants_decode_over_the_real_wire_format() {
        // The `#[serde(other)] Unknown` arms must hold over MessagePack, not just
        // JSON: a newer daemon's error code arrives inside a full frame and the
        // whole message still decodes (§10.1).
        #[derive(Serialize)]
        struct FutureProtocolError {
            code: &'static str,
            message: String,
            details: Option<String>,
        }
        #[derive(Serialize)]
        enum FutureDaemonMessage {
            #[allow(dead_code)]
            HelloAck(HelloAck),
            Response {
                request_id: u64,
                /// Boxed only to keep this stand-in's variants comparable in
                /// size: `Response::Snapshot` is far larger than a `HelloAck`.
                /// `Box<T>` serializes exactly as `T`, so the frame this test
                /// produces is byte-identical to an unboxed one.
                body: Box<Result<Response, FutureProtocolError>>,
            },
        }

        let msg = FutureDaemonMessage::Response {
            request_id: 3,
            body: Box::new(Err(FutureProtocolError {
                code: "QuotaExceeded",
                message: "too many sessions".to_string(),
                details: None,
            })),
        };
        let frame = encode_frame(&msg).unwrap();
        let mut dec = FrameDecoder::new();
        dec.push(&frame);
        let payload = dec.next_frame().unwrap().unwrap();

        let back: DaemonMessage = decode_payload(&payload).expect("unknown code must not fail");
        match back {
            DaemonMessage::Response {
                request_id,
                body: Err(err),
            } => {
                assert_eq!(request_id, 3);
                assert_eq!(err.code, ErrorCode::Unknown);
                assert_eq!(err.message, "too many sessions");
            }
            other => panic!("expected an Err response, got {other:?}"),
        }
    }

    #[test]
    fn oversized_frames_are_refused_on_both_sides_of_the_codec() {
        // ADR-004: 16 MiB cap. Encoding refuses to produce one, and a declared
        // length above the cap is fatal for the connection rather than an
        // unbounded allocation.
        assert_eq!(MAX_FRAME_SIZE, 16 * 1024 * 1024);

        let mut dec = FrameDecoder::new();
        dec.push(&u32::try_from(MAX_FRAME_SIZE + 1).unwrap().to_be_bytes());
        assert!(matches!(
            dec.next_frame(),
            Err(ProtocolCodecError::FrameTooLarge { .. })
        ));

        // Exactly at the cap the length is accepted and the decoder simply waits
        // for the payload, so the boundary is inclusive.
        let mut dec = FrameDecoder::new();
        dec.push(&u32::try_from(MAX_FRAME_SIZE).unwrap().to_be_bytes());
        assert!(matches!(dec.next_frame(), Ok(None)));
    }

    #[test]
    fn json_dump_helper_renders_pretty_json() {
        let msg = ClientMessage::Hello(Hello::new("0.1.0", ClientKind::Gui));
        let json = to_json_string(&msg).unwrap();
        assert!(json.contains("Hello"));
        assert!(json.contains("protocol_version"));
        // Pretty output spans multiple lines.
        assert!(json.contains('\n'));
    }
}
