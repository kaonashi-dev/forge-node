//! Export protocol wire fixtures for the Tauri frontend's round-trip tests.
//!
//! Writes two files per sample into `apps/tauri/tests/fixtures/`:
//!
//! - `<name>.msgpack` — the exact payload bytes the daemon puts on the wire,
//!   via [`rmp_serde::to_vec_named`] (MessagePack maps keyed by field name, the
//!   same call `protocol::framing::encode_frame` makes, minus the length
//!   prefix).
//! - `<name>.json`    — the structurally-equivalent JSON, via
//!   `serde_json::to_string_pretty`, as a human-readable oracle for the keys
//!   and the serde enum tagging.
//!
//! This binary is additive: it changes no library behaviour. Run it with
//! `cargo run -p protocol --bin export-fixtures`. All ids and timestamps are
//! fixed so re-running produces byte-stable fixtures.

use std::path::{Path, PathBuf};

use domain::{
    AgentCapabilities, AgentDescriptor, AgentProfile, AgentProfileId, AgentProviderId, Cell,
    CellFlags, Color, Cursor, CursorShape, DetectionResult, DetectionStatus, DiffFile, DiffStatus,
    FileContents, FileEntry, FileKind, FileTree, Job, JobId, JobState, MouseMode, Project,
    ProjectGroup, ProjectGroupId, ProjectId, ProviderUsage, PtySize, PullRequest, PullRequestLabel,
    PullRequestRelations, PullRequestSource, PullRequestSourceStatus, PullRequestState,
    PullRequestViewer, ReviewDecision, Row, ScrollbackRows, Session, SessionId, SessionKind,
    SessionRole, SessionState, SessionTitle, ShareAction, ShareCandidate, ShareClass, ShareCleanup,
    ShareRule, ShareRuleId, ShareState, ShareStatusEntry, ShareStrategy, ShareTrigger, ShareVerb,
    TermModes, TerminalDelta, TerminalId, TerminalSnapshot, Timestamp, UsageWindow, VersionProbe,
    Workspace, WorkspaceDiff, WorkspaceId, WorkspaceKind, WorkspaceStatus,
};
use protocol::{
    ClientKind, ClientMessage, DaemonEvent, DaemonMessage, ErrorCode, Hello, HelloAck, HelloReject,
    NoticeLevel, ProtocolError, ProviderInfo, Request, Response, PROTOCOL_VERSION,
};
use serde::Serialize;

/// A fixed UUID string so fixtures are deterministic across runs.
fn id<T: std::str::FromStr>(last: &str) -> T
where
    T::Err: std::fmt::Debug,
{
    format!("00000000-0000-7000-8000-0000000000{last}")
        .parse()
        .expect("valid fixed uuid")
}

/// A fixed timestamp so fixtures are deterministic across runs.
fn ts() -> Timestamp {
    Timestamp::parse_rfc3339("2026-08-30T12:00:00Z").expect("valid rfc3339")
}

fn project_id() -> ProjectId {
    id("01")
}
fn project_group_id() -> ProjectGroupId {
    id("02")
}
fn workspace_id() -> WorkspaceId {
    id("03")
}
fn session_id() -> SessionId {
    id("04")
}
fn terminal_id() -> TerminalId {
    id("05")
}
fn agent_profile_id() -> AgentProfileId {
    id("06")
}
fn job_id() -> JobId {
    id("07")
}

fn sample_project() -> Project {
    Project {
        id: project_id(),
        project_group_id: Some(project_group_id()),
        name: "forge".to_string(),
        icon: Some("🦀".to_string()),
        root_path: PathBuf::from("/home/dev/forge"),
        git_root: Some(PathBuf::from("/home/dev/forge")),
        created_at: ts(),
        last_opened_at: ts(),
    }
}

fn sample_project_group() -> ProjectGroup {
    ProjectGroup {
        id: project_group_id(),
        name: "Forge".to_string(),
        created_at: ts(),
    }
}

fn sample_workspace() -> Workspace {
    Workspace {
        id: workspace_id(),
        project_id: project_id(),
        kind: WorkspaceKind::Main,
        path: PathBuf::from("/home/dev/forge"),
        branch: Some("main".to_string()),
        display_name: None,
        managed_by_app: false,
        created_at: ts(),
        status: WorkspaceStatus::default(),
    }
}

fn sample_session() -> Session {
    Session {
        id: session_id(),
        workspace_id: workspace_id(),
        kind: SessionKind::Shell,
        role: SessionRole::Generic,
        parent_session_id: None,
        root_session_id: session_id(),
        terminal_id: Some(terminal_id()),
        agent_provider_id: Some(AgentProviderId::new("claude")),
        agent_profile_id: None,
        title: SessionTitle {
            user: Some("auth refactor".to_string()),
            terminal: Some("~/forge".to_string()),
        },
        state: SessionState::Running,
        created_at: ts(),
        launch_command: None,
        last_activity_at: ts(),
        ended_at: None,
        base_commit: Some("4f2b1ac".to_string()),
    }
}

fn sample_pty_size() -> PtySize {
    PtySize {
        cols: 80,
        rows: 24,
        pixel_width: 0,
        pixel_height: 0,
    }
}

fn sample_cell() -> Cell {
    Cell {
        text: "x".into(),
        fg: Color::Indexed(2),
        bg: Color::Rgb(9, 9, 9),
        flags: CellFlags::BOLD | CellFlags::UNDERLINE,
    }
}

fn sample_row() -> Row {
    Row {
        cells: vec![Cell::default(), sample_cell()],
        wrapped: true,
    }
}

fn sample_cursor() -> Cursor {
    Cursor {
        line: 3,
        col: 4,
        shape: CursorShape::Beam,
        visible: true,
    }
}

fn sample_modes() -> TermModes {
    TermModes {
        alt_screen: true,
        bracketed_paste: true,
        app_cursor_keys: false,
        app_keypad: false,
        mouse_mode: MouseMode::ButtonEvent,
        mouse_sgr: true,
        focus_events: false,
    }
}

fn sample_snapshot() -> TerminalSnapshot {
    TerminalSnapshot {
        seq: 42,
        size: sample_pty_size(),
        visible: vec![Row::blank(4), sample_row()],
        scrollback_tail: vec![sample_row()],
        scrollback_len: 7,
        cursor: sample_cursor(),
        modes: sample_modes(),
        title: Some("vim".to_string()),
    }
}

fn sample_delta() -> TerminalDelta {
    TerminalDelta {
        seq: 5,
        rows: vec![(0, Row::blank(2)), (7, sample_row())],
        scrolled_lines: 3,
        cursor: sample_cursor(),
        modes: TermModes::default(),
    }
}

fn sample_workspace_diff() -> WorkspaceDiff {
    WorkspaceDiff {
        workspace_id: workspace_id(),
        branch: Some("main".to_string()),
        files: vec![
            DiffFile {
                path: "src/main.rs".to_string(),
                status: DiffStatus::Modified,
                additions: 2,
                deletions: 1,
                patch: "@@ -1,3 +1,4 @@\n fn main() {\n-    println!(\"hi\");\n+    println!(\"hello\");\n+    println!(\"world\");\n }\n".to_string(),
                binary: false,
                truncated: false,
            },
            DiffFile {
                path: "assets/logo.png".to_string(),
                status: DiffStatus::Added,
                additions: 0,
                deletions: 0,
                patch: String::new(),
                binary: true,
                truncated: true,
            },
        ],
        truncated: true,
    }
}

fn sample_file_tree() -> FileTree {
    FileTree {
        workspace_id: workspace_id(),
        entries: vec![
            FileEntry {
                path: "README.md".to_string(),
                kind: FileKind::File,
                ignored: false,
            },
            FileEntry {
                path: "src/main.rs".to_string(),
                kind: FileKind::File,
                ignored: false,
            },
            FileEntry {
                path: "src/util/mod.rs".to_string(),
                kind: FileKind::File,
                ignored: false,
            },
            FileEntry {
                path: "target/debug/app".to_string(),
                kind: FileKind::File,
                ignored: true,
            },
        ],
        truncated: false,
    }
}

fn sample_file_contents() -> FileContents {
    FileContents {
        workspace_id: workspace_id(),
        path: "src/main.rs".to_string(),
        text: "fn main() {}\n".to_string(),
        revision: "sha256:1f0a".to_string(),
        language: "rust".to_string(),
        binary: false,
        too_large: false,
    }
}

fn sample_hello() -> Hello {
    Hello {
        protocol_version: PROTOCOL_VERSION,
        client_version: "0.1.0".to_string(),
        client_kind: ClientKind::Gui,
    }
}

fn sample_hello_ack() -> HelloAck {
    HelloAck {
        protocol_version: PROTOCOL_VERSION,
        daemon_version: "0.1.0".to_string(),
        instance_id: "forge-daemon-0001".to_string(),
        started_at: ts(),
    }
}

fn sample_hello_reject() -> HelloReject {
    HelloReject {
        daemon_protocol_version: PROTOCOL_VERSION,
        reason: "protocol version mismatch".to_string(),
    }
}

fn sample_scrollback_rows() -> ScrollbackRows {
    ScrollbackRows {
        from_line: 5,
        rows: vec![Row::blank(2), sample_row()],
    }
}

/// A provider with a *positive* detection result, so the populated snapshot
/// carries the shape a running daemon really sends (§13.1).
fn sample_provider() -> ProviderInfo {
    let provider_id = AgentProviderId::new("claude");
    ProviderInfo {
        descriptor: AgentDescriptor {
            id: provider_id.clone(),
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
        detection: sample_detection(),
    }
}

fn sample_detection() -> DetectionResult {
    DetectionResult {
        provider_id: AgentProviderId::new("claude"),
        status: DetectionStatus::Installed {
            executable: PathBuf::from("/usr/local/bin/claude"),
            version: Some("1.0.0".to_string()),
        },
        checked_at: ts(),
    }
}

fn sample_agent_profile() -> AgentProfile {
    AgentProfile {
        id: agent_profile_id(),
        provider_id: AgentProviderId::new("claude"),
        name: "Work".to_string(),
        executable: None,
        config_dir: Some(PathBuf::from(".claude-work")),
        args: vec!["--model".to_string(), "opus".to_string()],
        created_at: ts(),
    }
}

fn share_rule_id() -> ShareRuleId {
    id("0a")
}

fn sample_share_rule() -> ShareRule {
    ShareRule {
        id: share_rule_id(),
        project_id: project_id(),
        path: ".env".to_string(),
        strategy: ShareStrategy::Link,
        enabled: true,
        position: 0,
        created_at: ts(),
    }
}

fn sample_share_candidate() -> ShareCandidate {
    ShareCandidate {
        path: "node_modules".to_string(),
        class: ShareClass::Dependencies,
        is_dir: true,
        size_bytes: Some(912_384_000),
        entries: Some(48_211),
        suggested: ShareStrategy::Clone,
        already_ruled: false,
    }
}

fn sample_share_action() -> ShareAction {
    ShareAction {
        rule_id: share_rule_id(),
        path: ".env".to_string(),
        verb: ShareVerb::Link,
        bytes: Some(412),
        fallback: false,
        note: None,
    }
}

fn sample_share_status_entry() -> ShareStatusEntry {
    ShareStatusEntry {
        rule_id: share_rule_id(),
        path: ".env".to_string(),
        state: ShareState::Severed,
    }
}

fn sample_provider_usage() -> ProviderUsage {
    ProviderUsage {
        provider_id: AgentProviderId::new("claude"),
        // The default account. A profile's reading carries its id here.
        profile_id: None,
        windows: vec![UsageWindow {
            used_percent: 42,
            window: "5h".to_string(),
            resets_at: Some(ts()),
        }],
        collected_at: ts(),
    }
}

fn sample_job() -> Job {
    Job {
        id: job_id(),
        provider_id: AgentProviderId::new("claude"),
        workspace_id: workspace_id(),
        role: SessionRole::Executor,
        feature_id: Some(7),
        parent_session_id: Some(session_id()),
        state: JobState::Running,
        summary: "implement the spec".to_string(),
        prompt: "implement the spec for feature 7".to_string(),
        provider_session_id: Some("prov-1".to_string()),
        exit_code: None,
        last_line: Some("editing apps/tauri/src/App.tsx".to_string()),
        last_output_at: Some(domain::Timestamp::now()),
        started_at: ts(),
        finished_at: None,
        log_path: PathBuf::from("/tmp/forge/jobs/7.jsonl"),
    }
}

fn sample_pull_request_state() -> PullRequestState {
    PullRequestState {
        pull_requests: vec![PullRequest {
            project_id: Some(project_id()),
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
            created_at: ts(),
            updated_at: ts(),
            relations: PullRequestRelations {
                assigned: true,
                review_requested: false,
                authored: true,
            },
        }],
        viewers: vec![PullRequestViewer {
            host: "github.com".to_string(),
            login: "octocat".to_string(),
        }],
        sources: vec![PullRequestSource {
            project_id: project_id(),
            host: Some("github.com".to_string()),
            repository: Some("forge/forge-node".to_string()),
            status: PullRequestSourceStatus::Ready,
        }],
        failures: vec![],
        error: None,
        refreshed_at: Some(ts()),
    }
}

fn sample_protocol_error() -> ProtocolError {
    ProtocolError::with_details(
        ErrorCode::PreconditionFailed,
        "session is Running",
        "close it or kill it first",
    )
}

fn main() {
    let out = fixtures_dir();
    std::fs::create_dir_all(&out).expect("create Fixtures dir");

    // ---- Handshake ----
    write(&out, "client_kind_gui", &ClientKind::Gui);
    write(&out, "hello", &sample_hello());
    write(&out, "hello_ack", &sample_hello_ack());
    write(&out, "hello_reject", &sample_hello_reject());

    // ---- Errors ----
    write(&out, "error_code", &ErrorCode::NotFound);
    write(&out, "protocol_error", &sample_protocol_error());

    // ---- Terminal grid ----
    write(&out, "pty_size", &sample_pty_size());
    write(&out, "color_default", &Color::Default);
    write(&out, "color_indexed", &Color::Indexed(9));
    write(&out, "color_rgb", &Color::Rgb(1, 2, 3));
    write(
        &out,
        "cell_flags",
        &(CellFlags::BOLD | CellFlags::WIDE_CHAR),
    );
    write(&out, "cell", &sample_cell());
    write(&out, "row", &sample_row());
    write(&out, "cursor", &sample_cursor());
    write(&out, "term_modes", &sample_modes());
    write(&out, "terminal_snapshot", &sample_snapshot());
    write(&out, "terminal_delta", &sample_delta());
    write(&out, "scrollback_rows", &sample_scrollback_rows());
    write(&out, "cursor_shape_block", &CursorShape::Block);
    write(&out, "mouse_mode_any_event", &MouseMode::AnyEvent);

    // ---- Domain ----
    write(&out, "project", &sample_project());
    write(&out, "project_group", &sample_project_group());
    write(&out, "workspace", &sample_workspace());
    write(&out, "session", &sample_session());
    write(&out, "session_state_running", &SessionState::Running);
    write(
        &out,
        "session_state_exited",
        &SessionState::Exited {
            code: Some(0),
            signal: None,
        },
    );
    write(
        &out,
        "session_state_failed",
        &SessionState::Failed {
            reason: "spawn refused".to_string(),
        },
    );
    write(&out, "session_state_orphaned", &SessionState::Orphaned);
    write(
        &out,
        "session_role_custom",
        &SessionRole::Custom("harness".to_string()),
    );
    write(
        &out,
        "workspace_status",
        &WorkspaceStatus {
            dirty: true,
            ahead: Some(2),
            behind: Some(1),
            measured_at: Some(ts()),
        },
    );

    // ---- Diff & file browser (Phase 4B/4C) ----
    write(&out, "workspace_diff", &sample_workspace_diff());
    write(&out, "file_tree", &sample_file_tree());
    write(&out, "file_contents", &sample_file_contents());

    // ---- Requests ----
    write(&out, "request_get_snapshot", &Request::GetSnapshot);
    write(&out, "request_factory_reset", &Request::FactoryReset);
    // The terminal path: every request the GUI client sends while a shell is
    // on screen. These are the hottest frames on the wire and the ones a field
    // rename would break most quietly, so each gets its own byte fixture.
    write(
        &out,
        "request_attach_terminal",
        &Request::AttachTerminal {
            terminal_id: terminal_id(),
            size: sample_pty_size(),
        },
    );
    write(
        &out,
        "request_detach_terminal",
        &Request::DetachTerminal {
            terminal_id: terminal_id(),
        },
    );
    write(
        &out,
        "request_write_terminal_input",
        &Request::WriteTerminalInput {
            terminal_id: terminal_id(),
            bytes: vec![0x1b, b'[', b'A'],
        },
    );
    write(
        &out,
        "request_resize_terminal",
        &Request::ResizeTerminal {
            terminal_id: terminal_id(),
            size: sample_pty_size(),
        },
    );
    write(
        &out,
        "request_fetch_scrollback",
        &Request::FetchScrollback {
            terminal_id: terminal_id(),
            from_line: 5,
            count: 64,
        },
    );
    write(
        &out,
        "request_create_shell_session",
        &Request::CreateShellSession {
            workspace_id: workspace_id(),
            parent: None,
            role: SessionRole::Generic,
        },
    );
    write(
        &out,
        "request_refresh_workspace_status",
        &Request::RefreshWorkspaceStatus {
            workspace_id: workspace_id(),
        },
    );
    write(
        &out,
        "request_list_workspaces",
        &Request::ListWorkspaces {
            project_id: project_id(),
        },
    );
    write(
        &out,
        "request_get_workspace_diff",
        &Request::GetWorkspaceDiff {
            workspace_id: workspace_id(),
            context_lines: Some(3),
        },
    );
    write(
        &out,
        "request_list_files",
        &Request::ListFiles {
            workspace_id: workspace_id(),
        },
    );
    write(
        &out,
        "request_read_file",
        &Request::ReadFile {
            workspace_id: workspace_id(),
            path: "src/main.rs".to_string(),
        },
    );
    write(
        &out,
        "request_write_file",
        &Request::WriteFile {
            workspace_id: workspace_id(),
            path: "src/main.rs".to_string(),
            text: "fn main() { println!(\"saved\"); }\n".to_string(),
            expected_revision: "sha256:1f0a".to_string(),
        },
    );

    // ---- Responses ----
    write(&out, "response_ack", &Response::Ack);
    write(
        &out,
        "response_workspaces",
        &Response::Workspaces(vec![sample_workspace()]),
    );
    write(
        &out,
        "response_attach_ack",
        &Response::AttachAck {
            snapshot: sample_snapshot(),
        },
    );
    write(&out, "response_snapshot", &sample_snapshot_response());
    write(
        &out,
        "response_snapshot_populated",
        &populated_snapshot_response(),
    );
    write(
        &out,
        "response_session_created",
        &Response::SessionCreated {
            session_id: session_id(),
            terminal_id: terminal_id(),
        },
    );
    write(
        &out,
        "response_scrollback_rows",
        &Response::ScrollbackRows(sample_scrollback_rows()),
    );
    write(
        &out,
        "response_workspace_diff",
        &Response::WorkspaceDiff(sample_workspace_diff()),
    );
    write(
        &out,
        "response_file_tree",
        &Response::FileTree(sample_file_tree()),
    );
    write(
        &out,
        "response_share_candidates",
        &Response::ShareCandidates {
            project_id: project_id(),
            candidates: vec![sample_share_candidate()],
            truncated: false,
        },
    );
    write(
        &out,
        "response_share_plan",
        &Response::SharePlan {
            workspace_id: Some(workspace_id()),
            actions: vec![sample_share_action()],
        },
    );
    write(
        &out,
        "response_share_status",
        &Response::ShareStatus {
            workspace_id: workspace_id(),
            entries: vec![sample_share_status_entry()],
        },
    );
    write(
        &out,
        "request_set_project_shares",
        &Request::SetProjectShares {
            project_id: project_id(),
            rules: vec![sample_share_rule()],
        },
    );
    write(
        &out,
        "request_remove_share_rule",
        &Request::RemoveShareRule {
            project_id: project_id(),
            rule_id: share_rule_id(),
            cleanup: ShareCleanup::RemoveInjected,
        },
    );
    write(
        &out,
        "request_apply_shares",
        &Request::ApplyShares {
            workspace_id: workspace_id(),
            only: None,
        },
    );
    write(
        &out,
        "response_file_contents",
        &Response::FileContents(sample_file_contents()),
    );

    // ---- Events ----
    //
    // Every `DaemonEvent` the daemon can broadcast gets a fixture, because a
    // client that cannot decode one of them loses the *connection*, not just
    // the event: the frame reader gives up on the first undecodable payload
    // (`client/src/ipc.rs::reader_loop`). The frontend mirror must therefore
    // either model a variant or degrade it deliberately; the fixtures are what
    // proves which of the two happened.
    write(&out, "event_factory_reset", &DaemonEvent::FactoryReset);
    write(
        &out,
        "event_project_group_created",
        &DaemonEvent::ProjectGroupCreated(sample_project_group()),
    );
    write(
        &out,
        "event_project_group_removed",
        &DaemonEvent::ProjectGroupRemoved {
            project_group_id: project_group_id(),
        },
    );
    write(
        &out,
        "event_project_added",
        &DaemonEvent::ProjectAdded(sample_project()),
    );
    write(
        &out,
        "event_project_removed",
        &DaemonEvent::ProjectRemoved {
            project_id: project_id(),
        },
    );
    write(
        &out,
        "event_workspace_updated",
        &DaemonEvent::WorkspaceUpdated(sample_workspace()),
    );
    write(
        &out,
        "event_workspace_removed",
        &DaemonEvent::WorkspaceRemoved {
            workspace_id: workspace_id(),
        },
    );
    write(
        &out,
        "event_session_created",
        &DaemonEvent::SessionCreated(sample_session()),
    );
    write(
        &out,
        "event_session_updated",
        &DaemonEvent::SessionUpdated(sample_session()),
    );
    write(
        &out,
        "event_session_removed",
        &DaemonEvent::SessionRemoved {
            session_id: session_id(),
        },
    );
    write(
        &out,
        "event_terminal_delta",
        &DaemonEvent::TerminalDelta {
            terminal_id: terminal_id(),
            delta: sample_delta(),
        },
    );
    write(
        &out,
        "event_terminal_resync",
        &DaemonEvent::TerminalResync {
            terminal_id: terminal_id(),
            snapshot: sample_snapshot(),
        },
    );
    write(
        &out,
        "event_terminal_activity",
        &DaemonEvent::TerminalActivity {
            terminal_id: terminal_id(),
        },
    );
    write(
        &out,
        "event_terminal_bell",
        &DaemonEvent::TerminalBell {
            terminal_id: terminal_id(),
        },
    );
    write(
        &out,
        "event_remote_refs_updated",
        &DaemonEvent::RemoteRefsUpdated {
            project_id: project_id(),
            remote: "origin".to_string(),
            updated: 3,
            error: None,
        },
    );
    write(
        &out,
        "event_pull_request_opened",
        &DaemonEvent::PullRequestOpened {
            workspace_id: workspace_id(),
            url: Some("https://github.com/forge/forge-node/pull/42".to_string()),
            error: None,
        },
    );
    write(
        &out,
        "event_file_changed",
        &DaemonEvent::FileChanged {
            workspace_id: workspace_id(),
            path: "src/main.rs".to_string(),
        },
    );
    write(
        &out,
        "event_job_output",
        &DaemonEvent::JobOutput {
            job_id: job_id(),
            from_line: 12,
            lines: vec!["reading apps/tauri/src/App.tsx".to_string()],
        },
    );
    write(
        &out,
        "event_daemon_notice",
        &DaemonEvent::DaemonNotice {
            level: NoticeLevel::Warning,
            message: "path missing".to_string(),
        },
    );
    write(
        &out,
        "event_daemon_shutting_down",
        &DaemonEvent::DaemonShuttingDown {
            reason: "StopDaemon".to_string(),
        },
    );
    // Events whose payloads the frontend mirror does not model yet (Phases 4–5).
    // Exported anyway: they are the ones that prove an unmodeled variant
    // degrades instead of killing the socket.
    write(
        &out,
        "event_agent_detection_changed",
        &DaemonEvent::AgentDetectionChanged {
            results: vec![sample_detection()],
        },
    );
    write(
        &out,
        "event_agent_profiles_changed",
        &DaemonEvent::AgentProfilesChanged {
            profiles: vec![sample_agent_profile()],
        },
    );
    write(
        &out,
        "event_project_shares_changed",
        &DaemonEvent::ProjectSharesChanged {
            project_id: project_id(),
            rules: vec![sample_share_rule()],
        },
    );
    write(
        &out,
        "event_shares_applied",
        &DaemonEvent::SharesApplied {
            workspace_id: workspace_id(),
            trigger: ShareTrigger::Created,
            actions: vec![sample_share_action()],
            error: None,
        },
    );
    write(
        &out,
        "event_provider_usage_changed",
        &DaemonEvent::ProviderUsageChanged {
            usage: vec![sample_provider_usage()],
        },
    );
    write(
        &out,
        "event_job_updated",
        &DaemonEvent::JobUpdated(Box::new(sample_job())),
    );
    write(
        &out,
        "event_pull_requests_updated",
        &DaemonEvent::PullRequestsUpdated {
            state: sample_pull_request_state(),
        },
    );

    // ---- Top-level envelopes ----
    write(
        &out,
        "client_message_hello",
        &ClientMessage::Hello(sample_hello()),
    );
    write(
        &out,
        "client_message_request",
        &ClientMessage::Request {
            request_id: 42,
            body: Request::ListWorkspaces {
                project_id: project_id(),
            },
        },
    );
    write(
        &out,
        "daemon_message_hello_ack",
        &DaemonMessage::HelloAck(sample_hello_ack()),
    );
    write(
        &out,
        "daemon_message_hello_reject",
        &DaemonMessage::HelloReject(sample_hello_reject()),
    );
    write(
        &out,
        "daemon_message_response_ok",
        &DaemonMessage::Response {
            request_id: 7,
            body: Ok(Response::Ack),
        },
    );
    write(
        &out,
        "daemon_message_response_err",
        &DaemonMessage::Response {
            request_id: 8,
            body: Err(sample_protocol_error()),
        },
    );
    write(
        &out,
        "daemon_message_event",
        &DaemonMessage::Event(DaemonEvent::SessionUpdated(sample_session())),
    );

    println!("wrote fixtures to {}", out.display());
}

/// A `Response::Snapshot` whose heavy provider/profile/job/usage arrays are
/// empty: the probe needs the project/workspace/session lists to decode, and
/// the frontend models those collections as empty for Phase 1.
fn sample_snapshot_response() -> Response {
    Response::Snapshot {
        project_groups: vec![sample_project_group()],
        projects: vec![sample_project()],
        workspaces: vec![sample_workspace()],
        sessions: vec![sample_session()],
        providers: vec![],
        agent_profiles: vec![],
        worktree_shares: vec![],
        app_state: vec![("sidebar_width".to_string(), "280".to_string())],
        external_agents: vec![],
        pull_requests: PullRequestState::default(),
        jobs: vec![],
        usage: vec![],
    }
}

/// A `Response::Snapshot` with **every** collection populated — the shape a
/// daemon that has detected a provider, saved a profile, run a job and
/// refreshed pull requests actually sends on `GetSnapshot`.
///
/// The empty-collection snapshot above says nothing about whether a client can
/// decode the real thing; a client that models `providers` as an opaque record
/// still has to survive the bytes. This is the fixture that asks that question.
fn populated_snapshot_response() -> Response {
    Response::Snapshot {
        project_groups: vec![sample_project_group()],
        projects: vec![sample_project()],
        workspaces: vec![sample_workspace()],
        sessions: vec![sample_session()],
        providers: vec![sample_provider()],
        agent_profiles: vec![sample_agent_profile()],
        worktree_shares: vec![sample_share_rule()],
        app_state: vec![("ui.theme_base".to_string(), "gruvbox".to_string())],
        external_agents: vec![],
        pull_requests: sample_pull_request_state(),
        jobs: vec![sample_job()],
        usage: vec![sample_provider_usage()],
    }
}

/// Resolve `<repo>/apps/tauri/tests/fixtures` from this crate's manifest
/// directory (`<repo>/crates/protocol`).
fn fixtures_dir() -> PathBuf {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let repo_root = manifest
        .parent()
        .and_then(Path::parent)
        .expect("crates/protocol has a repo root two levels up");
    repo_root.join("apps/tauri/tests/fixtures")
}

/// Write both the MessagePack payload and the pretty JSON oracle for `value`.
fn write<T: Serialize>(dir: &Path, name: &str, value: &T) {
    let msgpack = rmp_serde::to_vec_named(value).expect("encode msgpack");
    std::fs::write(dir.join(format!("{name}.msgpack")), &msgpack).expect("write .msgpack");

    let json = serde_json::to_string_pretty(value).expect("encode json");
    std::fs::write(dir.join(format!("{name}.json")), format!("{json}\n")).expect("write .json");
}
